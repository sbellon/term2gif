// SPDX-FileCopyrightText: 2026 Stefan Bellon
//
// SPDX-License-Identifier: MIT

//! Detecting the font of the xterm (or uxterm) that term2gif runs in, so the
//! GIF looks like the terminal.
//!
//! Sources, in xterm's own order of precedence:
//! 1. the font in use, queried with OSC 50 (needs `allowFontOps: true`;
//!    also reflects changes made with the font menu),
//! 2. the xterm process's command line (`-fn`, `-fa`, `-fs`, `-class`),
//! 3. the X resources (`xrdb -query`),
//! 4. xterm's built-in default `fixed`.

// There is no xterm on Windows; only `detect` is used there.
#![cfg_attr(not(unix), allow(dead_code))]

/// The usual X resolution, if Xft.dpi is not set.
const DEFAULT_DPI: f32 = 96.0;
/// xterm's default `faceSize` in points.
const DEFAULT_FACE_SIZE: f32 = 8.0;

#[derive(Debug, PartialEq)]
pub enum XtermFont {
    /// An X11 core font name (alias, XLFD or pattern).
    Bitmap(String),
    /// A FreeType font (`faceName`): families, pixel size and fontconfig
    /// style (e.g. `Bold SemiCondensed`).
    Vector {
        families: Vec<String>,
        size: f32,
        style: Option<String>,
    },
}

impl XtermFont {
    /// XLFD patterns for a `Vector` font that is an X11 bitmap font found
    /// through fontconfig, such as `Misc Fixed:style=SemiCondensed:pixelsize=13`
    /// (fontconfig prefixes their family with the foundry).
    pub fn bitmap_patterns(&self) -> Vec<String> {
        let XtermFont::Vector {
            families,
            size,
            style,
        } = self
        else {
            return Vec::new();
        };
        let (mut weight, mut slant, mut width) = ("medium", "r", "normal".to_owned());
        for word in style.as_deref().unwrap_or("").split_whitespace() {
            match word.to_ascii_lowercase().as_str() {
                "bold" => weight = "bold",
                "italic" => slant = "i",
                "oblique" => slant = "o",
                "regular" | "medium" | "normal" | "roman" => {}
                other => other.clone_into(&mut width),
            }
        }
        let mut patterns = Vec::new();
        for family in families {
            let family = family.to_ascii_lowercase();
            let mut names = vec![("*", family.as_str())];
            if let Some((foundry, rest)) = family.split_once(' ') {
                names.insert(0, (foundry, rest));
            }
            for (foundry, name) in names {
                patterns.push(format!(
                    "-{foundry}-{name}-{weight}-{slant}-{width}--{size}-*"
                ));
            }
        }
        patterns
    }
}

pub struct Detection {
    pub font: XtermFont,
    /// Where the setting came from, for the user.
    pub source: &'static str,
}

/// Settings relevant to the font, from one source.
#[derive(Default, Debug)]
struct Settings {
    font: Option<String>,
    face_name: Option<String>,
    face_size: Option<f32>,
    render_font: Option<bool>,
}

impl Settings {
    fn is_empty(&self) -> bool {
        self.font.is_none() && self.face_name.is_none() && self.face_size.is_none()
    }

    /// Fills unset fields from `other` (lower precedence).
    fn or(self, other: Settings) -> Settings {
        Settings {
            font: self.font.or(other.font),
            face_name: self.face_name.or(other.face_name),
            face_size: self.face_size.or(other.face_size),
            render_font: self.render_font.or(other.render_font),
        }
    }

    /// The font xterm uses with these settings: a face name means FreeType
    /// rendering unless `renderFont` is off.
    fn resolve(self, dpi: f32) -> XtermFont {
        match self.face_name {
            Some(face) if self.render_font != Some(false) => {
                parse_face(&face, self.face_size.unwrap_or(DEFAULT_FACE_SIZE), dpi)
            }
            _ => XtermFont::Bitmap(self.font.unwrap_or_else(|| "fixed".to_owned())),
        }
    }
}

