//! `dossier.rs`: name resolution and call wiring.
//!
//! Most tests build a `Dossier` straight from source strings — no git — so the
//! module layout under test is explicit and each rung of the confidence ladder
//! can be isolated. One test at the bottom runs the same wiring through the real
//! baseline/working-tree pipeline to prove the two agree.

mod common;

use std::collections::BTreeMap;

use common::Fixture;
use watch_floor::dossier::{resolve, Confidence, Dossier, Operation, Subject};
use watch_floor::intercept::Interceptor;

/// Build a wired dossier from `(module, source)` pairs, as `collection.rs` does.
fn dossier_of(files: &[(&str, &str)]) -> Dossier {
    let mut interceptor = Interceptor::new().expect("interceptor");
    let mut dossier = Dossier::new("test");
    for (module, source) in files {
        let file = format!("{}.py", module.replace('.', "/"));
        let sweep = interceptor.sweep(source, module, &file);
        dossier.files += 1;
        for subject in sweep.subjects {
            dossier.insert_subject(subject);
        }
        for operation in sweep.operations {
            dossier.insert_operation(operation);
        }
        for binding in sweep.imports {
            dossier.insert_binding(module, binding);
        }
    }
    dossier.wire();
    dossier
}

/// A wired dossier holding a single module, `pkg.mod` (file `pkg/mod.py`).
fn one_module(source: &str) -> Dossier {
    dossier_of(&[("pkg.mod", source)])
}

fn op<'a>(dossier: &'a Dossier, callsign: &str) -> &'a Operation {
    dossier.operations.get(callsign).unwrap_or_else(|| {
        panic!(
            "no operation {callsign}; the dossier holds {:?}",
            dossier.operations.keys().collect::<Vec<_>>()
        )
    })
}

fn subject<'a>(dossier: &'a Dossier, qualname: &str) -> &'a Subject {
    dossier.subjects.get(qualname).unwrap_or_else(|| {
        panic!(
            "no subject {qualname}; the dossier holds {:?}",
            dossier.subjects.keys().collect::<Vec<_>>()
        )
    })
}

/// The resolved links of an operation, as comparable `(target, confidence)`.
fn wiring(operation: &Operation) -> Vec<(String, Confidence)> {
    operation
        .links
        .iter()
        .map(|link| (link.target.clone(), link.confidence))
        .collect()
}

fn link(target: &str, confidence: Confidence) -> (String, Confidence) {
    (target.to_string(), confidence)
}

/// Just the subjects, for the `resolve` tests.
fn subjects_of(files: &[(&str, &str)]) -> BTreeMap<String, Subject> {
    dossier_of(files).subjects
}

// ---------------------------------------------------------------- resolve() --

#[test]
fn resolve_finds_a_base_class_declared_in_the_same_module() {
    let subjects = subjects_of(&[("pkg.mod", "class Handler:\n    pass\n")]);

    assert_eq!(
        resolve(&subjects, "Handler", "pkg.mod"),
        Some("pkg.mod.Handler"),
        "a bare base name is looked up in the declaring module first"
    );
}

#[test]
fn resolve_prefers_the_local_class_over_a_same_named_class_elsewhere() {
    let subjects = subjects_of(&[
        ("pkg.mod", "class Handler:\n    pass\n"),
        ("pkg.other", "class Handler:\n    pass\n"),
    ]);

    assert_eq!(
        resolve(&subjects, "Handler", "pkg.mod"),
        Some("pkg.mod.Handler"),
        "the same-module hit wins outright, so the name is not ambiguous here"
    );
    assert_eq!(
        resolve(&subjects, "Handler", "pkg.other"),
        Some("pkg.other.Handler"),
        "and symmetrically from the other module"
    );
}

