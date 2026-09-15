//! The structure view: baseline against current, reduced to a single class
//! inheritance forest with per-class status.

use std::collections::{BTreeMap, HashSet};

use crate::dossier::{resolve, Dossier, Subject};
use crate::sitrep::{delta, flatten, leaf, Counts, NodeKey, Row, Status, TreeNode};

#[derive(Clone, Debug)]
pub struct Entry {
    /// Current-side subject, or the baseline-side one if the class is gone.
    pub subject: Subject,
    pub status: Status,
    /// Base classes at baseline, when they differ from the current ones.
    pub prior_bases: Vec<String>,
    /// Module the class was in before it moved.
    pub prior_module: Option<String>,
    /// Methods added / removed since baseline.
    pub gained: Vec<String>,
    pub lost: Vec<String>,
    /// Bases beyond the one used to place the class in the forest.
    pub mixins: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Structure {
    pub entries: BTreeMap<String, Entry>,
    pub roots: Vec<TreeNode>,
    pub counts: Counts,
}

impl Structure {
    pub fn compile(baseline: &Dossier, current: &Dossier) -> Structure {
        let mut entries = BTreeMap::new();

        for (qualname, now) in &current.subjects {
            let entry = match baseline.subjects.get(qualname) {
                Some(then) => {
                    let (gained, lost) = delta(&then.methods, &now.methods);
                    let status = if then.bases != now.bases {
                        Status::Realigned
                    } else if !gained.is_empty() || !lost.is_empty() {
                        Status::Amended
                    } else {
                        Status::Nominal
                    };
                    Entry {
                        subject: now.clone(),
                        status,
                        prior_bases: then.bases.clone(),
                        prior_module: None,
                        gained,
                        lost,
                        mixins: Vec::new(),
                    }
                }
                None => Entry {
                    subject: now.clone(),
                    status: Status::Activated,
                    prior_bases: Vec::new(),
                    prior_module: None,
                    gained: Vec::new(),
                    lost: Vec::new(),
                    mixins: Vec::new(),
                },
            };
            entries.insert(qualname.clone(), entry);
        }

        for (qualname, then) in &baseline.subjects {
            if current.subjects.contains_key(qualname) {
                continue;
            }
            entries.insert(
                qualname.clone(),
                Entry {
                    subject: then.clone(),
                    status: Status::Burned,
                    prior_bases: Vec::new(),
                    prior_module: None,
                    gained: Vec::new(),
                    lost: Vec::new(),
                    mixins: Vec::new(),
                },
            );
        }

        pair_relocations(&mut entries);

        let mut structure = Structure {
            entries,
            roots: Vec::new(),
            counts: Counts::default(),
        };
        for entry in structure.entries.values() {
            structure.counts.record(entry.status);
        }
        structure.build_forest();
        structure
    }

    /// Project the inheritance graph onto a tree: each class hangs off its first
    /// resolvable base, and any remaining bases are recorded as mixins.
    fn build_forest(&mut self) {
        let union: BTreeMap<String, Subject> = self
            .entries
            .iter()
            .map(|(k, e)| (k.clone(), e.subject.clone()))
            .collect();

        let mut by_parent: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut by_external: BTreeMap<String, Vec<String>> = BTreeMap::new();

        for (qualname, entry) in self.entries.iter_mut() {
            let mut internal = Vec::new();
            let mut external = Vec::new();
            for base in &entry.subject.bases {
                match resolve(&union, base, &entry.subject.module) {
                    Some(hit) => internal.push(hit.to_string()),
                    None => external.push(base.clone()),
                }
            }

            let (parent, mixins) = match (internal.first(), external.first()) {
                (Some(primary), _) => {
                    let mut mixins: Vec<String> =
                        internal[1..].iter().map(|q| leaf(q).to_string()).collect();
                    mixins.extend(external.iter().cloned());
                    (Parent::Subject(primary.clone()), mixins)
                }
                (None, Some(root)) => (Parent::External(root.clone()), external[1..].to_vec()),
                (None, None) => (Parent::External("object".to_string()), Vec::new()),
            };
            entry.mixins = mixins;

            match parent {
                Parent::Subject(p) if p != *qualname => {
                    by_parent.entry(p).or_default().push(qualname.clone())
                }
                // Self-inheritance is nonsense; park it at the root.
                Parent::Subject(_) => by_external
                    .entry("object".to_string())
                    .or_default()
                    .push(qualname.clone()),
                Parent::External(name) => {
                    by_external.entry(name).or_default().push(qualname.clone())
                }
            }
        }

        let entries = &self.entries;
        let order = |a: &String, b: &String| {
            let name = |q: &String| {
                entries
                    .get(q)
                    .map(|e| e.subject.name.clone())
                    .unwrap_or_default()
            };
            (name(a), a.clone()).cmp(&(name(b), b.clone()))
        };
        for kids in by_parent.values_mut().chain(by_external.values_mut()) {
            kids.sort_by(&order);
        }

        let mut placed = HashSet::new();
        let mut roots: Vec<TreeNode> = by_external
            .iter()
            .map(|(name, kids)| {
                let children: Vec<TreeNode> = kids
                    .iter()
                    .filter_map(|k| grow(k, &self.entries, &by_parent, &mut placed))
                    .collect();
                let mut node = TreeNode::new(NodeKey::External(name.clone()), name.clone());
                node.touched = children.iter().any(|c| c.touched);
                node.children = children;
                node
            })
            .collect();

        // Anything unreachable from a root sits in an inheritance cycle; surface
        // it rather than dropping it silently.
        let stranded: Vec<String> = self
            .entries
            .keys()
            .filter(|k| !placed.contains(*k))
            .cloned()
            .collect();
        if !stranded.is_empty() {
            let children: Vec<TreeNode> = stranded
                .iter()
                .filter_map(|k| grow(k, &self.entries, &by_parent, &mut placed))
                .collect();
            let mut node = TreeNode::new(
                NodeKey::External("<cycle>".to_string()),
                "<cycle>".to_string(),
            );
            node.touched = children.iter().any(|c| c.touched);
            node.children = children;
            roots.push(node);
        }

        self.roots = roots;
    }

