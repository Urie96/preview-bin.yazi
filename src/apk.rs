//! Android `.apk` previews.
//!
//! `aapt dump badging` supplies the label, package name, version and icon
//! path; the icon itself is pulled out of the archive with `unzip`. When `aapt`
//! is unavailable the file is shown as a plain archive listing.

use std::fs;
use std::path::{Path, PathBuf};

use crate::cache::{write_cache, GenError, Kind};
use crate::sys::{self, os};
use crate::Previewer;

impl Previewer {
    pub(crate) fn handle_apk(&self) -> i32 {
        let Some(aapt) = find_aapt() else {
            return self.handle_compress();
        };
        // One `aapt` run feeds both the icon and the metadata below.
        let dump = match badging(&aapt, &self.file) {
            Ok(dump) => dump,
            Err(err) => {
                sys::echo_err(&err, self.width);
                return 0;
            }
        };

        let _ = self
            .cache
            .with(&self.file, Some("icon"), Kind::Image, |cache| {
                let Some(icon) = attr_value(application_line(&dump), "icon") else {
                    return Ok(());
                };
                if icon.rsplit('.').next() == Some("xml") {
                    // Adaptive icons are not a single bitmap.
                    return Ok(());
                }

                let unzip = sys::which("unzip")
                    .ok_or_else(|| GenError::Msg("unzip: command not found".into()))?;
                let data =
                    sys::stdout_of(&unzip, &[os("-p"), os("-j"), self.path_arg(), os(&icon)])?;
                sys::write_file(cache, &data)?;
                Ok(())
            });

        if self.cache.image_present(&self.file, Some("icon")) {
            return 0;
        }

        let _ = self.cache.with(&self.file, None, Kind::Text, |cache| {
            let package = dump.lines().next().unwrap_or("");
            let application = application_line(&dump);
            let name = attr_value(package, "name").unwrap_or_default();
            let version = attr_value(package, "versionName").unwrap_or_default();
            let label = attr_value(application, "label").unwrap_or_default();
            let text = format!("App name: {label}\nPackage: {name}\nVersion name: {version}\n");
            let rendered = self.bat_capture(text.as_bytes(), &["-l", "yaml"]);
            write_cache(cache, &rendered)
        });
        0
    }
}

/// The `application: ...` line of `aapt dump badging` output.
fn application_line(dump: &str) -> &str {
    dump.lines()
        .find(|line| line.starts_with("application: "))
        .unwrap_or("")
}

/// `aapt` from `PATH`, else the newest
/// `~/Library/Android/sdk/build-tools/*/aapt`.
fn find_aapt() -> Option<PathBuf> {
    if let Some(found) = sys::which("aapt") {
        return Some(found);
    }
    let base = PathBuf::from(std::env::var_os("HOME")?).join("Library/Android/sdk/build-tools");
    let mut dirs: Vec<PathBuf> = fs::read_dir(base)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    dirs.reverse();
    dirs.into_iter()
        .map(|dir| dir.join("aapt"))
        .find(|candidate| sys::is_executable(candidate))
}

/// `aapt dump badging <file>`.
fn badging(aapt: &Path, file: &Path) -> Result<String, String> {
    let args = vec![os("dump"), os("badging"), file.as_os_str().to_owned()];
    sys::stdout_of(aapt, &args).map(|out| String::from_utf8_lossy(&out).into_owned())
}

/// Extract `key='value'` from an `aapt dump badging` line.
fn attr_value(line: &str, key: &str) -> Option<String> {
    let needle = format!("{key}='");
    let bytes = line.as_bytes();
    let mut search = 0usize;
    while let Some(rel) = line[search..].find(&needle) {
        let at = search + rel;
        let boundary = at == 0 || matches!(bytes[at - 1], b' ' | b'\t' | b'\n' | b'\r');
        let start = at + needle.len();
        if boundary {
            let rest = &line[start..];
            let end = rest.find('\'')?;
            return Some(rest[..end].to_string());
        }
        search = start;
    }
    None
}
