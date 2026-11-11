//! Rendering images as ANSI text with `chafa`.
//!
//! Images are normally announced with `@@image:<path>` and drawn by Yazi
//! through whatever terminal image protocol it picked (kitty graphics, sixel,
//! iTerm2 inline, …). Over SSH those protocols are slow: the encoded image is
//! pushed through the link on every hover.
//!
//! Coloured unicode blocks are plain text, so they travel cheaply and can be
//! cached like any other text preview (see `Cache::with_ansi`).

use std::path::Path;

use crate::cache::GenError;
use crate::sys::{self, argv, os};

/// Whether images should be rendered as text instead of announced as
/// `@@image:`.
///
/// `YAZI_CHAFA_PREVIEW` overrides the detection: `1` always renders with chafa,
/// `0` never does.
pub fn enabled() -> bool {
    match std::env::var("YAZI_CHAFA_PREVIEW").as_deref() {
        Ok("0") => false,
        Ok("1") => true,
        _ => in_ssh(),
    }
}

/// SSH sessions, i.e. a link slow enough that image protocols hurt.
fn in_ssh() -> bool {
    ["SSH_CONNECTION", "SSH_TTY", "SSH_CLIENT"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

/// Render `src` into `out` as unicode blocks fitting `width` x `height` cells.
///
/// The flags mirror Yazi's own `chafa` driver so both look the same.
pub fn render(
    src: &Path,
    width: Option<usize>,
    height: Option<usize>,
    out: &Path,
) -> Result<(), GenError> {
    let chafa =
        sys::which("chafa").ok_or_else(|| GenError::Msg("chafa: command not found".into()))?;

    let mut args = argv(&[
        "-f",
        "symbols",
        "--relative",
        "off",
        "--probe",
        "off",
        "--polite",
        "on",
        "--passthrough",
        "none",
        "--animate",
        "off",
    ]);
    if let Some(width) = width {
        // chafa omits the height when it is given as `WIDTHx`.
        let size = match height {
            Some(height) => format!("{width}x{height}"),
            None => format!("{width}x"),
        };
        args.push(os("--view-size"));
        args.push(os(&size));
    }
    args.push(src.as_os_str().to_owned());

    sys::write_stdout_to(&chafa, &args, out).map_err(GenError::Msg)
}
