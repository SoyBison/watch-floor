//! Tripwires on the watched paths. Any filesystem event on the package or on
//! git's refs means the picture may be stale.

use std::path::Path;
use std::sync::mpsc::{channel, Receiver, TryRecvError};

use anyhow::{Context, Result};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

pub struct Tripwire {
    // Held so the watcher thread stays alive for as long as we care.
    _watcher: RecommendedWatcher,
    signals: Receiver<()>,
}

impl Tripwire {
    pub fn arm(paths: &[(&Path, RecursiveMode)]) -> Result<Self> {
        let (tx, signals) = channel();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if res.is_ok() {
                let _ = tx.send(());
            }
        })
        .context("failed to start the filesystem watcher")?;

        for (path, mode) in paths {
            if path.exists() {
                watcher
                    .watch(path, *mode)
                    .with_context(|| format!("failed to watch {}", path.display()))?;
            }
        }

        Ok(Self {
            _watcher: watcher,
            signals,
        })
    }

    /// True if anything moved since the last check. Drains the backlog so a
    /// burst of events costs one resync, not one per event.
    pub fn tripped(&self) -> bool {
        let mut tripped = false;
        loop {
            match self.signals.try_recv() {
                Ok(()) => tripped = true,
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => return tripped,
            }
        }
    }
}
