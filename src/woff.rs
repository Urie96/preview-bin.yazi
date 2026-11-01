//! WOFF 1.0 / WOFF2 -> sfnt (TTF/OTF) conversion.
//!
//! This is a Rust port of the original `woff2otf.py` helper. WOFF 1.0 is
//! parsed here and inflated with `flate2`; WOFF2 (Brotli + transforms) is
//! delegated to the pure-Rust `wuff` crate. Both paths emit a normal sfnt.

use std::io::Read;

use flate2::read::ZlibDecoder;

const WOFF_SIGNATURE: u32 = 0x774f_4646; // 'wOFF'
const WOFF2_SIGNATURE: u32 = 0x774f_4632; // 'wOF2'
const HEAD_TAG: u32 = u32::from_be_bytes(*b"head");
const HEAD_CHECKSUM_MAGIC: u32 = 0xb1b0_afba;

/// Maximum size of a single inflated font table.
const MAX_TABLE_SIZE: usize = 256 * 1024 * 1024;
/// Maximum size of the generated sfnt.
const MAX_FONT_SIZE: usize = 512 * 1024 * 1024;
/// An sfnt offset table stores `numTables * 16` in a u16.
const MAX_TABLES: usize = 4095;

/// Convert WOFF bytes into standalone TTF/OTF bytes.
pub fn to_sfnt(woff: &[u8]) -> Result<Vec<u8>, String> {
    if woff.len() < 44 {
        return Err("woff: file is too short".into());
    }

    let signature = be32(woff, 0)?;
    if signature == WOFF2_SIGNATURE {
        return Err("woff2: conversion requires Brotli, only WOFF 1.0 is supported".into());
    }
    if signature != WOFF_SIGNATURE {
        return Err("woff: bad signature".into());
    }

    let flavor = be32(woff, 4)?;
    let num_tables = usize::from(be16(woff, 12)?);
    let reserved = be16(woff, 14)?;
    let total_sfnt_size = be32(woff, 16)? as usize;
    if reserved != 0 {
        return Err("woff: reserved header field is not zero".into());
    }
    if num_tables == 0 {
        return Err("woff: no tables".into());
    }
    if num_tables > MAX_TABLES {
        return Err(format!("woff: too many tables ({num_tables})"));
    }
    if total_sfnt_size > MAX_FONT_SIZE {
        return Err("woff: declared sfnt size is too large".into());
    }

    let directory_end = 44usize
        .checked_add(
            num_tables
                .checked_mul(20)
                .ok_or("woff: table count overflow")?,
        )
        .ok_or("woff: table directory overflow")?;
    if directory_end > woff.len() {
        return Err("woff: table directory is truncated".into());
    }

    let mut tables = Vec::with_capacity(num_tables);
    let mut total_table_bytes = 0usize;

    for index in 0..num_tables {
        let base = 44 + index * 20;
        let tag = be32(woff, base)?;
        let offset = be32(woff, base + 4)? as usize;
        let comp_length = be32(woff, base + 8)? as usize;
        let orig_length = be32(woff, base + 12)? as usize;
        // origChecksum is intentionally ignored: the output checksums are
        // recomputed after the tables have been laid out.
        let _ = be32(woff, base + 16)?;

        if orig_length > MAX_TABLE_SIZE {
            return Err(format!("woff: table is too large ({orig_length} bytes)"));
        }
        if comp_length > orig_length {
            return Err("woff: compressed table is larger than its original".into());
        }
        let end = offset
            .checked_add(comp_length)
            .ok_or("woff: table offset overflow")?;
        if end > woff.len() {
            return Err("woff: table data is truncated".into());
        }

        total_table_bytes = total_table_bytes
            .checked_add(orig_length)
            .ok_or("woff: total table size overflow")?;
        if total_table_bytes > MAX_FONT_SIZE {
            return Err("woff: uncompressed tables are too large".into());
        }

        let data = if comp_length == orig_length {
            woff[offset..end].to_vec()
        } else {
            zlib_decompress(&woff[offset..end], orig_length)
                .map_err(|err| format!("woff: table {index}: {err}"))?
        };
        if data.len() != orig_length {
            return Err("woff: inflated table has the wrong length".into());
        }

        tables.push(Table {
            tag,
            data,
            offset: 0,
        });
    }

    // sfnt requires directory entries in ascending tag order.
    tables.sort_by_key(|table| table.tag);
    if tables.windows(2).any(|pair| pair[0].tag == pair[1].tag) {
        return Err("woff: duplicate table tag".into());
    }

    // The head checksum is defined with checkSumAdjustment set to zero.
    // It will be filled in once the complete font has been assembled.
    for table in &mut tables {
        if table.tag == HEAD_TAG && table.data.len() >= 12 {
            table.data[8..12].fill(0);
        }
    }

    let (search_range, entry_selector, range_shift) = search_params(tables.len());

    let mut next_offset = 12usize
        .checked_add(
            tables
                .len()
                .checked_mul(16)
                .ok_or("woff: table directory overflow")?,
        )
        .ok_or("woff: table directory overflow")?;

    for table in &mut tables {
        table.offset = next_offset;
        next_offset = next_offset
            .checked_add(padded_len(table.data.len()))
            .ok_or("woff: font size overflow")?;
    }
    if next_offset > MAX_FONT_SIZE {
        return Err("woff: generated font is too large".into());
    }

    let mut sfnt = vec![0u8; next_offset];
    put_be32(&mut sfnt, 0, flavor);
    put_be16(&mut sfnt, 4, tables.len() as u16);
    put_be16(&mut sfnt, 6, search_range);
    put_be16(&mut sfnt, 8, entry_selector);
    put_be16(&mut sfnt, 10, range_shift);

    for (index, table) in tables.iter().enumerate() {
        let record = 12 + index * 16;
        put_be32(&mut sfnt, record, table.tag);
        put_be32(&mut sfnt, record + 4, checksum(&table.data));
        put_be32(&mut sfnt, record + 8, table.offset as u32);
        put_be32(&mut sfnt, record + 12, table.data.len() as u32);

        let start = table.offset;
        sfnt[start..start + table.data.len()].copy_from_slice(&table.data);
    }

    if let Some(head) = tables
        .iter()
        .find(|table| table.tag == HEAD_TAG && table.data.len() >= 12)
    {
        let adjustment = HEAD_CHECKSUM_MAGIC.wrapping_sub(checksum(&sfnt));
        put_be32(&mut sfnt, head.offset + 8, adjustment);
    }

    // Keep the declared size honest. This is not required for rendering, but
    // it catches truncated/incorrect WOFF files early.
    if total_sfnt_size != 0 && total_sfnt_size != sfnt.len() {
        return Err(format!(
            "woff: totalSfntSize is {total_sfnt_size}, generated font is {} bytes",
            sfnt.len()
        ));
    }

    Ok(sfnt)
}

