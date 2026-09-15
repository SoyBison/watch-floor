//! Collection: finding the package, and reading the two snapshots (working tree
//! from disk, baseline out of git). Plus `report`, the text rendering of both.

mod common;

use common::Fixture;

use watch_floor::app::View;
use watch_floor::collection::Target;
use watch_floor::intercept::Interceptor;
use watch_floor::report;
use watch_floor::sitrep::Status;

/// Subject keys of a dossier, so tests can assert on exact module naming.
fn class_names(dossier: &watch_floor::dossier::Dossier) -> Vec<String> {
    dossier.subjects.keys().cloned().collect()
}

// ---------------------------------------------------------------- acquisition

#[test]
fn acquire_accepts_the_package_directory_itself() {
    let fx = Fixture::new();
    fx.write("pkg/__init__.py", "");
    fx.write("pkg/sub/__init__.py", "");

    let target = Target::acquire(&fx.root.join("pkg")).expect("acquire the package directory");

    assert_eq!(target.pkg_dir, fx.root.join("pkg"));
    assert_eq!(target.repo_root, fx.root);
    assert_eq!(target.pkg_rel, "pkg");
    assert_eq!(
        target.pkg_name, "pkg",
        "a directory with __init__.py is the package, even though it contains another one"
    );
}

#[test]
fn acquire_auto_detects_a_single_package_one_level_down() {
    let fx = Fixture::new();
    fx.write("pkg/__init__.py", "");
    fx.write("tests/test_pkg.py", "");
    fx.write("README.md", "");

    let target = Target::acquire(&fx.root).expect("acquire from the repo root");

    assert_eq!(target.pkg_dir, fx.root.join("pkg"));
    assert_eq!(target.pkg_rel, "pkg");
    assert_eq!(
        target.pkg_name, "pkg",
        "tests/ has no __init__.py, so it is not a candidate"
    );
}

#[test]
fn acquire_auto_detects_the_src_layout() {
    let fx = Fixture::new();
    fx.write("src/pkg/__init__.py", "");
    fx.write(
        "src/pkg/core.py",
        "class Channel:\n    def open(self): ...\n",
    );
    fx.commit();

    let target = Target::acquire(&fx.root).expect("acquire from the repo root");

    assert_eq!(target.repo_root, fx.root);
    assert_eq!(target.pkg_dir, fx.root.join("src").join("pkg"));
    assert_eq!(
        target.pkg_rel, "src/pkg",
        "pkg_rel is relative to the repo root, not to src/"
    );
    assert_eq!(
        target.pkg_name, "pkg",
        "the module name is the directory name, src/ is not part of it"
    );

    // The baseline listing is filtered by pkg_rel, and the prefix has to come
    // back off again before the module name is worked out.
    let mut interceptor = Interceptor::new().expect("interceptor");
    let baseline = target
        .survey_baseline(&mut interceptor)
        .expect("survey baseline");
    assert_eq!(class_names(&baseline), vec!["pkg.core.Channel".to_string()]);
}

#[test]
fn acquire_errors_when_there_is_no_package() {
    let fx = Fixture::new();
    fx.write("notes/readme.txt", "no python here");

    let err = Target::acquire(&fx.root).expect_err("a repo with no package cannot be watched");

    assert_eq!(
        err.to_string(),
        format!(
            "no Python package under {} (looked for __init__.py here, in ./*/ and in ./src/*/)",
            fx.root.display()
        )
    );
}

#[test]
fn acquire_errors_when_several_packages_are_candidates() {
    let fx = Fixture::new();
    fx.write("alpha/__init__.py", "");
    fx.write("beta/__init__.py", "");

    let err = Target::acquire(&fx.root).expect_err("an ambiguous root cannot be watched");
    let message = err.to_string();

    // read_dir order is not defined, so the two names may come in either order.
    assert!(
        message.starts_with(&format!("several packages under {} (", fx.root.display())),
        "message should name the directory it looked in, got: {message}"
    );
    assert!(
        message.contains("alpha") && message.contains("beta"),
        "message should list both candidates, got: {message}"
    );
    assert!(
        message.ends_with(") — name the one to watch"),
        "message should say how to disambiguate, got: {message}"
    );
}

