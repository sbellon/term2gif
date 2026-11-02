# term2gif

Record a terminal session and save it as a GIF, in a single binary (`term2gif`)
for GNU/Linux and Windows, with no external tools to install.

```
term2gif [OPTIONS] <OUTPUT.gif>
```

```sh
term2gif demo.gif                                    # record a shell session
term2gif demo.gif -c 'ls -la'                        # record one command
term2gif demo.gif --theme monokai --speed 1.5 --idle-time-limit 2
term2gif demo.gif --keep-cast demo.cast              # also keep the .cast file
term2gif demo.gif --from-cast demo.cast              # render an existing recording
term2gif demo.gif --bitmap-font fixed                # look like xterm's default font
```

Inside xterm (or uxterm), the GIF uses xterm's font automatically (see
below).

End the recording by exiting the shell (`exit` / Ctrl-D).

## Options

Recording:

| Option | Meaning |
|---|---|
| `-c, --command <COMMAND>` | Record this command instead of an interactive shell |
| `--title <TITLE>` | Store a title in the recording |
| `--keep-cast <FILE>` | Also keep the asciicast (`.cast`) recording |
| `--from-cast <FILE>` | Do not record; render an existing asciicast v2 or v3 file |
| `--clear-screen` | Clear the terminal when the recording starts, so that the session starts at the top of the GIF (only has an effect on Windows) |

Rendering and timing:

| Option | Default | Meaning |
|---|---|---|
| `--theme <THEME>` | `asciinema` | `asciinema`, `dracula`, `monokai`, `nord`, `solarized-dark`, `solarized-light`, or 10 / 18 comma-separated hex colors (background, foreground, 8 normal [, 8 bright]) |
| `--font-family <FAMILIES>` | | System font families to prefer over the built-in font, comma-separated |
| `--bitmap-font <NAME>` | | X11 bitmap font as in xterm: `fixed`, `9x15`, …, an XLFD name (wildcards allowed) or a `.pcf` / `.pcf.gz` file; replaces `--font-size` and `--line-height` |
| `--font-size <PX>` | 14 | Font size in pixels |
| `--no-font-detect` | | Inside xterm, use the built-in font instead of xterm's |
| `--line-height <N>` | 1.4 | Line height as a multiple of the font size |
| `--scale <N>` | 1 | Make the GIF N times larger (1–8): bitmap fonts get larger pixels (stays crisp), other fonts a larger font size |
| `--bold-is-bright` | | Draw bold text in the bright variant of its color |
| `--speed <N>` | 1 | Playback speed multiplier |
| `--idle-time-limit <SECS>` | 5 | Shorten pauses to at most this long (0: keep all pauses) |
| `--fps-cap <FPS>` | 30 | Maximum frames per second (1–50) |
| `--last-frame-duration <SECS>` | 3 | How long to show the last frame |
| `--no-loop` | | Play the animation only once |
| `-q, --quiet` | | Print only errors |
| `-v, --verbose` | | Print details, such as the detected xterm font |

## How it works