#[test]
fn resolve_accepts_an_absolute_dotted_path_even_when_the_leaf_is_ambiguous() {
    let subjects = subjects_of(&[
        ("pkg.mod", ""),
        ("pkg.other", "class Handler:\n    pass\n"),
        ("pkg.third", "class Handler:\n    pass\n"),
    ]);

    assert_eq!(
        resolve(&subjects, "pkg.other.Handler", "pkg.mod"),
        Some("pkg.other.Handler"),
        "an exact qualified name needs no guessing"
    );
    assert_eq!(
        resolve(&subjects, "Handler", "pkg.mod"),
        None,
        "while the bare leaf stays ambiguous between the two modules"
    );
}

#[test]
fn resolve_matches_an_unambiguous_trailing_segment_across_modules() {
    let subjects = subjects_of(&[("pkg.mod", ""), ("pkg.other", "class Handler:\n    pass\n")]);

    assert_eq!(
        resolve(&subjects, "Handler", "pkg.mod"),
        Some("pkg.other.Handler"),
        "`from pkg.other import Handler` is not read, but the name is unique"
    );
    assert_eq!(
        resolve(&subjects, "other.Handler", "pkg.mod"),
        Some("pkg.other.Handler"),
        "a partially qualified reference matches on its trailing segment too"
    );
}

#[test]
fn resolve_refuses_to_guess_an_ambiguous_name() {
    let subjects = subjects_of(&[
        ("pkg.mod", ""),
        ("pkg.left", "class Handler:\n    pass\n"),
        ("pkg.right", "class Handler:\n    pass\n"),
    ]);

    assert_eq!(
        resolve(&subjects, "Handler", "pkg.mod"),
        None,
        "two candidates must resolve to nothing rather than to a guess"
    );
}

#[test]
fn resolve_returns_none_for_a_name_outside_the_package() {
    let subjects = subjects_of(&[("pkg.mod", "class Handler:\n    pass\n")]);

    assert_eq!(
        resolve(&subjects, "BaseModel", "pkg.mod"),
        None,
        "a third-party base resolves nowhere in the package"
    );
}

#[test]
fn in_package_bases_keeps_resolvable_bases_in_declaration_order_and_drops_external_ones() {
    let dossier = dossier_of(&[
        ("pkg.left", "class Left:\n    pass\n"),
        ("pkg.right", "class Right:\n    pass\n"),
        (
            "pkg.mod",
            "class Handler(Right, BaseModel, Left):\n    pass\n",
        ),
    ]);

    assert_eq!(
        dossier.in_package_bases("pkg.mod.Handler"),
        vec!["pkg.right.Right".to_string(), "pkg.left.Left".to_string()],
        "declaration order is preserved and the unresolvable base is dropped"
    );
    assert_eq!(
        dossier.in_package_bases("pkg.mod.Missing"),
        Vec::<String>::new(),
        "an unknown class has no bases rather than panicking"
    );
}

// --------------------------------------------------------- Subject accessors --

#[test]
fn subject_display_name_spells_out_enclosing_classes() {
    let dossier = one_module("class Outer:\n    class Inner:\n        pass\n");

    assert_eq!(subject(&dossier, "pkg.mod.Outer").display_name(), "Outer");
    assert_eq!(
        subject(&dossier, "pkg.mod.Outer.Inner").display_name(),
        "Outer.Inner",
        "a nested class reads as it is written inside its module"
    );
}

#[test]
fn subject_location_is_module_and_one_indexed_line() {
    let dossier = one_module("# header\n\nclass Handler:\n    pass\n");
    let handler = subject(&dossier, "pkg.mod.Handler");

    assert_eq!(handler.line, 3, "the `class` keyword is on line 3");
    assert_eq!(handler.location(), "pkg.mod:3");
    assert_eq!(handler.file, "pkg/mod.py", "location() is not a file path");
}

// ------------------------------------------------------- Operation accessors --

