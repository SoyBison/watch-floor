//! The dossier: what we know about one snapshot of the target package. A
//! snapshot is either the baseline (git HEAD) or the current working tree.
//!
//! Two kinds of observation live here: `Subject`s (classes, and the inheritance
//! between them) and `Operation`s (functions and methods, and the calls between
//! them). Calls are recorded as written and resolved later by [`Dossier::wire`],
//! once the whole snapshot has been read.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// A single class definition observed in a snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subject {
    /// Fully qualified name, e.g. `pkg.mod.Outer.Inner`.
    pub qualname: String,
    /// The bare class name, e.g. `Inner`.
    pub name: String,
    /// Dotted module path, e.g. `pkg.mod`.
    pub module: String,
    /// Path of the defining file, relative to the repository root.
    pub file: String,
    /// 1-indexed line of the `class` keyword.
    pub line: usize,
    /// Base-class expressions as written, normalized (subscripts and keyword
    /// arguments stripped), in declaration order.
    pub bases: Vec<String>,
    /// Names of methods defined directly on the class body, sorted.
    pub methods: Vec<String>,
}

impl Subject {
    /// Name as written in its module, including any enclosing classes.
    pub fn display_name(&self) -> String {
        strip_module(&self.qualname, &self.module, &self.name)
    }

    pub fn location(&self) -> String {
        format!("{}:{}", self.module, self.line)
    }
}

/// How much we trust a resolved call. Python dispatches at runtime, so some
/// edges are read straight off the syntax and others are inference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// Resolved structurally: a same-module name, or `self`/`super()` walked
    /// through the class hierarchy.
    Confirmed,
    /// Resolved by matching an unambiguous name across the package.
    Probable,
}

/// A name an `import` statement bound in some module, and the dotted path it
/// refers to. Following these turns "one operation happens to have this name"
/// into "this name is defined to mean that operation".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub local: String,
    pub target: String,
}

/// What a call expression turned out to be.
enum Resolved {
    /// A call to another operation in the package.
    To(Link),
    /// Resolved to something inside the package that has no operation to point
    /// at — constructing a class that defines no `__init__`. Not external.
    Inside,
    /// Left the package, or could not be resolved at all.
    Away,
}

/// A resolved call from one operation to another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub target: String,
    pub confidence: Confidence,
}

/// A function or method observed in a snapshot.
#[derive(Clone, Debug)]
pub struct Operation {
    /// Fully qualified name, e.g. `pkg.mod.Class.method`.
    pub callsign: String,
    /// The bare function name.
    pub name: String,
    /// Dotted module path.
    pub module: String,
    /// Qualified name of the enclosing class, when this is a method.
    pub owner: Option<String>,
    pub file: String,
    pub line: usize,
    /// Parameter names in order, with `*`/`**` kept.
    pub params: Vec<String>,
    /// Decorator expressions as written.
    pub decorators: Vec<String>,
    pub is_async: bool,
    /// Call targets exactly as written in the body, deduplicated.
    pub raw_calls: Vec<String>,
    /// Calls resolved to other operations in the package. Filled by `wire`.
    pub links: Vec<Link>,
    /// Calls that left the package (stdlib, third party, builtins).
    pub external: usize,
    /// Whether the operation calls itself.
    pub recursive: bool,
}

impl Operation {
    /// Name as written in its module, e.g. `Channel.open` or `main`.
    pub fn display_name(&self) -> String {
        strip_module(&self.callsign, &self.module, &self.name)
    }

    pub fn location(&self) -> String {
        format!("{}:{}", self.module, self.line)
    }

    /// Clickable path into the working tree.
    pub fn source(&self) -> String {
        format!("{}:{}", self.file, self.line)
    }

    pub fn signature(&self) -> String {
        format!("({})", self.params.join(", "))
    }

    pub fn targets(&self) -> Vec<String> {
        self.links.iter().map(|l| l.target.clone()).collect()
    }
}

/// Everything observed in one snapshot of the package.
#[derive(Clone, Debug, Default)]
pub struct Dossier {
    /// Human-readable provenance, e.g. `HEAD @ a1b2c3d`.
    pub label: String,
    /// Number of Python files read.
    pub files: usize,
    pub subjects: BTreeMap<String, Subject>,
    pub operations: BTreeMap<String, Operation>,
    /// module -> (local name -> dotted path it was imported from).
    pub imports: BTreeMap<String, BTreeMap<String, String>>,
}

