# preview-bin.yazi

Use a dedicated `preview` executable as Yazi's previewer.

> **Note:** the plugin directory must **not** be named `preview`, because `preview`
> is a built-in Yazi UI component name. Yazi's plugin loader resolves built-in
> names from its cache before looking in `~/.config/yazi/plugins`, so a plugin
> called `preview` silently loads the empty built-in `preview` component instead
> and previews fail with `Lua error during peek: error converting Lua nil to table`.
> Hence the directory name `preview-bin.yazi` and `run = "preview-bin"`.

The plugin runs the `preview` command from `PATH` with a fixed set of arguments — there is nothing to configure in `yazi.toml` beyond selecting the previewer.

Make sure the `preview` executable is available in your `PATH`:

```sh
command -v preview
```

## Usage

```toml
# ~/.config/yazi/yazi.toml
[[plugin.prepend_previewers]]
url = "*"
run = "preview-bin"
```

The command is invoked as:

```sh
preview --mime <MIME> --width <COLS> --height <ROWS> --offset <SKIP> <PATH>
```

All arguments are hard-coded inside `main.lua`; the previewer accepts no user-provided arguments.

## Output protocol

- Normal output is ANSI text and is displayed in the preview pane.
- If the first output line is `@@image:<absolute-path>`, the plugin asks Yazi to display that image.
- Exit code `3` is treated as "out of bounds" (for example, a PDF page or video thumbnail that doesn't exist), and Yazi steps back to the previous offset.

Scrolling follows the old previewer convention: one skip unit is half a page for text, one page for PDFs, and one thumbnail step for videos.

## Images over SSH

`@@image:` is drawn by Yazi through whatever terminal image protocol it detected
(kitty graphics, sixel, iTerm2 inline, …). Over SSH those are slow: the encoded
image is pushed through the link on every hover.

So inside an SSH session (`SSH_CONNECTION`, `SSH_TTY` or `SSH_CLIENT` set) the
`preview` binary renders images as coloured unicode blocks with [chafa]
instead. Everything that would have been announced with `@@image:` is covered
— images, PDF pages, video thumbnails, RAF/SVG/ICNS conversions — and the blocks
are cached as ANSI text next to the image caches, so revisiting a file costs a
plain file read.

| Variable | Effect |
| --- | --- |
| `YAZI_CHAFA_PREVIEW=1` | Always render with chafa, even locally |
| `YAZI_CHAFA_PREVIEW=0` | Always use the terminal image protocol, even over SSH |

Needs [chafa] ≥ 1.16 in `PATH`. Without it (or if it fails) the error is
reported and the image is announced with `@@image:` as usual, so nothing breaks.

[chafa]: https://hpjansson.org/chafa/

## License

This plugin is MIT-licensed. For more information check the [LICENSE](LICENSE) file.
