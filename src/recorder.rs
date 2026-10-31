// SPDX-FileCopyrightText: 2026 Stefan Bellon
//
// SPDX-License-Identifier: MIT

//! Terminal session recorder producing an asciicast v2 file.
//!
//! A PTY (`portable-pty`: Unix openpty / Windows ConPTY) runs the shell or
//! command. Everything it prints is mirrored to the real terminal and written
//! to the cast file (and, if asked, sent on as events for rendering during
//! the recording); keystrokes from the real terminal are forwarded to it.

use std::fs::File;
use std::io::{self, BufWriter, IsTerminal, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use crossterm::terminal;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde_json::json;

use crate::cast::Event;
use crate::utf8::decode_chunk;

#[derive(Default)]
pub struct Options {
    /// Command to run instead of the default shell (`-c`).
    pub command: Option<String>,
    pub title: Option<String>,
    /// Clear the terminal first (`--clear-screen`).
    #[cfg(windows)]
    pub clear_screen: bool,
    /// Receives the terminal size as a resize at time 0, then the events as
    /// they are recorded.
    pub live: Option<Sender<(f64, Event)>>,
}

/// Restores the terminal's original mode when dropped.
struct RawModeGuard;

impl RawModeGuard {
    fn enable() -> Result<Self> {
        terminal::enable_raw_mode().context("cannot switch the terminal to raw mode")?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

/// The Windows console needs VT input (so arrow keys etc. reach ConPTY as escape
/// sequences) and VT output processing (so ConPTY's output renders).
#[cfg(windows)]
mod winvt {
    use windows_sys::Win32::System::Console::{
        ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode,
        GetStdHandle, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleMode,
    };

    pub struct Guard {
        out_handle: windows_sys::Win32::Foundation::HANDLE,
        out_mode: Option<u32>,
    }

    /// Must be called *after* raw mode is enabled: crossterm restores the saved
    /// original input mode itself, which also drops the VT input flag again.
    /// Then clears the screen if asked to (see [`clear_screen`]).
    pub fn enable(clear: bool) -> Guard {
        let guard = unsafe {
            let hin = GetStdHandle(STD_INPUT_HANDLE);
            let mut mode = 0u32;
            if GetConsoleMode(hin, &raw mut mode) != 0 {
                SetConsoleMode(hin, mode | ENABLE_VIRTUAL_TERMINAL_INPUT);
            }

            let hout = GetStdHandle(STD_OUTPUT_HANDLE);
            let mut out_mode = 0u32;
            let saved = if GetConsoleMode(hout, &raw mut out_mode) != 0 {
                SetConsoleMode(hout, out_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
                Some(out_mode)
            } else {
                None
            };

            Guard {
                out_handle: hout,
                out_mode: saved,
            }
        };
        if clear {
            clear_screen();
        }
        guard
    }

    /// ConPTY asks for the cursor position at its start (the query waits in
    /// the pipe until the output thread forwards it) and then draws at
    /// absolute rows from there, while the cast is replayed from the top of an
    /// empty screen. Starting at the top of a cleared screen makes both agree.
    fn clear_screen() {
        use crossterm::{cursor::MoveTo, execute, terminal};

        let _ = execute!(
            std::io::stdout(),
            terminal::Clear(terminal::ClearType::All),
            MoveTo(0, 0)
        );
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            if let Some(mode) = self.out_mode {
                unsafe {
                    SetConsoleMode(self.out_handle, mode);
                }
            }
        }
    }
}

/// asciicast v2 writer, shared between the output and resize threads.
struct Cast {
    out: Mutex<CastOut>,
    start: Instant,
}

struct CastOut {
    file: BufWriter<File>,
    live: Option<Sender<(f64, Event)>>,
}

impl Cast {
    fn event(&self, event: Event) {
        let Ok(mut out) = self.out.lock() else {
            return;
        };
        // Taken under the lock, so that times never decrease.
        let t = (self.start.elapsed().as_secs_f64() * 1e6).round() / 1e6;
        let line = match &event {
            Event::Output(data) => json!([t, "o", data]),
            Event::Resize(cols, rows) => json!([t, "r", format!("{cols}x{rows}")]),
        };
        let _ = writeln!(out.file, "{line}");
        // A receiver that stopped listening only ends the live rendering.
        if let Some(live) = &out.live {
            if live.send((t, event)).is_err() {
                out.live = None;
            }
        }
    }

    fn flush(&self) -> io::Result<()> {
        let mut out = self
            .out
            .lock()
            .map_err(|_| io::Error::other("poisoned lock"))?;
        // Ends the live events.
        out.live = None;
        out.file.flush()
    }
}

fn build_command(command: Option<&str>) -> Result<CommandBuilder> {
    let mut cmd = match command {
        None => CommandBuilder::new_default_prog(),
        Some(c) => {
            #[cfg(windows)]
            {
                let mut b = CommandBuilder::new("cmd.exe");
                b.args(["/C", c]);
                b
            }
            #[cfg(not(windows))]
            {
                let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
                let mut b = CommandBuilder::new(shell);
                b.args(["-c", c]);
                b
            }
        }
    };

    // Same marker asciinema sets, so shell prompts can detect a recording.
    cmd.env("ASCIINEMA_REC", "1");

    // Behind term2gif's ConPTY, Clink does not detect Windows Terminal and
    // falls back to a mode that redraws multi-line prompts (e.g. Oh My Posh)
    // one line too high. Name the host for it, unless the user already did.
    #[cfg(windows)]
    if std::env::var_os("WT_SESSION").is_some() && std::env::var_os("CLINK_ANSI_HOST").is_none() {
        cmd.env("CLINK_ANSI_HOST", "winterminal");
    }

    cmd.cwd(std::env::current_dir().context("cannot determine the current directory")?);

    Ok(cmd)
}

/// Some terminals (e.g. serial consoles, some container or CI TTYs) report a
/// size of 0, which agg rejects; fall back to 80x24 like asciinema does.
fn effective_size((cols, rows): (u16, u16)) -> (u16, u16) {
    (
        if cols == 0 { 80 } else { cols },
        if rows == 0 { 24 } else { rows },
    )
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Copies PTY output to the real terminal and the cast until the PTY closes.
fn pump_output(mut reader: Box<dyn Read + Send>, cast: &Cast) {
    let mut buf = [0u8; 8192];
    let mut carry = Vec::new();
    let mut stdout = io::stdout();

    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let text = decode_chunk(&mut carry, &buf[..n]);

                // The Windows console rejects non-UTF-8 writes, so write the
                // decoded text there; elsewhere pass the bytes through untouched.
                #[cfg(windows)]
                let _ = stdout.write_all(text.as_bytes());
                #[cfg(not(windows))]
                let _ = stdout.write_all(&buf[..n]);
                let _ = stdout.flush();

                if !text.is_empty() {
                    cast.event(Event::Output(text));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            // On Linux the master returns EIO once the child side is closed.
            Err(_) => break,
        }
    }
}

/// Forwards keystrokes from the real terminal to the PTY. Detached: it may stay
/// blocked in `read` after the child exits, and ends with the process.
fn pump_input(mut writer: Box<dyn Write + Send>) {
    let mut stdin = io::stdin();
    let mut buf = [0u8; 1024];

    loop {
        match stdin.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if writer.write_all(&buf[..n]).is_err() {
                    break;
                }
                let _ = writer.flush();
            }
        }
    }
}

/// Records a session into `cast_path` (asciicast v2). Returns when the
/// recorded command or shell exits.
pub fn record(cast_path: &Path, opts: &Options) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("recording needs an interactive terminal (stdin and stdout must be a TTY)");
    }

    let (cols, rows) =
        effective_size(terminal::size().context("cannot determine the terminal size")?);

    let pair = native_pty_system()
        .openpty(pty_size(cols, rows))
        .map_err(|e| anyhow::anyhow!("cannot open a pseudo-terminal: {e}"))?;
    let (master, slave) = (pair.master, pair.slave);

    let cmd = build_command(opts.command.as_deref())?;

    // Header.
    let file = File::create(cast_path).context("cannot create the temporary cast file")?;
    let mut out = BufWriter::new(file);

    let mut env = serde_json::Map::new();
    for key in ["SHELL", "TERM"] {
        if let Ok(v) = std::env::var(key) {
            env.insert(key.to_owned(), json!(v));
        }
    }

    let mut header = json!({
        "version": 2,
        "width": cols,
        "height": rows,
        "timestamp": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        "env": env,
    });
    if let Some(title) = &opts.title {
        header["title"] = json!(title);
    }
    writeln!(out, "{header}")?;

    let mut child = slave
        .spawn_command(cmd)
        .map_err(|e| anyhow::anyhow!("cannot start the command: {e}"))?;
    // Only the child may hold the slave side, otherwise the master never sees EOF.
    drop(slave);

    let reader = master
        .try_clone_reader()
        .map_err(|e| anyhow::anyhow!("cannot read from the pseudo-terminal: {e}"))?;
    let writer = master
        .take_writer()
        .map_err(|e| anyhow::anyhow!("cannot write to the pseudo-terminal: {e}"))?;

    let live = opts.live.clone().filter(|live| {
        live.send((0.0, Event::Resize(cols.into(), rows.into())))
            .is_ok()
    });
    let cast = Arc::new(Cast {
        out: Mutex::new(CastOut { file: out, live }),
        start: Instant::now(),
    });
    let master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>> = Arc::new(Mutex::new(Some(master)));
    let stop = Arc::new(AtomicBool::new(false));

    // Declared in this order so they drop in reverse: Windows console modes
    // first, then raw mode.
    let raw = RawModeGuard::enable()?;
    #[cfg(windows)]
    let vt = winvt::enable(opts.clear_screen);

    let output_thread = {
        let cast = Arc::clone(&cast);
        thread::spawn(move || pump_output(reader, &cast))
    };
    thread::spawn(move || pump_input(writer));

    // Resize detection by polling: portable across Unix and Windows.
    let resize_thread = {
        let (cast, master, stop) = (Arc::clone(&cast), Arc::clone(&master), Arc::clone(&stop));
        thread::spawn(move || {
            let mut last = (cols, rows);
            while !stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(200));

                if let Ok(size) = terminal::size().map(effective_size) {
                    if size != last {
                        last = size;
                        if let Ok(guard) = master.lock() {
                            if let Some(m) = guard.as_ref() {
                                let _ = m.resize(pty_size(size.0, size.1));
                            }
                        }
                        cast.event(Event::Resize(size.0.into(), size.1.into()));
                    }
                }
            }
        })
    };

    let wait_result = child.wait();

    stop.store(true, Ordering::Relaxed);
    let _ = resize_thread.join();

    // ConPTY only signals end-of-output once the pseudo console is closed.
    #[cfg(windows)]
    drop(master.lock().ok().and_then(|mut m| m.take()));

    // Let the output thread drain what the child printed last. Background
    // processes that keep the PTY open could block it, so give up after 2s.
    let deadline = Instant::now() + Duration::from_secs(2);
    while !output_thread.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    if output_thread.is_finished() {
        let _ = output_thread.join();
    }

    drop(master.lock().ok().and_then(|mut m| m.take()));

    #[cfg(windows)]
    drop(vt);
    drop(raw);

    cast.flush().context("cannot write the cast file")?;
    wait_result.context("cannot wait for the recorded command")?;

    Ok(())
}
