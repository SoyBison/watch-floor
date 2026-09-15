//! Behaviour of the call-traffic diff in `network.rs`: per-operation status,
//! per-edge state, entry-point selection, the projection of the call graph into
//! a forest, and the current-snapshot tallies.

mod common;

use common::{diff_one_file, Fixture};
use watch_floor::network::{Entry, Network};
use watch_floor::sitrep::{Edge, NodeKey, Status, TreeNode};

// ---------------------------------------------------------------- helpers

fn entry<'a>(net: &'a Network, callsign: &str) -> &'a Entry {
    net.entries.get(callsign).unwrap_or_else(|| {
        panic!(
            "no entry for {callsign}; the network holds {:?}",
            net.entries.keys().collect::<Vec<_>>()
        )
    })
}

fn labels(nodes: &[TreeNode]) -> Vec<String> {
    nodes.iter().map(|n| n.label.clone()).collect()
}

fn find<'a>(nodes: &'a [TreeNode], label: &str) -> &'a TreeNode {
    nodes
        .iter()
        .find(|n| n.label == label)
        .unwrap_or_else(|| panic!("no node labelled {label} among {:?}", labels(nodes)))
}

/// A package whose working tree has drifted from the committed baseline, laid
/// out over two modules so relocation between files can be exercised.
fn two_module_fixture(core_before: &str, core_after: &str, edge_after: Option<&str>) -> Network {
    let fx = Fixture::new();
    fx.write("pkg/__init__.py", "");
    fx.write("pkg/core.py", core_before);
    fx.commit();
    fx.write("pkg/core.py", core_after);
    if let Some(edge) = edge_after {
        fx.write("pkg/edge.py", edge);
    }
    fx.network()
}

// ---------------------------------------------------------------- status

#[test]
fn function_with_no_baseline_counterpart_is_activated() {
    let (_, net) = diff_one_file(
        "def alpha():\n    pass\n",
        "def alpha():\n    pass\n\n\ndef beta(x):\n    pass\n",
    );

    let beta = entry(&net, "pkg.mod.beta");
    assert_eq!(beta.status, Status::Activated);
    assert_eq!(beta.operation.params, ["x"]);
    assert!(
        beta.prior_params.is_empty(),
        "an activated operation has no baseline signature to report, got {:?}",
        beta.prior_params
    );
    assert!(
        beta.gained.is_empty() && beta.lost.is_empty(),
        "an activated operation has no call delta, got +{:?} -{:?}",
        beta.gained,
        beta.lost
    );
    assert_eq!(net.counts.activated, 1);
    assert_eq!(net.counts.nominal, 1, "alpha is untouched");
    assert_eq!(net.counts.total(), 2);
}

#[test]
fn function_removed_from_the_working_tree_is_burned_at_its_baseline_location() {
    let (_, net) = diff_one_file(
        "def alpha():\n    pass\n\n\ndef retired():\n    pass\n",
        "def alpha():\n    pass\n",
    );

    let retired = entry(&net, "pkg.mod.retired");
    assert_eq!(retired.status, Status::Burned);
    assert_eq!(
        retired.operation.line, 5,
        "a burned entry keeps the baseline definition, which sat on line 5"
    );
    assert_eq!(net.counts.burned, 1);
    assert_eq!(net.counts.total(), 2, "both sides are unioned into entries");
}

#[test]
fn body_edit_that_keeps_calls_params_and_decorators_is_nominal() {
    let (_, net) = diff_one_file(
        "def alpha(x):\n    y = x + 1\n    helper()\n    return y\n\n\ndef helper():\n    pass\n",
        "def alpha(x):\n    y = x * 2\n    helper()\n    return y\n\n\ndef helper():\n    pass\n",
    );

    assert_eq!(entry(&net, "pkg.mod.alpha").status, Status::Nominal);
    assert_eq!(
        net.counts.changed(),
        0,
        "rewriting arithmetic changes no connection the traffic view tracks"
    );
    assert_eq!(net.counts.nominal, 2);
}

