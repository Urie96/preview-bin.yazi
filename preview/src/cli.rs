//! Command line: `<FILE>` plus `--mime` / `--width` / `--offset` / `--test`.

use std::path::PathBuf;

const USAGE: &str = "\
preview - file previewer for yazi

USAGE:
    preview [OPTIONS] <FILE>

OPTIONS:
    <FILE>            file to preview (required)
    --mime <MIME>     MIME type provided by the caller; without it, `file(1)`
                      is asked for one
    --width <N>       preview width in columns; when omitted, no wrapping is done
    --height <N>      accepted for compatibility; currently unused
    --offset <N>      page / thumbnail offset (default 0)
    --test            never generate, only reuse existing cache entries
    -h, --help        show this help
    -V, --version     show version, then exit
";

pub(crate) struct Args {
    pub(crate) path: PathBuf,
    pub(crate) mime: Option<String>,
    pub(crate) width: Option<usize>,
    pub(crate) height: Option<usize>,
    pub(crate) offset: i64,
    pub(crate) test: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            mime: None,
            width: None,
            height: None,
            offset: 0,
            test: false,
        }
    }
}

pub(crate) fn parse_args() -> Args {
    let mut args = Args::default();
    let mut it = std::env::args_os().skip(1);

    while let Some(raw) = it.next() {
        let flag = raw.to_string_lossy().into_owned();
        match flag.as_str() {
            "--path" => {
                if let Some(value) = it.next() {
                    args.path = PathBuf::from(value);
                }
            }
            "--mime" | "--mime-type" => {
                if let Some(value) = it.next() {
                    args.mime = Some(value.to_string_lossy().into_owned());
                }
            }
            "--width" => {
                if let Some(value) = it.next() {
                    args.width = value.to_string_lossy().parse().ok();
                }
            }
            "--height" => {
                if let Some(value) = it.next() {
                    args.height = value.to_string_lossy().parse().ok();
                }
            }
            "--offset" => {
                if let Some(value) = it.next() {
                    args.offset = value.to_string_lossy().parse().unwrap_or(0);
                }
            }
            "--test" => args.test = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("preview {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            _ => {
                // Anything that is not a flag is the file to preview.
                if !flag.starts_with('-') {
                    args.path = PathBuf::from(raw);
                }
            }
        }
    }

    args
}
