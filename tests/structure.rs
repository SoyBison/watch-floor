//! The structure view: class statuses, the inheritance forest, and the shared
//! tree machinery in `sitrep` that every view draws through.

mod common;

use common::{diff_one_file, Fixture};

use watch_floor::sitrep::{delta, flatten, leaf, Counts, Edge, NodeKey, Row, Status, TreeNode};
use watch_floor::structure::{Entry, Structure};

// ---------------------------------------------------------------- helpers --

fn entry<'a>(structure: &'a Structure, qualname: &str) -> &'a Entry {
    structure.entries.get(qualname).unwrap_or_else(|| {
        panic!(
            "no entry for {qualname}; have {:?}",
            structure.entries.keys().collect::<Vec<_>>()
        )
    })
}

fn status_of(structure: &Structure, qualname: &str) -> Status {
    entry(structure, qualname).status
}

fn keys(structure: &Structure) -> Vec<&str> {
    structure.entries.keys().map(String::as_str).collect()
}

/// One string per row: exactly what the floor draws, prefix included.
fn drawn(rows: &[Row<'_>]) -> Vec<String> {
    rows.iter()
        .map(|r| format!("{}{}", r.prefix, r.node.label))
        .collect()
}

fn twig(label: &str, touched: bool, children: Vec<TreeNode>) -> TreeNode {
    let mut node = TreeNode::new(NodeKey::External(label.to_string()), label.to_string());
    node.touched = touched;
    node.children = children;
    node
}

// ------------------------------------------------------ status per class --

#[test]
fn class_absent_from_the_baseline_is_activated() {
    let (structure, _) = diff_one_file(
        "class Channel:\n    def open(self): ...\n",
        "class Channel:\n    def open(self): ...\n\n\nclass Secure(Channel):\n    def seal(self): ...\n",
    );

    assert_eq!(status_of(&structure, "pkg.mod.Secure"), Status::Activated);
    assert_eq!(
        status_of(&structure, "pkg.mod.Channel"),
        Status::Nominal,
        "gaining a subclass does not change the base class itself"
    );
    let new = entry(&structure, "pkg.mod.Secure");
    assert!(
        new.prior_bases.is_empty() && new.prior_module.is_none(),
        "a brand-new class has no baseline side: {new:?}"
    );
    assert_eq!(structure.counts.activated, 1);
    assert_eq!(structure.counts.changed(), 1);
}

#[test]
fn class_gone_from_the_working_tree_is_burned_and_still_drawn() {
    let (structure, _) = diff_one_file(
        "class Channel:\n    def open(self): ...\n\n\nclass Secure(Channel):\n    def seal(self): ...\n",
        "class Channel:\n    def open(self): ...\n",
    );

    let gone = entry(&structure, "pkg.mod.Secure");
    assert_eq!(gone.status, Status::Burned);
    assert_eq!(
        gone.subject.module, "pkg.mod",
        "a burned entry carries the baseline-side subject"
    );
    assert_eq!(
        gone.subject.line, 5,
        "line of the `class` keyword at baseline"
    );
    assert_eq!(structure.counts.burned, 1);
    assert_eq!(
        drawn(&structure.rows(false)),
        vec!["object", "  └─ Channel", "     └─ Secure"],
        "a burned class stays in the forest — that is how you see what vanished"
    );
}

#[test]
fn class_whose_shape_is_untouched_is_nominal() {
    let (structure, _) = diff_one_file(
        "class Channel:\n    def open(self):\n        print(1)\n",
        "# a comment\ndef helper(): ...\n\n\nclass Channel:\n    def open(self):\n        helper()\n        print(2)\n",
    );

    assert_eq!(
        status_of(&structure, "pkg.mod.Channel"),
        Status::Nominal,
        "rewriting a method body and adding a free function leaves the class shape alone"
    );
    assert_eq!(structure.counts.changed(), 0);
    assert_eq!(structure.counts.nominal, 1);
    assert!(structure.feed().is_empty());
}

#[test]
fn class_that_swaps_base_classes_is_realigned() {
    let (structure, _) = diff_one_file(
        "class Alpha: ...\nclass Beta: ...\nclass Gamma(Alpha):\n    def run(self): ...\n",
        "class Alpha: ...\nclass Beta: ...\nclass Gamma(Beta):\n    def run(self): ...\n",
    );

    let moved = entry(&structure, "pkg.mod.Gamma");
    assert_eq!(moved.status, Status::Realigned);
    assert_eq!(moved.prior_bases, vec!["Alpha".to_string()]);
    assert_eq!(moved.subject.bases, vec!["Beta".to_string()]);
    assert!(
        moved.gained.is_empty() && moved.lost.is_empty(),
        "the method set held: {moved:?}"
    );
    assert_eq!(structure.counts.realigned, 1);
    assert_eq!(
        drawn(&structure.rows(false)),
        vec!["object", "  ├─ Alpha", "  └─ Beta", "     └─ Gamma"],
        "the forest follows the new base, not the old one"
    );
}

#[test]
fn class_that_changes_method_set_but_keeps_its_bases_is_amended() {
    let (structure, _) = diff_one_file(
        "class Alpha: ...\nclass Gamma(Alpha):\n    def run(self): ...\n    def stop(self): ...\n",
        "class Alpha: ...\nclass Gamma(Alpha):\n    def run(self): ...\n    def start(self): ...\n",
    );

    let touched = entry(&structure, "pkg.mod.Gamma");
    assert_eq!(touched.status, Status::Amended);
    assert_eq!(touched.gained, vec!["start".to_string()]);
    assert_eq!(touched.lost, vec!["stop".to_string()]);
    assert_eq!(
        touched.subject.bases,
        vec!["Alpha".to_string()],
        "bases held, which is what keeps this Amended rather than Realigned"
    );
    assert_eq!(structure.counts.amended, 1);
    assert_eq!(structure.counts.realigned, 0);
}

#[test]
fn class_moved_to_another_module_is_relocated_and_leaves_no_burned_twin() {
    let fx = Fixture::new();
    fx.write("pkg/__init__.py", "");
    fx.write(
        "pkg/agents.py",
        "class Handler:\n    def brief(self): ...\n",
    );
    fx.write("pkg/core.py", "class Channel:\n    def open(self): ...\n");
    fx.commit();
    fx.write("pkg/agents.py", "");
    fx.write(
        "pkg/core.py",
        "class Channel:\n    def open(self): ...\n\n\nclass Handler:\n    def brief(self): ...\n",
    );
    let structure = fx.structure();

    assert_eq!(
        keys(&structure),
        vec!["pkg.core.Channel", "pkg.core.Handler"],
        "the burned twin is folded into the relocated entry, not left beside it"
    );
    let moved = entry(&structure, "pkg.core.Handler");
    assert_eq!(moved.status, Status::Relocated);
    assert_eq!(moved.prior_module, Some("pkg.agents".to_string()));
    assert!(
        moved.gained.is_empty() && moved.lost.is_empty(),
        "nothing but the module changed: {moved:?}"
    );
    assert_eq!(structure.counts.relocated, 1);
    assert_eq!(structure.counts.burned, 0);
    assert_eq!(structure.counts.activated, 0);
    assert_eq!(structure.counts.total(), 2);
    assert_eq!(
        drawn(&structure.rows(false)),
        vec!["object", "  ├─ Channel", "  └─ Handler"],
        "one node for the moved class, not a live one and a burned one"
    );
}

#[test]
fn move_that_also_changes_bases_is_not_a_relocation() {
    let fx = Fixture::new();
    fx.write("pkg/__init__.py", "");
    fx.write(
        "pkg/agents.py",
        "class Base: ...\n\n\nclass Handler(Base):\n    def brief(self): ...\n",
    );
    fx.write("pkg/core.py", "");
    fx.commit();
    fx.write("pkg/agents.py", "class Base: ...\n");
    fx.write("pkg/core.py", "class Handler:\n    def brief(self): ...\n");
    let structure = fx.structure();

    assert_eq!(
        status_of(&structure, "pkg.agents.Handler"),
        Status::Burned,
        "dropping the base makes this a rewrite, so the old class is still burned"
    );
    assert_eq!(status_of(&structure, "pkg.core.Handler"), Status::Activated);
    assert_eq!(structure.counts.relocated, 0);
    assert_eq!(structure.counts.activated, 1);
    assert_eq!(structure.counts.burned, 1);
}

#[test]
fn move_with_two_candidate_destinations_is_not_a_relocation() {
    let fx = Fixture::new();
    fx.write("pkg/__init__.py", "");
    fx.write("pkg/a.py", "class Widget:\n    def draw(self): ...\n");
    fx.commit();
    fx.write("pkg/a.py", "");
    fx.write("pkg/b.py", "class Widget:\n    def draw(self): ...\n");
    fx.write("pkg/c.py", "class Widget:\n    def draw(self): ...\n");
    let structure = fx.structure();

    assert_eq!(
        keys(&structure),
        vec!["pkg.a.Widget", "pkg.b.Widget", "pkg.c.Widget"],
        "an ambiguous move keeps every entry rather than guessing a pairing"
    );
    assert_eq!(status_of(&structure, "pkg.a.Widget"), Status::Burned);
    assert_eq!(status_of(&structure, "pkg.b.Widget"), Status::Activated);
    assert_eq!(status_of(&structure, "pkg.c.Widget"), Status::Activated);
    assert_eq!(structure.counts.relocated, 0);
}

// ------------------------------------------------------------- the forest --

#[test]
fn class_hangs_off_its_first_in_package_base_with_the_rest_as_mixins() {
    let src = "class Alpha: ...\nclass Beta: ...\nclass Gamma(Alpha, Beta, Ext): ...\n";
    let (structure, _) = diff_one_file(src, src);

    assert_eq!(
        drawn(&structure.rows(false)),
        vec!["object", "  ├─ Alpha", "  │  └─ Gamma", "  └─ Beta"],
        "Gamma hangs off Alpha, its first base, and appears exactly once"
    );
    assert_eq!(
        entry(&structure, "pkg.mod.Gamma").mixins,
        vec!["Beta".to_string(), "Ext".to_string()],
        "every base beyond the one it hangs off rides along as a mixin"
    );
}

#[test]
fn an_external_base_is_skipped_in_favour_of_the_first_in_package_one() {
    let src = "class Alpha: ...\nclass Gamma(Ext, Alpha): ...\n";
    let (structure, _) = diff_one_file(src, src);

    assert_eq!(
        drawn(&structure.rows(false)),
        vec!["object", "  └─ Alpha", "     └─ Gamma"],
        "placement follows the first base that resolves inside the package"
    );
    assert_eq!(
        entry(&structure, "pkg.mod.Gamma").mixins,
        vec!["Ext".to_string()],
        "the unresolvable base is demoted to a mixin, not made a root"
    );
}

#[test]
fn class_with_no_bases_at_all_sits_under_the_object_root() {
    let src = "class Lonely:\n    def ping(self): ...\n";
    let (structure, _) = diff_one_file(src, src);

    assert_eq!(structure.roots.len(), 1);
    assert_eq!(structure.roots[0].key, NodeKey::External("object".into()));
    assert_eq!(structure.roots[0].label, "object");
    assert_eq!(
        structure.roots[0]
            .children
            .iter()
            .map(|c| c.key.clone())
            .collect::<Vec<_>>(),
        vec![NodeKey::Subject("pkg.mod.Lonely".into())]
    );
    assert!(
        entry(&structure, "pkg.mod.Lonely").mixins.is_empty(),
        "the implicit object root is not a mixin"
    );
}

#[test]
fn unknown_base_becomes_an_external_root_named_after_it() {
    let src = "class Report(BaseModel, Serializable): ...\n";
    let (structure, _) = diff_one_file(src, src);

    assert_eq!(
        structure.roots.len(),
        1,
        "no object root is invented when the class already has a base we cannot see"
    );
    assert_eq!(
        structure.roots[0].key,
        NodeKey::External("BaseModel".into())
    );
    assert_eq!(
        drawn(&structure.rows(false)),
        vec!["BaseModel", "  └─ Report"]
    );
    assert_eq!(
        entry(&structure, "pkg.mod.Report").mixins,
        vec!["Serializable".to_string()],
        "only the first external base becomes the root; the rest are mixins"
    );
}

#[test]
fn inheritance_cycle_is_surfaced_under_the_cycle_root() {
    let src = "class Alpha(Beta): ...\nclass Beta(Alpha): ...\n";
    let (structure, _) = diff_one_file(src, src);

    assert_eq!(
        structure.roots.len(),
        1,
        "a pure cycle reaches no external root, so only the cycle bucket is drawn"
    );
    assert_eq!(structure.roots[0].key, NodeKey::External("<cycle>".into()));
    assert_eq!(
        drawn(&structure.rows(false)),
        vec!["<cycle>", "  └─ Alpha", "     └─ Beta"],
        "both classes are surfaced rather than dropped, each drawn once"
    );
}

#[test]
fn counts_tally_every_status_and_add_up_to_the_entry_count() {
    let (structure, _) = diff_one_file(
        "class Keep: ...\nclass Gone: ...\nclass Rebase(Keep): ...\nclass Methods:\n    def a(self): ...\n",
        "class Keep: ...\nclass Rebase(Outside): ...\nclass Methods:\n    def a(self): ...\n    def b(self): ...\nclass Fresh: ...\n",
    );

    let c = structure.counts;
    assert_eq!(
        (
            c.activated,
            c.burned,
            c.realigned,
            c.relocated,
            c.amended,
            c.nominal
        ),
        (1, 1, 1, 0, 1, 1),
        "Fresh activated, Gone burned, Rebase realigned, Methods amended, Keep nominal"
    );
    assert_eq!(c.rerouted, 0, "rerouted belongs to the traffic view");
    assert_eq!(c.changed(), 4);
    assert_eq!(c.total(), 5);
    assert_eq!(
        c.total(),
        structure.entries.len(),
        "every entry is counted exactly once"
    );
}

#[test]
fn feed_lists_only_changes_ordered_by_severity_then_name() {
    let (structure, _) = diff_one_file(
        "class Alpha: ...\nclass Bravo: ...\nclass Delta(Bravo): ...\nclass Echo:\n    def one(self): ...\nclass Foxtrot: ...\n",
        "class Alpha: ...\nclass Delta(Alpha): ...\nclass Echo:\n    def one(self): ...\n    def two(self): ...\nclass Zulu: ...\nclass Abel: ...\n",
    );

    let feed: Vec<(&str, Status)> = structure
        .feed()
        .iter()
        .map(|e| (e.subject.qualname.as_str(), e.status))
        .collect();
    assert_eq!(
        feed,
        vec![
            ("pkg.mod.Abel", Status::Activated),
            ("pkg.mod.Zulu", Status::Activated),
            ("pkg.mod.Bravo", Status::Burned),
            ("pkg.mod.Foxtrot", Status::Burned),
            ("pkg.mod.Delta", Status::Realigned),
            ("pkg.mod.Echo", Status::Amended),
        ],
        "activated, then burned, then realigned, then amended; ties broken by qualname"
    );
    assert!(
        !feed.iter().any(|(q, _)| *q == "pkg.mod.Alpha"),
        "the nominal class is left out of the feed"
    );
}

// ---------------------------------------------------- sitrep: flattening --

#[test]
fn flatten_draws_branch_connectors_and_continuation_bars() {
    let roots = vec![twig(
        "root",
        false,
        vec![
            twig(
                "a",
                false,
                vec![twig("a1", false, vec![]), twig("a2", false, vec![])],
            ),
            twig("b", false, vec![twig("b1", false, vec![])]),
        ],
    )];

    let rows = flatten(&roots, false);
    assert_eq!(
        drawn(&rows),
        vec![
            "root",
            "  ├─ a",
            "  │  ├─ a1",
            "  │  └─ a2",
            "  └─ b",
            "     └─ b1",
        ],
        "a non-last branch carries a │ down its children; the last one carries spaces"
    );
    assert_eq!(rows[0].prefix, "", "roots are drawn flush left");
}

#[test]
fn flatten_draws_every_root_flush_left() {
    let roots = vec![
        twig("one", false, vec![twig("kid", false, vec![])]),
        twig("two", false, vec![]),
    ];

    let rows = flatten(&roots, false);
    assert_eq!(
        rows.iter().map(|r| r.prefix.as_str()).collect::<Vec<_>>(),
        vec!["", "  └─ ", ""],
        "roots carry no connector however many there are; only their children do"
    );
}

#[test]
fn flatten_changes_only_prunes_quiet_branches_but_keeps_ancestors() {
    let roots = vec![
        twig(
            "root",
            true,
            vec![
                twig("quiet", false, vec![twig("quiet_kid", false, vec![])]),
                twig(
                    "loud",
                    true,
                    vec![twig("spark", true, vec![]), twig("dull", false, vec![])],
                ),
            ],
        ),
        twig("cold", false, vec![]),
    ];

    assert_eq!(
        drawn(&flatten(&roots, false)),
        vec![
            "root",
            "  ├─ quiet",
            "  │  └─ quiet_kid",
            "  └─ loud",
            "     ├─ spark",
            "     └─ dull",
            "cold",
        ]
    );
    assert_eq!(
        drawn(&flatten(&roots, true)),
        vec!["root", "  └─ loud", "     └─ spark"],
        "untouched branches and roots go, but the touched node's ancestors stay"
    );
}

#[test]
fn structure_rows_changes_only_keep_the_chain_down_to_the_changed_class() {
    let (structure, _) = diff_one_file(
        "class Base: ...\nclass Mid(Base): ...\nclass Leaf(Mid):\n    def one(self): ...\nclass Side: ...\n",
        "class Base: ...\nclass Mid(Base): ...\nclass Leaf(Mid):\n    def one(self): ...\n    def two(self): ...\nclass Side: ...\n",
    );

    assert_eq!(status_of(&structure, "pkg.mod.Leaf"), Status::Amended);
    assert_eq!(
        drawn(&structure.rows(false)),
        vec![
            "object",
            "  ├─ Base",
            "  │  └─ Mid",
            "  │     └─ Leaf",
            "  └─ Side",
        ]
    );
    assert_eq!(
        drawn(&structure.rows(true)),
        vec!["object", "  └─ Base", "     └─ Mid", "        └─ Leaf"],
        "Side is pruned, and Base/Mid become last-children so their connectors change"
    );
}

// ------------------------------------------------- sitrep: vocabulary --

#[test]
fn delta_reports_what_was_gained_and_lost_in_sorted_order() {
    let before = vec!["b".to_string(), "a".to_string(), "keep".to_string()];
    let after = vec!["z".to_string(), "keep".to_string(), "m".to_string()];

    let (gained, lost) = delta(&before, &after);
    assert_eq!(gained, vec!["m".to_string(), "z".to_string()]);
    assert_eq!(lost, vec!["a".to_string(), "b".to_string()]);

    let (gained, lost) = delta(&before, &before);
    assert!(
        gained.is_empty() && lost.is_empty(),
        "an identical list yields no delta"
    );

    let (gained, lost) = delta(&[], &["only".to_string()]);
    assert_eq!(gained, vec!["only".to_string()]);
    assert!(lost.is_empty());
}

#[test]
fn status_glyphs_and_tags_are_the_legend_the_readme_documents() {
    let table = [
        (Status::Activated, "+", "ACTIVATED"),
        (Status::Burned, "-", "BURNED"),
        (Status::Realigned, "~", "REALIGNED"),
        (Status::Rerouted, "~", "REROUTED"),
        (Status::Relocated, "→", "RELOCATED"),
        (Status::Amended, "*", "AMENDED"),
        (Status::Nominal, "·", ""),
    ];
    for (status, glyph, tag) in table {
        assert_eq!(status.glyph(), glyph, "glyph for {status:?}");
        assert_eq!(status.tag(), tag, "tag for {status:?}");
        assert_eq!(
            status.is_change(),
            status != Status::Nominal,
            "only Nominal means nothing happened ({status:?})"
        );
    }
    assert_eq!(
        Status::STRUCTURE,
        [
            Status::Activated,
            Status::Burned,
            Status::Realigned,
            Status::Relocated,
            Status::Amended
        ],
        "the structure legend never offers Rerouted"
    );
}

#[test]
fn edge_tags_name_only_the_traffic_that_changed() {
    assert_eq!(Edge::Live.tag(), "");
    assert_eq!(Edge::Opened.tag(), "NEW TRAFFIC");
    assert_eq!(Edge::Closed.tag(), "WENT DARK");
    assert!(!Edge::Live.is_change());
    assert!(Edge::Opened.is_change());
    assert!(Edge::Closed.is_change());
}

#[test]
fn counts_record_and_report_each_status_separately() {
    let mut counts = Counts::default();
    assert_eq!((counts.changed(), counts.total()), (0, 0));

    counts.record(Status::Activated);
    counts.record(Status::Activated);
    counts.record(Status::Rerouted);
    counts.record(Status::Nominal);

    assert_eq!(counts.of(Status::Activated), 2);
    assert_eq!(counts.of(Status::Rerouted), 1);
    assert_eq!(counts.of(Status::Nominal), 1);
    assert_eq!(counts.of(Status::Burned), 0);
    assert_eq!(counts.changed(), 3, "nominal is not a change");
    assert_eq!(counts.total(), 4);
}

#[test]
fn leaf_takes_the_last_dotted_segment() {
    assert_eq!(leaf("pkg.mod.Outer.Inner"), "Inner");
    assert_eq!(leaf("bare"), "bare");
    assert_eq!(leaf(""), "");
}