#[test]
fn operation_display_name_qualifies_methods_and_leaves_functions_bare() {
    let dossier =
        one_module("class Channel:\n    def open(self):\n        pass\n\ndef main():\n    pass\n");

    assert_eq!(
        op(&dossier, "pkg.mod.Channel.open").display_name(),
        "Channel.open",
        "a method carries its owning class"
    );
    assert_eq!(
        op(&dossier, "pkg.mod.main").display_name(),
        "main",
        "a module-level function is just its name"
    );
}

#[test]
fn operation_display_name_includes_the_nesting_class_of_a_nested_class_method() {
    let dossier =
        one_module("class Outer:\n    class Inner:\n        def ping(self):\n            pass\n");

    assert_eq!(
        op(&dossier, "pkg.mod.Outer.Inner.ping").display_name(),
        "Outer.Inner.ping"
    );
}

#[test]
fn operation_location_is_module_and_line_while_source_is_file_and_line() {
    let dossier = one_module("def first():\n    pass\n\n\ndef second():\n    pass\n");
    let second = op(&dossier, "pkg.mod.second");

    assert_eq!(second.line, 5);
    assert_eq!(second.location(), "pkg.mod:5", "location() is dotted");
    assert_eq!(
        second.source(),
        "pkg/mod.py:5",
        "source() is a clickable path into the tree"
    );
}

#[test]
fn operation_signature_lists_parameter_names_in_order() {
    let dossier = one_module(
        "class Channel:\n    def send(self, payload: bytes, priority=0, *rest, **opts):\n        pass\n\ndef bare():\n    pass\n",
    );

    assert_eq!(
        op(&dossier, "pkg.mod.Channel.send").signature(),
        "(self, payload, priority, *rest, **opts)",
        "annotations and defaults are dropped, splats are kept"
    );
    assert_eq!(
        op(&dossier, "pkg.mod.bare").signature(),
        "()",
        "no parameters still renders a pair of parentheses"
    );
}

// ------------------------------------------------ wire(): the confidence ladder --

#[test]
fn bare_call_to_a_module_level_def_in_the_same_module_is_confirmed() {
    let dossier = one_module("def helper():\n    pass\n\ndef main():\n    helper()\n");

    assert_eq!(
        wiring(op(&dossier, "pkg.mod.main")),
        vec![link("pkg.mod.helper", Confidence::Confirmed)],
        "a name defined alongside the caller needs no inference"
    );
    assert_eq!(op(&dossier, "pkg.mod.main").external, 0);
}

#[test]
fn constructing_a_class_links_to_its_dunder_init_as_confirmed() {
    let dossier = one_module(
        "class Channel:\n    def __init__(self, url):\n        pass\n\ndef main():\n    Channel(\"u\")\n",
    );

    assert_eq!(
        wiring(op(&dossier, "pkg.mod.main")),
        vec![link("pkg.mod.Channel.__init__", Confidence::Confirmed)],
        "constructing a same-module class runs its __init__"
    );
}

#[test]
fn self_method_call_resolves_on_the_owning_class_as_confirmed() {
    let dossier = one_module(
        "class Channel:\n    def open(self):\n        self.handshake()\n\n    def handshake(self):\n        pass\n",
    );

    assert_eq!(
        wiring(op(&dossier, "pkg.mod.Channel.open")),
        vec![link("pkg.mod.Channel.handshake", Confidence::Confirmed)],
    );
}

#[test]
fn self_method_call_resolves_an_inherited_base_method_as_confirmed() {
    let dossier = dossier_of(&[
        (
            "pkg.core",
            "class Channel:\n    def handshake(self):\n        pass\n",
        ),
        (
            "pkg.agents",
            "class Agent(Channel):\n    def brief(self):\n        self.handshake()\n",
        ),
    ]);

    assert_eq!(
        wiring(op(&dossier, "pkg.agents.Agent.brief")),
        vec![link("pkg.core.Channel.handshake", Confidence::Confirmed)],
        "wire() walks the class graph, across modules, before giving up"
    );
    assert_eq!(op(&dossier, "pkg.agents.Agent.brief").external, 0);
}

