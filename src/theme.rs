//! Color themes: background, foreground and the 16 ANSI colors.

use anyhow::{Result, anyhow, bail};

pub type Rgb = [u8; 3];

#[derive(Clone, Debug)]
pub struct Theme {
    pub background: Rgb,
    pub foreground: Rgb,
    /// ANSI colors 0-7 (normal) and 8-15 (bright).
    pub palette: [Rgb; 16],
}

/// Built-in themes as "bg,fg,8 normal colors,8 bright colors".
const BUILTIN: &[(&str, &str)] = &[
    (
        "asciinema",
        "121314,cccccc,\
         000000,dd3c69,4ebf22,ddaf3c,26b0d7,b954e1,54e1b9,d9d9d9,\
         4d4d4d,dd3c69,4ebf22,ddaf3c,26b0d7,b954e1,54e1b9,ffffff",
    ),
    (
        "dracula",
        "282a36,f8f8f2,\
         21222c,ff5555,50fa7b,f1fa8c,bd93f9,ff79c6,8be9fd,f8f8f2,\
         6272a4,ff6e6e,69ff94,ffffa5,d6acff,ff92df,a4ffff,ffffff",
    ),
    (
        "monokai",
        "272822,f8f8f2,\
         272822,f92672,a6e22e,f4bf75,66d9ef,ae81ff,a1efe4,f8f8f2,\
         75715e,f92672,a6e22e,f4bf75,66d9ef,ae81ff,a1efe4,f9f8f5",
    ),
    (
        "nord",
        "2e3440,d8dee9,\
         3b4252,bf616a,a3be8c,ebcb8b,81a1c1,b48ead,88c0d0,e5e9f0,\
         4c566a,bf616a,a3be8c,ebcb8b,81a1c1,b48ead,8fbcbb,eceff4",
    ),
    (
        "solarized-dark",
        "002b36,839496,\
         073642,dc322f,859900,b58900,268bd2,d33682,2aa198,eee8d5,\
         002b36,cb4b16,586e75,657b83,839496,6c71c4,93a1a1,fdf6e3",
    ),
    (
        "solarized-light",
        "fdf6e3,657b83,\
         073642,dc322f,859900,b58900,268bd2,d33682,2aa198,eee8d5,\
         002b36,cb4b16,586e75,657b83,839496,6c71c4,93a1a1,fdf6e3",
    ),
];

pub fn builtin_names() -> impl Iterator<Item = &'static str> {
    BUILTIN.iter().map(|(name, _)| *name)
}

fn parse_hex(s: &str) -> Option<Rgb> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 || !s.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// Parses "bg,fg,8 colors" (the bright colors then equal the normal ones) or
/// "bg,fg,16 colors".
fn parse_colors(s: &str) -> Result<Theme> {
    let colors = s
        .split(',')
        .map(|c| parse_hex(c).ok_or_else(|| anyhow!("invalid color `{}`", c.trim())))
        .collect::<Result<Vec<_>>>()?;

    let mut palette = [[0; 3]; 16];
    match colors.len() {
        10 => {
            palette[..8].copy_from_slice(&colors[2..]);
            palette[8..].copy_from_slice(&colors[2..]);
        }
        18 => palette.copy_from_slice(&colors[2..]),
        n => bail!("a custom theme needs 10 or 18 colors, got {n}"),
    }

    Ok(Theme {
        background: colors[0],
        foreground: colors[1],
        palette,
    })
}

impl Theme {
    pub fn parse(s: &str) -> Result<Theme> {
        if s.contains(',') {
            return parse_colors(s).map_err(|e| anyhow!("invalid --theme: {e}"));
        }
        let colors = BUILTIN
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(s))
            .map(|(_, colors)| *colors)
            .ok_or_else(|| {
                let names = builtin_names().collect::<Vec<_>>().join(", ");
                anyhow!("unknown theme `{s}` (available: {names}, or a list of hex colors)")
            })?;
        Ok(parse_colors(colors).expect("built-in themes are valid"))
    }

    /// The color for an xterm 256-color index.
    pub fn indexed(&self, n: u8) -> Rgb {
        match n {
            0..=15 => self.palette[n as usize],
            16..=231 => {
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                let n = n - 16;
                [level(n / 36), level(n / 6 % 6), level(n % 6)]
            }
            232..=255 => {
                let v = 8 + (n - 232) * 10;
                [v, v, v]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_parse() {
        for name in builtin_names() {
            Theme::parse(name).unwrap();
        }
    }

    #[test]
    fn custom_ten_colors() {
        let t =
            Theme::parse("000000,ffffff,010101,020202,030303,040404,050505,060606,070707,#080808")
                .unwrap();
        assert_eq!(t.palette[7], [8, 8, 8]);
        assert_eq!(t.palette[15], [8, 8, 8]);
    }

    #[test]
    fn xterm_colors() {
        let t = Theme::parse("asciinema").unwrap();
        assert_eq!(t.indexed(16), [0, 0, 0]);
        assert_eq!(t.indexed(196), [255, 0, 0]);
        assert_eq!(t.indexed(231), [255, 255, 255]);
        assert_eq!(t.indexed(232), [8, 8, 8]);
        assert_eq!(t.indexed(255), [238, 238, 238]);
    }
}
