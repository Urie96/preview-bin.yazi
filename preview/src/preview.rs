//! The `Previewer` type: the file being previewed plus the output plumbing
//! every handler builds on (`bat`, `exiftool`, streaming pipelines).

use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::cache::Cache;
use crate::cli::Args;
use crate::sys::{self, argv, os};

/// One preview run: the target file, the caller's options and the cache.
pub(crate) struct Previewer {
    pub(crate) file: PathBuf,
    pub(crate) width: Option<usize>,
    pub(crate) offset: i64,
    pub(crate) mime: Option<String>,
    pub(crate) cache: Cache,
}

impl Previewer {
    pub(crate) fn new(args: Args) -> Self {
        let cache = Cache::new(args.test, args.width);
        Self {
            file: args.path,
            width: args.width,
            offset: args.offset,
            mime: args.mime,
            cache,
        }
    }

    pub(crate) fn run(&self) -> i32 {
        if let Some(code) = self.handle_extension() {
            return code;
        }
        // `--mime` wins; without it the MIME type is asked from `file(1)`, as
        // the original script did.
        let mime = self.mime.clone().or_else(|| detect_mime(&self.file));
        if let Some(mime) = mime.as_deref() {
            if let Some(code) = self.handle_mime(mime) {
                return code;
            }
        }
        self.fallback();
        0
    }

    pub(crate) fn path_arg(&self) -> OsString {
        self.file.as_os_str().to_owned()
    }

    // -----------------------------------------------------------------------
    // output helpers
    // -----------------------------------------------------------------------

    pub(crate) fn bat_common(&self) -> Vec<OsString> {
        let mut args = argv(&[
            "--color=always",
            "--paging=never",
            "--style=plain",
            "--line-range",
            ":5000",
        ]);
        if let Some(width) = self.width {
            args.push(os("--wrap=character"));
            args.push(os("--terminal-width"));
            args.push(os(&width.to_string()));
        }
        args
    }

    /// Highlight a file with bat (plain dump when bat is unavailable).
    ///
    /// bat's stdout/stderr are inherited, so yazi renders the preview while
    /// bat is still running.

    pub(crate) fn bat_file(&self, extra: &[&str]) -> i32 {
        match sys::which("bat") {
            Some(bat) => {
                let mut args = self.bat_common();
                args.extend(extra.iter().map(OsString::from));
                args.push(os("--"));
                args.push(self.path_arg());
                match sys::stream(&bat, &args) {
                    Ok(code) => code,
                    Err(err) => {
                        sys::echo_err(&err, self.width);
                        1
                    }
                }
            }
            None => {
                if let Err(err) = sys::stream_file(&self.file) {
                    sys::echo_err(&err, self.width);
                }
                0
            }
        }
    }

    /// Highlight already-buffered data with bat (the input is buffered, but
    /// bat's output still streams).

    pub(crate) fn bat_pipe(&self, data: &[u8], extra: &[&str]) {
        match sys::which("bat") {
            Some(bat) => {
                let mut args = self.bat_common();
                args.extend(extra.iter().map(OsString::from));
                args.push(os("-"));
                if let Err(err) = sys::stream_with_stdin(&bat, &args, data) {
                    sys::echo_err(&err, self.width);
                }
            }
            None => sys::write_stdout(data),
        }
    }

    /// `producer | bat`, streamed end to end.

    pub(crate) fn pipe_to_bat(
        &self,
        producer: &Path,
        producer_args: Vec<OsString>,
        extra: &[&str],
    ) {
        let mut stages = vec![sys::stage(producer, producer_args)];
        if let Some(bat) = sys::which("bat") {
            let mut args = self.bat_common();
            args.extend(extra.iter().map(OsString::from));
            args.push(os("-"));
            stages.push(sys::stage(&bat, args));
        }
        if let Err(err) = sys::pipeline(&stages, None) {
            sys::echo_err(&err, self.width);
        }
    }

    /// Highlight piped data with bat and return the bytes (for caching).

    pub(crate) fn bat_capture(&self, data: &[u8], extra: &[&str]) -> Vec<u8> {
        match sys::which("bat") {
            Some(bat) => {
                let mut args = self.bat_common();
                args.extend(extra.iter().map(OsString::from));
                args.push(os("-"));
                match sys::run_with_stdin(&bat, &args, data) {
                    Ok(out) if !out.stdout.is_empty() => out.stdout,
                    _ => data.to_vec(),
                }
            }
            None => data.to_vec(),
        }
    }

    pub(crate) fn run_simple(&self, program: &Path, args: &[OsString]) -> i32 {
        match sys::stream(program, args) {
            Ok(code) => code,
            Err(err) => {
                sys::echo_err(&err, self.width);
                1
            }
        }
    }

    pub(crate) fn exiftool_base(&self) -> Vec<OsString> {
        argv(&[
            "--ExifTool*",
            "--Directory",
            "--File*",
            "--ProfileDescription*",
        ])
    }

    pub(crate) fn exiftool_yaml(&self, extra: &[&str]) -> i32 {
        let exiftool = match sys::which("exiftool") {
            Some(exiftool) => exiftool,
            None => {
                sys::echo_err("exiftool: command not found", self.width);
                return 0;
            }
        };
        let mut args = self.exiftool_base();
        args.extend(extra.iter().map(OsString::from));
        args.push(self.path_arg());
        self.pipe_to_bat(&exiftool, args, &["-l", "yaml"]);
        0
    }

    /// Last resort: strip non-printable bytes (C locale `[:print:]`) and fold.
    pub(crate) fn fallback(&self) {
        let mut data = Vec::new();
        if let Ok(mut file) = fs::File::open(&self.file) {
            let mut buffer = vec![0u8; 100_000];
            if let Ok(read) = file.read(&mut buffer) {
                data.extend_from_slice(&buffer[..read]);
            }
        }
        let printable: Vec<u8> = data
            .into_iter()
            .filter(|byte| (0x20..=0x7e).contains(byte))
            .collect();
        let text = String::from_utf8_lossy(&printable);
        match self.width {
            Some(width) => println!("{}", sys::fold(&text, width)),
            None => println!("{text}"),
        }
    }
}

/// `file -bL --mime-type -- <path>`: only consulted when no `--mime` was given.
fn detect_mime(file: &Path) -> Option<String> {
    let file_cmd = sys::which("file")?;
    let args = vec![
        os("-bL"),
        os("--mime-type"),
        os("--"),
        file.as_os_str().to_owned(),
    ];
    let out = sys::stdout_of(&file_cmd, &args).ok()?;
    let mime = String::from_utf8_lossy(&out).trim().to_string();
    (!mime.is_empty()).then_some(mime)
}