#[test]
fn cls_method_call_is_wired_exactly_like_self() {
    let dossier = one_module(
        "class Registry:\n    @classmethod\n    def build(cls):\n        cls.reset()\n\n    @classmethod\n    def reset(cls):\n        pass\n",
    );

    assert_eq!(
        wiring(op(&dossier, "pkg.mod.Registry.build")),
        vec![link("pkg.mod.Registry.reset", Confidence::Confirmed)],
    );
}

#[test]
fn super_method_call_starts_one_level_up_and_skips_the_override() {
    let dossier = one_module(
        "class Base:\n    def run(self):\n        pass\n\nclass Child(Base):\n    def run(self):\n        super().run()\n",
    );
    let child_run = op(&dossier, "pkg.mod.Child.run");

    assert_eq!(
        wiring(child_run),
        vec![link("pkg.mod.Base.run", Confidence::Confirmed)],
        "super().run() must reach the base, not loop back onto Child.run"
    );
    assert!(
        !child_run.recursive,
        "super().run() is not self-recursion even though the names match"
    );
}

#[test]
fn super_method_call_with_no_in_package_base_is_dropped() {
    let dossier =
        one_module("class Child(BaseModel):\n    def run(self):\n        super().run()\n");

    assert_eq!(
        wiring(op(&dossier, "pkg.mod.Child.run")),
        vec![],
        "nothing in the package can answer this call"
    );
}

#[test]
fn fully_qualified_cross_module_call_is_confirmed() {
    let dossier = dossier_of(&[
        ("pkg.core", "def encode(payload):\n    pass\n"),
        ("pkg.agents", "def transmit():\n    pkg.core.encode(1)\n"),
    ]);

    assert_eq!(
        wiring(op(&dossier, "pkg.agents.transmit")),
        vec![link("pkg.core.encode", Confidence::Confirmed)],
        "the qualified name exists verbatim, so there is nothing to infer"
    );
}

#[test]
fn bare_call_to_a_module_level_def_in_another_module_is_probable() {
    let dossier = dossier_of(&[
        ("pkg.core", "def encode(payload):\n    pass\n"),
        ("pkg.agents", "def transmit():\n    encode(1)\n"),
    ]);

    assert_eq!(
        wiring(op(&dossier, "pkg.agents.transmit")),
        vec![link("pkg.core.encode", Confidence::Probable)],
        "imports are not followed, so the unique name is inference"
    );
}

#[test]
fn receiver_call_with_exactly_one_matching_operation_is_probable() {
    let dossier = dossier_of(&[
        (
            "pkg.core",
            "class Channel:\n    def send(self, m):\n        pass\n",
        ),
        (
            "pkg.agents",
            "def dispatch(channel):\n    channel.send(1)\n",
        ),
    ]);
    let dispatch = op(&dossier, "pkg.agents.dispatch");

    assert_eq!(
        wiring(dispatch),
        vec![link("pkg.core.Channel.send", Confidence::Probable)],
        "the receiver's type is unknowable from syntax, but the name is unique"
    );
    assert_eq!(dispatch.external, 0, "an inferred link is not lost traffic");
}

#[test]
fn receiver_call_matching_two_operations_is_dropped_into_external() {
    let dossier = dossier_of(&[
        (
            "pkg.core",
            "class Channel:\n    def send(self, m):\n        pass\n",
        ),
        (
            "pkg.radio",
            "class Radio:\n    def send(self, m):\n        pass\n",
        ),
        (
            "pkg.agents",
            "def dispatch(channel):\n    channel.send(1)\n",
        ),
    ]);
    let dispatch = op(&dossier, "pkg.agents.dispatch");

    assert_eq!(
        wiring(dispatch),
        vec![],
        "two operations named `send` means the call is never guessed at"
    );
    assert_eq!(
        dispatch.external, 1,
        "the ambiguous call is counted as traffic leaving the package"
    );
}

