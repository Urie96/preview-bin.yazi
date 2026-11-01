# preview

yazi 的文件预览器，由原来的单文件 Bash 脚本移植为不依赖外部脚本的 Rust 程序；WOFF/WOFF2 解压使用纯 Rust 的 `flate2` / `wuff` crate。

## 构建

```sh
cargo build --release
# 产物：target/release/preview
```

## 与旧脚本的差异

| 项目 | 旧脚本 | 现在 |
| --- | --- | --- |
| 图片通道 | `__preview__image__path__ <path>` | `@@image:<绝对路径>` |
| 图片后续内容 | 图片后继续输出 exiftool 元数据 | `@@image:` 之后直接结束，不再输出文本 |
| 停止自动窥视 | 输出 `__disable_auto_peek__` | 已删除（保留退出码 `3` 的约定） |
| MIME | 总是调用 `file` 探测 | 优先用调用方传入的 `--mime`；未传时回退 `file -bL --mime-type` |
| 缓存键 | `ls -l \| md5` 外部命令 | 进程内 FNV-1a（路径 + 大小 + mtime + inode） |
| PDF 缩略图 | `pdftoppm` 写到当前目录，缓存里没有文件 | 写入临时文件后 rename 进缓存，真正可用 |
| 输出方式 | 子进程 stdout 直接继承，流式 | 同样流式：`bat`/`cat` 继承 stdout，管道逐块流动 |
| RAW（`.raf`） | `exiftool -previewimage -b` | 内置解析：头部 `0x54`/`0x58` 的 offset/length，失败后退化为扫描 `SOI..EOI` |
| WOFF/WOFF2 转换 | 调用 `woff2otf.py` / `woff2_decompress` | 进程内解析并重建 sfnt；WOFF 用 `flate2`，WOFF2 用 `wuff`，并重算校验和 |

原 Bash 脚本保留在 `legacy/preview.bash` 以便对照。

## 用法

```sh
preview [--mime <MIME>] [--width N] [--height N] [--offset N] [--test] <FILE>
```

| 参数 | 说明 |
| --- | --- |
| `<FILE>` | 要预览的文件（位置参数，必填） |
| `--mime <MIME>` | 调用方提供的 MIME 类型，例如 `text/plain; charset=utf-8`、`video/mp4`；参数中的 `; charset=...` 会被忽略。不传时由 `file -bL --mime-type -- <FILE>` 探测 |
| `--mime-type <MIME>` | `--mime` 的别名 |
| `--width <N>` | 预览宽度（列）。不传时不启用 `bat --wrap` / `--terminal-width`，错误信息和兜底文本也不折行 |
| `--height <N>` | 仅为兼容旧接口而接受，当前未使用 |
| `--offset <N>` | PDF 页码 / 视频缩略图序号（默认 0） |
| `--test` | 只读缓存，不生成新缓存 |
| `-h, --help` | 帮助 |
| `-V, --version` | 版本 |

分派顺序：**扩展名 → MIME（`--mime`，缺失时 `file` 探测）→ 原始文本兜底**。扩展名命中后不会再走 MIME，与旧脚本一致。

## 输出协议

- 普通预览内容：ANSI 文本写到 stdout。
- 图片预览：输出一行 `@@image:/绝对/路径` 后立即结束，不再输出后续文本。只有图片没能生成时（例如缺少 `rsvg-convert`、`pdftoppm`）才回退为文本元数据。
- 退出码 `3` 表示越界（PDF 页码超出范围、视频 offset 不在 0–9），供 yazi 停止继续窥视。
- 其他错误：退出码 `255`（文件不存在/不可读）或 `0` + stderr 错误信息。

## 流式输出

- **文本/JSON/ANSI**：`bat` 直接继承 stdout，读到一行就输出一行（`bat` 缺失时用 Rust 内置的分块拷贝）；JSON 仍受 `bat --line-range :5000` 限制。
- **进程管道**：`pandoc | glow`、`exiftool | bat`、`ffprobe | bat`、`sqlite3 | sed | bat`、`xlsx2csv | head | mlr`、`baksmali | tree`、`nc2mp3 | bat` 等由 `sys::pipeline` 直接连接文件描述符，下游边读边写，中间不经过内存缓冲。
- **CSV/TSV**：`mlr` 的 stdin 直接指向文件，不预读整个 CSV。
- **压缩包**：文件列表先由 `bsdtar`/`lsar` 取得，随后 `tree` 的输出直接流到 stdout。
- **缓存类**（Markdown、DOCX、EPUB、字体、缩略图等）：生成阶段必须落盘，但子进程 stdout 直接重定向到缓存文件；命中缓存后 `io::copy` 分块写到 stdout，不整文件读入内存。
- **兜底文本**：最多读 100 KB，过滤不可打印字符后输出。
- **图片路径**：`@@image:` 立即可输出；缓存图片的路径要等生成完成（这是缓存机制本身决定的）。
- 子进程的 stderr 直接继承，错误/警告会即时出现在 yazi 的 stderr 中，不再由本程序折行包装（传了 `--width` 时仍会按该宽度折行）。
- 不传 `--width` 时，缓存键与传了宽度时不同，不会误用另一种宽度生成的缓存。

