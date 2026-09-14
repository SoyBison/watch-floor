//! The traffic view: who calls whom, and how that changed. Call resolution is
//! best-effort — Python dispatches at runtime — so every edge carries a
//! confidence, and unresolved calls are counted as traffic leaving the package
//! rather than guessed at.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::dossier::{Confidence, Dossier, Operation};
use crate::sitrep::{delta, flatten, leaf, Counts, Edge, NodeKey, Row, Status, TreeNode};

#[derive(Clone, Debug)]
pub struct Entry {
    /// Current-side operation, or the baseline-side one if it is gone.
    pub operation: Operation,
    pub status: Status,
    /// Module the operation was in before it moved.
    pub prior_module: Option<String>,
    /// Signature at baseline, when it differs from the current one.
    pub prior_params: Vec<String>,
    /// Call targets picked up / dropped since baseline, as display names.
    pub gained: Vec<String>,
    pub lost: Vec<String>,
    /// The inherited method this one overrides, as a display name.
    pub overrides: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Network {
    pub entries: BTreeMap<String, Entry>,
    pub roots: Vec<TreeNode>,
    pub counts: Counts,
    /// Resolved edges in the current snapshot.
    pub links: usize,
    /// Of those, how many rest on an inferred name match.
    pub probable: usize,
    /// Calls in the current snapshot that left the package.
    pub external: usize,
}

/// A call edge in the union of both snapshots.
#[derive(Clone, Copy, Debug)]
struct Wire {
    edge: Edge,
    confidence: Confidence,
}

impl Network {
    pub fn compile(baseline: &Dossier, current: &Dossier) -> Network {
        let mut entries = BTreeMap::new();

        for (callsign, now) in &current.operations {
            let entry = match baseline.operations.get(callsign) {
                Some(then) => {
                    let (gained, lost) = delta(&then.targets(), &now.targets());
                    let status = if !gained.is_empty() || !lost.is_empty() {
                        Status::Rerouted
                    } else if then.params != now.params
                        || then.decorators != now.decorators
                        || then.is_async != now.is_async
                    {
                        Status::Amended
                    } else {
                        Status::Nominal
                    };
                    Entry {
                        operation: now.clone(),
                        status,
                        prior_module: None,
                        prior_params: then.params.clone(),
                        gained,
                        lost,
                        overrides: None,
                    }
                }
                None => Entry {
                    operation: now.clone(),
                    status: Status::Activated,
                    prior_module: None,
                    prior_params: Vec::new(),
                    gained: Vec::new(),
                    lost: Vec::new(),
                    overrides: None,
                },
            };
            entries.insert(callsign.clone(), entry);
        }

        for (callsign, then) in &baseline.operations {
            if current.operations.contains_key(callsign) {
                continue;
            }
            entries.insert(
                callsign.clone(),
                Entry {
                    operation: then.clone(),
                    status: Status::Burned,
                    prior_module: None,
                    prior_params: Vec::new(),
                    gained: Vec::new(),
                    lost: Vec::new(),
                    overrides: None,
                },
            );
        }

        let remap = pair_relocations(&mut entries);

        // Resolve call targets to readable names, and note overrides.
        let names: BTreeMap<String, String> = entries
            .iter()
            .map(|(k, e)| (k.clone(), e.operation.display_name()))
            .collect();
        let show = |callsign: &String| {
            names
                .get(callsign)
                .cloned()
                .unwrap_or_else(|| leaf(callsign).to_string())
        };
        for entry in entries.values_mut() {
            entry.gained = entry.gained.iter().map(show).collect();
            entry.lost = entry.lost.iter().map(show).collect();
            let source = if current.operations.contains_key(&entry.operation.callsign) {
                current
            } else {
                baseline
            };
            entry.overrides = source
                .overridden(&entry.operation)
                .map(|hit| show(&hit))
                .filter(|_| entry.operation.owner.is_some());
        }

        let mut network = Network {
            entries,
            roots: Vec::new(),
            counts: Counts::default(),
            links: current.operations.values().map(|o| o.links.len()).sum(),
            probable: current
                .operations
                .values()
                .flat_map(|o| &o.links)
                .filter(|l| l.confidence == Confidence::Probable)
                .count(),
            external: current.operations.values().map(|o| o.external).sum(),
        };
        for entry in network.entries.values() {
            network.counts.record(entry.status);
        }
        network.build_forest(baseline, current, &remap);
        network
    }

