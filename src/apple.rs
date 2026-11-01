//! `Info.plist` helpers shared by the `.ipa` and `.dmg` previews.
//!
//! An app bundle on either platform is a directory containing
//! `Contents/Info.plist` (macOS) or `Info.plist` (iOS payload); both are read
//! with `/usr/libexec/PlistBuddy` and rendered through `bat`.

use std::fs;
use std::path::Path;

use crate::sys::{self, os};
use crate::Previewer;

/// `/usr/libexec/PlistBuddy`, when the platform provides it.
fn plist_buddy() -> Option<&'static Path> {
    let path = Path::new("/usr/libexec/PlistBuddy");
    sys::is_executable(path).then_some(path)
}

/// The `CFBundle*` keys worth showing, highlighted as ini.
pub fn app_info(previewer: &Previewer, plist: &Path) -> Result<Vec<u8>, String> {
    let buddy = plist_buddy().ok_or("/usr/libexec/PlistBuddy: not available")?;
    let args = vec![os("-c"), os("Print"), plist.as_os_str().to_owned()];
    let out = sys::stdout_of(buddy, &args)?;
    let text = String::from_utf8_lossy(&out);

    let keys = [
        "CFBundleIdentifier",
        "CFBundleDisplayName",
        "CFBundleName",
        "CFBundleShortVersionString",
    ];
    let filtered: Vec<&str> = text
        .lines()
        .filter(|line| keys.iter().any(|key| line.contains(key)))
        .map(|line| line.trim_start())
        .collect();

    Ok(previewer.bat_capture(filtered.join("\n").as_bytes(), &["-l", "ini"]))
}

/// The bundle icon named by `plist`, converted to PNG with `sips`.
pub fn app_icon(plist: &Path) -> Result<Vec<u8>, String> {
    let buddy = plist_buddy().ok_or("/usr/libexec/PlistBuddy: not available")?;
    let args = vec![
        os("-c"),
        os("Print :CFBundleIconFile"),
        plist.as_os_str().to_owned(),
    ];
    let name = String::from_utf8_lossy(&sys::stdout_of(buddy, &args)?)
        .trim()
        .to_string();

    let resources = plist
        .parent()
        .map(|dir| dir.join("Resources"))
        .ok_or("cannot locate app Resources directory")?;
    let icon = fs::read_dir(&resources)
        .map_err(|e| format!("{}: {e}", resources.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .map(|file_name| file_name.to_string_lossy().starts_with(&name))
                .unwrap_or(false)
        })
        .ok_or_else(|| format!("icon {name} not found in {}", resources.display()))?;

    let sips = sys::which("sips").ok_or("sips: command not found")?;
    let tmp = sys::temp_path("png");
    let args = vec![
        os("-s"),
        os("format"),
        os("png"),
        icon.into_os_string(),
        os("--out"),
        tmp.as_os_str().to_owned(),
    ];
    sys::status_checked(&sips, &args)?;

    let data = fs::read(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let _ = fs::remove_file(&tmp);
    Ok(data)
}
