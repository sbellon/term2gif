// SPDX-FileCopyrightText: 2026 Stefan Bellon
//
// SPDX-License-Identifier: MIT

//! term2gif: record a terminal session and save it as a GIF, in a single binary.
//!
//! Recording is done by a built-in PTY recorder (asciicast v2); the recording
//! is replayed through a virtual terminal (avt) and rendered to a GIF while
//! it is going on.

mod boxdraw;
mod cast;
mod convert;
mod encode;
mod font;
mod pcf;
mod progress;
mod recorder;
mod render;
mod theme;
mod utf8;
mod xfonts;
mod xterm;

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::Parser;
use tempfile::NamedTempFile;

use crate::cast::Event;
use crate::convert::{Converter, Summary, Timing};
use crate::render::Renderer;
use crate::theme::Theme;

const THEME_HELP: &str = "Color theme: asciinema, dracula, monokai, nord, solarized-dark, \
    solarized-light, or 10 or 18 comma-separated hex colors \
    (background, foreground, 8 normal [, 8 bright] colors)";

/// Record a terminal session and save it as a GIF.
///
/// End the recording by exiting the shell (`exit` or Ctrl-D).
#[derive(Parser)]
#[command(name = "term2gif", version)]
#[allow(clippy::struct_excessive_bools)] // command line flags
struct Cli {
    /// GIF file to write
    output: PathBuf,

    /// Record this command instead of an interactive shell
    #[arg(short = 'c', long, value_name = "COMMAND", help_heading = "Recording")]
    command: Option<String>,

    /// Title stored in the recording
    #[arg(long, help_heading = "Recording")]
    title: Option<String>,

    /// Also keep the asciicast (.cast) recording at this path
    #[arg(long, value_name = "FILE", help_heading = "Recording")]
    keep_cast: Option<PathBuf>,

    /// Windows only, no effect elsewhere: clear the terminal when the
    /// recording starts, so that the session starts at the top of the GIF
    /// (otherwise, e.g. with an interactive shell, it may continue further
    /// down, on the row the cursor was on)
    #[arg(long, conflicts_with = "from_cast", help_heading = "Recording")]
    clear_screen: bool,

    /// Do not record; render this existing asciicast (v2 or v3) file instead
    #[arg(
        long,
        value_name = "FILE",
        conflicts_with_all = ["command", "title", "keep_cast"],
        help_heading = "Recording"
    )]
    from_cast: Option<PathBuf>,

    #[arg(long, default_value = "asciinema", help = THEME_HELP, help_heading = "Rendering")]
    theme: String,

    /// System font families to prefer over the built-in font (DejaVu Sans
    /// Mono), comma-separated; missing characters are taken from any
    /// installed font
    #[arg(
        long,
        value_name = "FAMILIES",
        value_delimiter = ',',
        help_heading = "Rendering"
    )]
    font_family: Vec<String>,

    /// X11 bitmap font as in xterm, e.g. `fixed`, `9x15`, an XLFD name, or a
    /// .pcf/.pcf.gz file; replaces --font-size and --line-height
    #[arg(
        long,
        value_name = "NAME",
        conflicts_with_all = ["font_size", "line_height"],
        help_heading = "Rendering"
    )]
    bitmap_font: Option<String>,

    /// Font size in pixels [default: 14]
    #[arg(long, value_name = "PX", help_heading = "Rendering")]
    font_size: Option<f32>,

    /// Line height as a multiple of the font size [default: 1.4]
    #[arg(long, help_heading = "Rendering")]
    line_height: Option<f32>,

    /// Inside xterm, the GIF uses xterm's font unless a font option is
    /// given; this uses the built-in font instead
    #[arg(long, help_heading = "Rendering")]
    no_font_detect: bool,

    /// Make the GIF this many times larger (1-8): bitmap fonts get larger
    /// pixels, other fonts a larger font size
    #[arg(
        long,
        default_value_t = 1,
        value_name = "N",
        help_heading = "Rendering"
    )]
    scale: u8,

    /// Draw bold text in the bright variant of its color
    #[arg(long, help_heading = "Rendering")]
    bold_is_bright: bool,

    /// Playback speed multiplier
    #[arg(long, default_value_t = 1.0, help_heading = "Timing")]
    speed: f64,

    /// Shorten pauses to at most this many seconds (0: keep all pauses)
    #[arg(
        long,
        default_value_t = 5.0,
        value_name = "SECS",
        help_heading = "Timing"
    )]
    idle_time_limit: f64,

    /// Maximum frames per second (1-50)
    #[arg(
        long,
        default_value_t = 30,
        value_name = "FPS",
        help_heading = "Timing"
    )]
    fps_cap: u32,

    /// How long to show the last frame, in seconds
    #[arg(
        long,
        default_value_t = 3.0,
        value_name = "SECS",
        help_heading = "Timing"
    )]
    last_frame_duration: f64,

    /// Play the animation only once
    #[arg(long, help_heading = "Timing")]
    no_loop: bool,

    /// Print only errors
    #[arg(short, long)]
    quiet: bool,

    /// Print details, such as the detected xterm font
    #[arg(short, long, conflicts_with = "quiet")]
    verbose: bool,
}

