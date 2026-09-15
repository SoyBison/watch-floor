//! Application state: the two snapshots, the views compiled from them, and the
//! operator's position on the floor.

use std::time::Instant;

use anyhow::Result;
use clap::ValueEnum;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::collection::Target;
use crate::dossier::Dossier;
use crate::intercept::Interceptor;
use crate::network::Network;
use crate::structure::Structure;

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
    pub target: Target,
    pub interceptor: Interceptor,
    pub baseline: Dossier,
    pub current: Dossier,
    pub structure: Structure,
    pub network: Network,
    pub head_id: Option<String>,
    pub last_sync: Instant,
    pub scan_ms: u128,
    pub error: Option<String>,
    pub view: View,
    pub changes_only: bool,
    pub focus: Pane,
    pub tree_scroll: usize,
    pub feed_scroll: usize,
    pub help: bool,
    pub help_scroll: usize,
}

impl App {
    pub fn stand_up(target: Target, view: View, changes_only: bool) -> Result<Self> {
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
    pub fn resync(&mut self, force_baseline: bool) {
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

    pub fn show(&mut self, view: View) {
        if self.view != view {
            self.view = view;
            self.tree_scroll = 0;
            self.feed_scroll = 0;
        }
    }

    /// Returns true when the operator wants out.
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
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
