//! Fujifilm RAF preview extraction.
//!
//! RAF files carry the camera JPEG preview at a known location: the header
//! holds two big-endian `u32`s at `0x54`/`0x58` with the preview's offset and
//! length (what exiftool reports as `PreviewImage`). When those fields are not
//! usable, the file is scanned for the first `SOI..EOI` pair instead.
//!
//! This replaces `exiftool -previewimage -b`, so previewing a RAF needs no
//! external tool at all.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

const MAGIC: &[u8] = b"FUJIFILMCCD-RAW ";
const JPEG_SOI: &[u8] = &[0xff, 0xd8, 0xff];
const JPEG_EOI: &[u8] = &[0xff, 0xd9];
const HEADER_LEN: usize = 0x60;
const OFFSET_AT: usize = 0x54;
const CHUNK: usize = 256 * 1024;

/// Extract the embedded preview JPEG from `path`.
pub fn preview(path: &Path) -> Result<Vec<u8>, String> {
    let display = path.display();
    let mut file = BufReader::with_capacity(
        CHUNK,
        File::open(path).map_err(|e| format!("{display}: {e}"))?,
    );

    if let Some(jpeg) = from_header(&mut file).map_err(|e| format!("{display}: {e}"))? {
        return Ok(jpeg);
    }

    file.seek(SeekFrom::Start(0))
        .and_then(|_| scan(&mut file))
        .map_err(|e| format!("{display}: {e}"))?
        .ok_or_else(|| format!("{display}: no embedded JPEG preview"))
}

/// Locate the preview through the offset/length recorded in the RAF header.
fn from_header(file: &mut BufReader<File>) -> io::Result<Option<Vec<u8>>> {
    let mut header = [0u8; HEADER_LEN];
    let mut filled = 0;
    while filled < HEADER_LEN {
        match file.read(&mut header[filled..])? {
            0 => return Ok(None),
            n => filled += n,
        }
    }
    if !header.starts_with(MAGIC) {
        return Ok(None);
    }

    let offset = u32::from_be_bytes(header[OFFSET_AT..OFFSET_AT + 4].try_into().unwrap()) as u64;
    let length =
        u32::from_be_bytes(header[OFFSET_AT + 4..OFFSET_AT + 8].try_into().unwrap()) as u64;
    let file_len = file.seek(SeekFrom::End(0))?;
    if offset == 0 || length == 0 || offset.saturating_add(length) > file_len {
        return Ok(None);
    }

    let Ok(length) = usize::try_from(length) else {
        return Ok(None);
    };

    file.seek(SeekFrom::Start(offset))?;
    let mut jpeg = vec![0u8; length];
    file.read_exact(&mut jpeg)?;
    if !jpeg.starts_with(JPEG_SOI) || !jpeg.ends_with(JPEG_EOI) {
        return Ok(None);
    }
    Ok(Some(jpeg))
}

/// Fallback: stream the file and keep the first `SOI..EOI` range.
fn scan(file: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut chunk = vec![0u8; CHUNK];
    let mut jpeg: Vec<u8> = Vec::new();
    let mut started = false;
    // Bytes kept from the previous chunk so a marker can straddle its end.
    let mut carry: Vec<u8> = Vec::new();

    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            return Ok(None);
        }

        let mut data = std::mem::take(&mut carry);
        data.extend_from_slice(&chunk[..read]);

        if started {
            jpeg.extend_from_slice(&data);
        } else {
            match find(&data, JPEG_SOI) {
                Some(pos) => {
                    started = true;
                    jpeg.extend_from_slice(&data[pos..]);
                }
                None => {
                    let keep = data.len().min(JPEG_SOI.len() - 1);
                    carry.extend_from_slice(&data[data.len() - keep..]);
                    continue;
                }
            }
        }

        // Rewind far enough to catch an EOI split across the chunk boundary.
        let from = jpeg.len().saturating_sub(data.len() + JPEG_EOI.len());
        if let Some(pos) = find(&jpeg[from..], JPEG_EOI) {
            jpeg.truncate(from + pos + JPEG_EOI.len());
            return Ok(Some(jpeg));
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// `SOI + payload + EOI`.
    fn jpeg(payload: usize) -> Vec<u8> {
        let mut blob = JPEG_SOI.to_vec();
        blob.extend(std::iter::repeat(0x11).take(payload));
        blob.extend_from_slice(JPEG_EOI);
        blob
    }

    fn padded(prefix: usize, blob: &[u8]) -> Vec<u8> {
        let mut data = vec![0xaa; prefix];
        data.extend_from_slice(blob);
        data
    }

    fn raf_header(offset: u32, length: u32) -> Vec<u8> {
        let mut data = vec![0u8; 0x80];
        data[..MAGIC.len()].copy_from_slice(MAGIC);
        data[OFFSET_AT..OFFSET_AT + 4].copy_from_slice(&offset.to_be_bytes());
        data[OFFSET_AT + 4..OFFSET_AT + 8].copy_from_slice(&length.to_be_bytes());
        data
    }

    fn write(name: &str, data: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("preview-raf-test-{name}.RAF"));
        fs::write(&path, data).unwrap();
        path
    }

    fn check(name: &str, data: &[u8], expected: &[u8]) {
        let path = write(name, data);
        let found = preview(&path).unwrap();
        let _ = fs::remove_file(&path);
        assert_eq!(found, expected);
    }

    #[test]
    fn header_offset_wins() {
        let blob = jpeg(1000);
        let decoy = jpeg(16);
        // Decoy JPEG inside the header area: the header must still win.
        let mut data = raf_header(0x200, blob.len() as u32);
        data.extend_from_slice(&decoy);
        data.resize(0x200, 0);
        data.extend_from_slice(&blob);
        check("header", &data, &blob);
    }

    #[test]
    fn falls_back_to_scan() {
        let blob = jpeg(500);
        let mut data = raf_header(0, 0);
        data.extend_from_slice(&padded(3000, &blob));
        check("scan", &data, &blob);
    }

    #[test]
    fn scan_spans_chunk_boundaries() {
        // The SOI starts on the last byte of the first chunk, the EOI lands
        // several chunks later.
        let blob = jpeg(400_000);
        let data = padded(CHUNK - 1, &blob);
        check("boundary", &data, &blob);
    }

    #[test]
    fn scans_non_raf_files_too() {
        let blob = jpeg(42);
        check("plain", &padded(10, &blob), &blob);
    }

    #[test]
    fn missing_preview_is_an_error() {
        let path = write("empty", &vec![0u8; 0x100]);
        assert!(preview(&path).is_err());
        let _ = fs::remove_file(path);
    }
}