/// Parses a fontconfig-style face name such as `DejaVu Sans Mono`,
/// `Mono-12`, `xft:Mono:size=12` or `Mono:pixelsize=16`.
fn parse_face(face: &str, points: f32, dpi: f32) -> XtermFont {
    let face = face.strip_prefix("xft:").unwrap_or(face);
    let mut parts = face.split(':');
    let mut family = parts.next().unwrap_or("").trim().to_owned();
    let mut size = points * dpi / 72.0;
    let mut style = None;

    // `Family-12`: a point size after the last dash.
    if let Some((name, pt)) = family.rsplit_once('-') {
        if let Ok(pt) = pt.trim().parse::<f32>() {
            size = pt * dpi / 72.0;
            family = name.trim().to_owned();
        }
    }
    for prop in parts {
        if let Some((key, value)) = prop.split_once('=') {
            match (key.trim(), value.trim().parse::<f32>()) {
                ("size", Ok(pt)) => size = pt * dpi / 72.0,
                ("pixelsize", Ok(px)) => size = px,
                ("style", _) => style = Some(value.trim().to_owned()),
                // fontconfig's numeric widths, as words for the XLFD.
                ("width", Ok(w)) => {
                    const WIDTHS: [(f32, &str); 8] = [
                        (57.0, "UltraCondensed"),
                        (69.0, "ExtraCondensed"),
                        (81.0, "Condensed"),
                        (94.0, "SemiCondensed"),
                        (107.0, "Normal"),
                        (119.0, "SemiExpanded"),
                        (137.0, "Expanded"),
                        (175.0, "ExtraExpanded"),
                    ];
                    let width = WIDTHS
                        .iter()
                        .find(|(limit, _)| w < *limit)
                        .map_or("UltraExpanded", |(_, name)| name);
                    style = Some(format!("{} {width}", style.unwrap_or_default()));
                }
                _ => {}
            }
        }
    }

    let families = family
        .split(',')
        .map(|f| f.trim().replace('\\', ""))
        .filter(|f| !f.is_empty())
        .collect();
    XtermFont::Vector {
        families,
        size: size.round().max(4.0),
        style,
    }
}

/// Whether an OSC 50 reply is a fontconfig pattern (`Family:key=value`)
/// rather than an X11 core font name or a file path.
fn is_face(name: &str) -> bool {
    name.contains(':') && !name.starts_with('-') && !name.contains('/')
}

/// Settings from xterm's command line arguments; also returns the class
/// (`UXTerm` for uxterm).
fn parse_args(args: &[String]) -> (Settings, Option<String>) {
    let mut s = Settings::default();
    let mut class = None;
    let mut it = args.iter().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-fn" | "-font" => s.font = it.next().cloned(),
            "-fa" => s.face_name = it.next().cloned(),
            "-fs" => s.face_size = it.next().and_then(|v| v.parse().ok()),
            "-class" => class = it.next().cloned(),
            "-e" => break, // the rest is the command to run
            _ => {}
        }
    }
    (s, class)
}

/// Settings from `xrdb -query` output, for xterm with the given class (the
/// instance name is `xterm` in either case).
/// Bindings are ranked roughly like Xt does: entries naming the class or
/// instance beat `*` entries, and entries mentioning the VT100 widget beat
/// those that do not. Entries for other widgets (menus, Tek window) are
/// ignored.
fn parse_resources(text: &str, class: &str) -> (Settings, Option<f32>) {
    let mut best: [(i32, Option<String>); 4] = Default::default();
    let mut dpi = None;

    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if key == "Xft.dpi" {
            dpi = value.parse().ok();
            continue;
        }

        let parts: Vec<&str> = key.split(['.', '*']).collect();
        let (last, path) = parts.split_last().unwrap();
        let slot = match *last {
            "font" => 0,
            "faceName" => 1,
            "faceSize" => 2,
            "renderFont" => 3,
            _ => continue,
        };

        let mut rank = 0;
        let mut ok = true;
        for (i, p) in path.iter().enumerate() {
            match *p {
                "" => {}
                p if i == 0 && (p == class || p == "xterm") => rank += 2,
                // Settings for the other class (xterm vs. uxterm).
                "XTerm" | "UXTerm" | "xterm" | "uxterm" if i == 0 => ok = false,
                "vt100" | "VT100" => rank += 1,
                _ => ok = false,
            }
        }
        if ok && (best[slot].1.is_none() || rank >= best[slot].0) {
            best[slot] = (rank, Some(value.to_owned()));
        }
    }

    let [font, face, size, render] = best.map(|(_, v)| v);
    let settings = Settings {
        font,
        face_name: face,
        face_size: size.and_then(|v| v.parse().ok()),
        render_font: render
            .map(|v| !matches!(v.to_ascii_lowercase().as_str(), "false" | "off" | "no")),
    };
    (settings, dpi)
}

