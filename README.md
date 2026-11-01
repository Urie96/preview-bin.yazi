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

## License

This plugin is MIT-licensed. For more information check the [LICENSE](LICENSE) file.
