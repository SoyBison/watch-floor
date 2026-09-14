//! watch-floor — live surveillance of a Python package, comparing the working
//! tree against the baseline committed at git HEAD. Two views: the class
//! inheritance structure, and the call traffic between functions.

mod collection;
mod dossier;
mod floor;
mod intercept;
mod manual;
mod network;
mod sitrep;
mod structure;
mod tripwire;

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser as ClapParser, ValueEnum};
use notify::RecursiveMode;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;

use collection::Target;
use dossier::Dossier;
use intercept::Interceptor;
use network::Network;
use sitrep::{leaf, NodeKey, Status};
use structure::Structure;
use tripwire::Tripwire;

#[derive(ClapParser, Debug)]
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

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
pub enum View {
    /// Class inheritance.
    Structure,
    /// Calls between functions and methods.
    Network,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Tree,
    Feed,
}

pub struct App {
    target: Target,
    interceptor: Interceptor,
    baseline: Dossier,
    current: Dossier,
    structure: Structure,
    network: Network,
    head_id: Option<String>,
    last_sync: Instant,
    scan_ms: u128,
    error: Option<String>,
    view: View,
    changes_only: bool,
    focus: Pane,
    tree_scroll: usize,
    feed_scroll: usize,
    help: bool,
    help_scroll: usize,
}

impl App {
    fn stand_up(target: Target, view: View, changes_only: bool) -> Result<Self> {
        let mut app = App {
            target,
            interceptor: Interceptor::new()?,
            baseline: Dossier::new("HEAD"),
            current: Dossier::new("working tree"),
            structure: Structure::default(),
            network: Network::default(),
            head_id: None,
            last_sync: Instant::now(),
            scan_ms: 0,
            error: None,
            view,
            changes_only,
            focus: Pane::Tree,
            tree_scroll: 0,
            feed_scroll: 0,
            help: false,
            help_scroll: 0,
        };
        app.resync(true);
        Ok(app)
    }

    /// Re-read both snapshots and recompile every view. The baseline is only
    /// re-read when HEAD moved, since that is the expensive side.
    fn resync(&mut self, force_baseline: bool) {
        let started = Instant::now();
        let head_id = self.target.head_id();

        if force_baseline || head_id != self.head_id {
            match self.target.survey_baseline(&mut self.interceptor) {
                Ok(baseline) => {
                    self.baseline = baseline;
                    self.head_id = head_id;
                }
                Err(err) => {
                    self.error = Some(format!("baseline unreadable: {err}"));
                    return;
                }
            }
        }

        match self.target.survey_working_tree(&mut self.interceptor) {
            Ok(current) => {
                self.current = current;
                self.error = None;
            }
            Err(err) => {
                self.error = Some(format!("working tree unreadable: {err}"));
                return;
            }
        }

        self.structure = Structure::compile(&self.baseline, &self.current);
        self.network = Network::compile(&self.baseline, &self.current);
        self.last_sync = Instant::now();
        self.scan_ms = started.elapsed().as_millis();
    }

    fn show(&mut self, view: View) {
        if self.view != view {
            self.view = view;
            self.tree_scroll = 0;
            self.feed_scroll = 0;
        }
    }

    /// Returns true when the operator wants out.
    fn on_key(&mut self, key: KeyEvent) -> bool {
        // While the manual is open it owns the scroll keys.
        let scroll = if self.help {
            &mut self.help_scroll
        } else {
            match self.focus {
                Pane::Tree => &mut self.tree_scroll,
                Pane::Feed => &mut self.feed_scroll,
            }
        };
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
            KeyCode::Char('q') => return true,
            KeyCode::Esc => {
                if self.help {
                    self.help = false;
                } else {
                    return true;
                }
            }
            KeyCode::Char('?') | KeyCode::Char('h') => {
                self.help = !self.help;
                self.help_scroll = 0;
            }
            KeyCode::Char('r') => self.resync(true),
            KeyCode::Char('1') => self.show(View::Structure),
            KeyCode::Char('2') => self.show(View::Network),
            KeyCode::Char('v') => {
                let next = match self.view {
                    View::Structure => View::Network,
                    View::Network => View::Structure,
                };
                self.show(next);
            }
            KeyCode::Char('c') => {
                self.changes_only = !self.changes_only;
                self.tree_scroll = 0;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match self.focus {
                    Pane::Tree => Pane::Feed,
                    Pane::Feed => Pane::Tree,
                }
            }
            KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => *scroll = scroll.saturating_add(1),
            KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
            KeyCode::PageDown => *scroll = scroll.saturating_add(10),
            KeyCode::Home | KeyCode::Char('g') => *scroll = 0,
            KeyCode::End | KeyCode::Char('G') => *scroll = usize::MAX,
            _ => {}
        }
        false
    }
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
        report(&app, args.view);
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

/// Plain-text rendering of the same picture, for `--once`.
fn report(app: &App, only: Option<View>) {
    println!(
        "WATCH FLOOR · target {} ({}/)",
        app.target.pkg_name, app.target.pkg_rel
    );
    println!(
        "  baseline {} ({} files)   current {} ({} files)   {}ms",
        app.baseline.label, app.baseline.files, app.current.label, app.current.files, app.scan_ms
    );

    if only != Some(View::Network) {
        report_structure(app);
    }
    if only != Some(View::Structure) {
        report_network(app);
    }
}

