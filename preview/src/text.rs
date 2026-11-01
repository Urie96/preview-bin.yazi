//! Text-ish previews: plain text, JSON, Markdown, CSV/TSV, notebooks,
//! Office documents and HTML (via `pandoc`/`docx2txt`), spreadsheets, plists
//! and `asciinema` recordings.

use std::path::Path;

use crate::cache::{GenError, Kind};
use crate::sys::{self, argv, os};
use crate::Previewer;

impl Previewer {
    // -----------------------------------------------------------------------
    // plain text
    // -----------------------------------------------------------------------

    pub(crate) fn handle_text(&self) -> i32 {
        self.bat_file(&[])
    }

    pub(crate) fn handle_json(&self) -> i32 {
        self.bat_file(&["-l", "json", "--theme=TwoDark"])
    }

    pub(crate) fn handle_raw(&self) -> i32 {
        if let Err(err) = sys::stream_file(&self.file) {
            sys::echo_err(&err, self.width);
        }
        0
    }

    pub(crate) fn handle_md(&self) -> i32 {
        let glow = match sys::which("glow") {
            Some(glow) => glow,
            None => return self.handle_text(),
        };

        let _ = self.cache.with(&self.file, None, Kind::Text, |cache| {
            let args = vec![os("-s"), os("dracula"), self.path_arg()];
            sys::pipeline(&[sys::stage(&glow, args)], Some(cache)).map_err(GenError::Msg)
        });
        0
    }

    pub(crate) fn handle_delimited(&self) -> i32 {
        let mlr = match sys::which("mlr") {
            Some(mlr) => mlr,
            None => return self.handle_text(),
        };
        let args = argv(&[
            "--icsv",
            "--opprint",
            "-C",
            "--key-color",
            "darkcyan",
            "--value-color",
            "grey70",
            "head",
            "-n",
            "500",
        ]);
        // mlr reads the file directly, so the rows stream instead of being
        // loaded into memory first.
        match sys::stream_from_file(&mlr, &args, &self.file) {
            Ok(0) => 0,
            _ => self.handle_text(),
        }
    }

    pub(crate) fn handle_ipynb(&self) -> i32 {
        let nbconvert = sys::which("jupyter-nbconvert");
        let nbpreview = sys::which("nbpreview");
        if nbconvert.is_none() && nbpreview.is_none() {
            return self.handle_json();
        }

        let _ = self.cache.with(&self.file, None, Kind::Text, |cache| {
            if let Some(nb) = nbconvert.as_ref() {
                let args = vec![os("--to"), os("markdown"), os("--stdout"), self.path_arg()];
                let mut stages = vec![sys::stage(nb, args)];
                if let Some(glow) = sys::which("glow") {
                    stages.push(sys::stage(&glow, argv(&["-s", "dracula", "-"])));
                }
                sys::pipeline(&stages, Some(cache)).map_err(GenError::Msg)
            } else {
                let nb = nbpreview.as_ref().expect("checked above");
                let mut args = argv(&[
                    "--no-paging",
                    "--nerd-font",
                    "--decorated",
                    "--no-files",
                    "--unicode",
                    "--color",
                    "--images",
                    "--color-system=standard",
                    "--theme=ansi_dark",
                ]);
                args.push(self.path_arg());
                sys::pipeline(&[sys::stage(nb, args)], Some(cache)).map_err(GenError::Msg)
            }
        });
        0
    }

    // -----------------------------------------------------------------------
    // office documents
    // -----------------------------------------------------------------------

    pub(crate) fn handle_docx(&self) -> i32 {
        let pandoc = match sys::which("pandoc") {
            Some(pandoc) => pandoc,
            None => return self.handle_doc(),
        };

        let _ = self.cache.with(&self.file, None, Kind::Text, |cache| {
            let mut args = argv(&["-s", "-t", "markdown", "--"]);
            args.push(self.path_arg());
            let mut stages = vec![sys::stage(&pandoc, args)];
            if let Some(glow) = sys::which("glow") {
                stages.push(sys::stage(&glow, argv(&["-s", "dracula", "-"])));
            }
            sys::pipeline(&stages, Some(cache)).map_err(GenError::Msg)
        });
        0
    }

    pub(crate) fn handle_doc(&self) -> i32 {
        let _ = self.cache.with(&self.file, None, Kind::Text, |cache| {
            if let Some(docx2txt) = sys::which("docx2txt") {
                let args = vec![self.path_arg(), os("-")];
                sys::pipeline(&[sys::stage(&docx2txt, args)], Some(cache)).map_err(GenError::Msg)
            } else if let Some(textutil) = sys::which("textutil") {
                let args = vec![os("-stdout"), os("-cat"), os("txt"), self.path_arg()];
                sys::pipeline(&[sys::stage(&textutil, args)], Some(cache)).map_err(GenError::Msg)
            } else {
                Err(GenError::Msg("need docx2txt or textutil".into()))
            }
        });
        0
    }

    pub(crate) fn handle_table(&self, converter: &str) -> i32 {
        let tool = match sys::which(converter) {
            Some(tool) => tool,
            None => {
                sys::echo_err(&format!("{converter}: command not found"), self.width);
                return 0;
            }
        };

        let _ = self.cache.with(&self.file, None, Kind::Text, |cache| {
            let args = vec![os("--"), self.path_arg()];
            let mut stages = vec![sys::stage(&tool, args)];
            if let Some(head) = sys::which("head") {
                stages.push(sys::stage(&head, argv(&["-n", "500"])));
            }
            if let Some(mlr) = sys::which("mlr") {
                stages.push(sys::stage(
                    &mlr,
                    argv(&[
                        "--icsv",
                        "--opprint",
                        "-C",
                        "--key-color",
                        "darkcyan",
                        "--value-color",
                        "grey70",
                        "head",
                        "-n",
                        "500",
                    ]),
                ));
            }
            sys::pipeline(&stages, Some(cache)).map_err(GenError::Msg)
        });
        0
    }

    // -----------------------------------------------------------------------
    // plists and recordings
    // -----------------------------------------------------------------------

    pub(crate) fn handle_plist(&self) -> i32 {
        let plist_buddy = Path::new("/usr/libexec/PlistBuddy");
        if sys::is_executable(plist_buddy) {
            let args = vec![os("-c"), os("print"), self.path_arg()];
            self.pipe_to_bat(plist_buddy, args, &["-l", "nix"]);
            0
        } else {
            self.handle_text()
        }
    }

    pub(crate) fn handle_cast(&self) -> i32 {
        match sys::which("asciinema") {
            Some(tool) => {
                let args = vec![os("cat"), self.path_arg()];
                self.run_simple(&tool, &args)
            }
            None => {
                sys::echo_err("asciinema: command not found", self.width);
                0
            }
        }
    }
}