impl Cli {
    fn validate(&self) -> Result<()> {
        if self.font_size.is_some_and(|s| !(4.0..=200.0).contains(&s)) {
            bail!("--font-size must be between 4 and 200");
        }
        if self.line_height.is_some_and(|h| !(1.0..=4.0).contains(&h)) {
            bail!("--line-height must be between 1 and 4");
        }
        if !(1..=8).contains(&self.scale) {
            bail!("--scale must be between 1 and 8");
        }
        if !(self.speed > 0.0 && self.speed.is_finite()) {
            bail!("--speed must be greater than 0");
        }
        if self.idle_time_limit.is_nan() || self.idle_time_limit < 0.0 {
            bail!("--idle-time-limit must not be negative");
        }
        if !(1..=50).contains(&self.fps_cap) {
            bail!("--fps-cap must be between 1 and 50");
        }
        if !(self.last_frame_duration >= 0.0 && self.last_frame_duration <= 600.0) {
            bail!("--last-frame-duration must be between 0 and 600");
        }
        Ok(())
    }
}

/// What became of rendering a recording while it was going on.
enum Live {
    /// The recording did not start.
    NotStarted,
    /// The terminal grew beyond its size at the start, which the GIF cannot
    /// follow: the recording has to be rendered once more when it is done.
    Outgrown,
    Done(Summary),
}

/// Records a session and renders it to the GIF at the same time. The outer
/// error is a recording that did not start, which leaves the output alone;
/// with the inner one, the output is to be removed.
fn record_and_render(
    cli: &Cli,
    renderer: &mut Renderer,
    timing: &Timing,
) -> Result<Result<Summary>> {
    // The temporary cast file is removed when `cast` is dropped, on every path.
    let cast = NamedTempFile::with_prefix("term2gif-").context("cannot create a temporary file")?;

    if !cli.quiet {
        let output = cli.output.display();
        match &cli.command {
            Some(command) => eprintln!("term2gif: recording `{command}` to {output}"),
            None if cfg!(windows) => {
                eprintln!("term2gif: recording to {output}, type `exit` to finish");
            }
            None => {
                eprintln!("term2gif: recording to {output}, press Ctrl-D or type `exit` to finish");
            }
        }
    }

    let (sender, events) = mpsc::channel();
    let opts = recorder::Options {
        command: cli.command.clone(),
        title: cli.title.clone(),
        #[cfg(windows)]
        clear_screen: cli.clear_screen,
        live: Some(sender),
    };
    let start = Instant::now();
    // The recorder runs on its own thread; the renderer stays on this one.
    // `opts` moves along, so that the events end with the recording.
    let cast_path = cast.path();
    let (recorded, live) = thread::scope(|s| {
        let recording = s.spawn(move || recorder::record(cast_path, &opts));
        let live = render_live(cli, renderer, timing, events, start, || {
            recording.is_finished()
        });
        let recorded = recording
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        (recorded, live)
    });

    if let Err(e) = recorded {
        return match live {
            Ok(Live::NotStarted) => Err(e),
            _ => Ok(Err(e)),
        };
    }

    if let Some(path) = &cli.keep_cast {
        // Not fs::copy: that would also copy the temporary file's 0600 mode.
        let saved = File::create(path)
            .and_then(|mut f| std::io::copy(&mut File::open(cast.path())?, &mut f))
            .with_context(|| format!("cannot save the recording to {}", path.display()));
        if let Err(e) = saved {
            return Ok(Err(e));
        }
    }

    Ok(match live {
        Ok(Live::Done(summary)) => Ok(summary),
        Ok(live) => {
            if cli.verbose && matches!(live, Live::Outgrown) {
                eprintln!("term2gif: the terminal grew, rendering the recording once more");
            }
            read_cast(cast.path()).and_then(|rec| render(cli, &rec, renderer, timing))
        }
        Err(e) => Err(e),
    })
}