/// Extracts the replies to XTVERSION and OSC 50 from what the terminal sent.
fn parse_replies(buf: &[u8]) -> (Option<String>, Option<String>) {
    let text = String::from_utf8_lossy(buf);
    let between = |start: &str| {
        let i = text.find(start)? + start.len();
        let rest = &text[i..];
        let end = rest.find(['\x07', '\x1b']).unwrap_or(rest.len());
        Some(rest[..end].to_owned())
    };
    (
        between("\x1bP>|"),
        between("\x1b]50;").filter(|f| !f.is_empty()),
    )
}

#[cfg(unix)]
mod unix {
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::process::Command;
    use std::time::{Duration, Instant};

    use crossterm::terminal;

    /// Sends XTVERSION, OSC 50 and DA1 queries and collects the replies up
    /// to the DA1 reply, which every terminal sends, so there is no waiting
    /// for replies that never come.
    pub fn query_terminal() -> Option<Vec<u8>> {
        let mut tty = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .ok()?;
        let was_raw = terminal::is_raw_mode_enabled().unwrap_or(false);
        if !was_raw {
            terminal::enable_raw_mode().ok()?;
        }
        let result = exchange(&mut tty);
        if !was_raw {
            let _ = terminal::disable_raw_mode();
        }
        result
    }

    fn exchange(tty: &mut File) -> Option<Vec<u8>> {
        tty.write_all(b"\x1b[>0q\x1b]50;?\x07\x1b[c").ok()?;
        tty.flush().ok()?;

        let deadline = Instant::now() + Duration::from_millis(1000);
        let mut buf = Vec::new();
        let mut chunk = [0u8; 256];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let mut pfd = libc::pollfd {
                fd: tty.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd for a file descriptor we own.
            let timeout = i32::try_from(left.as_millis()).unwrap_or(i32::MAX);
            let ready = unsafe { libc::poll(&raw mut pfd, 1, timeout) };
            if ready <= 0 {
                break;
            }
            let n = tty.read(&mut chunk).ok()?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if da1_complete(&buf) {
                break;
            }
        }
        Some(buf)
    }

    fn da1_complete(buf: &[u8]) -> bool {
        let text = String::from_utf8_lossy(buf);
        text.find("\x1b[?")
            .is_some_and(|i| text[i + 3..].bytes().any(|b| b == b'c'))
    }

    /// The command line of the closest `xterm` ancestor process (Linux).
    pub fn xterm_ancestor_args() -> Option<Vec<String>> {
        let mut pid = std::process::id();
        for _ in 0..64 {
            let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
            let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
            if comm.trim() == "xterm" {
                let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
                return Some(
                    cmdline
                        .split(|&b| b == 0)
                        .filter(|a| !a.is_empty())
                        .map(|a| String::from_utf8_lossy(a).into_owned())
                        .collect(),
                );
            }
            pid = status
                .lines()
                .find_map(|l| l.strip_prefix("PPid:"))?
                .trim()
                .parse()
                .ok()?;
            if pid <= 1 {
                return None;
            }
        }
        None
    }

    pub fn xrdb_query() -> Option<String> {
        let out = Command::new("xrdb").arg("-query").output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// Detects the xterm font, if term2gif runs in an xterm.
#[cfg(unix)]
pub fn detect() -> Option<Detection> {
    use std::io::IsTerminal;

    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return None;
    }

    let (version, osc50) = unix::query_terminal()
        .map(|buf| parse_replies(&buf))
        .unwrap_or_default();
    let ancestor = unix::xterm_ancestor_args();
    let is_xterm = version.as_deref().is_some_and(|v| v.starts_with("XTerm("))
        || (version.is_none() && ancestor.is_some());
    if !is_xterm {
        return None;
    }

    let (args, class) = ancestor.map(|a| parse_args(&a)).unwrap_or_default();
    let class = class.unwrap_or_else(|| "XTerm".to_owned());
    let (resources, dpi) = unix::xrdb_query()
        .map(|text| parse_resources(&text, &class))
        .unwrap_or_default();
    let dpi = dpi.unwrap_or(DEFAULT_DPI);

    if let Some(name) = osc50 {
        // xterm reports a FreeType font by its face name, with an "xft:"
        // prefix only if the face name has one; X11 core font names have
        // no `:`.
        let font = if name.starts_with("xft:") || is_face(&name) {
            let size = args
                .face_size
                .or(resources.face_size)
                .unwrap_or(DEFAULT_FACE_SIZE);
            parse_face(&name, size, dpi)
        } else {
            XtermFont::Bitmap(name)
        };
        return Some(Detection {
            font,
            source: "the running xterm",
        });
    }

    let source = if !args.is_empty() {
        "the xterm command line"
    } else if !resources.is_empty() {
        "the X resources"
    } else {
        "xterm's default"
    };
    Some(Detection {
        font: args.or(resources).resolve(dpi),
        source,
    })
}

#[cfg(not(unix))]
pub fn detect() -> Option<Detection> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies() {
        let buf =
            b"\x1bP>|XTerm(411)\x1b\\\x1b]50;-misc-fixed-medium-r-normal--13-*\x07\x1b[?64;1;2c";
        let (version, font) = parse_replies(buf);
        assert_eq!(version.as_deref(), Some("XTerm(411)"));
        assert_eq!(font.as_deref(), Some("-misc-fixed-medium-r-normal--13-*"));

        let (version, font) = parse_replies(b"\x1b[?62;22c");
        assert_eq!((version, font), (None, None));
    }