#[test]
fn acquire_errors_on_a_nonexistent_path() {
    let fx = Fixture::new();
    let missing = fx.root.join("nowhere");

    let err = Target::acquire(&missing).expect_err("a path that does not exist cannot be watched");

    assert_eq!(
        err.to_string(),
        format!("cannot open {}", missing.display())
    );
}

// -------------------------------------------------------------- module naming

#[test]
fn module_names_follow_the_path_below_the_package_root() {
    let fx = Fixture::new();
    fx.write("pkg/__init__.py", "class Root: ...\n");
    fx.write("pkg/mod.py", "class Flat: ...\n");
    fx.write("pkg/sub/__init__.py", "class Nested: ...\n");
    fx.write("pkg/sub/deep.py", "class Deep: ...\n");
    fx.commit();

    let (baseline, current) = fx.snapshots();
    let expected = vec![
        "pkg.Root".to_string(),          // pkg/__init__.py    -> pkg
        "pkg.mod.Flat".to_string(),      // pkg/mod.py         -> pkg.mod
        "pkg.sub.Nested".to_string(),    // pkg/sub/__init__.py -> pkg.sub
        "pkg.sub.deep.Deep".to_string(), // pkg/sub/deep.py    -> pkg.sub.deep
    ];
    assert_eq!(class_names(&current), expected);
    assert_eq!(
        class_names(&baseline),
        expected,
        "git-side paths name modules the same way as disk-side ones"
    );
}

// ------------------------------------------------------------ the two sources

#[test]
fn survey_baseline_reads_committed_content_rather_than_disk() {
    let fx = Fixture::new();
    fx.write("pkg/__init__.py", "");
    fx.write("pkg/mod.py", "class Alpha:\n    def a(self): ...\n");
    fx.commit();
    fx.write("pkg/mod.py", "class Beta:\n    def b(self): ...\n");

    let (baseline, current) = fx.snapshots();

    assert_eq!(
        class_names(&baseline),
        vec!["pkg.mod.Alpha".to_string()],
        "the baseline is the blob at HEAD, not the file on disk"
    );
    assert_eq!(class_names(&current), vec!["pkg.mod.Beta".to_string()]);
    assert_eq!(baseline.files, 2);
    assert_eq!(current.files, 2);
    assert_eq!(
        baseline.label,
        format!(
            "HEAD @ {}",
            fx.target().head_id().expect("head after commit")
        ),
        "the baseline is labelled with the commit it came from"
    );
    assert_eq!(current.label, "working tree");
}

#[test]
fn unborn_head_yields_an_empty_baseline_and_no_error() {
    let fx = Fixture::new();
    fx.write("pkg/__init__.py", "");
    fx.write("pkg/mod.py", "class Alpha: ...\n");
    // Deliberately no commit: the branch has no HEAD yet.

    let target = fx.target();
    let mut interceptor = Interceptor::new().expect("interceptor");
    let baseline = target
        .survey_baseline(&mut interceptor)
        .expect("an unborn branch is not an error");

    assert_eq!(baseline.label, "HEAD (unborn)");
    assert_eq!(baseline.files, 0);
    assert!(baseline.subjects.is_empty());
    assert!(baseline.operations.is_empty());

    let current = target
        .survey_working_tree(&mut interceptor)
        .expect("survey working tree");
    assert_eq!(
        class_names(&current),
        vec!["pkg.mod.Alpha".to_string()],
        "the working tree is still readable with nothing committed"
    );
}

#[test]
fn head_id_is_none_before_the_first_commit_and_a_short_sha_after() {
    let fx = Fixture::new();
    fx.write("pkg/__init__.py", "");

    let target = fx.target();
    assert_eq!(
        target.head_id(),
        None,
        "an unborn branch has no object to report"
    );

    fx.commit();
    let full = fx.git(&["rev-parse", "HEAD"]);
    let short = target.head_id().expect("HEAD exists after a commit");

    assert!(
        full.starts_with(&short),
        "head_id {short} should abbreviate HEAD {full}"
    );
    assert!(
        short.len() < full.len() && short.len() >= 4,
        "head_id should be an abbreviation, got {short}"
    );
}

