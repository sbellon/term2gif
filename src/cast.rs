// SPDX-FileCopyrightText: 2026 Stefan Bellon
//
// SPDX-License-Identifier: MIT

//! Reading asciicast recordings (v2 as written by the recorder, and v3 as
//! written by asciinema 3).

use std::io::BufRead;

use anyhow::{Context, Result, bail};
use serde_json::Value;

pub enum Event {
    Output(String),
    Resize(usize, usize),
}

pub struct Recording {
    pub cols: usize,
    pub rows: usize,
    /// Events with their absolute time in seconds, in order.
    pub events: Vec<(f64, Event)>,
}

impl Recording {
    /// The largest terminal size during the whole recording.
    pub fn max_size(&self) -> (usize, usize) {
        self.events
            .iter()
            .fold((self.cols, self.rows), |(c, r), (_, e)| match e {
                Event::Resize(ec, er) => (c.max(*ec), r.max(*er)),
                Event::Output(_) => (c, r),
            })
    }
}

fn size_field(header: &Value, path: &[&str]) -> Option<usize> {
    path.iter()
        .try_fold(header, |v, key| v.get(key))?
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .filter(|&n| n > 0)
}

fn parse_size(s: &str) -> Option<(usize, usize)> {
    let (c, r) = s.split_once('x')?;
    let (c, r) = (c.trim().parse().ok()?, r.trim().parse().ok()?);
    (c > 0 && r > 0).then_some((c, r))
}

pub fn read(input: impl BufRead) -> Result<Recording> {
    let mut lines = input.lines().enumerate();

    let header: Value = loop {
        let Some((_, line)) = lines.next() else {
            bail!("the recording is empty");
        };
        let line = line.context("cannot read the recording")?;
        if !line.trim().is_empty() {
            break serde_json::from_str(&line).context("invalid asciicast header")?;
        }
    };

    let version = header.get("version").and_then(Value::as_u64);
    let (cols, rows, relative) = match version {
        Some(2) => (
            size_field(&header, &["width"]),
            size_field(&header, &["height"]),
            false,
        ),
        Some(3) => (
            size_field(&header, &["term", "cols"]),
            size_field(&header, &["term", "rows"]),
            true,
        ),
        _ => bail!("unsupported asciicast version (only v2 and v3 are supported)"),
    };
    let (Some(cols), Some(rows)) = (cols, rows) else {
        bail!("the recording has no valid terminal size");
    };

    let mut events = Vec::new();
    let mut time = 0.0;

    for (n, line) in lines {
        let line = line.context("cannot read the recording")?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let event: (f64, String, String) = serde_json::from_str(line)
            .with_context(|| format!("invalid event in line {} of the recording", n + 1))?;
        let (t, code, data) = event;

        // v3 stores the interval since the previous event, v2 absolute times.
        time = if relative {
            time + t.max(0.0)
        } else {
            t.max(time)
        };

        match code.as_str() {
            "o" => events.push((time, Event::Output(data))),
            "r" => {
                if let Some((c, r)) = parse_size(&data) {
                    events.push((time, Event::Resize(c, r)));
                }
            }
            // Input, markers, exit status etc. do not affect the picture.
            _ => {}
        }
    }

    Ok(Recording { cols, rows, events })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2() {
        let cast = "{\"version\":2,\"width\":80,\"height\":24}\n\
                    [0.5,\"o\",\"hi\"]\n\
                    [1.0,\"i\",\"x\"]\n\
                    [1.5,\"r\",\"100x30\"]\n";
        let rec = read(cast.as_bytes()).unwrap();
        assert_eq!((rec.cols, rec.rows), (80, 24));
        assert_eq!(rec.events.len(), 2);
        assert_eq!(rec.events[1].0, 1.5);
        assert_eq!(rec.max_size(), (100, 30));
    }

    #[test]
    fn v3_relative_times_and_comments() {
        let cast = "{\"version\":3,\"term\":{\"cols\":120,\"rows\":40}}\n\
                    # a comment\n\
                    [0.5,\"o\",\"a\"]\n\
                    [0.25,\"o\",\"b\"]\n";
        let rec = read(cast.as_bytes()).unwrap();
        assert_eq!((rec.cols, rec.rows), (120, 40));
        assert_eq!(rec.events[1].0, 0.75);
    }

    #[test]
    fn rejects_zero_size() {
        let cast = "{\"version\":2,\"width\":0,\"height\":0}\n";
        assert!(read(cast.as_bytes()).is_err());
    }
}
