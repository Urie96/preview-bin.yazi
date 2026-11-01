//! Archives and other binary listings: compression formats, shared
//! libraries, SQLite schemas, DEX class lists and torrent metadata.

use crate::cache::{GenError, Kind};
use crate::sys::{self, argv, os};
use crate::Previewer;

impl Previewer {
    pub(crate) fn archive_listing(&self) -> Result<Vec<u8>, String> {
        let mut errors = Vec::new();

        if let Some(bsdtar) = sys::which("bsdtar") {
            let args = vec![os("--list"), os("--file"), self.path_arg()];
            match sys::stdout_of(&bsdtar, &args) {
                Ok(data) => return Ok(data),
                Err(err) => errors.push(err),
            }
        }

        if let Some(lsar) = sys::which("lsar") {
            let args = vec![self.path_arg()];
            match sys::stdout_of(&lsar, &args) {
                Ok(data) => return Ok(sys::drop_first_line(&data).to_vec()),
                Err(err) => errors.push(err),
            }
        }

        if errors.is_empty() {
            Err("need bsdtar or lsar to list archives".to_string())
        } else {
            Err(errors.join("\n"))
        }
    }

    pub(crate) fn compress_bytes(&self) -> Result<Vec<u8>, String> {
        let listing = self.archive_listing()?;
        match sys::which("tree") {
            Some(tree) => {
                let args = argv(&["-C", "--fromfile", "."]);
                sys::stdout_with_stdin(&tree, &args, &listing)
            }
            None => Ok(listing),
        }
    }

    pub(crate) fn handle_compress(&self) -> i32 {
        match self.archive_listing() {
            Ok(listing) => match sys::which("tree") {
                Some(tree) => {
                    if let Err(err) =
                        sys::stream_with_stdin(&tree, &argv(&["-C", "--fromfile", "."]), &listing)
                    {
                        sys::echo_err(&err, self.width);
                    }
                }
                None => sys::write_stdout(&listing),
            },
            Err(err) => sys::echo_err(&err, self.width),
        }
        0
    }

    pub(crate) fn handle_bin(&self) -> i32 {
        if let Some(ldd) = sys::which("ldd") {
            let args = vec![self.path_arg()];
            return self.run_simple(&ldd, &args);
        }
        if let Some(otool) = sys::which("otool") {
            let args = vec![os("-L"), self.path_arg()];
            return self.run_simple(&otool, &args);
        }
        if let Some(nm) = sys::which("nm") {
            let args = vec![self.path_arg()];
            if let Ok(out) = sys::run(&nm, &args) {
                if out.status.success() {
                    sys::write_stdout(&out.stdout);
                    return 0;
                }
            }
            let args = vec![os("-D"), self.path_arg()];
            return self.run_simple(&nm, &args);
        }
        sys::echo_err("need ldd, otool or nm", self.width);
        0
    }

    pub(crate) fn preview_sqlite3(&self) -> i32 {
        let sqlite = match sys::which("sqlite3") {
            Some(sqlite) => sqlite,
            None => {
                sys::echo_err("sqlite3: command not found", self.width);
                return 0;
            }
        };
        let uri = format!("file:{}?immutable=1", self.file.display());
        let args = vec![os("-readonly"), os(&uri), os(".schema")];

        match sys::which("sed") {
            Some(sed) => {
                let mut stages = vec![
                    sys::stage(&sqlite, args),
                    sys::stage(&sed, vec![os("s/;/;\\n/g")]),
                ];
                if let Some(bat) = sys::which("bat") {
                    let mut bat_args = self.bat_common();
                    bat_args.extend(argv(&["-l", "sql"]));
                    bat_args.push(os("-"));
                    stages.push(sys::stage(&bat, bat_args));
                }
                if let Err(err) = sys::pipeline(&stages, None) {
                    sys::echo_err(&err, self.width);
                }
            }
            None => match sys::stdout_of(&sqlite, &args) {
                Ok(data) => {
                    let text = String::from_utf8_lossy(&data).replace(';', ";\n");
                    self.bat_pipe(text.as_bytes(), &["-l", "sql"]);
                }
                Err(err) => sys::echo_err(&err, self.width),
            },
        }
        0
    }

    pub(crate) fn handle_dex(&self) -> i32 {
        let baksmali = match sys::which("baksmali") {
            Some(baksmali) => baksmali,
            None => {
                sys::echo_err("baksmali: command not found", self.width);
                return 0;
            }
        };

        let _ = self.cache.with(&self.file, None, Kind::Text, |cache| {
            let args = vec![os("list"), os("class"), self.path_arg()];
            let mut stages = vec![sys::stage(&baksmali, args)];
            if let Some(tree) = sys::which("tree") {
                stages.push(sys::stage(&tree, argv(&["-C", "--fromfile", "."])));
            }
            sys::pipeline(&stages, Some(cache)).map_err(GenError::Msg)
        });
        0
    }

    pub(crate) fn handle_torrent(&self) -> i32 {
        match sys::which("transmission-show") {
            Some(tool) => {
                let args = vec![os("--"), self.path_arg()];
                self.run_simple(&tool, &args)
            }
            None => {
                sys::echo_err("transmission-show: command not found", self.width);
                0
            }
        }
    }
}
