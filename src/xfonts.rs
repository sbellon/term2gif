// SPDX-FileCopyrightText: 2026 Stefan Bellon
//
// SPDX-License-Identifier: MIT

//! Finding X11 bitmap fonts the way xterm names them: aliases such as
//! `fixed` or `9x15`, XLFD names (wildcards allowed), or file paths.
//!
//! Like xterm in a UTF-8 locale (uxterm), the ISO 10646 version of a font is
//! used where one exists, the bold font is the same XLFD with weight `bold`,
//! and the font for wide (CJK) characters is one of the same pixel size and
//! twice the average width.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

const FONT_ROOTS: &[&str] = &[
    "/usr/share/fonts/X11",
    "/usr/share/fonts",
    "/usr/lib/X11/fonts",
    "/usr/X11R6/lib/X11/fonts",
    "/usr/local/share/fonts",
];

pub struct BitmapFontFiles {
    pub regular: PathBuf,
    pub bold: Option<PathBuf>,
    pub wide: Option<PathBuf>,
}

/// Font directories: those with a `fonts.dir`, up to three levels below the
/// roots.
fn font_dirs() -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        if dir.join("fonts.dir").is_file() && !out.iter().any(|d| d == dir) {
            out.push(dir.to_owned());
        }
        if depth == 0 {
            return;
        }
        if let Ok(entries) = fs::read_dir(dir) {
            let mut subdirs: Vec<_> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            subdirs.sort();
            for sub in subdirs {
                walk(&sub, depth - 1, out);
            }
        }
    }

    let mut dirs = Vec::new();
    for root in FONT_ROOTS {
        walk(Path::new(root), 3, &mut dirs);
    }
    dirs
}

/// `(file, XLFD)` entries of all `fonts.dir` files.
fn font_entries(dirs: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let mut entries = Vec::new();
    for dir in dirs {
        let Ok(text) = fs::read_to_string(dir.join("fonts.dir")) else {
            continue;
        };
        for line in text.lines().skip(1) {
            if let Some((file, name)) = line.trim().split_once(char::is_whitespace) {
                entries.push((dir.join(file), name.trim().to_ascii_lowercase()));
            }
        }
    }
    entries
}

/// Looks `name` up in the `fonts.alias` files.
fn alias(dirs: &[PathBuf], name: &str) -> Option<String> {
    for dir in dirs {
        let Ok(text) = fs::read_to_string(dir.join("fonts.alias")) else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('!') {
                continue;
            }
            if let Some((alias, target)) = line.split_once(char::is_whitespace) {
                if alias.trim_matches('"').eq_ignore_ascii_case(name) {
                    return Some(target.trim().trim_matches('"').to_ascii_lowercase());
                }
            }
        }
    }
    None
}

/// Glob match with `*` and `?`, as XLFD patterns use them.
fn glob(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some((p, t));
            p += 1;
        } else if let Some((sp, st)) = star {
            p = sp + 1;
            t = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}

/// Replaces XLFD field `index` (1 = foundry, 3 = weight, 13 = registry).
fn with_field(xlfd: &str, index: usize, value: &str) -> String {
    let mut fields: Vec<&str> = xlfd.split('-').collect();
    if let Some(f) = fields.get_mut(index) {
        *f = value;
    }
    fields.join("-")
}

/// Pattern for the double-width companion of `xlfd`, as xterm derives its
/// default wide font.
fn wide_pattern(xlfd: &str) -> Option<String> {
    let f: Vec<&str> = xlfd.split('-').collect();
    if f.len() < 15 {
        return None;
    }
    let width: u32 = f[12].parse().ok()?;
    Some(format!(
        "-{}-{}-*-*-*-*-{}-*-*-*-*-{}-iso10646-1",
        f[1],
        f[2],
        f[7],
        width * 2
    ))
}

fn unicode_variant(xlfd: &str) -> String {
    let fields: Vec<&str> = xlfd.split('-').collect();
    if fields.len() < 15 {
        return xlfd.to_owned();
    }
    with_field(&with_field(xlfd, 13, "iso10646"), 14, "1")
}

pub fn find(name: &str) -> Result<BitmapFontFiles> {
    let dirs = font_dirs();
    let entries = font_entries(&dirs);
    let file_of = |xlfd: &str| {
        entries
            .iter()
            .find(|(_, n)| n == xlfd)
            .map(|(f, _)| f.clone())
    };

    let wide_of = |xlfd: &str| {
        let pattern = wide_pattern(xlfd)?;
        entries
            .iter()
            .find(|(_, n)| glob(pattern.as_bytes(), n.as_bytes()))
            .map(|(f, _)| f.clone())
    };

    // A file path: find its XLFD to look for bold and wide variants.
    let path = Path::new(name);
    if path.is_file() {
        let canonical = fs::canonicalize(path).ok();
        let xlfd = entries
            .iter()
            .find(|(f, _)| fs::canonicalize(f).ok() == canonical)
            .map(|(_, n)| n.clone());
        let bold = xlfd
            .as_ref()
            .and_then(|x| file_of(&with_field(x, 3, "bold")));
        return Ok(BitmapFontFiles {
            regular: path.to_owned(),
            bold: bold.filter(|b| fs::canonicalize(b).ok() != canonical),
            wide: xlfd.and_then(|x| wide_of(&x)),
        });
    }

    let pattern = alias(&dirs, name).unwrap_or_else(|| name.to_ascii_lowercase());
    let Some(xlfd) = entries
        .iter()
        .map(|(_, n)| n)
        .find(|n| glob(pattern.as_bytes(), n.as_bytes()))
        .cloned()
    else {
        if dirs.is_empty() {
            bail!("bitmap font `{name}` not found (no X11 font directories found)");
        }
        bail!("bitmap font `{name}` not found");
    };

    let unicode = unicode_variant(&xlfd);
    let xlfd = if file_of(&unicode).is_some() {
        unicode
    } else {
        xlfd
    };
    let regular = file_of(&xlfd).expect("found above");
    let bold = file_of(&with_field(&xlfd, 3, "bold")).filter(|b| *b != regular);
    let wide = wide_of(&xlfd);

    Ok(BitmapFontFiles {
        regular,
        bold,
        wide,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matching() {
        assert!(glob(
            b"-misc-fixed-*-iso10646-1",
            b"-misc-fixed-medium-r-normal--13-iso10646-1"
        ));
        assert!(glob(b"6x1?", b"6x13"));
        assert!(!glob(b"6x1?", b"6x130"));
        assert!(glob(b"*", b""));
        assert!(!glob(b"a*b", b"acd"));
    }

    #[test]
    fn xlfd_variants() {
        let x = "-misc-fixed-medium-r-semicondensed--13-120-75-75-c-60-iso8859-1";
        assert_eq!(
            unicode_variant(x),
            "-misc-fixed-medium-r-semicondensed--13-120-75-75-c-60-iso10646-1"
        );
        assert_eq!(
            with_field(x, 3, "bold"),
            "-misc-fixed-bold-r-semicondensed--13-120-75-75-c-60-iso8859-1"
        );
    }

    #[test]
    fn finds_fixed() {
        // Only where xfonts-base is installed.
        if !Path::new("/usr/share/fonts/X11/misc/6x13.pcf.gz").exists() {
            return;
        }
        let files = find("fixed").unwrap();
        assert!(files.regular.ends_with("6x13.pcf.gz"));
        assert!(files.bold.unwrap().ends_with("6x13B.pcf.gz"));
        assert!(files.wide.unwrap().ends_with("12x13ja.pcf.gz"));
    }
}
