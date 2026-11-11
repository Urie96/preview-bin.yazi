//! `preview` — a file previewer for yazi.
//!
//! Rust port of the original bash script. Differences from the shell version:
//!
//! * image previews are announced with `@@image:<absolute path>` instead of
//!   `__preview__image__path__ <path>`, and nothing is printed after it;
//! * inside an SSH session images are rendered as cached ANSI text with
//!   `chafa` instead (see `chafa.rs`), since terminal image protocols are slow
//!   over a remote link;
//! * Fujifilm RAF previews are extracted in-process (no `exiftool`);
//! * WOFF 1.0 and WOFF2 files are converted to sfnt in-process (no helper scripts);
//! * `__disable_auto_peek__` is gone;
//! * the MIME type comes from `--mime`; when that is absent `file(1)` is
//!   asked, as before;
//! * cache keys are computed in-process (no `ls -l | md5`).
//!
//! `main` only routes an extension or MIME type; each handler lives in its own
//! module (`text`, `media`, `archive`, `font`, `apk`, `ipa`, `dmg`).

mod apk;
mod apple;
mod archive;
mod cache;
mod chafa;
mod cli;
mod dmg;
mod epub;
mod font;
mod ipa;
mod media;
mod preview;
mod raf;
mod sys;
mod text;
mod woff;

use std::fs;
use std::io::Write;

use cli::parse_args;
use preview::Previewer;

fn main() {
    let args = parse_args();
    let width = args.width;

    if args.path.as_os_str().is_empty() || !args.path.is_file() {
        sys::die("<No such file>", width);
    }
    if fs::File::open(&args.path).is_err() {
        sys::die("<Permission denied>", width);
    }

    // Forward Ctrl-C / kill / hangup to the child processes we spawn.
    sys::install_signal_forwarding();

    let code = Previewer::new(args).run();
    let _ = std::io::stdout().flush();
    std::process::exit(code);
}

impl Previewer {
    // -----------------------------------------------------------------------
    // extension dispatch
    // -----------------------------------------------------------------------

    pub(crate) fn handle_extension(&self) -> Option<i32> {
        let ext = self
            .file
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();

        let code = match ext.as_str() {
            "mp3" | "flac" | "wav" | "m4a" => self.handle_audio(),
            "json" => self.handle_json(),
            "pdf" => self.handle_pdf(),
            "toml" | "tmpl" | "xml" | "arsc" => self.handle_text(),
            "md" => self.handle_md(),
            "ipynb" => self.handle_ipynb(),
            "dex" => self.handle_dex(),
            "apk" => self.handle_apk(),
            "csv" | "tsv" => self.handle_delimited(),
            "uc!" => self.handle_netease(),
            "sqlite3" | "sqlite" => self.preview_sqlite3(),
            "so" | "dylib" => self.handle_bin(),
            "torrent" => self.handle_torrent(),
            "doc" => self.handle_doc(),
            "docx" | "htm" | "html" | "xhtml" | "rtf" => self.handle_docx(),
            "xlsx" => self.handle_table("xlsx2csv"),
            "xls" => self.handle_table("xls2csv"),
            "svg" => self.handle_svg(),
            "epub" => self.handle_epub(),
            "plist" => self.handle_plist(),
            "icns" => self.handle_icns(),
            "ipa" => self.handle_ipa(),
            "dmg" => self.handle_dmg(),
            "ttf" | "ttc" | "otf" => self.handle_font(),
            "woff" => self.handle_woff(),
            "woff2" => self.handle_woff2(),
            "ansi" => self.handle_raw(),
            "cast" => self.handle_cast(),
            "raf" => self.handle_raf(),
            "7z" | "rar" | "ace" | "alz" | "arc" | "arj" | "bz" | "bz2" | "cab" | "cpio"
            | "deb" | "gz" | "jar" | "lha" | "lz" | "lzh" | "lzma" | "lzo" | "rpm" | "rz"
            | "t7z" | "tar" | "tbz" | "tbz2" | "tgz" | "tlz" | "txz" | "tz" | "tzo" | "war"
            | "xpi" | "xz" | "z" | "zip" | "nds" | "iso" | "pkg" => self.handle_compress(),
            _ => return None,
        };

        Some(code)
    }

    // -----------------------------------------------------------------------
    // MIME dispatch (type supplied by the caller)
    // -----------------------------------------------------------------------

    pub(crate) fn handle_mime(&self, mime: &str) -> Option<i32> {
        let mime = mime
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();

        let code = if mime.ends_with("archive") || mime.ends_with("zip") {
            self.handle_compress()
        } else if mime.starts_with("text/")
            || mime.ends_with("/xml")
            || mime.ends_with("/javascript")
        {
            self.handle_text()
        } else if mime.ends_with("/json") {
            self.handle_json()
        } else if mime.starts_with("image/") {
            self.handle_image()
        } else if mime.starts_with("video/") {
            self.handle_video()
        } else if mime.starts_with("audio/") {
            self.handle_audio()
        } else if mime.ends_with("/pdf") {
            self.handle_pdf()
        } else if mime.ends_with("/x-sharedlib")
            || mime.ends_with("/x-object")
            || mime.ends_with("/x-mach-binary")
            || mime.ends_with("executable")
        {
            self.handle_bin()
        } else if mime.ends_with("sqlite3") {
            self.preview_sqlite3()
        } else if mime == "application/octet-stream"
            && self
                .file
                .extension()
                .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("ts"))
                .unwrap_or(false)
        {
            self.handle_video()
        } else {
            return None;
        };

        Some(code)
    }
}