#[test]
fn builtin_calls_are_counted_as_external_traffic() {
    let dossier = one_module("def report(items):\n    print(len(items))\n");
    let report = op(&dossier, "pkg.mod.report");

    assert_eq!(report.raw_calls, vec!["print", "len"]);
    assert_eq!(wiring(report), vec![], "builtins are outside the package");
    assert_eq!(report.external, 2, "both print and len are counted");
}

// --------------------------------------------------------- wire(): bookkeeping --

#[test]
fn self_recursion_sets_the_flag_and_creates_no_self_link() {
    let dossier = one_module("def walk(n):\n    walk(n - 1)\n");
    let walk = op(&dossier, "pkg.mod.walk");

    assert!(walk.recursive, "walk() calls itself");
    assert_eq!(
        wiring(walk),
        vec![],
        "a self-link would be noise in the tree"
    );
    assert_eq!(
        walk.external, 0,
        "recursion is resolved traffic, not lost traffic"
    );
}

#[test]
fn self_recursion_through_a_method_receiver_also_sets_the_flag() {
    let dossier = one_module(
        "class Walker:\n    def walk(self, n):\n        self.walk(n - 1)\n        self.stop()\n\n    def stop(self):\n        pass\n",
    );
    let walk = op(&dossier, "pkg.mod.Walker.walk");

    assert!(walk.recursive, "self.walk() resolves back onto Walker.walk");
    assert_eq!(
        wiring(walk),
        vec![link("pkg.mod.Walker.stop", Confidence::Confirmed)],
        "the recursive edge is dropped, the other one survives"
    );
}

#[test]
fn links_are_deduplicated_by_target_and_sorted() {
    let dossier = one_module(
        "def alpha():\n    pass\n\ndef zebra():\n    pass\n\ndef run():\n    zebra()\n    alpha()\n    pkg.mod.zebra()\n",
    );
    let run = op(&dossier, "pkg.mod.run");

    assert_eq!(
        run.raw_calls,
        vec!["zebra", "alpha", "pkg.mod.zebra"],
        "three call sites were written, in this order"
    );
    assert_eq!(
        wiring(run),
        vec![
            link("pkg.mod.alpha", Confidence::Confirmed),
            link("pkg.mod.zebra", Confidence::Confirmed),
        ],
        "two spellings of zebra collapse to one link, and links come out sorted"
    );
    assert_eq!(
        run.targets(),
        vec!["pkg.mod.alpha".to_string(), "pkg.mod.zebra".to_string()],
    );
}

#[test]
fn a_duplicate_target_keeps_the_confidence_of_the_first_spelling() {
    let dossier = dossier_of(&[
        ("pkg.core", "def encode(payload):\n    pass\n"),
        (
            "pkg.agents",
            "def transmit():\n    pkg.core.encode(1)\n    encode(2)\n",
        ),
    ]);

    assert_eq!(
        wiring(op(&dossier, "pkg.agents.transmit")),
        vec![link("pkg.core.encode", Confidence::Confirmed)],
        "the qualified spelling was seen first, so the edge stays Confirmed"
    );
}

// ------------------------------------------------------------ lookup_method() --

#[test]
fn lookup_method_finds_a_method_on_the_class_itself() {
    let dossier = one_module("class Channel:\n    def open(self):\n        pass\n");

    assert_eq!(
        dossier.lookup_method("pkg.mod.Channel", "open"),
        Some("pkg.mod.Channel.open".to_string())
    );
    assert_eq!(
        dossier.lookup_method("pkg.mod.Channel", "close"),
        None,
        "a method nobody defined resolves nowhere"
    );
}

#[test]
fn lookup_method_walks_two_levels_up_the_inheritance_chain() {
    let dossier = dossier_of(&[
        (
            "pkg.base",
            "class Root:\n    def audit(self):\n        pass\n",
        ),
        ("pkg.mid", "class Middle(Root):\n    pass\n"),
        ("pkg.leaf", "class Leaf(Middle):\n    pass\n"),
    ]);

    assert_eq!(
        dossier.lookup_method("pkg.leaf.Leaf", "audit"),
        Some("pkg.base.Root.audit".to_string()),
        "inheritance is followed across modules and through an empty middle"
    );
}