- **Recording**: the shell or command runs in a PTY via
  [`portable-pty`](https://crates.io/crates/portable-pty) (Unix PTY, Windows
  ConPTY). Its output is shown in your terminal and written to an
  [asciicast v2](https://docs.asciinema.org/manual/asciicast/v2/) file in a
  temporary location, which is deleted afterwards unless you use `--keep-cast`.
- **Rendering**: the recording is replayed through a virtual terminal
  ([`avt`](https://crates.io/crates/avt), the one asciinema uses), and each frame
  is drawn with the built-in DejaVu Sans Mono font
  ([`swash`](https://crates.io/crates/swash) for rasterizing). Box drawing and
  block characters are drawn directly, so lines connect at any line height.
  The GIF is rendered while the recording is going on, so it is usually done
  right after the session ends. Only if the terminal grows beyond its size at
  the start, the recording is rendered once more when it is done.
- **X11 bitmap fonts** (`--bitmap-font`): the classic xterm fonts in PCF
  format, found through the `fonts.dir` / `fonts.alias` files of the X11 font
  directories (on Debian: package `xfonts-base`). As in xterm (uxterm), the
  ISO 10646 version of the font is used, with its bold variant (or overstriking,
  if there is none) and a double-width font of the same height for CJK
  characters (e.g. `12x13ja` for `fixed`). Characters the bitmap fonts lack
  are drawn with the other fonts, scaled to the cell size. On Windows, pass a
  font file.
- **xterm font detection**: when term2gif runs inside xterm (recognized by its
  reply to the XTVERSION query, or an `xterm` parent process) and no font
  option is given, it uses the font xterm uses. It asks xterm for its current
  font (OSC 50; needs `XTerm*allowFontOps: true`, and then also follows
  changes from the font menu), and otherwise takes it from the xterm command
  line (`-fn`, `-fa`, `-fs`, `-class UXTerm`), the X resources
  (`xrdb -query`: `font`, `faceName`, `faceSize`, `renderFont`) or xterm's
  default `fixed`. TrueType fonts (`faceName`) are used at xterm's size, with
  the font's own line height as in xterm.
- **Unicode**: wide characters (CJK, emoji) take two cells. Characters missing
  from the font are taken from any installed font
  ([`fontdb`](https://crates.io/crates/fontdb)); color emoji fonts (e.g. Noto
  Color Emoji, Segoe UI Emoji) are preferred for emoji.
- **Resizing**: window resizes during recording are forwarded to the program
  and recorded. A GIF has one fixed size, so it is as large as the largest
  terminal size during the session; while the terminal is smaller, the unused
  area shows the background color.
- **GIF encoding**: [`gif`](https://crates.io/crates/gif). Each frame stores
  only the changed area, and terminal colors are kept exact.

## Install

- **Prebuilt binary**: download the archive for GNU/Linux (x86_64, static, so
  it runs on any distribution) or Windows (x86_64) from the
  [releases](https://github.com/sbellon/term2gif/releases) and put
  `term2gif` on your `PATH`.
- **With [uv](https://docs.astral.sh/uv/)** (PyPI package `term2gif-rs`, which
  contains the same binary): `uv tool install term2gif-rs`, or without
  installing: `uvx --from term2gif-rs term2gif demo.gif`.
- **With cargo**: `cargo binstall term2gif` (downloads the prebuilt binary) or
  `cargo install --locked term2gif` (builds it, needs Rust 1.86 or newer).

Windows needs Windows 10 version 1809 or newer (ConPTY).

## Build

With Rust 1.86 or newer, straight from GitHub:

```
cargo install --locked --git https://github.com/sbellon/term2gif
```

or from a local checkout:

```
cargo install --locked --path .
```

Either installs `term2gif` into `~/.cargo/bin`. Or build it with
`cargo build --release` and put `target/release/term2gif` (`term2gif.exe` on
Windows) on your `PATH` yourself.

## Checks

The typical Rust flow must stay clean (clippy runs with the `pedantic`
group enabled via `[lints.clippy]` in `Cargo.toml`):

```
cargo fmt --check
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo doc --no-deps --all-features
```

plus a clippy pass for the other platform's code via a cross target
(`x86_64-pc-windows-msvc` from GNU/Linux, `x86_64-unknown-linux-musl` from
Windows), e.g.:

```
cargo clippy --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings
```

A pre-commit hook runs all of it automatically; enable it once per clone with:

```
git config core.hooksPath .githooks
```

## Limitations

- Keystrokes are not captured in the recording (output only).
- Combining characters (e.g. a separate accent after a letter) are dropped by the
  virtual terminal; precomposed characters like `ä` work.
- No ligatures or right-to-left text shaping; each character is drawn in its
  own cell, as in a terminal.

## License

MIT, see [LICENSE](https://github.com/sbellon/term2gif/blob/main/LICENSE). The
bundled DejaVu Sans Mono fonts are under the Bitstream Vera license, see
[fonts/DejaVu-LICENSE.txt](https://github.com/sbellon/term2gif/blob/main/fonts/DejaVu-LICENSE.txt).
Dependencies are under MIT, Apache-2.0 or similar permissive licenses.
