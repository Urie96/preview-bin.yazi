//! macOS `.dmg` previews.
//!
//! The image is attached with `hdiutil`, the first `.app` at the top level is
//! described from its `Contents/Info.plist` (plus icon), and the image is
//! detached again. Images without an app fall back to a two-level `tree`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::apple;
use crate::cache::{write_cache, Kind};
use crate::sys::{self, os};
use crate::Previewer;

/// Everything a `.dmg` preview needs, loaded once per process.
struct DmgInfo {
    info: Vec<u8>,
    icon: Option<Vec<u8>>,
}

impl Previewer {
    pub(crate) fn handle_dmg(&self) -> i32 {
        let mut cell: Option<Result<DmgInfo, String>> = None;

        let _ = self
            .cache
            .with(&self.file, Some("png"), Kind::Image, |cache| {
                let info = self.ensure_dmg(&mut cell)?;
                match info.icon.as_ref() {
                    Some(icon) => write_cache(cache, icon),
                    None => Ok(()),
                }
            });
        if self.cache.image_present(&self.file, Some("png")) {
            return 0;
        }
        let _ = self.cache.with(&self.file, None, Kind::Text, |cache| {
            let info = self.ensure_dmg(&mut cell)?;
            write_cache(cache, &info.info)
        });
        0
    }

    fn ensure_dmg<'a>(
        &self,
        cell: &'a mut Option<Result<DmgInfo, String>>,
    ) -> Result<&'a DmgInfo, String> {
        if cell.is_none() {
            *cell = Some(self.load_dmg_info());
        }
        match cell.as_ref().expect("just initialised") {
            Ok(info) => Ok(info),
            Err(err) => Err(err.clone()),
        }
    }

    fn load_dmg_info(&self) -> Result<DmgInfo, String> {
        let hdiutil = sys::which("hdiutil").ok_or("hdiutil: command not found")?;
        let out = sys::stdout_of(&hdiutil, &[os("attach"), self.path_arg()])?;
        let text = String::from_utf8_lossy(&out);
        let mount = text
            .lines()
            .find_map(|line| {
                line.find("/Volumes/")
                    .map(|idx| line[idx..].trim().to_string())
            })
            .ok_or_else(|| "hdiutil attach: no /Volumes path in output".to_string())?;

        let contents = self.dmg_contents(Path::new(&mount));
        let _ = sys::status_of(&hdiutil, &[os("detach"), os(&mount)]);
        contents
    }

    fn dmg_contents(&self, mount: &Path) -> Result<DmgInfo, String> {
        let mut apps: Vec<PathBuf> = fs::read_dir(mount)
            .map_err(|e| format!("{}: {e}", mount.display()))?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_dir()
                    && path
                        .extension()
                        .map(|ext| ext.eq_ignore_ascii_case("app"))
                        .unwrap_or(false)
            })
            .collect();
        apps.sort();

        for app in apps {
            let plist = app.join("Contents/Info.plist");
            if plist.is_file() {
                let info = apple::app_info(self, &plist)?;
                let icon = apple::app_icon(&plist).ok();
                return Ok(DmgInfo { info, icon });
            }
        }

        let mut info = self.bat_capture(
            b"# app directory not found, fallback to tree two levels of depth:",
            &["-l", "sh"],
        );
        if let Some(tree) = sys::which("tree") {
            let args = vec![os("-C"), os("-L"), os("2"), mount.as_os_str().to_owned()];
            info.extend_from_slice(&sys::stdout_of(&tree, &args)?);
        }
        Ok(DmgInfo { info, icon: None })
    }
}
