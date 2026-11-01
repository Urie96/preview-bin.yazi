//! Fonts: rasterize a charset sample with ImageMagick, converting WOFF/WOFF2
//! to sfnt in-process first.

use std::fs;
use std::path::Path;

use crate::cache::{GenError, Kind};
use crate::sys::{self, os};
use crate::woff;
use crate::Previewer;

/// Convert a `Result<(), GenError>` into the `String`-error world.
fn gen_message(result: Result<(), GenError>) -> Result<(), String> {
    match result {
        Ok(()) => Ok(()),
        Err(GenError::Msg(msg)) => Err(msg),
        Err(GenError::StopPeek) => Err("unexpected stop request".to_string()),
    }
}

impl Previewer {
    pub(crate) fn process_ttf(&self, font: &Path, cache: &Path) -> Result<(), GenError> {
        let convert = sys::which("convert")
            .ok_or_else(|| GenError::Msg("convert: command not found".into()))?;
        let charset = "abcdefghijklmnopqrstuvwxyz\nABCDEFGHIJKLMNOPQRSTUVWXYZ\n1234567890\n!@#$%^&*()-_=+[{]}\n\\|;:'\",<.>/?`~";
        let label = format!("label:{charset}");
        let args = vec![
            os("-font"),
            font.as_os_str().to_owned(),
            os("-background"),
            os("black"),
            os("-fill"),
            os("white"),
            os("-pointsize"),
            os("50"),
            os(&label),
            cache.as_os_str().to_owned(),
        ];
        sys::status_checked(&convert, &args).map_err(GenError::Msg)
    }

    pub(crate) fn handle_font(&self) -> i32 {
        let _ = self
            .cache
            .with(&self.file, Some("png"), Kind::Image, |cache| {
                self.process_ttf(&self.file, cache)
            });
        if self.cache.image_present(&self.file, Some("png")) {
            return 0;
        }
        self.exiftool_yaml(&[])
    }

    pub(crate) fn handle_woff(&self) -> i32 {
        self.render_web_font(woff::to_sfnt)
    }

    pub(crate) fn handle_woff2(&self) -> i32 {
        self.render_web_font(woff::woff2_to_sfnt)
    }

    fn render_web_font<F>(&self, convert: F) -> i32
    where
        F: FnOnce(&[u8]) -> Result<Vec<u8>, String>,
    {
        let _ = self
            .cache
            .with(&self.file, Some("png"), Kind::Image, |cache| {
                let source = fs::read(&self.file)
                    .map_err(|e| GenError::Msg(format!("{}: {e}", self.file.display())))?;
                let sfnt = convert(&source).map_err(GenError::Msg)?;

                let tmp = sys::temp_path("otf");
                sys::write_file(&tmp, &sfnt).map_err(GenError::Msg)?;
                let rendered = gen_message(self.process_ttf(&tmp, cache));
                let _ = fs::remove_file(&tmp);
                rendered.map_err(GenError::Msg)
            });
        0
    }
}