#[test]
fn swapping_a_callee_is_rerouted_and_names_it_by_display_name() {
    let before = "class Radio:\n    def send(self):\n        pass\n\n    def recv(self):\n        pass\n\n\ndef run():\n    r = Radio()\n    r.send()\n";
    let after = "class Radio:\n    def send(self):\n        pass\n\n    def recv(self):\n        pass\n\n\ndef run():\n    r = Radio()\n    r.recv()\n";
    let (_, net) = diff_one_file(before, after);

    let run = entry(&net, "pkg.mod.run");
    assert_eq!(run.status, Status::Rerouted);
    assert_eq!(
        run.gained,
        ["Radio.recv"],
        "gained targets are reported as display names, not callsigns"
    );
    assert_eq!(run.lost, ["Radio.send"]);
    assert_eq!(net.counts.rerouted, 1);
    assert_eq!(
        entry(&net, "pkg.mod.Radio.send").status,
        Status::Nominal,
        "losing a caller is not a change to the callee itself"
    );
}

#[test]
fn adding_a_parameter_while_calls_hold_is_amended() {
    let (_, net) = diff_one_file(
        "def alpha(x):\n    helper()\n\n\ndef helper():\n    pass\n",
        "def alpha(x, y):\n    helper()\n\n\ndef helper():\n    pass\n",
    );

    let alpha = entry(&net, "pkg.mod.alpha");
    assert_eq!(alpha.status, Status::Amended);
    assert_eq!(alpha.prior_params, ["x"]);
    assert_eq!(alpha.operation.params, ["x", "y"]);
    assert!(
        alpha.gained.is_empty() && alpha.lost.is_empty(),
        "an amended operation calls exactly what it called before"
    );
    assert_eq!(net.counts.amended, 1);
}

#[test]
fn adding_a_decorator_while_calls_hold_is_amended() {
    let (_, net) = diff_one_file(
        "def alpha(x):\n    helper()\n\n\ndef helper():\n    pass\n",
        "@trace\ndef alpha(x):\n    helper()\n\n\ndef helper():\n    pass\n",
    );

    let alpha = entry(&net, "pkg.mod.alpha");
    assert_eq!(alpha.status, Status::Amended);
    assert_eq!(alpha.operation.decorators, ["trace"]);
    assert_eq!(net.counts.amended, 1);
}

#[test]
fn making_a_function_async_while_calls_hold_is_amended() {
    let (_, net) = diff_one_file(
        "def alpha(x):\n    helper()\n\n\ndef helper():\n    pass\n",
        "async def alpha(x):\n    helper()\n\n\ndef helper():\n    pass\n",
    );

    let alpha = entry(&net, "pkg.mod.alpha");
    assert_eq!(alpha.status, Status::Amended);
    assert!(alpha.operation.is_async);
    assert_eq!(net.counts.amended, 1);
}

#[test]
fn a_changed_call_set_outranks_a_changed_signature() {
    let (_, net) = diff_one_file(
        "def alpha(x):\n    helper()\n\n\ndef helper():\n    pass\n\n\ndef other():\n    pass\n",
        "def alpha(x, y):\n    other()\n\n\ndef helper():\n    pass\n\n\ndef other():\n    pass\n",
    );

    let alpha = entry(&net, "pkg.mod.alpha");
    assert_eq!(
        alpha.status,
        Status::Rerouted,
        "who it calls is the more disruptive fact, so it wins over the signature"
    );
    assert_eq!(alpha.gained, ["other"]);
    assert_eq!(alpha.lost, ["helper"]);
    assert_eq!(alpha.prior_params, ["x"], "the old signature is still kept");
    assert_eq!(net.counts.amended, 0);
}

// ------------------------------------------------------------ relocation

#[test]
fn moving_a_function_between_modules_relocates_it_and_retires_its_burned_twin() {
    let net = two_module_fixture(
        "def driver():\n    relay()\n\n\ndef relay():\n    work()\n\n\ndef work():\n    pass\n",
        "def driver():\n    relay()\n\n\ndef work():\n    pass\n",
        Some("def relay():\n    work()\n"),
    );

    let relay = entry(&net, "pkg.edge.relay");
    assert_eq!(relay.status, Status::Relocated);
    assert_eq!(relay.prior_module.as_deref(), Some("pkg.core"));
    assert!(
        !net.entries.contains_key("pkg.core.relay"),
        "the burned twin is folded into the relocated entry, not listed twice"
    );
    assert_eq!(net.counts.relocated, 1);
    assert_eq!(net.counts.activated, 0, "the move is not a new operation");
    assert_eq!(net.counts.burned, 0, "the move is not a deletion");
}