struct Table {
    tag: u32,
    data: Vec<u8>,
    offset: usize,
}

/// Convert WOFF2 bytes into standalone TTF/OTF bytes.
pub fn woff2_to_sfnt(woff2: &[u8]) -> Result<Vec<u8>, String> {
    wuff::decompress_woff2(woff2).map_err(|err| format!("woff2: {err}"))
}

fn zlib_decompress(input: &[u8], expected: usize) -> Result<Vec<u8>, String> {
    let decoder = ZlibDecoder::new(input);
    let mut out = Vec::with_capacity(expected);
    decoder
        .take(expected as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|err| format!("zlib: {err}"))?;
    if out.len() != expected {
        return Err(format!(
            "zlib: decompressed size {} does not match expected {expected}",
            out.len()
        ));
    }
    Ok(out)
}

fn padded_len(len: usize) -> usize {
    (len + 3) & !3
}

/// sfnt offset-table values: largest power of two <= numTables.
fn search_params(num_tables: usize) -> (u16, u16, u16) {
    if num_tables == 0 {
        return (0, 0, 0);
    }
    let mut power = 1usize;
    let mut selector = 0u16;
    while power * 2 <= num_tables {
        power *= 2;
        selector += 1;
    }
    let search_range = (power * 16) as u16;
    let range_shift = (num_tables * 16 - search_range as usize) as u16;
    (search_range, selector, range_shift)
}

fn checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    for chunk in data.chunks(4) {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum = sum.wrapping_add(u32::from_be_bytes(word));
    }
    sum
}