    pub fn rows(&self, changes_only: bool) -> Vec<Row<'_>> {
        flatten(&self.roots, changes_only)
    }

    /// Changed entries, ordered for the feed: most disruptive first.
    pub fn feed(&self) -> Vec<&Entry> {
        let mut feed: Vec<&Entry> = self
            .entries
            .values()
            .filter(|e| e.status.is_change())
            .collect();
        feed.sort_by(|a, b| (a.status, &a.subject.qualname).cmp(&(b.status, &b.subject.qualname)));
        feed
    }
}

enum Parent {
    Subject(String),
    External(String),
}

fn grow(
    qualname: &str,
    entries: &BTreeMap<String, Entry>,
    by_parent: &BTreeMap<String, Vec<String>>,
    placed: &mut HashSet<String>,
) -> Option<TreeNode> {
    if !placed.insert(qualname.to_string()) {
        return None;
    }
    let entry = entries.get(qualname)?;
    let children: Vec<TreeNode> = by_parent
        .get(qualname)
        .into_iter()
        .flatten()
        .filter_map(|k| grow(k, entries, by_parent, placed))
        .collect();
    let mut node = TreeNode::new(
        NodeKey::Subject(qualname.to_string()),
        entry.subject.display_name(),
    );
    node.touched = entry.status.holds_branch() || children.iter().any(|c| c.touched);
    node.children = children;
    Some(node)
}

/// A class that vanished from one module and appeared in another, unchanged,
/// has moved rather than been rewritten.
fn pair_relocations(entries: &mut BTreeMap<String, Entry>) {
    let mut activated: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut burned: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (qualname, entry) in entries.iter() {
        match entry.status {
            Status::Activated => activated
                .entry(entry.subject.name.clone())
                .or_default()
                .push(qualname.clone()),
            Status::Burned => burned
                .entry(entry.subject.name.clone())
                .or_default()
                .push(qualname.clone()),
            _ => {}
        }
    }

    let mut retired = Vec::new();
    for (name, new_keys) in activated {
        let Some(old_keys) = burned.get(&name) else {
            continue;
        };
        // Only an unambiguous one-for-one swap counts as a move.
        if new_keys.len() != 1 || old_keys.len() != 1 {
            continue;
        }
        let (new_key, old_key) = (&new_keys[0], &old_keys[0]);
        let old = entries[old_key].subject.clone();
        let new = &entries[new_key].subject;
        if old.bases != new.bases {
            continue;
        }
        let (gained, lost) = delta(&old.methods, &new.methods);
        let entry = entries.get_mut(new_key).expect("key just read");
        entry.status = Status::Relocated;
        entry.prior_module = Some(old.module.clone());
        entry.gained = gained;
        entry.lost = lost;
        retired.push(old_key.clone());
    }
    for key in retired {
        entries.remove(&key);
    }
}
