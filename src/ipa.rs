//! iOS `.ipa` previews.
//!
//! The icon and `Info.plist` are pulled straight out of `Payload/*.app` with
//! `unzip`; the icon goes through `sips` so any embedded PNG variant is
//! normalized before it reaches the cache.

use std::fs;

use crate::apple;
use crate::cache::{write_cache, GenError, Kind};
use crate::sys::{self, os};
use crate::Previewer;

impl Previewer {
    pub(crate) fn handle_ipa(&self) -> i32 {
        let _ = self
            .cache
            .with(&self.file, Some("png"), Kind::Image, |cache| {
                let unzip = sys::which("unzip")
                    .ok_or_else(|| GenError::Msg("unzip: command not found".into()))?;
                let listing = sys::stdout_of(&unzip, &[os("-q"), os("-l"), self.path_arg()])?;
                let Some(icon) = payload_entry(&listing, |path| {
                    path.contains(".app/AppIcon") && path.ends_with(".png")
                }) else {
                    return Ok(());
                };

                let tmp = sys::temp_path("png");
                let data =
                    sys::stdout_of(&unzip, &[os("-p"), os("-j"), self.path_arg(), os(&icon)])?;
                sys::write_file(&tmp, &data)?;

                let sips = sys::which("sips")
                    .ok_or_else(|| GenError::Msg("sips: command not found".into()))?;
                let args = vec![
                    os("-s"),
                    os("format"),
                    os("png"),
                    tmp.as_os_str().to_owned(),
                    os("--out"),
                    cache.as_os_str().to_owned(),
                ];
                let result = sys::status_checked(&sips, &args);
                let _ = fs::remove_file(&tmp);
                result.map_err(GenError::Msg)
            });

        if self.cache.image_present(&self.file, Some("png")) {
            return 0;
        }

        let _ = self.cache.with(&self.file, None, Kind::Text, |cache| {
            let unzip = sys::which("unzip")
                .ok_or_else(|| GenError::Msg("unzip: command not found".into()))?;
            let listing = sys::stdout_of(&unzip, &[os("-q"), os("-l"), self.path_arg()])?;

            if let Some(plist) = payload_entry(&listing, |path| path.ends_with(".app/Info.plist")) {
                let tmp = sys::temp_path("plist");
                let data =
                    sys::stdout_of(&unzip, &[os("-p"), os("-j"), self.path_arg(), os(&plist)])?;
                sys::write_file(&tmp, &data)?;
                let info = apple::app_info(self, &tmp);
                let _ = fs::remove_file(&tmp);
                write_cache(cache, &info?)
            } else {
                let data = self.compress_bytes()?;
                write_cache(cache, &data)
            }
        });
        0
    }
}

/// Find an archive entry path starting at `Payload/`.
fn payload_entry<F>(listing: &[u8], predicate: F) -> Option<String>
where
    F: Fn(&str) -> bool,
{
    for line in String::from_utf8_lossy(listing).lines() {
        if let Some(idx) = line.find("Payload/") {
            let candidate = line[idx..].trim();
            if predicate(candidate) {
                return Some(candidate.to_string());
            }
        }
    }
    None
}