#[test]
fn a_moved_function_that_also_changed_its_calls_is_not_a_relocation() {
    let net = two_module_fixture(
        "def relay():\n    work()\n\n\ndef work():\n    pass\n\n\ndef spare():\n    pass\n",
        "def work():\n    pass\n\n\ndef spare():\n    pass\n",
        Some("def relay():\n    spare()\n"),
    );

    assert_eq!(
        entry(&net, "pkg.edge.relay").status,
        Status::Activated,
        "the calls as written differ, so this is a rewrite rather than a move"
    );
    assert_eq!(entry(&net, "pkg.core.relay").status, Status::Burned);
    assert_eq!(net.counts.relocated, 0);
}

#[test]
fn relocation_redirects_baseline_edges_so_callers_keep_a_live_link() {
    let net = two_module_fixture(
        "def driver():\n    relay()\n\n\ndef relay():\n    work()\n\n\ndef work():\n    pass\n",
        "def driver():\n    relay()\n\n\ndef work():\n    pass\n",
        Some("def relay():\n    work()\n"),
    );

    assert_eq!(labels(&net.roots), ["driver"]);
    let driver = find(&net.roots, "driver");
    assert_eq!(
        labels(&driver.children),
        ["relay"],
        "the caller must not carry a second, dangling link to the old callsign"
    );
    let relay = find(&driver.children, "relay");
    assert_eq!(
        relay.edge,
        Edge::Live,
        "the baseline edge is remapped onto the moved operation, so nothing went dark"
    );
    assert_eq!(
        find(&relay.children, "work").edge,
        Edge::Live,
        "the moved operation still calls what it called before"
    );
}

#[test]
fn caller_of_a_relocated_function_is_not_itself_rerouted() {
    let net = two_module_fixture(
        "def driver():\n    relay()\n\n\ndef relay():\n    work()\n\n\ndef work():\n    pass\n",
        "def driver():\n    relay()\n\n\ndef work():\n    pass\n",
        Some("def relay():\n    work()\n"),
    );

    let driver = entry(&net, "pkg.core.driver");
    assert_eq!(
        driver.status,
        Status::Wake,
        "driver calls the same operation it always did; only the callee's module moved, \
         which is a wake rather than a reroute (observed: {:?} with +{:?} -{:?})",
        driver.status,
        driver.gained,
        driver.lost
    );
    assert!(
        driver.gained.is_empty() && driver.lost.is_empty(),
        "a wake reports no call-set delta, least of all the same name twice: +{:?} -{:?}",
        driver.gained,
        driver.lost
    );
    assert_eq!(
        driver.moved,
        vec!["relay moved to pkg.edge".to_string()],
        "the wake names the callee that moved, and where it went"
    );
    assert!(
        !driver.status.holds_branch(),
        "a wake is too soft to drag its branch through the changes-only filter"
    );
}

// ----------------------------------------------------------- edge states

#[test]
fn a_call_added_by_an_existing_caller_opens_its_edge() {
    let (_, net) = diff_one_file(
        "def relay():\n    alpha()\n\n\ndef alpha():\n    pass\n\n\ndef beta():\n    pass\n",
        "def relay():\n    alpha()\n    beta()\n\n\ndef alpha():\n    pass\n\n\ndef beta():\n    pass\n",
    );

    assert_eq!(labels(&net.roots), ["relay"]);
    let relay = find(&net.roots, "relay");
    assert_eq!(labels(&relay.children), ["alpha", "beta"]);
    assert_eq!(
        find(&relay.children, "alpha").edge,
        Edge::Live,
        "the pre-existing call is unremarkable"
    );
    assert_eq!(find(&relay.children, "beta").edge, Edge::Opened);
    assert!(
        find(&relay.children, "beta").touched,
        "an opened edge is a change, so the branch is kept under changes-only"
    );
}

#[test]
fn a_call_dropped_by_an_existing_caller_closes_its_edge() {
    let (_, net) = diff_one_file(
        "def relay():\n    alpha()\n    beta()\n\n\ndef alpha():\n    pass\n\n\ndef beta():\n    pass\n",
        "def relay():\n    alpha()\n\n\ndef alpha():\n    pass\n\n\ndef beta():\n    pass\n",
    );

    assert_eq!(
        labels(&net.roots),
        ["relay"],
        "beta is still drawn under its old caller, so it is not promoted to an entry point"
    );
    let relay = find(&net.roots, "relay");
    assert_eq!(labels(&relay.children), ["alpha", "beta"]);
    assert_eq!(find(&relay.children, "alpha").edge, Edge::Live);
    assert_eq!(find(&relay.children, "beta").edge, Edge::Closed);
    assert_eq!(entry(&net, "pkg.mod.relay").lost, ["beta"]);
}

