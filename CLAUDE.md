# CLAUDE.md

term2gif records a terminal session (like `asciinema rec`) and renders it as
a GIF (like `agg`), in one binary for GNU/Linux and Windows. Behavior and
usage are documented in README.md; this file covers what matters when
changing it.

## Commits

End AI-assisted commit messages with the trailer `Assisted-By: <model name>`
(e.g. `Assisted-By: Claude Fable 5`), without an email address. Do not use
`Co-Authored-By` trailers or `noreply@` addresses.

## Checks and workflow

Every commit is gated by `.githooks/pre-commit` (enable once per clone with
`git config core.hooksPath .githooks`), which runs the typical Rust flow —
fmt --check, check, clippy `-D warnings` (pedantic group is enabled in
Cargo.toml), test, doc with `RUSTDOCFLAGS="-D warnings"` — plus a cross-target
clippy pass. Run the same commands yourself before committing; they must stay
clean. CI runs check, clippy, build and test on both Ubuntu and Windows.

- The lossy numeric cast lints (`cast_possible_truncation`,
  `cast_possible_wrap`, `cast_precision_loss`, `cast_sign_loss`) are allowed
  per file in the pixel/font modules (`render`, `encode`, `font`, `pcf`,
  `boxdraw`), where such casts are pervasive and intended. Elsewhere fix them
  or allow them on the single item with a reason. Use `From` for lossless
  casts everywhere.
- Proper names that trip `doc_markdown` go into `clippy.toml`.
- Every Rust file starts with an SPDX header (`// SPDX-FileCopyrightText:
  <years> <name>`, `//`, `// SPDX-License-Identifier: MIT`), checked by
  `tests/spdx.rs`. The hook adds it to new `.rs` files and extends the
  committer's year range (`2026` → `2026-2027`) in changed files, then stops
  so the change can be staged. Files that cannot carry a header (the fonts)
  are covered by `REUSE.toml`.
- Cross-target matters: Windows-only code (`winvt` in `src/recorder.rs`) is
  invisible to native checks on Linux, and the Unix parts of `src/xterm.rs`
  on Windows. Lint the other side with
  `cargo clippy --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings`
  (or `x86_64-unknown-linux-musl` from Windows; cargo check/clippy don't
  link, so no cross toolchain is needed).

## Architecture

Recording, then rendering; `--from-cast` skips the recording.

- `src/main.rs` — command line, font selection (options, or xterm detection),
  and the overall flow.
- `src/recorder.rs` — runs the shell/command in a PTY (`portable-pty`: Unix
  PTY, Windows ConPTY), mirrors output to the terminal, writes asciicast v2.
  `src/utf8.rs` decodes PTY output that splits multi-byte characters.
- `src/cast.rs` — reads asciicast v2 and v3.
- `src/convert.rs` — replays events through `avt` (virtual terminal),
  applies idle limit / speed / fps cap, and feeds frames to the encoder.
- `src/render.rs` — draws the terminal state into an RGB canvas (largest
  size of the session; `--scale` enlarges pixels for bitmap fonts).
- `src/font.rs` — bundled DejaVu Sans Mono (`fonts/`), preferred system
  families, X11 bitmap fonts, per-character fallback via `fontdb`, glyph
  rasterization via `swash`. `src/boxdraw.rs` draws box drawing and block
  characters procedurally so they connect at any line height.
- `src/pcf.rs` reads X11 PCF bitmap fonts; `src/xfonts.rs` resolves xterm
  font names (`fonts.alias`, `fonts.dir`, XLFD); `src/xterm.rs` detects the
  font of the xterm term2gif runs in.
- `src/encode.rs` — GIF output (`gif` crate): changed-rectangle frames,
  unchanged pixels transparent, per-frame palette.

## Invariants

- MIT licensed. Dependencies must be permissive (MIT, Apache-2.0, BSD,
  Zlib, …): no GPL/AGPL crates. This is why the project does not use `agg`
  (GPL), `gifski` (AGPL) or `imagequant` (GPL). Bundled fonts need a
  permissive license, kept next to them in `fonts/`.
- A single binary with no external tools to install. `xrdb` is only an
  optional source during xterm font detection.
- Terminal colors stay exact in the GIF: the palette is exact for up to 255
  colors per frame; beyond that the most frequent colors stay exact and the
  rest map to the nearest. No NeuQuant-style per-frame palettes (they make
  colors flicker between frames).
- Font detection must never block or leak into the recorded session: the
  terminal queries end with DA1, which every terminal answers, and are
  bounded by a timeout. Detection only applies inside xterm (XTVERSION reply
  or an `xterm` ancestor process — `XTERM_VERSION` alone is unreliable) and
  when no font option is given.
- Windows is a first-class target: keep the ConPTY path, console mode
  handling and user-facing messages (e.g. "type `exit`") working there.

## Verifying end to end

Unit tests cover the pieces; `tests/cli.rs` renders a small recording with
the real binary. For visual checks, render a test `.cast` with
`--from-cast` (colors, attributes, box drawing, blocks, CJK, emoji, a resize
event) and look at frames, e.g. `convert out.gif -coalesce frame_%02d.png`;
compare `--bitmap-font fixed` (and `--scale 2`) too. A real recording without
an interactive terminal works through `script -qec "term2gif out.gif -c '…'"
/dev/null`. xterm detection can be exercised with a small PTY harness that
answers the XTVERSION/OSC 50/DA1 queries like xterm (or another terminal)
would.