    #[test]
    fn resources() {
        let text = "*VT100.font:\tfixed\n\
                    XTerm*SimpleMenu*font:\tmenu-font\n\
                    UXTerm*vt100.font:\t9x15\n\
                    Xft.dpi:\t120\n";
        let (s, dpi) = parse_resources(text, "XTerm");
        assert_eq!(s.font.as_deref(), Some("fixed"));
        assert_eq!(dpi, Some(120.0));
        let (s, _) = parse_resources(text, "UXTerm");
        assert_eq!(s.font.as_deref(), Some("9x15"));
    }

    #[test]
    fn face_name_wins_unless_render_font_is_off() {
        let text = "XTerm*faceName:\tDejaVu Sans Mono\nXTerm*faceSize:\t12\nXTerm*font:\t9x15\n";
        let (s, _) = parse_resources(text, "XTerm");
        assert_eq!(
            s.resolve(96.0),
            XtermFont::Vector {
                families: vec!["DejaVu Sans Mono".to_owned()],
                size: 16.0,
                style: None,
            }
        );
        let (s, _) = parse_resources(&format!("{text}XTerm*renderFont:\tfalse\n"), "XTerm");
        assert_eq!(s.resolve(96.0), XtermFont::Bitmap("9x15".to_owned()));
    }

    #[test]
    fn command_line() {
        let args: Vec<String> = [
            "xterm",
            "-class",
            "UXTerm",
            "-fa",
            "Mono:size=10",
            "-e",
            "-fn",
            "x",
        ]
        .map(String::from)
        .to_vec();
        let (s, class) = parse_args(&args);
        assert_eq!(class.as_deref(), Some("UXTerm"));
        assert_eq!(s.font, None);
        assert_eq!(
            s.resolve(96.0),
            XtermFont::Vector {
                families: vec!["Mono".to_owned()],
                size: 13.0,
                style: None,
            }
        );
    }

    #[test]
    fn face_names() {
        let v = |families: &[&str], size| XtermFont::Vector {
            families: families.iter().map(ToString::to_string).collect(),
            size,
            style: None,
        };
        assert_eq!(
            parse_face("xft:DejaVu Sans Mono-12", 8.0, 96.0),
            v(&["DejaVu Sans Mono"], 16.0)
        );
        assert_eq!(
            parse_face("Mono:pixelsize=15", 8.0, 96.0),
            v(&["Mono"], 15.0)
        );
        assert_eq!(parse_face("A,B", 9.0, 96.0), v(&["A", "B"], 12.0));
    }

    #[test]
    fn bitmap_faces() {
        let name = "Misc Fixed:style=SemiCondensed:pixelsize=13";
        assert!(is_face(name));
        assert!(!is_face("-misc-fixed-medium-r-semicondensed--13-*"));
        assert!(!is_face("9x15"));
        let font = parse_face(name, 8.0, 96.0);
        assert_eq!(
            font.bitmap_patterns(),
            [
                "-misc-fixed-medium-r-semicondensed--13-*",
                "-*-misc fixed-medium-r-semicondensed--13-*",
            ]
        );
        let font = parse_face("Misc Fixed:width=87:pixelsize=13", 8.0, 96.0);
        assert_eq!(
            font.bitmap_patterns()[0],
            "-misc-fixed-medium-r-semicondensed--13-*"
        );
        let font = parse_face("Fixed:style=Bold Oblique:pixelsize=13", 8.0, 96.0);
        assert_eq!(font.bitmap_patterns(), ["-*-fixed-bold-o-normal--13-*"]);
    }

    #[test]
    fn default_is_fixed() {
        assert_eq!(
            Settings::default().resolve(96.0),
            XtermFont::Bitmap("fixed".to_owned())
        );
    }
}
