# ChangeLog

## Unreleased

### Fixed
- builds with Rust 1.86, the minimum supported version that
  `rust-version` declares (it needed 1.89 so far); CI now checks that

## 0.1.0 (2026-10-17)

### Added
- first release: record a terminal session (or render an asciicast v2/v3
  file) as a GIF, on GNU/Linux and Windows
- prebuilt binaries on GitHub, and packages on crates.io (`term2gif`) and
  PyPI (`term2gif-rs`)
