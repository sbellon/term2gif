# ChangeLog

## Unreleased

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
