//! Visual media: images (including SVG, ICNS and RAF), audio cover art and
//! tags, video thumbnails, PDF page thumbnails and EPUB covers.

use std::fs;
use std::path::PathBuf;

use crate::cache::{GenError, Kind};
use crate::sys::{self, argv, os};
use crate::Previewer;
use crate::{epub, raf};

impl Previewer {
    // -----------------------------------------------------------------------
    // images
    // -----------------------------------------------------------------------

    pub(crate) fn handle_image(&self) -> i32 {
        // `@@image:` is terminal: nothing is printed after it.
        let abs = fs::canonicalize(&self.file).unwrap_or_else(|_| self.file.clone());
        println!("@@image:{}", abs.display());
        0
    }

    pub(crate) fn handle_svg(&self) -> i32 {
        let _ = self
            .cache
            .with(&self.file, Some("png"), Kind::Image, |cache| {
                if let Some(rsvg) = sys::which("rsvg-convert") {
                    let mut args = vec![self.path_arg(), os("-o")];
                    args.push(cache.as_os_str().to_owned());
                    sys::status_checked(&rsvg, &args).map_err(GenError::Msg)
                } else if let Some(convert) = sys::which("convert") {
                    let args = vec![self.path_arg(), cache.as_os_str().to_owned()];
                    sys::status_checked(&convert, &args).map_err(GenError::Msg)
                } else {
                    Err(GenError::Msg(
                        "svg: need rsvg-convert or ImageMagick convert".into(),
                    ))
                }
            });
        if self.cache.image_present(&self.file, Some("png")) {
            return 0;
        }
        self.exiftool_yaml(&["-ImageSize", "-*"])
    }

    pub(crate) fn handle_icns(&self) -> i32 {
        let _ = self
            .cache
            .with(&self.file, Some("png"), Kind::Image, |cache| {
                let sips = sys::which("sips")
                    .ok_or_else(|| GenError::Msg("sips: command not found".into()))?;
                let args = vec![
                    os("-s"),
                    os("format"),
                    os("png"),
                    self.path_arg(),
                    os("--out"),
                    cache.as_os_str().to_owned(),
                ];
                sys::status_checked(&sips, &args).map_err(GenError::Msg)
            });
        if self.cache.image_present(&self.file, Some("png")) {
            return 0;
        }
        self.exiftool_yaml(&["-ImageSize", "-*"])
    }

    pub(crate) fn handle_pdf(&self) -> i32 {
        let page_ext = self.offset.to_string();
        let stop = self
            .cache
            .with(&self.file, Some(&page_ext), Kind::Image, |cache| {
                let pdftoppm = sys::which("pdftoppm")
                    .ok_or_else(|| GenError::Msg("pdftoppm: command not found".into()))?;
                let page = (self.offset + 1).to_string();
                let prefix = PathBuf::from(format!("{}.part", cache.display()));
                let args = vec![
                    os("-singlefile"),
                    os("-jpeg"),
                    os("-jpegopt"),
                    os("quality=75"),
                    os("-f"),
                    os(&page),
                    self.path_arg(),
                    prefix.as_os_str().to_owned(),
                ];
                let out = sys::run(&pdftoppm, &args).map_err(GenError::Msg)?;
                if out.status.code() == Some(99) {
                    return Err(GenError::StopPeek);
                }

                // With `-singlefile` pdftoppm appends ".jpg" to the prefix.
                let produced = PathBuf::from(format!("{}.jpg", prefix.display()));
                let result = if produced.is_file() {
                    fs::rename(&produced, cache)
                        .map_err(|e| GenError::Msg(format!("{}: {e}", cache.display())))
                } else {
                    Err(GenError::Msg(sys::err_text(&out)))
                };
                let _ = fs::remove_file(&produced);
                match result {
                    Ok(()) => match sys::note_text(&out) {
                        Some(msg) => Err(GenError::Msg(msg)),
                        None => Ok(()),
                    },
                    Err(err) => Err(err),
                }
            });
        if let Some(code) = stop {
            return code;
        }
        if self.cache.image_present(&self.file, Some(&page_ext)) {
            return 0;
        }

        println!("{}", self.offset);

        if let Some(pdfinfo) = sys::which("pdfinfo") {
            let args = vec![self.path_arg()];
            self.pipe_to_bat(&pdfinfo, args, &["-l", "yaml"]);
        }
        0
    }

    pub(crate) fn handle_raf(&self) -> i32 {
        // Fujifilm RAF: the preview JPEG is extracted in-process, no exiftool.
        let _ = self
            .cache
            .with(&self.file, Some("jpg"), Kind::Image, |cache| {
                let jpeg = raf::preview(&self.file)?;
                sys::write_file(cache, &jpeg)?;
                Ok(())
            });
        0
    }

    // -----------------------------------------------------------------------
    // audio / video
    // -----------------------------------------------------------------------