#[test]
fn lookup_method_takes_the_first_base_that_defines_the_method() {
    let dossier = one_module(
        "class Left:\n    def run(self):\n        pass\n\nclass Right:\n    def run(self):\n        pass\n\nclass Leaf(Left, Right):\n    pass\n",
    );

    assert_eq!(
        dossier.lookup_method("pkg.mod.Leaf", "run"),
        Some("pkg.mod.Left.run".to_string()),
        "bases are searched breadth-first in declaration order, like Python's MRO"
    );
}

#[test]
fn lookup_method_prefers_the_class_over_the_base_it_overrides() {
    let dossier = one_module(
        "class Base:\n    def run(self):\n        pass\n\nclass Child(Base):\n    def run(self):\n        pass\n",
    );

    assert_eq!(
        dossier.lookup_method("pkg.mod.Child", "run"),
        Some("pkg.mod.Child.run".to_string())
    );
}

#[test]
fn lookup_method_terminates_on_an_inheritance_cycle() {
    let dossier =
        one_module("class A(B):\n    pass\n\nclass B(A):\n    def run(self):\n        pass\n");

    assert_eq!(
        dossier.lookup_method("pkg.mod.A", "run"),
        Some("pkg.mod.B.run".to_string())
    );
    assert_eq!(
        dossier.lookup_method("pkg.mod.A", "missing"),
        None,
        "a cyclic base chain must be visited once, not forever"
    );
}

// --------------------------------------------------------------- overridden() --

#[test]
fn overridden_names_the_base_method_a_method_replaces() {
    let dossier = dossier_of(&[
        (
            "pkg.core",
            "class Channel:\n    def send(self, m):\n        pass\n",
        ),
        (
            "pkg.agents",
            "class Agent(Channel):\n    def send(self, m):\n        pass\n",
        ),
    ]);

    assert_eq!(
        dossier.overridden(op(&dossier, "pkg.agents.Agent.send")),
        Some("pkg.core.Channel.send".to_string())
    );
}

#[test]
fn overridden_reaches_through_a_base_that_does_not_define_the_method() {
    let dossier = one_module(
        "class Root:\n    def run(self):\n        pass\n\nclass Middle(Root):\n    pass\n\nclass Leaf(Middle):\n    def run(self):\n        pass\n",
    );

    assert_eq!(
        dossier.overridden(op(&dossier, "pkg.mod.Leaf.run")),
        Some("pkg.mod.Root.run".to_string()),
        "the override is against the nearest definition up the chain"
    );
}

#[test]
fn overridden_is_none_for_a_plain_function() {
    let dossier =
        one_module("def run():\n    pass\n\nclass Base:\n    def run(self):\n        pass\n");

    assert_eq!(
        dossier.overridden(op(&dossier, "pkg.mod.run")),
        None,
        "a module-level function has no owner and so overrides nothing"
    );
}

#[test]
fn overridden_is_none_for_a_method_no_base_declares() {
    let dossier = one_module(
        "class Base:\n    def run(self):\n        pass\n\nclass Child(Base):\n    def extra(self):\n        pass\n",
    );

    assert_eq!(
        dossier.overridden(op(&dossier, "pkg.mod.Child.extra")),
        None,
        "extra() is new on Child"
    );
    assert_eq!(
        dossier.overridden(op(&dossier, "pkg.mod.Base.run")),
        None,
        "Base has no in-package base to override"
    );
}

// ------------------------------------------------------ through the real pipeline --