impl Dossier {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            files: 0,
            subjects: BTreeMap::new(),
            operations: BTreeMap::new(),
            imports: BTreeMap::new(),
        }
    }

    pub fn insert_subject(&mut self, subject: Subject) {
        self.subjects.insert(subject.qualname.clone(), subject);
    }

    pub fn insert_operation(&mut self, operation: Operation) {
        self.operations
            .insert(operation.callsign.clone(), operation);
    }

    pub fn insert_binding(&mut self, module: &str, binding: Binding) {
        self.imports
            .entry(module.to_string())
            .or_default()
            .insert(binding.local, binding.target);
    }

    /// Turn the raw call text collected during interception into resolved links.
    /// Must run after every file in the snapshot has been read.
    pub fn wire(&mut self) {
        // Index of bare function name -> callsigns, for unambiguous-name matching.
        let mut by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for op in self.operations.values() {
            by_name
                .entry(op.name.clone())
                .or_default()
                .push(op.callsign.clone());
        }

        let traced: Vec<(String, Vec<Link>, usize, bool)> = self
            .operations
            .values()
            .map(|op| {
                // Keyed by target so one callee counts once however many times
                // it is called, and keeping the BEST grade rather than the
                // first: `pkg.core.encode()` and a bare `encode()` in the same
                // body are the same edge, and it is Confirmed if either
                // spelling pins it down. Without this the grade would depend on
                // which call site happened to be written first.
                let mut best: BTreeMap<String, Confidence> = BTreeMap::new();
                let mut external = 0;
                let mut recursive = false;
                for raw in &op.raw_calls {
                    match self.trace(op, raw, &by_name) {
                        Resolved::To(link) if link.target == op.callsign => recursive = true,
                        Resolved::To(link) => {
                            let slot = best.entry(link.target).or_insert(link.confidence);
                            *slot = (*slot).min(link.confidence);
                        }
                        Resolved::Inside => {}
                        Resolved::Away => external += 1,
                    }
                }
                let links = best
                    .into_iter()
                    .map(|(target, confidence)| Link { target, confidence })
                    .collect();
                (op.callsign.clone(), links, external, recursive)
            })
            .collect();

        for (callsign, links, external, recursive) in traced {
            if let Some(op) = self.operations.get_mut(&callsign) {
                op.links = links;
                op.external = external;
                op.recursive = recursive;
            }
        }
    }

    /// Resolve one call expression, in descending order of certainty.
    fn trace(
        &self,
        op: &Operation,
        raw: &str,
        by_name: &BTreeMap<String, Vec<String>>,
    ) -> Resolved {
        let to = |target: String, confidence: Confidence| Resolved::To(Link { target, confidence });
        let imported = self.imports.get(&op.module);

        // `self.m()` / `cls.m()`: walk the class hierarchy we already mapped.
        for prefix in ["self.", "cls."] {
            if let Some(rest) = raw.strip_prefix(prefix) {
                if rest.contains('.') {
                    break; // `self.thing.m()` — the receiver is not this class
                }
                return match op.owner.as_ref().and_then(|o| self.lookup_method(o, rest)) {
                    Some(hit) => to(hit, Confidence::Confirmed),
                    None => Resolved::Away,
                };
            }
        }

        // `super().m()`: start one level up.
        if let Some(rest) = raw.strip_prefix("super().") {
            let Some(owner) = op.owner.as_ref() else {
                return Resolved::Away;
            };
            for base in self.in_package_bases(owner) {
                if let Some(hit) = self.lookup_method(&base, rest) {
                    return to(hit, Confidence::Confirmed);
                }
            }
            return Resolved::Away;
        }

        if !raw.contains('.') {
            // A module-level function in the same file.
            let local = format!("{}.{}", op.module, raw);
            if self.operations.contains_key(&local) {
                return to(local, Confidence::Confirmed);
            }
            // Constructing a class in the same file runs its __init__.
            if self.subjects.contains_key(&local) {
                return self.construct(&local, Confidence::Confirmed);
            }
            // An imported name is not a guess: the import statement says what
            // it binds to, so this is as certain as a same-module reference.
            if let Some(target) = imported.and_then(|b| b.get(raw)) {
                if self.operations.contains_key(target) {
                    return to(target.clone(), Confidence::Confirmed);
                }
                if self.subjects.contains_key(target) {
                    return self.construct(target, Confidence::Confirmed);
                }
            }
            // Nothing named it outright; fall back to a unique module-level
            // function carrying the name.
            return match unique(by_name, raw, |callsign| {
                self.operations
                    .get(callsign)
                    .is_some_and(|o| o.owner.is_none())
            }) {
                Some(hit) => to(hit, Confidence::Probable),
                None => Resolved::Away,
            };
        }

        if self.operations.contains_key(raw) {
            return to(raw.to_string(), Confidence::Confirmed);
        }
        let qualified = format!("{}.{}", op.module, raw);
        if self.operations.contains_key(&qualified) {
            return to(qualified, Confidence::Confirmed);
        }
        // `core.transmit()` where `core` was imported: the head names a module
        // we know, so the whole path is determined.
        if let Some((head, rest)) = raw.split_once('.') {
            if let Some(base) = imported.and_then(|b| b.get(head)) {
                let candidate = format!("{base}.{rest}");
                if self.operations.contains_key(&candidate) {
                    return to(candidate, Confidence::Confirmed);
                }
                if self.subjects.contains_key(&candidate) {
                    return self.construct(&candidate, Confidence::Confirmed);
                }
            }
        }
        // A dotted path naming a class: again, construction runs __init__.
        if let Some(class) = resolve(&self.subjects, raw, &op.module) {
            let class = class.to_string();
            return self.construct(&class, Confidence::Probable);
        }
        // `receiver.m()` where we cannot know the receiver's type: accept the
        // match only when exactly one operation in the package carries the name.
        let leaf = raw.rsplit('.').next().unwrap_or(raw);
        match unique(by_name, leaf, |_| true) {
            Some(hit) => to(hit, Confidence::Probable),
            None => Resolved::Away,
        }
    }

    /// Constructing a class runs its `__init__`. A class of ours that defines
    /// none is still ours — it is not traffic leaving the package.
    fn construct(&self, class: &str, confidence: Confidence) -> Resolved {
        match self.lookup_method(class, "__init__") {
            Some(init) => Resolved::To(Link {
                target: init,
                confidence,
            }),
            None => Resolved::Inside,
        }
    }

    /// Find `method` on a class or anything it inherits from in-package.
    pub fn lookup_method(&self, class: &str, method: &str) -> Option<String> {
        let mut queue = VecDeque::from([class.to_string()]);
        let mut seen = BTreeSet::new();
        while let Some(current) = queue.pop_front() {
            if !seen.insert(current.clone()) {
                continue;
            }
            let candidate = format!("{current}.{method}");
            if self.operations.contains_key(&candidate) {
                return Some(candidate);
            }
            queue.extend(self.in_package_bases(&current));
        }
        None
    }

    /// Base classes of `class` that are themselves defined in the package.
    pub fn in_package_bases(&self, class: &str) -> Vec<String> {
        let Some(subject) = self.subjects.get(class) else {
            return Vec::new();
        };
        subject
            .bases
            .iter()
            .filter_map(|base| {
                resolve(&self.subjects, base, &subject.module).map(|q| q.to_string())
            })
            .collect()
    }

    /// The inherited method this operation overrides, if any.
    pub fn overridden(&self, op: &Operation) -> Option<String> {
        let owner = op.owner.as_ref()?;
        self.in_package_bases(owner)
            .iter()
            .find_map(|base| self.lookup_method(base, &op.name))
    }
}