#[test]
fn every_callee_of_a_brand_new_caller_is_live_not_opened() {
    let (_, net) = diff_one_file(
        "def alpha():\n    pass\n",
        "def launch():\n    alpha()\n    sidekick()\n\n\ndef alpha():\n    pass\n\n\ndef sidekick():\n    pass\n",
    );

    let launch = find(&net.roots, "launch");
    assert_eq!(entry(&net, "pkg.mod.launch").status, Status::Activated);
    assert_eq!(labels(&launch.children), ["alpha", "sidekick"]);
    for child in &launch.children {
        assert_eq!(
            child.edge,
            Edge::Live,
            "{} hangs off a caller that did not exist at baseline, so flagging the edge \
             as new traffic would say nothing the ACTIVATED status does not",
            child.label
        );
    }
}

#[test]
fn every_callee_of_a_burned_caller_is_live_not_closed() {
    let (_, net) = diff_one_file(
        "def retired():\n    helper()\n\n\ndef helper():\n    pass\n",
        "def helper():\n    pass\n",
    );

    assert_eq!(entry(&net, "pkg.mod.retired").status, Status::Burned);
    let retired = find(&net.roots, "retired");
    assert_eq!(labels(&retired.children), ["helper"]);
    assert_eq!(
        find(&retired.children, "helper").edge,
        Edge::Live,
        "the whole caller went away; its links did not individually go dark"
    );
}

// ---------------------------------------------------------- entry points

#[test]
fn entry_points_are_the_operations_nothing_in_the_package_calls() {
    let (_, net) = diff_one_file(
        "def solo():\n    pass\n",
        "def solo():\n    pass\n\n\ndef main():\n    worker()\n\n\ndef worker():\n    pass\n",
    );

    assert_eq!(
        labels(&net.roots),
        ["main", "solo"],
        "worker has an in-package caller, so it is not an entry point"
    );
    assert_eq!(labels(&find(&net.roots, "main").children), ["worker"]);
}

#[test]
fn a_new_caller_demotes_an_existing_entry_point_to_a_child() {
    let fx = Fixture::new();
    fx.write("pkg/__init__.py", "");
    fx.write(
        "pkg/mod.py",
        "def alpha():\n    pass\n\n\ndef beta():\n    pass\n",
    );
    fx.commit();

    let clean = fx.network();
    assert_eq!(
        labels(&clean.roots),
        ["alpha", "beta"],
        "nothing calls anything yet, so both operations are entry points"
    );

    fx.write(
        "pkg/mod.py",
        "def alpha():\n    pass\n\n\ndef beta():\n    pass\n\n\ndef gamma():\n    alpha()\n",
    );
    let net = fx.network();
    assert_eq!(
        labels(&net.roots),
        ["beta", "gamma"],
        "alpha stops being an entry point the moment gamma calls it"
    );
    assert_eq!(labels(&find(&net.roots, "gamma").children), ["alpha"]);
}

// -------------------------------------------------------- forest shaping

#[test]
fn a_callee_shared_by_two_callers_expands_once_and_repeats_as_a_leaf() {
    let (_, net) = diff_one_file(
        "def top():\n    pass\n",
        "def top():\n    left()\n    right()\n\n\ndef left():\n    shared()\n\n\ndef right():\n    shared()\n\n\ndef shared():\n    tail()\n\n\ndef tail():\n    pass\n",
    );

    assert_eq!(labels(&net.roots), ["top"]);
    let top = find(&net.roots, "top");
    assert_eq!(labels(&top.children), ["left", "right"]);

    let first = find(&find(&top.children, "left").children, "shared");
    assert!(!first.repeat, "the first caller expands the shared callee");
    assert_eq!(labels(&first.children), ["tail"]);

    let second = find(&find(&top.children, "right").children, "shared");
    assert!(
        second.repeat,
        "the second caller draws the shared callee as an already-expanded leaf"
    );
    assert!(
        second.children.is_empty(),
        "a repeat node stops the branch; it must not re-expand {:?}",
        labels(&second.children)
    );
    assert_eq!(second.key, NodeKey::Operation("pkg.mod.shared".to_string()));
}

