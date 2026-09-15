//! Collection: locating the target package and pulling source out of the two
//! channels we compare — the working tree, and the baseline held in git HEAD.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

use crate::dossier::Dossier;
use crate::intercept::Interceptor;

/// The package under observation, plus the repository it lives in.
#[derive(Clone, Debug)]
pub struct Target {
    /// Repository root (the git toplevel).
    pub repo_root: PathBuf,
    /// Absolute path to the package directory.
    pub pkg_dir: PathBuf,
    /// Package directory relative to `repo_root`, with `/` separators.
    pub pkg_rel: String,
    /// Top-level module name, e.g. `mypkg`.
    pub pkg_name: String,
}

impl Target {
    /// Work out what we are watching, starting from a user-supplied path.
    pub fn acquire(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .with_context(|| format!("cannot open {}", path.display()))?;
        let pkg_dir = find_package(&path)?;
        let repo_root = git_root(&pkg_dir)?;
        let pkg_rel = pkg_dir
            .strip_prefix(&repo_root)
            .map_err(|_| anyhow!("{} is outside the repository", pkg_dir.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        let pkg_name = pkg_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or_else(|| anyhow!("package has no directory name"))?;
        Ok(Self {
            repo_root,
            pkg_dir,
            pkg_rel,
            pkg_name,
        })
    }

    /// The short object id of HEAD, or `None` on an unborn branch.
    pub fn head_id(&self) -> Option<String> {
        let out = git(&self.repo_root, &["rev-parse", "--short", "HEAD"]).ok()?;
        let id = String::from_utf8_lossy(&out).trim().to_string();
        (!id.is_empty()).then_some(id)
    }

    /// A path relative to the package directory, given one relative to the
    /// repository root.
    fn package_path<'a>(&self, repo_rel: &'a str) -> &'a str {
        if self.pkg_rel.is_empty() {
            repo_rel
        } else {
            repo_rel
                .strip_prefix(&format!("{}/", self.pkg_rel))
                .unwrap_or(repo_rel)
        }
    }

    /// A path relative to the repository root, given one relative to the
    /// package directory.
    fn repo_path(&self, pkg_rel_path: &str) -> String {
        if self.pkg_rel.is_empty() {
            pkg_rel_path.to_string()
        } else {
            format!("{}/{}", self.pkg_rel, pkg_rel_path)
        }
    }

    /// Read the package as it exists on disk right now.
    pub fn survey_working_tree(&self, interceptor: &mut Interceptor) -> Result<Dossier> {
        let mut dossier = Dossier::new("working tree");
        let mut files = Vec::new();
        collect_py(&self.pkg_dir, &mut files)?;
        files.sort();
        for file in files {
            let Ok(source) = fs::read_to_string(&file) else {
                continue;
            };
            let rel = file
                .strip_prefix(&self.pkg_dir)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            let module = module_of(&rel, &self.pkg_name);
            let shown = self.repo_path(&rel);
            dossier.files += 1;
            let sweep = interceptor.sweep(&source, &module, &shown);
            for subject in sweep.subjects {
                dossier.insert_subject(subject);
            }
            for operation in sweep.operations {
                dossier.insert_operation(operation);
            }
            for binding in sweep.imports {
                dossier.insert_binding(&module, binding);
            }
        }
        dossier.wire();
        Ok(dossier)
    }