fn be16(data: &[u8], offset: usize) -> Result<u16, String> {
    let bytes = data
        .get(offset..offset + 2)
        .ok_or("woff: truncated header")?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn be32(data: &[u8], offset: usize) -> Result<u32, String> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or("woff: truncated header")?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn put_be16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn put_be32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::{checksum, to_sfnt, HEAD_CHECKSUM_MAGIC};

    const TEST_TAG: u32 = u32::from_be_bytes(*b"test");
    const TEST_PAYLOAD: &[u8] = b"abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz";

    // zlib.compress(TEST_PAYLOAD, 9)
    const TEST_ZLIB: &[u8] = &[
        0x78, 0xda, 0x4b, 0x4c, 0x4a, 0x4e, 0x49, 0x4d, 0x4b, 0xcf, 0xc8, 0xcc, 0xca, 0xce, 0xc9,
        0xcd, 0xcb, 0x2f, 0x28, 0x2c, 0x2a, 0x2e, 0x29, 0x2d, 0x2b, 0xaf, 0xa8, 0xac, 0x4a, 0x1c,
        0x31, 0x32, 0x00, 0x7c, 0x32, 0x6f, 0x37,
    ];

    fn build_woff() -> Vec<u8> {
        let head = vec![0u8; 54];
        let head_offset = 44 + 2 * 20;
        let test_offset = head_offset + 54;
        let total = test_offset + TEST_ZLIB.len();

        let mut woff = Vec::new();
        woff.extend_from_slice(&0x774f_4646u32.to_be_bytes()); // signature
        woff.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // flavor
        woff.extend_from_slice(&(total as u32).to_be_bytes()); // length
        woff.extend_from_slice(&2u16.to_be_bytes()); // numTables
        woff.extend_from_slice(&0u16.to_be_bytes()); // reserved
        let sfnt_size = 12 + 2 * 16 + 56 + TEST_PAYLOAD.len();
        woff.extend_from_slice(&(sfnt_size as u32).to_be_bytes());
        woff.extend_from_slice(&1u16.to_be_bytes()); // majorVersion
        woff.extend_from_slice(&0u16.to_be_bytes()); // minorVersion
        woff.extend_from_slice(&0u32.to_be_bytes()); // metaOffset
        woff.extend_from_slice(&0u32.to_be_bytes()); // metaLength
        woff.extend_from_slice(&0u32.to_be_bytes()); // metaOrigLength
        woff.extend_from_slice(&0u32.to_be_bytes()); // privOffset
        woff.extend_from_slice(&0u32.to_be_bytes()); // privLength

        // head (stored uncompressed)
        woff.extend_from_slice(b"head");
        woff.extend_from_slice(&(head_offset as u32).to_be_bytes());
        woff.extend_from_slice(&54u32.to_be_bytes());
        woff.extend_from_slice(&54u32.to_be_bytes());
        woff.extend_from_slice(&0u32.to_be_bytes());
        // test (zlib-compressed)
        woff.extend_from_slice(&TEST_TAG.to_be_bytes());
        woff.extend_from_slice(&(test_offset as u32).to_be_bytes());
        woff.extend_from_slice(&(TEST_ZLIB.len() as u32).to_be_bytes());
        woff.extend_from_slice(&(TEST_PAYLOAD.len() as u32).to_be_bytes());
        woff.extend_from_slice(&0u32.to_be_bytes());

        woff.extend_from_slice(&head);
        woff.extend_from_slice(TEST_ZLIB);
        woff
    }

    #[test]
    fn converts_woff_to_sfnt() {
        let sfnt = to_sfnt(&build_woff()).unwrap();

        assert_eq!(&sfnt[0..4], &0x0001_0000u32.to_be_bytes());
        assert_eq!(u16::from_be_bytes([sfnt[4], sfnt[5]]), 2);
        assert_eq!(&sfnt[12..16], b"head");
        assert_eq!(&sfnt[28..32], b"test");

        let head_offset = u32::from_be_bytes(sfnt[20..24].try_into().unwrap()) as usize;
        let test_offset = u32::from_be_bytes(sfnt[36..40].try_into().unwrap()) as usize;
        assert_eq!(&sfnt[head_offset..head_offset + 8], &[0u8; 8]);
        assert_eq!(&sfnt[head_offset + 12..head_offset + 54], &[0u8; 42]);
        assert_eq!(
            &sfnt[test_offset..test_offset + TEST_PAYLOAD.len()],
            TEST_PAYLOAD
        );

        // The finalized font must sum to the sfnt magic value.
        assert_eq!(checksum(&sfnt), HEAD_CHECKSUM_MAGIC);
    }

    #[test]
    fn rejects_woff2_in_woff1_converter() {
        let mut woff = build_woff();
        woff[0..4].copy_from_slice(&0x774f_4632u32.to_be_bytes());
        assert!(to_sfnt(&woff).is_err());
    }

    #[test]
    fn rejects_garbage_woff2() {
        assert!(super::woff2_to_sfnt(b"not a font").is_err());
    }
}