#[test]
fn operations_reachable_only_through_a_cycle_hang_off_the_cycle_root() {
    let (_, net) = diff_one_file(
        "def solo():\n    pass\n",
        "def solo():\n    pass\n\n\ndef ping():\n    pong()\n\n\ndef pong():\n    ping()\n",
    );

    assert_eq!(
        labels(&net.roots),
        ["solo", "<cycle>"],
        "the cycle bucket is appended after the real entry points"
    );
    let cycle = find(&net.roots, "<cycle>");
    assert_eq!(cycle.key, NodeKey::External("<cycle>".to_string()));
    assert_eq!(labels(&cycle.children), ["ping", "pong"]);

    let ping = find(&cycle.children, "ping");
    assert!(!ping.repeat);
    let pong_under_ping = find(&ping.children, "pong");
    assert!(
        find(&pong_under_ping.children, "ping").repeat,
        "following the cycle back round stops at a repeat leaf"
    );
    assert!(
        find(&cycle.children, "pong").repeat,
        "pong was already expanded under ping, so its own bucket entry is a leaf"
    );
    assert!(
        find(&cycle.children, "pong").children.is_empty(),
        "the repeat leaf carries no children"
    );
}

#[test]
fn a_self_recursive_call_is_not_an_edge_and_leaves_the_operation_an_entry_point() {
    let (_, net) = diff_one_file(
        "def spin(n):\n    pass\n",
        "def spin(n):\n    spin(n - 1)\n",
    );

    assert_eq!(
        labels(&net.roots),
        ["spin"],
        "calling itself does not give an operation an in-package caller"
    );
    assert!(
        find(&net.roots, "spin").children.is_empty(),
        "recursion is reported on the operation, not drawn as a child"
    );
    assert!(entry(&net, "pkg.mod.spin").operation.recursive);
}

// -------------------------------------------------------------- tallies

#[test]
fn link_probable_and_external_tallies_describe_the_current_snapshot_only() {
    let before = "import json\n\n\ndef alpha():\n    beta()\n    gamma()\n    json.dumps({})\n    json.loads(\"\")\n\n\ndef beta():\n    gamma()\n\n\ndef gamma():\n    pass\n";
    let after = "import json\n\n\ndef alpha():\n    beta()\n    json.dumps({})\n\n\ndef beta():\n    Widget().poke()\n\n\nclass Widget:\n    def __init__(self):\n        pass\n\n    def poke(self):\n        self.helper()\n\n    def helper(self):\n        pass\n";
    let (_, net) = diff_one_file(before, after);

    assert_eq!(
        net.links, 4,
        "alpha->beta, beta->Widget.__init__, beta->Widget.poke, Widget.poke->Widget.helper"
    );
    assert_eq!(
        net.probable, 1,
        "only `Widget().poke` rests on an unambiguous-name match"
    );
    assert_eq!(
        net.external, 1,
        "json.dumps leaves the package; the baseline's second json call is not counted"
    );
}

#[test]
fn an_inferred_edge_is_annotated_in_the_tree() {
    let (_, net) = diff_one_file(
        "def run():\n    pass\n",
        "def run():\n    handle.poke()\n\n\nclass Widget:\n    def poke(self):\n        pass\n",
    );

    let run = find(&net.roots, "run");
    let poke = find(&run.children, "Widget.poke");
    assert_eq!(
        poke.note.as_deref(),
        Some("?"),
        "the receiver's type is unknowable from syntax, so the edge is only probable"
    );
    assert_eq!(net.probable, 1);
    assert_eq!(net.links, 1);
}

// ------------------------------------------------------------- overrides

#[test]
fn overrides_names_the_inherited_method_a_method_shadows() {
    let source = "class Base:\n    def ping(self):\n        pass\n\n\nclass Derived(Base):\n    def ping(self):\n        pass\n\n    def solo(self):\n        pass\n\n\ndef free():\n    pass\n";
    let (_, net) = diff_one_file(source, source);

    assert_eq!(
        entry(&net, "pkg.mod.Derived.ping").overrides.as_deref(),
        Some("Base.ping"),
        "the shadowed method is reported by display name"
    );
    assert_eq!(
        entry(&net, "pkg.mod.Derived.solo").overrides,
        None,
        "a method with no counterpart on any base overrides nothing"
    );
    assert_eq!(
        entry(&net, "pkg.mod.Base.ping").overrides,
        None,
        "a root class inherits nothing in-package"
    );
    assert_eq!(
        entry(&net, "pkg.mod.free").overrides,
        None,
        "a module-level function has no owner and so cannot override"
    );
}