    /// Read the package as it was committed at HEAD.
    pub fn survey_baseline(&self, interceptor: &mut Interceptor) -> Result<Dossier> {
        let Some(head) = self.head_id() else {
            return Ok(Dossier::new("HEAD (unborn)"));
        };
        let mut dossier = Dossier::new(format!("HEAD @ {head}"));

        // An empty pkg_rel means the repository root is itself the package;
        // git rejects "" as a pathspec and wants "." for "everything".
        let pathspec = if self.pkg_rel.is_empty() {
            "."
        } else {
            self.pkg_rel.as_str()
        };
        let listing = git(
            &self.repo_root,
            &["ls-tree", "-r", "-z", "--name-only", "HEAD", "--", pathspec],
        )?;
        let listing = String::from_utf8_lossy(&listing);

        for path in listing.split('\0').filter(|p| p.ends_with(".py")) {
            let rel = self.package_path(path);
            // The working-tree walk skips these; the baseline must agree, or a
            // committed __pycache__ shows up as classes that exist only at
            // baseline and are reported BURNED on an untouched tree.
            if in_skipped_dir(rel) {
                continue;
            }
            let Ok(blob) = git(&self.repo_root, &["show", &format!("HEAD:{path}")]) else {
                continue;
            };
            let source = String::from_utf8_lossy(&blob);
            let module = module_of(rel, &self.pkg_name);
            dossier.files += 1;
            let sweep = interceptor.sweep(&source, &module, path);
            for subject in sweep.subjects {
                dossier.insert_subject(subject);
            }
            for operation in sweep.operations {
                dossier.insert_operation(operation);
            }
            for binding in sweep.imports {
                dossier.insert_binding(&module, binding);
            }
        }
        dossier.wire();
        Ok(dossier)
    }
}

/// A path within the package becomes a dotted module path.
/// `sub/mod.py` -> `pkg.sub.mod`, `sub/__init__.py` -> `pkg.sub`.
fn module_of(rel: &str, pkg_name: &str) -> String {
    let mut parts = vec![pkg_name.to_string()];
    for part in rel.split('/') {
        let part = part.strip_suffix(".py").unwrap_or(part);
        if part == "__init__" || part.is_empty() {
            continue;
        }
        parts.push(part.to_string());
    }
    parts.join(".")
}

/// Whether any DIRECTORY component of a package-relative path is one the
/// working-tree walk refuses to descend into. The file name itself is exempt,
/// matching `collect_py`, which only applies the rule to directories.
fn in_skipped_dir(rel: &str) -> bool {
    let mut parts: Vec<&str> = rel.split('/').collect();
    parts.pop();
    parts
        .iter()
        .any(|part| part.starts_with('.') || matches!(*part, "__pycache__" | "node_modules"))
}

fn collect_py(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if entry.file_type()?.is_dir() {
            if name.starts_with('.') || matches!(name.as_str(), "__pycache__" | "node_modules") {
                continue;
            }
            collect_py(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "py") {
            out.push(path);
        }
    }
    Ok(())
}

/// Accept either the package directory itself, or a directory that contains
/// exactly one package (the usual flat or `src/` layout).
fn find_package(path: &Path) -> Result<PathBuf> {
    if path.join("__init__.py").is_file() {
        return Ok(path.to_path_buf());
    }

    let mut candidates = Vec::new();
    for dir in [path.to_path_buf(), path.join("src")] {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let child = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == "__pycache__" {
                continue;
            }
            if child.is_dir() && child.join("__init__.py").is_file() {
                candidates.push(child);
            }
        }
    }

    match candidates.len() {
        1 => Ok(candidates.remove(0)),
        0 => bail!(
            "no Python package under {} (looked for __init__.py here, in ./*/ and in ./src/*/)",
            path.display()
        ),
        _ => {
            let names: Vec<_> = candidates
                .iter()
                .filter_map(|c| c.file_name().map(|n| n.to_string_lossy().to_string()))
                .collect();
            bail!(
                "several packages under {} ({}) — name the one to watch",
                path.display(),
                names.join(", ")
            )
        }
    }
}

fn git_root(from: &Path) -> Result<PathBuf> {
    let out = git(from, &["rev-parse", "--show-toplevel"])
        .with_context(|| format!("{} is not inside a git repository", from.display()))?;
    Ok(PathBuf::from(String::from_utf8_lossy(&out).trim()))
}

fn git(cwd: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .context("failed to run git")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}
