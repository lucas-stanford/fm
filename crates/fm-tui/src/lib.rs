//! The terminal UI for managing automations.
//!
//! Built against [`fm_model::Host`] only: every screen is drawn from what the host returns, and
//! every action goes through it. [`fake::FakeHost`] stands in for tests and demos
//! (`cargo run -p fm-tui --example fake`).
//!
//! Screens mirror pi-automations' `/automations` page: the list page (Needs you, Running now,
//! Coming up, Watching, Off and disabled), an automation page, a run page with `g` for the
//! flowchart, a check page, the consent preview, and the agent picker.

pub mod app;
pub mod editor;
pub mod fake;
pub mod glyphs;
pub mod opener;
pub mod text;
pub mod theme;
pub mod triage;
pub mod view;
pub mod words;

use std::io::{self, Stdout};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fm_model::Host;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::{Hide, Show};
use ratatui::crossterm::event::{self, Event};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

use crate::app::{App, Effect};
use crate::editor::Suspend;
use crate::glyphs::Glyphs;
use crate::opener::{Opener, SystemOpener};
use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiOptions {
    pub theme: Theme,
    /// Draw with ASCII only, for fonts without the glyphs.
    pub ascii: bool,
    /// The editor command, instead of `$VISUAL` / `$EDITOR`.
    pub editor: Option<String>,
    /// Re-read this often while something runs.
    pub refresh_busy: Duration,
    /// Re-read this often otherwise.
    pub refresh_idle: Duration,
}

impl Default for TuiOptions {
    fn default() -> Self {
        TuiOptions {
            theme: Theme::default(),
            ascii: false,
            editor: None,
            refresh_busy: Duration::from_secs(1),
            refresh_idle: Duration::from_secs(10),
        }
    }
}

impl TuiOptions {
    /// Colour from `NO_COLOR`, `COLORTERM`, `COLORFGBG` and `FM_THEME`; ASCII from `FM_ASCII=1`.
    pub fn from_env() -> TuiOptions {
        TuiOptions {
            theme: Theme::from_env(),
            ascii: std::env::var("FM_ASCII").is_ok_and(|v| !v.is_empty() && v != "0"),
            ..TuiOptions::default()
        }
    }
}

fn enter_screen() -> io::Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, Hide)
}

fn leave_screen() -> io::Result<()> {
    let raw = disable_raw_mode();
    execute!(io::stdout(), LeaveAlternateScreen, Show)?;
    raw
}

/// The real terminal: entered on creation, left on drop, and on panic by the hook `run` installs.
struct Tty {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl Tty {
    fn enter() -> io::Result<Tty> {
        enter_screen()?;
        match Terminal::new(CrosstermBackend::new(io::stdout())) {
            Ok(terminal) => Ok(Tty { terminal }),
            Err(e) => {
                let _ = leave_screen();
                Err(e)
            }
        }
    }
}

impl Drop for Tty {
    fn drop(&mut self) {
        let _ = leave_screen();
    }
}

impl Suspend for Tty {
    fn suspend(&mut self) -> io::Result<()> {
        leave_screen()
    }

    fn resume(&mut self) -> io::Result<()> {
        enter_screen()?;
        self.terminal.clear()
    }
}

type PanicHook = Arc<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync + 'static>;

/// Restores the terminal before a panic message prints, and the previous hook afterwards.
struct PanicGuard {
    previous: PanicHook,
}

impl PanicGuard {
    fn install() -> PanicGuard {
        let previous: PanicHook = Arc::from(std::panic::take_hook());
        let chained = previous.clone();
        std::panic::set_hook(Box::new(move |info| {
            let _ = leave_screen();
            chained(info);
        }));
        PanicGuard { previous }
    }
}

impl Drop for PanicGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let previous = self.previous.clone();
        std::panic::set_hook(Box::new(move |info| previous(info)));
    }
}

/// Run the UI on `host` until the person closes it.
pub fn run(host: &mut dyn Host, opts: TuiOptions) -> io::Result<()> {
    run_with(host, opts, &mut SystemOpener)
}

/// [`run`] on a [`fake::FakeHost`] with demo data, for `fm tui --fake`.
pub fn run_fake(opts: TuiOptions) -> io::Result<()> {
    let mut host = fake::FakeHost::demo();
    run(&mut host, opts)
}

fn local_now() -> words::Now {
    chrono::Local::now().fixed_offset()
}

/// [`run`] with the link opener supplied.
pub fn run_with(host: &mut dyn Host, opts: TuiOptions, opener: &mut dyn Opener) -> io::Result<()> {
    let _panic = PanicGuard::install();
    let mut tty = Tty::enter()?;
    let mut app = App::load(host, local_now(), opts.theme, Glyphs::new(opts.ascii));
    let candidates = match &opts.editor {
        Some(cmd) => vec![editor::split_command(cmd)],
        None => editor::editor_candidates(&|k| std::env::var(k).ok(), cfg!(windows)),
    };
    let mut last_read = Instant::now();
    loop {
        app.now = local_now();
        app.expire_flash(Instant::now());
        let size = tty.terminal.size()?;
        app.page_rows = view::body_rows(size.height as usize);
        app.prepare(host, size.width);
        tty.terminal.draw(|f| view::draw(f, &app))?;

        let every = app.refresh_every(opts.refresh_idle, opts.refresh_busy);
        // Wake at least once a second, so times and messages move even when nothing is read.
        let wait = every
            .saturating_sub(last_read.elapsed())
            .min(Duration::from_secs(1));
        if event::poll(wait)? {
            match event::read()? {
                Event::Key(k) => match app.handle_event(host, k) {
                    Effect::Quit => break,
                    Effect::Open(url) => {
                        let r = opener.open(&url);
                        app.after_open(&url, r);
                    }
                    Effect::Edit { key, path } => {
                        let before = app.status(&key).cloned();
                        let answer = editor::with_terminal_released(&mut tty, &mut || {
                            editor::launch(&candidates, &path)
                        })?;
                        app.after_edit(host, &key, before, answer);
                        last_read = Instant::now();
                    }
                    Effect::None => {}
                },
                Event::Resize(..) => {}
                _ => {}
            }
            if app.quit {
                break;
            }
        }
        if last_read.elapsed() >= every {
            app.refresh(host);
            last_read = Instant::now();
        }
    }
    Ok(())
}