    /// Lay the call graph out from its entry points. Cycles and shared callees
    /// are expanded once; later appearances are drawn as leaves.
    fn build_forest(&mut self, baseline: &Dossier, current: &Dossier, remap: &BTreeMap<String, String>) {
        let mut wires: BTreeMap<String, BTreeMap<String, Wire>> = BTreeMap::new();
        // A relocated operation is keyed by where it is now; its baseline self
        // sits under the callsign it used to have.
        let was: BTreeMap<&String, &String> = remap.iter().map(|(old, new)| (new, old)).collect();

        for callsign in self.entries.keys() {
            let before_key = was.get(callsign).copied().unwrap_or(callsign);
            let then = baseline.operations.get(before_key);
            let now = current.operations.get(callsign);
            // An operation that only exists on one side already says so through
            // its own status; marking all of its traffic as new would be noise.
            let both_sides = then.is_some() && now.is_some();

            let before: BTreeSet<String> = then
                .map(|op| {
                    op.links
                        .iter()
                        .map(|l| remap.get(&l.target).unwrap_or(&l.target).clone())
                        .collect()
                })
                .unwrap_or_default();

            let outgoing = wires.entry(callsign.clone()).or_default();
            if let Some(now) = now {
                for link in &now.links {
                    let edge = if both_sides && !before.contains(&link.target) {
                        Edge::Opened
                    } else {
                        Edge::Live
                    };
                    outgoing.insert(
                        link.target.clone(),
                        Wire {
                            edge,
                            confidence: link.confidence,
                        },
                    );
                }
            }
            if let Some(then) = then {
                for link in &then.links {
                    let target = remap.get(&link.target).unwrap_or(&link.target).clone();
                    outgoing.entry(target).or_insert(Wire {
                        edge: if both_sides { Edge::Closed } else { Edge::Live },
                        confidence: link.confidence,
                    });
                }
            }
            outgoing.retain(|target, _| self.entries.contains_key(target));
        }

        // Entry points: nothing in the package calls them.
        let called: BTreeSet<&String> = wires.values().flat_map(|w| w.keys()).collect();
        let mut roots: Vec<String> = self
            .entries
            .keys()
            .filter(|k| !called.contains(k))
            .cloned()
            .collect();
        let order = |a: &String, b: &String| {
            let name = |k: &String| {
                self.entries
                    .get(k)
                    .map(|e| e.operation.display_name())
                    .unwrap_or_default()
            };
            (name(a), a.clone()).cmp(&(name(b), b.clone()))
        };
        roots.sort_by(&order);

        let mut expanded = HashSet::new();
        let mut forest: Vec<TreeNode> = roots
            .iter()
            .filter_map(|k| self.grow(k, Edge::Live, Confidence::Confirmed, &wires, &mut expanded))
            .collect();

        // Whatever is left is only reachable through a cycle.
        let stranded: Vec<String> = {
            let mut left: Vec<String> = self
                .entries
                .keys()
                .filter(|k| !expanded.contains(*k))
                .cloned()
                .collect();
            left.sort_by(&order);
            left
        };
        if !stranded.is_empty() {
            let children: Vec<TreeNode> = stranded
                .iter()
                .filter_map(|k| {
                    self.grow(k, Edge::Live, Confidence::Confirmed, &wires, &mut expanded)
                })
                .collect();
            let mut node = TreeNode::new(
                NodeKey::External("<cycle>".to_string()),
                "<cycle>".to_string(),
            );
            node.touched = children.iter().any(|c| c.touched);
            node.children = children;
            forest.push(node);
        }

        self.roots = forest;
    }

    fn grow(
        &self,
        callsign: &str,
        edge: Edge,
        confidence: Confidence,
        wires: &BTreeMap<String, BTreeMap<String, Wire>>,
        expanded: &mut HashSet<String>,
    ) -> Option<TreeNode> {
        let entry = self.entries.get(callsign)?;
        let mut node = TreeNode::new(
            NodeKey::Operation(callsign.to_string()),
            entry.operation.display_name(),
        );
        node.edge = edge;
        if confidence == Confidence::Probable {
            node.note = Some("?".to_string());
        }

        if !expanded.insert(callsign.to_string()) {
            // Seen further up or in an earlier branch: draw the node, stop here.
            node.repeat = true;
            node.touched = entry.status.is_change() || edge.is_change();
            return Some(node);
        }

        let mut children: Vec<(String, TreeNode)> = wires
            .get(callsign)
            .into_iter()
            .flatten()
            .filter_map(|(target, wire)| {
                self.grow(target, wire.edge, wire.confidence, wires, expanded)
                    .map(|child| (child.label.clone(), child))
            })
            .collect();
        children.sort_by(|a, b| a.0.cmp(&b.0));

        node.children = children.into_iter().map(|(_, c)| c).collect();
        node.touched = entry.status.is_change()
            || edge.is_change()
            || node.children.iter().any(|c| c.touched);
        Some(node)
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
        feed.sort_by(|a, b| {
            (a.status, &a.operation.callsign).cmp(&(b.status, &b.operation.callsign))
        });
        feed
    }
}

/// An operation that vanished from one place and appeared in another, making
/// the same calls, has moved rather than been rewritten. Returns the mapping
/// from retired callsigns to their replacements so edges can be redirected.
fn pair_relocations(entries: &mut BTreeMap<String, Entry>) -> BTreeMap<String, String> {
    let mut activated: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut burned: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (callsign, entry) in entries.iter() {
        match entry.status {
            Status::Activated => activated
                .entry(entry.operation.name.clone())
                .or_default()
                .push(callsign.clone()),
            Status::Burned => burned
                .entry(entry.operation.name.clone())
                .or_default()
                .push(callsign.clone()),
            _ => {}
        }
    }

    let mut remap = BTreeMap::new();
    for (name, new_keys) in activated {
        let Some(old_keys) = burned.get(&name) else {
            continue;
        };
        if new_keys.len() != 1 || old_keys.len() != 1 {
            continue;
        }
        let (new_key, old_key) = (&new_keys[0], &old_keys[0]);
        let old = entries[old_key].operation.clone();
        let new = &entries[new_key].operation;
        // Compare the calls as written: resolved targets shift when the callee
        // moves too, but the source text does not.
        if old.raw_calls != new.raw_calls {
            continue;
        }
        let entry = entries.get_mut(new_key).expect("key just read");
        entry.status = Status::Relocated;
        entry.prior_module = Some(old.module.clone());
        entry.prior_params = old.params.clone();
        remap.insert(old_key.clone(), new_key.clone());
    }
    for key in remap.keys() {
        entries.remove(key);
    }
    remap
}
