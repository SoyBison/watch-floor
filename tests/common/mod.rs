//! A throwaway git repo holding a Python package, so tests exercise the real
//! baseline-vs-working-tree path instead of mocking git out.
//!
//! Typical shape:
//!
//! ```ignore
//! let fx = Fixture::new();
//! fx.write("pkg/core.py", "class A: ...");
//! fx.commit();                      // this is now the baseline
//! fx.write("pkg/core.py", "class A(B): ...");   // working tree drifts
//! let (baseline, current) = fx.snapshots();
//! ```

#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;
use watch_floor::app::{App, View};
use watch_floor::collection::Target;
use watch_floor::dossier::Dossier;
use watch_floor::intercept::Interceptor;
use watch_floor::network::Network;
use watch_floor::structure::Structure;

pub struct Fixture {
    _dir: TempDir,
    pub root: PathBuf,
    config: PathBuf,
}

impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}

impl Fixture {
    /// An empty repo on branch `main`, with a package directory ready to fill.
    pub fn new() -> Fixture {
        let dir = tempfile::tempdir().expect("temp dir");
        // Canonicalize up front: on macOS the temp dir is a symlink, and git
        // reports the resolved path, which would not match otherwise.
        let base = dir.path().canonicalize().expect("canonical temp dir");
        // The repo lives one level down so the isolating git config can sit
        // beside it rather than inside the tree under test.
        let root = base.join("repo");
        fs::create_dir_all(&root).expect("create repo dir");
        let config = base.join("gitconfig");
        fs::write(&config, "").expect("write empty git config");
        let fx = Fixture {
            _dir: dir,
            root,
            config,
        };
        fx.git(&["init", "-q", "-b", "main"]);
        fx
    }

    /// Write a file, creating parent directories as needed. Path is relative to
    /// the repo root, with `/` separators.
    pub fn write(&self, rel: &str, contents: &str) -> &Self {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(&path, contents).expect("write file");
        self
    }

    pub fn remove(&self, rel: &str) -> &Self {
        fs::remove_file(self.root.join(rel)).expect("remove file");
        self
    }

    /// Commit everything currently on disk. That snapshot becomes the baseline.
    pub fn commit(&self) -> &Self {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", "snapshot", "--allow-empty"]);
        self
    }

    /// Run git with a fully isolated configuration, so a developer's global
    /// settings (or a bare CI runner with none) cannot change the result.
    pub fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", &self.config)
            .env("GIT_CONFIG_SYSTEM", &self.config)
            .env("GIT_AUTHOR_NAME", "watch floor")
            .env("GIT_AUTHOR_EMAIL", "floor@example.invalid")
            .env("GIT_COMMITTER_NAME", "watch floor")
            .env("GIT_COMMITTER_EMAIL", "floor@example.invalid")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    pub fn target(&self) -> Target {
        Target::acquire(&self.root).expect("acquire target")
    }

    /// Both snapshots: `(baseline, current)`.
    pub fn snapshots(&self) -> (Dossier, Dossier) {
        let target = self.target();
        let mut interceptor = Interceptor::new().expect("interceptor");
        let baseline = target
            .survey_baseline(&mut interceptor)
            .expect("survey baseline");
        let current = target
            .survey_working_tree(&mut interceptor)
            .expect("survey working tree");
        (baseline, current)
    }

    pub fn structure(&self) -> Structure {
        let (baseline, current) = self.snapshots();
        Structure::compile(&baseline, &current)
    }

    pub fn network(&self) -> Network {
        let (baseline, current) = self.snapshots();
        Network::compile(&baseline, &current)
    }

    pub fn app(&self, view: View) -> App {
        App::stand_up(self.target(), view, false).expect("stand up app")
    }
}

/// Parse a single source string with no git involved, for unit-level checks.
pub fn sweep(source: &str) -> watch_floor::intercept::Sweep {
    let mut interceptor = Interceptor::new().expect("interceptor");
    interceptor.sweep(source, "pkg.mod", "pkg/mod.py")
}

/// A one-file package, committed, with the working tree then replaced.
/// Returns the compiled views for the before/after pair.
pub fn diff_one_file(before: &str, after: &str) -> (Structure, Network) {
    let fx = Fixture::new();
    fx.write("pkg/__init__.py", "");
    fx.write("pkg/mod.py", before);
    fx.commit();
    fx.write("pkg/mod.py", after);
    (fx.structure(), fx.network())
}