#[test]
fn wiring_holds_up_through_the_git_backed_snapshots() {
    let fx = Fixture::new();
    fx.write("pkg/__init__.py", "");
    fx.write(
        "pkg/core.py",
        "class Channel:\n    def handshake(self):\n        pass\n",
    );
    fx.write(
        "pkg/agents.py",
        "from pkg.core import Channel\n\n\nclass Agent(Channel):\n    def brief(self):\n        self.handshake()\n",
    );
    fx.commit();
    // Drift the working tree: brief() stops calling the inherited method.
    fx.write(
        "pkg/agents.py",
        "from pkg.core import Channel\n\n\nclass Agent(Channel):\n    def brief(self):\n        pass\n",
    );

    let (baseline, current) = fx.snapshots();

    let before = op(&baseline, "pkg.agents.Agent.brief");
    assert_eq!(
        wiring(before),
        vec![link("pkg.core.Channel.handshake", Confidence::Confirmed)],
        "the committed baseline wires self.handshake() up the class graph"
    );
    assert_eq!(
        before.source(),
        "pkg/agents.py:5",
        "source() is repo-relative on both sides"
    );
    assert_eq!(before.display_name(), "Agent.brief");
    assert_eq!(
        baseline.overridden(op(&baseline, "pkg.agents.Agent.brief")),
        None
    );

    assert_eq!(
        wiring(op(&current, "pkg.agents.Agent.brief")),
        vec![],
        "the working tree has dropped the call"
    );
}

// ------------------------------------------------------------ suspected bugs --

#[test]
fn a_confirmed_spelling_wins_over_a_probable_one_for_the_same_target() {
    // Same pair of call sites as `a_duplicate_target_keeps_the_confidence_of_the_first_spelling`,
    // written in the other order. The qualified spelling still proves the edge,
    // so the surviving link should be Confirmed either way.
    let dossier = dossier_of(&[
        ("pkg.core", "def encode(payload):\n    pass\n"),
        (
            "pkg.agents",
            "def transmit():\n    encode(2)\n    pkg.core.encode(1)\n",
        ),
    ]);

    assert_eq!(
        wiring(op(&dossier, "pkg.agents.transmit")),
        vec![link("pkg.core.encode", Confidence::Confirmed)],
        "one call site names pkg.core.encode verbatim, which is not inference"
    );
}

#[test]
fn constructing_a_class_with_no_init_is_not_traffic_leaving_the_package() {
    let dossier = one_module(
        "class Channel:\n    def open(self):\n        pass\n\ndef main():\n    Channel()\n",
    );
    let main = op(&dossier, "pkg.mod.main");

    assert_eq!(
        wiring(main),
        vec![],
        "there is no __init__ operation to link to"
    );
    assert_eq!(
        main.external, 0,
        "Channel is defined in the package, so the call did not leave it"
    );
}

// ---------------------------------------------------------------------------
// Imports. A name an import statement binds is not a guess: the statement says
// which definition it refers to.
// ---------------------------------------------------------------------------

/// `pkg.other` also defines `encode`, so the unambiguous-name fallback cannot
/// fire. Only reading the import can resolve this call.
const AMBIGUOUS_ENCODE: (&str, &str) = ("pkg.other", "def encode(x):\n    return x\n");

#[test]
fn an_imported_name_is_confirmed_rather_than_guessed() {
    let dossier = dossier_of(&[
        ("pkg.core", "def encode(x):\n    return x\n"),
        AMBIGUOUS_ENCODE,
        (
            "pkg.agents",
            "from .core import encode\n\n\ndef driver():\n    encode(1)\n",
        ),
    ]);
    assert_eq!(
        wiring(op(&dossier, "pkg.agents.driver")),
        vec![("pkg.core.encode".to_string(), Confidence::Confirmed)],
        "two modules define `encode`, so only the import statement can say which \
         one is meant — and having said it, the edge is certain"
    );
}

