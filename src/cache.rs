//! On-disk preview cache.
//!
//! Cache entries live in `$TMPDIR/yazi/<key>[.<ext>]`, keyed by the file path,
//! size, mtime and inode. This mirrors the original bash implementation
//! (`ls -l | md5`) while avoiding a dependency on `md5`/`sha1sum`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::sys;

/// How a cache entry is presented once it exists.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Emit the cached bytes verbatim (default extension: `.ansi`).
    Text,
    /// Emit `@@image:<path>` pointing at the cached file.
    Image,
}

/// Failure modes for a cache generation closure.
pub enum GenError {
    /// Tell yazi to stop auto-peeking (exit code 3), e.g. page out of range.
    StopPeek,
    /// Print this message on stderr.
    Msg(String),
}

impl From<String> for GenError {
    fn from(msg: String) -> Self {
        GenError::Msg(msg)
    }
}

impl From<&str> for GenError {
    fn from(msg: &str) -> Self {
        GenError::Msg(msg.to_string())
    }
}

pub struct Cache {
    dir: PathBuf,
    test: bool,
    width: Option<usize>,
}

impl Cache {
    pub fn new(test: bool, width: Option<usize>) -> Self {
        Self {
            dir: std::env::temp_dir().join("yazi"),
            test,
            width,
        }
    }

    pub fn path_for(&self, file: &Path, ext: Option<&str>) -> PathBuf {
        let mut name = cache_key(file, self.width);
        if let Some(ext) = ext {
            let ext = ext.trim_start_matches('.');
            if !ext.is_empty() {
                name.push('.');
                name.push_str(ext);
            }
        }
        self.dir.join(name)
    }

    /// True when the image cache entry for `file` exists and is non-empty,
    /// i.e. `with(.., Kind::Image, ..)` would have emitted `@@image:`.
    pub fn image_present(&self, file: &Path, ext: Option<&str>) -> bool {
        fs::metadata(self.path_for(file, ext))
            .map(|meta| meta.len() > 0)
            .unwrap_or(false)
    }

    /// Ensure a cache entry exists, then present it according to `kind`.
    ///
    /// Returns `Some(3)` when the generator asked to stop peeking.
    pub fn with<F>(&self, file: &Path, ext: Option<&str>, kind: Kind, generate: F) -> Option<i32>
    where
        F: FnOnce(&Path) -> Result<(), GenError>,
    {
        let ext = match (kind, ext) {
            (Kind::Text, None) => Some("ansi"),
            _ => ext,
        };
        let path = self.path_for(file, ext);

        if !path.exists() && !self.test {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            match generate(&path) {
                Ok(()) => {}
                Err(GenError::StopPeek) => return Some(3),
                Err(GenError::Msg(msg)) => {
                    if !msg.trim().is_empty() {
                        sys::echo_err(&msg, self.width);
                    }
                }
            }
        }

        match kind {
            Kind::Text => {
                // Stream the cache entry out instead of loading it into memory.
                if let Ok(meta) = fs::metadata(&path) {
                    if meta.len() > 0 {
                        let _ = sys::stream_file(&path);
                    }
                }
            }
            Kind::Image => {
                let non_empty = fs::metadata(&path).map(|m| m.len() > 0).unwrap_or(false);
                if non_empty {
                    let abs = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                    println!("@@image:{}", abs.display());
                }
            }
        }
        None
    }
}

fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// Stable cache key: FNV-1a over width + path + size + mtime + inode.
fn cache_key(file: &Path, width: Option<usize>) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;

    // Bump this tag when the rendering rules change.
    fnv1a(&mut hash, b"preview-v2");
    match width {
        Some(width) => {
            fnv1a(&mut hash, b"w");
            fnv1a(&mut hash, &(width as u64).to_le_bytes());
        }
        None => fnv1a(&mut hash, b"W"),
    }

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        fnv1a(&mut hash, file.as_os_str().as_bytes());
    }
    #[cfg(not(unix))]
    {
        fnv1a(&mut hash, file.to_string_lossy().as_bytes());
    }

    if let Ok(meta) = fs::metadata(file) {
        fnv1a(&mut hash, &meta.len().to_le_bytes());
        if let Ok(modified) = meta.modified() {
            if let Ok(since) = modified.duration_since(UNIX_EPOCH) {
                fnv1a(&mut hash, &since.as_nanos().to_le_bytes());
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            fnv1a(&mut hash, &meta.ino().to_le_bytes());
        }
    }

    format!("{hash:016x}")
}

/// Cache-generation tail: write bytes, mapping I/O errors into `GenError`.
pub fn write_cache(cache: &Path, data: &[u8]) -> Result<(), GenError> {
    sys::write_file(cache, data).map_err(GenError::Msg)
}