/// Renders the `events` of a recording that started at `start` while it is
/// going on, on a canvas of the terminal's size at the start. Once
/// `finished` says that the recording is done, the remaining events are
/// rendered with a progress bar and Ctrl-C handling, as after a recording.
fn render_live(
    cli: &Cli,
    renderer: &mut Renderer,
    timing: &Timing,
    events: Receiver<(f64, Event)>,
    start: Instant,
    finished: impl Fn() -> bool,
) -> Result<Live> {
    let Ok((_, Event::Resize(cols, rows))) = events.recv() else {
        return Ok(Live::NotStarted);
    };
    let output = File::create(&cli.output)
        .with_context(|| format!("cannot create {}", cli.output.display()))?;
    let mut conv = Converter::new(
        renderer,
        timing,
        (cols, rows),
        (cols, rows),
        !cli.no_loop,
        BufWriter::new(output),
    )?;

    // Once the recording is done: the progress bar, and the recording times
    // it goes from and to.
    let mut rest: Option<(progress::Progress, f64, f64)> = None;
    for (t, event) in events {
        if let Event::Resize(cols, rows) = event {
            if !conv.fits(cols, rows) {
                return Ok(Live::Outgrown);
            }
        }
        if rest.is_none() && finished() {
            set_ctrlc_handler();
            let end = start.elapsed().as_secs_f64();
            rest = Some((progress::Progress::new(!cli.quiet), t, end));
        }
        if let Some((progress, from, to)) = &mut rest {
            if INTERRUPTED.load(Ordering::Relaxed) {
                return Err(convert::Interrupted.into());
            }
            progress.update(millis(t - *from), millis(*to - *from));
        }
        conv.event(t, &event)?;
    }

    let (mut out, summary) = conv.finish()?;
    out.flush().context("cannot write the GIF")?;
    Ok(Live::Done(summary))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // progress steps
fn millis(secs: f64) -> usize {
    (secs * 1000.0).max(0.0) as usize
}

fn read_cast(path: &Path) -> Result<cast::Recording> {
    let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    cast::read(BufReader::new(file)).with_context(|| format!("cannot read {}", path.display()))
}

/// Renders a whole recording to the GIF.
fn render(
    cli: &Cli,
    rec: &cast::Recording,
    renderer: &mut Renderer,
    timing: &Timing,
) -> Result<Summary> {
    set_ctrlc_handler();
    let output = File::create(&cli.output)
        .with_context(|| format!("cannot create {}", cli.output.display()))?;
    let (mut out, summary) = convert::to_gif(
        rec,
        renderer,
        timing,
        !cli.no_loop,
        BufWriter::new(output),
        &mut progress::Progress::new(!cli.quiet),
        &INTERRUPTED,
    )?;
    out.flush().context("cannot write the GIF")?;
    Ok(summary)
}

/// From now on, Ctrl-C stops the rendering, which removes the incomplete
/// GIF; a second Ctrl-C exits right away. Not while recording: there Ctrl-C
/// belongs to the recorded program. Without the handler, Ctrl-C simply ends
/// the program, so a failure to install it is no reason to stop.
fn set_ctrlc_handler() {
    let _ = ctrlc::set_handler(|| {
        if INTERRUPTED.swap(true, Ordering::Relaxed) {
            exit_interrupted();
        }
    });
}

fn load_bitmap_font(name: &str) -> Result<font::BitmapFonts> {
    let files = xfonts::find(name)?;
    Ok(font::BitmapFonts {
        regular: pcf::PcfFont::load(&files.regular)?,
        bold: files.bold.as_deref().map(pcf::PcfFont::load).transpose()?,
        wide: files.wide.as_deref().map(pcf::PcfFont::load).transpose()?,
    })
}

const DEFAULT_FONT_SIZE: f32 = 14.0;
const DEFAULT_LINE_HEIGHT: f32 = 1.4;

/// The fonts from the options or, without font options, from the xterm the
/// program runs in. Reports which xterm font is used if `verbose`.
fn choose_fonts(cli: &Cli) -> Result<font::Fonts> {
    let explicit = cli.bitmap_font.is_some()
        || !cli.font_family.is_empty()
        || cli.font_size.is_some()
        || cli.line_height.is_some();
    let scale = f32::from(cli.scale);
    let size = cli.font_size.unwrap_or(DEFAULT_FONT_SIZE) * scale;
    let line_height = Some(cli.line_height.unwrap_or(DEFAULT_LINE_HEIGHT));

    let detected = (!explicit && !cli.no_font_detect)
        .then(xterm::detect)
        .flatten();
    let Some(detection) = detected else {
        let bitmap = cli
            .bitmap_font
            .as_deref()
            .map(load_bitmap_font)
            .transpose()?;
        return font::Fonts::new(&cli.font_family, size, line_height, bitmap);
    };

    let note = |what: &str| {
        if cli.verbose {
            eprintln!(
                "term2gif: using xterm's font {what} (from {})",
                detection.source
            );
        }
    };
    let warn = |what: &str, e: anyhow::Error| {
        eprintln!("term2gif: cannot use xterm's font {what} ({e:#}), using the built-in font");
    };

    match &detection.font {
        xterm::XtermFont::Bitmap(name) => match load_bitmap_font(name) {
            Ok(bitmap) => {
                note(&format!("`{name}`"));
                font::Fonts::new(&[], size, line_height, Some(bitmap))
            }
            Err(e) => {
                warn(&format!("`{name}`"), e);
                font::Fonts::new(&[], size, line_height, None)
            }
        },
        // xterm's cell height is the font's own height, no extra spacing.
        xterm::XtermFont::Vector { families, size, .. } => {
            let what = format!("`{}` at {size} px", families.join(", "));
            let size = size * scale;
            match font::Fonts::new(families, size, None, None) {
                Ok(fonts) => {
                    note(&what);
                    Ok(fonts)
                }
                Err(e) => {
                    // fontconfig also offers X11 bitmap fonts, which only
                    // the bitmap font loader reads.
                    let bitmap = detection
                        .font
                        .bitmap_patterns()
                        .iter()
                        .find_map(|p| load_bitmap_font(p).ok());
                    if bitmap.is_some() {
                        note(&what);
                    } else {
                        warn(&what, e);
                    }
                    font::Fonts::new(&[], size, line_height, bitmap)
                }
            }
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    cli.validate()?;

    // Set up rendering first, so a typo in an option does not cost the user a
    // whole recording session.
    let theme = Theme::parse(&cli.theme)?;
    let fonts = choose_fonts(&cli)?;
    // Vector fonts are scaled by their size (in choose_fonts), bitmap fonts
    // by enlarging the picture.
    let pixel_scale = if fonts.is_bitmap() {
        cli.scale.into()
    } else {
        1
    };
    let mut renderer = Renderer::new(fonts, theme, cli.bold_is_bright, pixel_scale);

    let timing = Timing {
        speed: cli.speed,
        idle_time_limit: (cli.idle_time_limit > 0.0).then_some(cli.idle_time_limit),
        fps_cap: cli.fps_cap,
        last_frame_duration: cli.last_frame_duration,
    };

    let result = match &cli.from_cast {
        Some(path) => {
            let rec = read_cast(path)?;
            render(&cli, &rec, &mut renderer, &timing)
        }
        None => record_and_render(&cli, &mut renderer, &timing)?,
    };

    match result {
        Ok(summary) => {
            if !cli.quiet {
                eprintln!(
                    "term2gif: wrote {} ({}x{} pixels, {} frame{})",
                    cli.output.display(),
                    summary.width,
                    summary.height,
                    summary.frames,
                    if summary.frames == 1 { "" } else { "s" }
                );
            }
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&cli.output);
            Err(e)
        }
    }
}

/// Set by Ctrl-C while rendering.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Ends the program the way Ctrl-C does without a handler, so that the
/// caller sees the interruption (a shell, e.g., then also stops a loop or
/// script that runs term2gif).
fn exit_interrupted() -> ! {
    #[cfg(unix)]
    // SAFETY: restoring the default action of a signal and raising it have
    // no preconditions; this ends the process.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
        libc::raise(libc::SIGINT);
    }
    #[cfg(windows)]
    std::process::exit(windows_sys::Win32::Foundation::STATUS_CONTROL_C_EXIT);
    // Only if raising the signal failed: the status shells report for it.
    #[allow(unreachable_code)]
    std::process::exit(130)
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.is::<convert::Interrupted>() => {
            eprintln!("term2gif: {e:#}");
            exit_interrupted()
        }
        Err(e) => {
            eprintln!("term2gif: {e:#}");
            ExitCode::FAILURE
        }
    }
}
