//! Shared vocabulary for every view on the floor: how a thing changed, how the
//! change is tallied, and how a forest of changes is flattened for drawing.

use std::collections::BTreeSet;

/// What happened to a class or an operation between the baseline and now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// New, no baseline counterpart.
    Activated,
    /// Present at baseline, gone from the working tree.
    Burned,
    /// A class changed base classes.
    Realigned,
    /// An operation changed who it calls.
    Rerouted,
    /// Same thing, same connections, different module.
    Relocated,
    /// Contents changed but the connections held: a class's method set, or an
    /// operation's signature or decorators.
    Amended,
    /// Untouched itself, but sitting in the wake of a move made elsewhere: a
    /// callee relocated out from under it. Reported so the fact is visible,
    /// but too soft to count as this operation having changed.
    Wake,
    /// No observed change.
    Nominal,
}

impl Status {
    /// Statuses a class can carry, in legend order.
    pub const STRUCTURE: [Status; 5] = [
        Status::Activated,
        Status::Burned,
        Status::Realigned,
        Status::Relocated,
        Status::Amended,
    ];

    /// Statuses an operation can carry, in legend order.
    pub const NETWORK: [Status; 6] = [
        Status::Activated,
        Status::Burned,
        Status::Rerouted,
        Status::Relocated,
        Status::Amended,
        Status::Wake,
    ];

    pub fn tag(self) -> &'static str {
        match self {
            Status::Activated => "ACTIVATED",
            Status::Burned => "BURNED",
            Status::Realigned => "REALIGNED",
            Status::Rerouted => "REROUTED",
            Status::Relocated => "RELOCATED",
            Status::Amended => "AMENDED",
            Status::Wake => "WAKE",
            Status::Nominal => "",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Status::Activated => "+",
            Status::Burned => "-",
            // Realigned and Rerouted never share a view, so they share a glyph.
            Status::Realigned | Status::Rerouted => "~",
            Status::Relocated => "→",
            Status::Amended => "*",
            Status::Wake => "≈",
            Status::Nominal => "·",
        }
    }

    pub fn is_change(self) -> bool {
        self != Status::Nominal
    }

    /// Whether this status alone justifies keeping a branch on screen when the
    /// changes-only filter is on. A wake is worth reporting but not worth
    /// dragging a whole branch into view: a widely-called function moving
    /// would otherwise light up every one of its callers.
    pub fn holds_branch(self) -> bool {
        self.is_change() && self != Status::Wake
    }
}

/// The state of the link that reaches a node from its parent. Inheritance links
/// are static, so only call traffic uses anything but `Live`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Live,
    /// Traffic that did not exist at baseline.
    Opened,
    /// Traffic present at baseline and now silent.
    Closed,
}

impl Edge {
    pub fn tag(self) -> &'static str {
        match self {
            Edge::Live => "",
            Edge::Opened => "NEW TRAFFIC",
            Edge::Closed => "WENT DARK",
        }
    }

    pub fn is_change(self) -> bool {
        self != Edge::Live
    }
}

/// What a tree node stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKey {
    /// A class, by qualified name.
    Subject(String),
    /// A function or method, by callsign.
    Operation(String),
    /// Something outside the package we only know by name.
    External(String),
}

#[derive(Clone, Debug)]
pub struct TreeNode {
    pub key: NodeKey,
    pub label: String,
    pub children: Vec<TreeNode>,
    /// True when this node, its link, or anything below it changed.
    pub touched: bool,
    pub edge: Edge,
    /// Already expanded elsewhere in the forest; drawn as a leaf.
    pub repeat: bool,
    /// Short dim annotation drawn after the label.
    pub note: Option<String>,
}

impl TreeNode {
    pub fn new(key: NodeKey, label: String) -> Self {
        Self {
            key,
            label,
            children: Vec::new(),
            touched: false,
            edge: Edge::Live,
            repeat: false,
            note: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Counts {
    pub activated: usize,
    pub burned: usize,
    pub realigned: usize,
    pub rerouted: usize,
    pub relocated: usize,
    pub amended: usize,
    pub wake: usize,
    pub nominal: usize,
}

impl Counts {
    pub fn record(&mut self, status: Status) {
        match status {
            Status::Activated => self.activated += 1,
            Status::Burned => self.burned += 1,
            Status::Realigned => self.realigned += 1,
            Status::Rerouted => self.rerouted += 1,
            Status::Relocated => self.relocated += 1,
            Status::Amended => self.amended += 1,
            Status::Wake => self.wake += 1,
            Status::Nominal => self.nominal += 1,
        }
    }

    pub fn of(&self, status: Status) -> usize {
        match status {
            Status::Activated => self.activated,
            Status::Burned => self.burned,
            Status::Realigned => self.realigned,
            Status::Rerouted => self.rerouted,
            Status::Relocated => self.relocated,
            Status::Amended => self.amended,
            Status::Wake => self.wake,
            Status::Nominal => self.nominal,
        }
    }

    pub fn changed(&self) -> usize {
        self.activated
            + self.burned
            + self.realigned
            + self.rerouted
            + self.relocated
            + self.amended
    }

    pub fn total(&self) -> usize {
        self.changed() + self.nominal
    }
}

/// One rendered line of a forest.
pub struct Row<'a> {
    pub prefix: String,
    pub node: &'a TreeNode,
}

/// Flatten a forest into drawable rows, optionally keeping only the branches
/// that lead to a change.
pub fn flatten(roots: &[TreeNode], changes_only: bool) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    let visible: Vec<&TreeNode> = roots
        .iter()
        .filter(|n| !changes_only || n.touched)
        .collect();
    let last = visible.len().saturating_sub(1);
    for (i, node) in visible.into_iter().enumerate() {
        emit(node, "", i == last, changes_only, &mut rows);
    }
    rows
}

fn emit<'a>(
    node: &'a TreeNode,
    prefix: &str,
    is_last: bool,
    changes_only: bool,
    rows: &mut Vec<Row<'a>>,
) {
    let connector = if prefix.is_empty() {
        String::new()
    } else if is_last {
        "└─ ".to_string()
    } else {
        "├─ ".to_string()
    };
    rows.push(Row {
        prefix: format!("{prefix}{connector}"),
        node,
    });

    let child_prefix = if prefix.is_empty() {
        "  ".to_string()
    } else if is_last {
        format!("{prefix}   ")
    } else {
        format!("{prefix}│  ")
    };

    let visible: Vec<&TreeNode> = node
        .children
        .iter()
        .filter(|c| !changes_only || c.touched)
        .collect();
    let last = visible.len().saturating_sub(1);
    for (i, child) in visible.into_iter().enumerate() {
        emit(child, &child_prefix, i == last, changes_only, rows);
    }
}

/// What `after` gained and lost relative to `before`.
pub fn delta(before: &[String], after: &[String]) -> (Vec<String>, Vec<String>) {
    let before: BTreeSet<&String> = before.iter().collect();
    let after: BTreeSet<&String> = after.iter().collect();
    (
        after.difference(&before).map(|s| s.to_string()).collect(),
        before.difference(&after).map(|s| s.to_string()).collect(),
    )
}

pub fn leaf(qualname: &str) -> &str {
    qualname.rsplit('.').next().unwrap_or(qualname)
}
