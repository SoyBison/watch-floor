//! Command-line shell around the watch-floor library: parse arguments, take the
//! terminal, and hold the watch until the operator leaves.

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use notify::RecursiveMode;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;

use watch_floor::app::{App, View};
use watch_floor::collection::Target;
use watch_floor::tripwire::Tripwire;
use watch_floor::{floor, manual, report};

#[derive(Parser, Debug)]
#[command(
    name = "watch-floor",
    about = "Live view of what your edits are doing to a Python package"
)]
struct Args {
    /// Package directory, or a directory containing exactly one package.
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Print one report to stdout and exit instead of holding the watch.
    #[arg(long)]
    once: bool,

    /// Start with the tree pruned to changed branches only.
    #[arg(short, long)]
    changes_only: bool,

    /// Which view to open on (`--once` prints both unless this is given).
    #[arg(long, value_enum)]
    view: Option<View>,

    /// Print the field manual to stdout and exit. Needs no target.
    #[arg(long)]
    manual: bool,
}

fn main() -> Result<()> {
    // Without this, piping `--once` into `head` kills us with a panic instead
    // of the quiet exit every other command-line tool manages.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let args = Args::parse();

    if args.manual {
        let width = ratatui::crossterm::terminal::size()
            .map(|(w, _)| w)
            .unwrap_or(80)
            .clamp(60, 100);
        print!("{}", manual::plain(width));
        return Ok(());
    }

    let target = Target::acquire(&args.path)?;
    let app = App::stand_up(
        target,
        args.view.unwrap_or(View::Structure),
        args.changes_only,
    )?;

    if args.once {
        print!("{}", report::render(&app, args.view));
        return Ok(());
    }
    hold_the_watch(app)
}

fn hold_the_watch(mut app: App) -> Result<()> {
    let git_dir = app.target.repo_root.join(".git");
    let tripwire = Tripwire::arm(&[
        (app.target.pkg_dir.as_path(), RecursiveMode::Recursive),
        (git_dir.as_path(), RecursiveMode::NonRecursive),
        (&git_dir.join("refs"), RecursiveMode::Recursive),
    ])?;

    let mut terminal = take_the_screen()?;
    let outcome = (|| -> Result<()> {
        let mut pending: Option<Instant> = None;
        loop {
            terminal.draw(|frame| floor::draw(frame, &mut app))?;

            if event::poll(Duration::from_millis(200))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press && app.on_key(key) {
                        return Ok(());
                    }
                }
            }

            if tripwire.tripped() {
                pending = Some(Instant::now());
            }
            // Settle briefly so a burst of writes costs one resync.
            if pending.is_some_and(|at| at.elapsed() >= Duration::from_millis(150)) {
                pending = None;
                app.resync(false);
            }
        }
    })();

    give_back_the_screen(&mut terminal)?;
    outcome
}

type Screen = Terminal<CrosstermBackend<Stdout>>;

fn take_the_screen() -> Result<Screen> {
    enable_raw_mode().context("failed to enter raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    // A panic must not leave the operator staring at a broken terminal.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        hook(info);
    }));

    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn give_back_the_screen(terminal: &mut Screen) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