    pub(crate) fn handle_audio(&self) -> i32 {
        let _ = self.cache.with(&self.file, None, Kind::Image, |cache| {
            let exiftool = sys::which("exiftool")
                .ok_or_else(|| GenError::Msg("exiftool: command not found".into()))?;
            let mut args = self.exiftool_base();
            args.extend([os("-b"), os("-CoverArt"), os("-Picture"), self.path_arg()]);
            sys::write_stdout_to(&exiftool, &args, cache).map_err(GenError::Msg)
        });
        if self.cache.image_present(&self.file, None) {
            return 0;
        }

        self.exiftool_yaml(&[
            "-Title",
            "-SortName",
            "-TitleSort",
            "-TitleSortOrder",
            "-Artist",
            "-SortArtist",
            "-ArtistSort",
            "-PerformerSortOrder",
            "-Album",
            "-SortAlbum",
            "-AlbumSort",
            "-AlbumSortOrder",
            "-AlbumArtist",
            "-SortAlbumArtist",
            "-AlbumArtistSort",
            "-AlbumArtistSortOrder",
            "-Genre",
            "-TrackNumber",
            "-Year",
            "-Duration",
            "-SampleRate",
            "-AudioSampleRate",
            "-AudioBitrate",
            "-AvgBitrate",
            "-Channels",
            "-AudioChannels",
        ])
    }

    pub(crate) fn handle_video(&self) -> i32 {
        if !(0..=9).contains(&self.offset) {
            return 3;
        }

        let offset_ext = self.offset.to_string();
        let stop = self
            .cache
            .with(&self.file, Some(&offset_ext), Kind::Image, |cache| {
                let thumb = sys::which("ffmpegthumbnailer")
                    .ok_or_else(|| GenError::Msg("ffmpegthumbnailer: command not found".into()))?;
                let time = (self.offset * 10).to_string();
                let mut args = argv(&["-q", "6", "-c", "jpeg", "-i"]);
                args.push(self.path_arg());
                args.extend([
                    os("-o"),
                    os("/dev/stdout"),
                    os("-t"),
                    os(&time),
                    os("-s"),
                    os("600"),
                ]);
                sys::write_stdout_to(&thumb, &args, cache).map_err(GenError::Msg)
            });
        if let Some(code) = stop {
            return code;
        }
        if self.cache.image_present(&self.file, Some(&offset_ext)) {
            return 0;
        }

        println!("{}", self.offset);

        match sys::which("ffprobe") {
            Some(probe) => {
                let mut args = argv(&[
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "format=duration,bit_rate:stream=codec_name,width,height,avg_frame_rate,r_frame_rate,display_aspect_ratio,duration:format_tags",
                    "-sexagesimal",
                    "-v",
                    "quiet",
                    "-of",
                    "flat",
                ]);
                args.push(self.path_arg());
                self.pipe_to_bat(&probe, args, &["-l", "ini"]);
            }
            None => {
                self.exiftool_yaml(&["-AvgBitrate", "-ImageSize", "-Video*", "-Media*", "-Audio*"]);
            }
        }
        0
    }

    pub(crate) fn handle_netease(&self) -> i32 {
        let nc2mp3 = match sys::which("nc2mp3") {
            Some(nc2mp3) => nc2mp3,
            None => {
                sys::echo_err("nc2mp3: command not found", self.width);
                return 0;
            }
        };

        let _ = self.cache.with(&self.file, None, Kind::Text, |cache| {
            let args = vec![os("--info"), self.path_arg()];
            let mut stages = vec![sys::stage(&nc2mp3, args)];
            if let Some(bat) = sys::which("bat") {
                let mut bat_args = self.bat_common();
                bat_args.extend(argv(&["-l", "json", "--theme=TwoDark"]));
                bat_args.push(os("-"));
                stages.push(sys::stage(&bat, bat_args));
            }
            sys::pipeline(&stages, Some(cache)).map_err(GenError::Msg)
        });
        0
    }

    // -----------------------------------------------------------------------
    // e-books
    // -----------------------------------------------------------------------

    pub(crate) fn handle_epub(&self) -> i32 {
        let _ = self.cache.with(&self.file, None, Kind::Image, |cache| {
            let book = epub::Epub::open(&self.file)?;
            match book.cover() {
                Some(cover) => {
                    let data = fs::read(&cover)
                        .map_err(|e| GenError::Msg(format!("{}: {e}", cover.display())))?;
                    sys::write_file(cache, &data)?;
                    Ok(())
                }
                None => Ok(()),
            }
        });
        if self.cache.image_present(&self.file, None) {
            return 0;
        }

        let _ = self.cache.with(&self.file, None, Kind::Text, |cache| {
            let book = epub::Epub::open(&self.file)?;
            let mut out = String::new();
            if let Some(title) = book.title() {
                out.push_str(&format!("\x1b[1;36m📖 书名: {title}\x1b[m\n"));
            }
            if let Some(creator) = book.creator() {
                out.push_str(&format!("\x1b[1;36m✍️ 作者: {creator}\x1b[m\n"));
            }
            if let Some(language) = book.language() {
                out.push_str(&format!("\x1b[1;36m🌐 语言: {language}\x1b[m\n"));
            }
            if let Some(description) = book.description() {
                out.push_str(&format!("\x1b[1;36m📝 描述: {description}\x1b[m\n"));
            }
            sys::write_file(cache, out.as_bytes())?;
            Ok(())
        });
        0
    }
}