## 信号与子进程

- **终端 Ctrl-C（SIGINT）**：内核把信号发给整个前台进程组，`preview` 和它 spawn 的子进程（同一个 pgid）都会收到，一起退出。
- **`kill -TERM/-INT/-HUP/-QUIT <preview-pid>`**：`sys::install_signal_forwarding()` 注册的 handler 会先把信号转发给所有已登记的子进程，然后恢复默认动作并重新对自己触发该信号，因此 `preview` 也会按原语义退出。
- **`kill -KILL <preview-pid>`**：SIGKILL 无法捕获。Linux 上每个子进程在 `pre_exec` 里设置了 `PR_SET_PDEATHSIG=SIGTERM`，父进程一死，子进程会收到 SIGTERM（macOS 没有等价机制，只能依赖前面的信号转发）。
- 这一点对 yazi 很重要：yazi 的 `Command` 使用 tokio 的 `kill_on_drop(true)`，取消预览时等价于给 previewer 发 SIGKILL；Linux 上的 PDEATHSIG 能保证 `bat`、`pandoc`、`ffmpegthumbnailer` 等不会变成孤儿进程。
- 孙进程（子进程自己再 fork 出来的进程，例如某些 Jupyter 内核）不在转发/PDEATHSIG 覆盖范围内。

## yazi 集成示例

MIME 可以不传，此时本程序会自己调用 `file` 探测；如果 yazi 已经拿到了 MIME，直接传 `--mime` 能省掉一个进程：

```sh
#!/bin/sh
# preview-wrapper.sh <path> [width] [height] [offset]
path=$1
width=$2
height=$3
offset=${4:-0}

set -- "$path" --offset "$offset"
[ -n "$width" ] && set -- "$@" --width "$width"
[ -n "$height" ] && set -- "$@" --height "$height"

exec /path/to/preview "$@"
```

如果不传 `--width`，`bat` 不会做任何折行，兜底文本也会保持原样（可能是一整行）。

包装脚本现在只是把 yazi 的位置参数转成同样的位置参数，也可以直接调用：

```sh
preview --width 100 --offset 0 /path/to/file
```

如果 wrapper 里还想像旧脚本那样自己探测 MIME，把 `--mime "$(file -bL --mime-type -- "$path")"` 加回去即可，效果等价。

## 需要的外部工具

所有工具都是可选的，缺失时只会影响对应类型，并在 stderr 给出提示：

| 类型 | 工具 |
| --- | --- |
| 文本/代码高亮 | `bat`（缺失时直接输出原文） |
| Markdown | `glow`（缺失时回退 `bat`） |
| CSV/TSV/XLS(X) | `mlr`、`xlsx2csv` / `xls2csv` |
| HTML/DOCX/RTF | `pandoc`、`glow`；回退 `docx2txt` / `textutil` |
| Jupyter | `jupyter-nbconvert` + `glow`，或 `nbpreview` |
| 压缩包 | `bsdtar` 或 `lsar`，再经 `tree` |
| PDF | `pdftoppm`、`pdfinfo` |
| 视频 | `ffmpegthumbnailer`；（无缩略图时）`ffprobe` 或 `exiftool` |
| 音频 | `exiftool`（封面与标签） |
| 图片 | 无（直接输出原文件路径） |
| RAW（`.raf`） | 无（内嵌 JPEG 预览在程序内提取） |
| SVG / ICNS | `rsvg-convert` 或 ImageMagick `convert`、`sips`（失败时回退 `exiftool` 元数据） |
| 字体 | ImageMagick `convert`（失败时回退 `exiftool` 元数据）；WOFF/WOFF2 均在进程内转换为 sfnt |
| ELF / Mach-O | `ldd`、`otool`、`nm` |
| SQLite | `sqlite3` |
| APK | `aapt`、`unzip` |
| IPA / DMG / ICNS / plist | `unzip`、`sips`、`hdiutil`、`PlistBuddy`（macOS） |
| EPUB | `unzip`（OPF 解析和封面提取在程序内完成） |
| DEX | `baksmali`、`tree` |
| 网易云 `.uc!` | `nc2mp3` |
| 种子 | `transmission-show` |
| 录屏 | `asciinema` |

## 缓存

缓存位于 `$TMPDIR/yazi/<16位hex>[.<ext>]`，键为 `--width`、文件路径、大小、mtime、inode 的 FNV-1a 哈希。
缓存文件已存在（且非空）时不会重新生成；`--test` 会完全跳过生成。