fn report_structure(app: &App) {
    let counts = app.structure.counts;
    print!("\nINHERITANCE STRUCTURE  ");
    for status in Status::STRUCTURE {
        print!("{} {} {}  ", status.glyph(), counts.of(status), status.tag());
    }
    println!("· {} classes", counts.total());

    for row in app.structure.rows(app.changes_only) {
        match &row.node.key {
            NodeKey::External(name) => println!("{}{name} <external>", row.prefix),
            NodeKey::Subject(qualname) => {
                let Some(entry) = app.structure.entries.get(qualname) else {
                    continue;
                };
                let mixins = if entry.mixins.is_empty() {
                    String::new()
                } else {
                    format!(" +{}", entry.mixins.join(" +"))
                };
                println!(
                    "{}{} {}{mixins}  {}{}",
                    row.prefix,
                    entry.status.glyph(),
                    row.node.label,
                    entry.subject.location(),
                    tag(entry.status)
                );
            }
            NodeKey::Operation(_) => {}
        }
    }

    let feed = app.structure.feed();
    println!("\nSITREP · structure · {} items", feed.len());
    for entry in feed {
        println!(
            " {} {:<9} {}  ({})",
            entry.status.glyph(),
            entry.status.tag(),
            entry.subject.display_name(),
            entry.subject.module
        );
        match entry.status {
            Status::Realigned => {
                println!("            was  {}", bases(&entry.prior_bases));
                println!("            now  {}", bases(&entry.subject.bases));
            }
            Status::Relocated => println!(
                "            moved {} → {}",
                entry.prior_module.clone().unwrap_or_default(),
                entry.subject.module
            ),
            Status::Activated => {
                println!("            inherits {}", bases(&entry.subject.bases))
            }
            Status::Burned => println!(
                "            held {} at {}",
                bases(&entry.subject.bases),
                entry.subject.location()
            ),
            _ => {}
        }
        print_delta(&entry.gained, &entry.lost);
    }
}

fn report_network(app: &App) {
    let counts = app.network.counts;
    print!("\nCALL TRAFFIC  ");
    for status in Status::NETWORK {
        print!("{} {} {}  ", status.glyph(), counts.of(status), status.tag());
    }
    println!(
        "· {} operations · {} links ({} inferred) · {} calls leave the package",
        counts.total(),
        app.network.links,
        app.network.probable,
        app.network.external
    );

    for row in app.network.rows(app.changes_only) {
        match &row.node.key {
            NodeKey::External(name) => println!("{}{name}", row.prefix),
            NodeKey::Operation(callsign) => {
                let Some(entry) = app.network.entries.get(callsign) else {
                    continue;
                };
                let note = row.node.note.clone().unwrap_or_default();
                let place = if row.node.repeat {
                    "⋯".to_string()
                } else {
                    entry.operation.location()
                };
                let edge = if row.node.edge.is_change() {
                    format!("  {}", row.node.edge.tag())
                } else {
                    String::new()
                };
                println!(
                    "{}{} {}{}{note}  {place}{edge}{}",
                    row.prefix,
                    entry.status.glyph(),
                    row.node.label,
                    if entry.operation.recursive { " ↻" } else { "" },
                    tag(entry.status)
                );
            }
            NodeKey::Subject(_) => {}
        }
    }

    let feed = app.network.feed();
    println!("\nSITREP · traffic · {} items", feed.len());
    for entry in feed {
        let op = &entry.operation;
        println!(
            " {} {:<9} {}{}  ({})",
            entry.status.glyph(),
            entry.status.tag(),
            op.display_name(),
            op.signature(),
            op.module
        );
        match entry.status {
            Status::Activated => {
                let calls: Vec<&str> = op.links.iter().map(|l| leaf(&l.target)).collect();
                println!(
                    "            calls {}",
                    if calls.is_empty() {
                        format!("nothing in-package, {} out", op.external)
                    } else {
                        calls.join(", ")
                    }
                );
            }
            Status::Burned => println!("            was at {}", op.source()),
            Status::Amended => println!(
                "            ({}) → {}",
                entry.prior_params.join(", "),
                op.signature()
            ),
            Status::Relocated => println!(
                "            moved {} → {}",
                entry.prior_module.clone().unwrap_or_default(),
                op.module
            ),
            _ => {}
        }
        print_delta(&entry.gained, &entry.lost);
        if let Some(base) = &entry.overrides {
            println!("            overrides {base}");
        }
    }
}

fn print_delta(gained: &[String], lost: &[String]) {
    if gained.is_empty() && lost.is_empty() {
        return;
    }
    let marks: Vec<String> = gained
        .iter()
        .map(|m| format!("+{m}"))
        .chain(lost.iter().map(|m| format!("-{m}")))
        .collect();
    println!("            {}", marks.join(" "));
}

fn tag(status: Status) -> String {
    if status.is_change() {
        format!("  {}", status.tag())
    } else {
        String::new()
    }
}

fn bases(list: &[String]) -> String {
    if list.is_empty() {
        "object".to_string()
    } else {
        list.join(", ")
    }
}