#[test]
fn a_file_deleted_from_the_working_tree_burns_its_classes() {
    let fx = Fixture::new();
    fx.write("pkg/__init__.py", "");
    fx.write("pkg/kept.py", "class Kept: ...\n");
    fx.write("pkg/gone.py", "class Ghost:\n    def haunt(self): ...\n");
    fx.commit();
    fx.remove("pkg/gone.py");

    let (baseline, current) = fx.snapshots();
    assert!(
        baseline.subjects.contains_key("pkg.gone.Ghost"),
        "the deleted file is still committed, so it is still at baseline"
    );
    assert!(!current.subjects.contains_key("pkg.gone.Ghost"));
    assert_eq!(baseline.files, 3);
    assert_eq!(current.files, 2);

    let structure = fx.structure();
    let ghost = structure
        .entries
        .get("pkg.gone.Ghost")
        .expect("a burned class still has an entry");
    assert_eq!(ghost.status, Status::Burned);
    assert_eq!(ghost.subject.module, "pkg.gone");
    assert_eq!(
        structure.entries.get("pkg.kept.Kept").map(|e| e.status),
        Some(Status::Nominal),
        "the untouched file is unaffected by the deletion"
    );
    assert_eq!(structure.counts.of(Status::Burned), 1);
}

#[test]
fn pycache_and_dot_directories_are_skipped_by_the_working_tree_walk() {
    let fx = Fixture::new();
    fx.write("pkg/__init__.py", "");
    fx.write("pkg/real.py", "class Real: ...\n");
    fx.commit();
    fx.write("pkg/__pycache__/real.py", "class Cached: ...\n");
    fx.write("pkg/sub/__pycache__/stale.py", "class Stale: ...\n");
    fx.write("pkg/.hidden/secret.py", "class Secret: ...\n");
    fx.write("pkg/.venv/lib/vendored.py", "class Vendored: ...\n");

    let (_, current) = fx.snapshots();

    assert_eq!(
        class_names(&current),
        vec!["pkg.real.Real".to_string()],
        "caches and dot-directories are not source"
    );
    assert_eq!(
        current.files, 2,
        "only pkg/__init__.py and pkg/real.py are read"
    );
}

#[test]
fn the_baseline_skips_the_same_directories_as_the_working_tree() {
    let fx = Fixture::new();
    fx.write("pkg/__init__.py", "");
    fx.write("pkg/real.py", "class Real: ...\n");
    // Jupyter checkpoints and caches do occasionally get committed.
    fx.write(
        "pkg/.ipynb_checkpoints/real-checkpoint.py",
        "class Real: ...\n",
    );
    fx.write("pkg/__pycache__/real.py", "class Cached: ...\n");
    fx.commit();

    let (baseline, current) = fx.snapshots();

    assert_eq!(
        class_names(&baseline),
        class_names(&current),
        "nothing changed on disk, so the two snapshots must agree"
    );
    assert_eq!(
        baseline.files, 2,
        "caches and dot-directories are not source"
    );

    let structure = fx.structure();
    let burned: Vec<&String> = structure
        .entries
        .iter()
        .filter(|(_, e)| e.status == Status::Burned)
        .map(|(k, _)| k)
        .collect();
    assert!(
        burned.is_empty(),
        "an unmodified tree cannot have burned classes, got {burned:?}"
    );
}

#[test]
fn a_repository_that_is_itself_the_package_still_has_a_baseline() {
    let fx = Fixture::new();
    fx.write("__init__.py", "");
    fx.write("mod.py", "class Alpha: ...\n");
    fx.commit();

    let target = fx.target();
    assert_eq!(target.pkg_rel, "", "the package is the repository root");

    let mut interceptor = Interceptor::new().expect("interceptor");
    let baseline = target
        .survey_baseline(&mut interceptor)
        .expect("a root-level package has a readable baseline");
    let current = target
        .survey_working_tree(&mut interceptor)
        .expect("survey working tree");

    assert_eq!(
        class_names(&current),
        vec![format!("{}.mod.Alpha", target.pkg_name)]
    );
    assert_eq!(
        class_names(&baseline),
        class_names(&current),
        "nothing has changed since the commit"
    );
}

// -------------------------------------------------------------------- reports

/// A package with one changed class and one changed call, so both sections of
/// the report have something to say.
fn reporting_fixture() -> Fixture {
    let fx = Fixture::new();
    fx.write("pkg/__init__.py", "");
    fx.write(
        "pkg/core.py",
        "class Channel:\n    def open(self): ...\n\n\ndef transmit():\n    pass\n",
    );
    fx.commit();
    fx.write(
        "pkg/core.py",
        "class Channel:\n    def open(self): ...\n\n\nclass Secure(Channel):\n    def seal(self): ...\n\n\ndef transmit():\n    encode()\n\n\ndef encode():\n    pass\n",
    );
    fx
}

