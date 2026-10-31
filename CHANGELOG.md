# ChangeLog

## Unreleased

### Added
- `--clear-screen` clears the terminal when the recording starts, so that
  on Windows the session starts at the top of the GIF instead of on the row
  the cursor was on, with the rows above it empty (elsewhere the session
  starts at the top anyway, and the option has no effect)

## 0.2.0 (2026-10-26)

### Changed
- the GIF is rendered while the session is recorded, so it is usually
  written right after the session ends instead of only then being rendered
  (if the terminal grows beyond its size at the start, the recording is
  still rendered afterwards); the progress bar shows what is left after the
  session
- the output file is created when the recording starts

## 0.1.3 (2026-10-24)

### Fixed
- Ctrl-C while the GIF is rendered stops cleanly: the incomplete GIF is
  removed and the progress bar is cleared before term2gif ends as
  interrupted by Ctrl-C, so a calling shell script stops too (a second
  Ctrl-C exits at once)

## 0.1.2 (2026-10-20)

### Added
- a progress bar with the estimated time left while the GIF is rendered,
  shown on a terminal when that takes longer than a second (not with
  `--quiet`)

## 0.1.1 (2026-10-20)

### Fixed
- builds with Rust 1.86, the minimum supported version that
  `rust-version` declares (it needed 1.89 so far); CI now checks that
- xterm font detection: a face name without the `xft:` prefix, as xterm
  reports it when asked for its font, is no longer mistaken for an X11 core
  font name, and X11 bitmap fonts that xterm draws through fontconfig (e.g.
  `faceName: Misc Fixed:width=87:pixelsize=13`) are used instead of the
  built-in font

## 0.1.0 (2026-10-17)

### Added
- first release: record a terminal session (or render an asciicast v2/v3
  file) as a GIF, on GNU/Linux and Windows
- prebuilt binaries on GitHub, and packages on crates.io (`term2gif`) and
  PyPI (`term2gif-rs`)