#[test]
fn an_unimported_ambiguous_name_stays_unresolved() {
    let dossier = dossier_of(&[
        ("pkg.core", "def encode(x):\n    return x\n"),
        AMBIGUOUS_ENCODE,
        ("pkg.agents", "def driver():\n    encode(1)\n"),
    ]);
    let driver = op(&dossier, "pkg.agents.driver");
    assert!(
        wiring(driver).is_empty(),
        "with no import to disambiguate, the call is not guessed at: {:?}",
        wiring(driver)
    );
    assert_eq!(driver.external, 1, "it is counted as leaving the package");
}

#[test]
fn an_aliased_import_is_followed_to_the_real_name() {
    let dossier = dossier_of(&[
        ("pkg.core", "def encode(x):\n    return x\n"),
        AMBIGUOUS_ENCODE,
        (
            "pkg.agents",
            "from .core import encode as enc\n\n\ndef driver():\n    enc(1)\n",
        ),
    ]);
    assert_eq!(
        wiring(op(&dossier, "pkg.agents.driver")),
        vec![("pkg.core.encode".to_string(), Confidence::Confirmed)],
        "the local name is `enc`, but the import says it means pkg.core.encode"
    );
}

#[test]
fn an_imported_module_resolves_a_dotted_call() {
    let dossier = dossier_of(&[
        ("pkg.core", "def encode(x):\n    return x\n"),
        AMBIGUOUS_ENCODE,
        (
            "pkg.agents",
            "from . import core\n\n\ndef driver():\n    core.encode(1)\n",
        ),
    ]);
    assert_eq!(
        wiring(op(&dossier, "pkg.agents.driver")),
        vec![("pkg.core.encode".to_string(), Confidence::Confirmed)],
        "`core` was imported, so the whole dotted path is determined"
    );
}

#[test]
fn a_relative_import_climbs_one_level_per_extra_dot() {
    let dossier = dossier_of(&[
        ("pkg.core", "def encode(x):\n    return x\n"),
        AMBIGUOUS_ENCODE,
        (
            "pkg.deep.worker",
            "from ..core import encode\n\n\ndef driver():\n    encode(1)\n",
        ),
    ]);
    assert_eq!(
        wiring(op(&dossier, "pkg.deep.worker.driver")),
        vec![("pkg.core.encode".to_string(), Confidence::Confirmed)],
        "`..` from pkg.deep.worker is pkg, so ..core is pkg.core"
    );
}

#[test]
fn confidence_does_not_depend_on_the_order_call_sites_are_written() {
    // The same callee, named two ways in one body. Whichever is written first,
    // the edge should carry the better of the two grades.
    let qualified_first = dossier_of(&[
        ("pkg.core", "def encode(x):\n    return x\n"),
        (
            "pkg.agents",
            "def driver():\n    pkg.core.encode(1)\n    encode(2)\n",
        ),
    ]);
    let bare_first = dossier_of(&[
        ("pkg.core", "def encode(x):\n    return x\n"),
        (
            "pkg.agents",
            "def driver():\n    encode(2)\n    pkg.core.encode(1)\n",
        ),
    ]);

    let a = wiring(op(&qualified_first, "pkg.agents.driver"));
    let b = wiring(op(&bare_first, "pkg.agents.driver"));
    assert_eq!(
        a, b,
        "swapping two lines must not change the call graph: {a:?} vs {b:?}"
    );
    assert_eq!(
        a,
        vec![("pkg.core.encode".to_string(), Confidence::Confirmed)],
        "one spelling names the target outright, so the edge is Confirmed"
    );
}

#[test]
fn one_callee_called_many_ways_is_a_single_edge() {
    let dossier = dossier_of(&[
        ("pkg.core", "def encode(x):\n    return x\n"),
        (
            "pkg.agents",
            "from .core import encode\n\n\ndef driver():\n    encode(1)\n    \
             encode(2)\n    pkg.core.encode(3)\n",
        ),
    ]);
    assert_eq!(
        wiring(op(&dossier, "pkg.agents.driver")).len(),
        1,
        "three call sites, one callee, one edge"
    );
}