/// Best-effort resolution of a base-class expression to a qualified name within
/// the package. Python imports are not followed; we match on structure alone,
/// which covers the common cases (same-module reference, absolute dotted path,
/// and a name imported from elsewhere in the package).
pub fn resolve<'a>(
    subjects: &'a BTreeMap<String, Subject>,
    base: &str,
    from_module: &str,
) -> Option<&'a str> {
    // Same module: `class B(A)` where A lives alongside it.
    let local = format!("{from_module}.{base}");
    if let Some((k, _)) = subjects.get_key_value(&local) {
        return Some(k.as_str());
    }

    // Absolute dotted path: `class B(pkg.mod.A)`.
    if let Some((k, _)) = subjects.get_key_value(base) {
        return Some(k.as_str());
    }

    // Imported name: match the trailing segment against every known subject and
    // accept only an unambiguous hit.
    let leaf = base.rsplit('.').next().unwrap_or(base);
    let tail = format!(".{leaf}");
    let mut hits = subjects.keys().filter(|k| k.ends_with(&tail));
    let first = hits.next()?;
    if hits.next().is_some() {
        return None; // ambiguous — treat as external rather than guess
    }
    Some(first.as_str())
}

/// The single candidate carrying `name`, or nothing if the name is ambiguous.
fn unique(
    by_name: &BTreeMap<String, Vec<String>>,
    name: &str,
    accept: impl Fn(&String) -> bool,
) -> Option<String> {
    let mut hits = by_name.get(name)?.iter().filter(|c| accept(c));
    let first = hits.next()?;
    hits.next().is_none().then(|| first.clone())
}

fn strip_module(qualname: &str, module: &str, fallback: &str) -> String {
    qualname
        .strip_prefix(&format!("{module}."))
        .unwrap_or(fallback)
        .to_string()
}