#[test]
fn report_without_a_filter_contains_both_views() {
    let fx = reporting_fixture();
    let app = fx.app(View::Structure);

    let text = report::render(&app, None);

    assert!(text.contains("INHERITANCE STRUCTURE"), "got:\n{text}");
    assert!(text.contains("CALL TRAFFIC"), "got:\n{text}");
    assert!(text.contains("SITREP · structure ·"), "got:\n{text}");
    assert!(text.contains("SITREP · traffic ·"), "got:\n{text}");
    assert!(
        text.ends_with('\n'),
        "every line, including the last, is terminated"
    );
}

#[test]
fn report_restricted_to_structure_omits_the_traffic_view() {
    let fx = reporting_fixture();
    let app = fx.app(View::Network);

    let text = report::render(&app, Some(View::Structure));

    assert!(text.contains("INHERITANCE STRUCTURE"), "got:\n{text}");
    assert!(text.contains("SITREP · structure ·"), "got:\n{text}");
    assert!(
        !text.contains("CALL TRAFFIC"),
        "the filter, not app.view, decides what is rendered; got:\n{text}"
    );
    assert!(!text.contains("SITREP · traffic ·"), "got:\n{text}");
}

#[test]
fn report_restricted_to_network_omits_the_structure_view() {
    let fx = reporting_fixture();
    let app = fx.app(View::Structure);

    let text = report::render(&app, Some(View::Network));

    assert!(text.contains("CALL TRAFFIC"), "got:\n{text}");
    assert!(text.contains("SITREP · traffic ·"), "got:\n{text}");
    assert!(
        !text.contains("INHERITANCE STRUCTURE"),
        "the filter, not app.view, decides what is rendered; got:\n{text}"
    );
    assert!(!text.contains("SITREP · structure ·"), "got:\n{text}");
}

#[test]
fn report_header_names_the_target_and_both_snapshots() {
    let fx = reporting_fixture();
    let app = fx.app(View::Structure);

    let text = report::render(&app, Some(View::Structure));
    let mut lines = text.lines();

    assert_eq!(lines.next(), Some("WATCH FLOOR · target pkg (pkg/)"));
    let head = fx.target().head_id().expect("head after commit");
    let provenance = lines.next().expect("second line");
    assert!(
        provenance.starts_with(&format!(
            "  baseline HEAD @ {head} (2 files)   current working tree (2 files)   "
        )) && provenance.ends_with("ms"),
        "got: {provenance}"
    );
}

#[test]
fn report_structure_section_counts_and_lists_the_new_class() {
    let fx = reporting_fixture();
    let app = fx.app(View::Structure);

    let text = report::render(&app, Some(View::Structure));

    assert!(
        text.contains("INHERITANCE STRUCTURE  + 1 ACTIVATED  - 0 BURNED  ~ 0 REALIGNED  → 0 RELOCATED  * 0 AMENDED  · 2 classes"),
        "got:\n{text}"
    );
    assert!(
        text.lines().any(|l| l.contains("+ Secure")
            && l.contains("pkg.core:5")
            && l.ends_with("ACTIVATED")),
        "the activated class is drawn under Channel with its location; got:\n{text}"
    );
    assert!(
        text.contains(" + ACTIVATED Secure  (pkg.core)"),
        "the sitrep feed reports it too; got:\n{text}"
    );
    assert!(
        text.contains("            inherits Channel"),
        "got:\n{text}"
    );
}

#[test]
fn report_traffic_section_reports_the_new_call() {
    let fx = reporting_fixture();
    let app = fx.app(View::Structure);

    let text = report::render(&app, Some(View::Network));

    assert!(
        text.lines().any(|l| l.starts_with("CALL TRAFFIC  ")
            && l.contains("+ 2 ACTIVATED")
            && l.contains("~ 1 REROUTED")
            && l.contains("· 4 operations · 1 links (0 inferred) ·")),
        "got:\n{text}"
    );
    assert!(
        text.contains(" ~ REROUTED  transmit()  (pkg.core)"),
        "transmit gained a call, so it is rerouted; got:\n{text}"
    );
    assert!(text.contains("            +encode"), "got:\n{text}");
}
