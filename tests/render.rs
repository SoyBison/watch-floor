//! Rendering: `floor::draw` against ratatui's TestBackend, and the field
//! manual's own layout rules. No real terminal is involved.

mod common;

use common::Fixture;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier};
use ratatui::text::Line;
use ratatui::Terminal;
use watch_floor::app::{App, View};
use watch_floor::floor;
use watch_floor::manual;
use watch_floor::sitrep::{Edge, Status};

// ---------------------------------------------------------------- fixtures

/// The baseline package: Agent.brief calls Channel.send.
const BEFORE: &str = "\
class Channel:
    def open(self): ...
    def send(self, payload):
        self.open()

class Agent(Channel):
    def brief(self):
        self.send(\"orders\")

def run():
    a = Agent()
    a.brief()
";

/// The working tree: Channel gains close(), brief reroutes onto it, and a new
/// entry point `sweep` appears above `run`.
const AFTER: &str = "\
class Channel:
    def open(self): ...
    def close(self): ...
    def send(self, payload):
        self.open()

class Agent(Channel):
    def brief(self):
        self.close()

def run():
    a = Agent()
    a.brief()

def sweep():
    run()
";

/// A committed package whose working tree has drifted, plus an App on it.
fn watched(view: View) -> (Fixture, App) {
    let fx = Fixture::new();
    fx.write("pkg/__init__.py", "");
    fx.write("pkg/core.py", BEFORE);
    fx.commit();
    fx.write("pkg/core.py", AFTER);
    let app = fx.app(view);
    (fx, app)
}

// ------------------------------------------------------------ buffer tools

fn render(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal
        .draw(|frame| floor::draw(frame, app))
        .expect("draw onto the test backend");
    terminal
}

fn row(buffer: &Buffer, y: u16) -> String {
    let width = buffer.area().width;
    (0..width)
        .map(|x| buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(""))
        .collect()
}

fn screen(buffer: &Buffer) -> Vec<String> {
    (0..buffer.area().height).map(|y| row(buffer, y)).collect()
}

/// The screen as one string, so `contains` can span nothing but a single line
/// is still easy to spot with a needle that has no newline in it.
fn flat(buffer: &Buffer) -> String {
    screen(buffer).join("\n")
}

/// The index of the first row whose text contains `needle`.
fn row_with(buffer: &Buffer, needle: &str) -> u16 {
    (0..buffer.area().height)
        .find(|y| row(buffer, *y).contains(needle))
        .unwrap_or_else(|| panic!("no rendered row contains {needle:?}:\n{}", flat(buffer)))
}

/// The column at which `needle` starts on row `y`, matched cell by cell so
/// multi-byte glyphs cannot throw the index off.
fn col_of(buffer: &Buffer, y: u16, needle: &str) -> u16 {
    let wanted: Vec<String> = needle.chars().map(|c| c.to_string()).collect();
    'start: for x in 0..buffer.area().width {
        for (i, ch) in wanted.iter().enumerate() {
            match buffer.cell((x + i as u16, y)) {
                Some(cell) if cell.symbol() == ch => {}
                _ => continue 'start,
            }
        }
        return x;
    }
    panic!("row {y} does not contain {needle:?}: {:?}", row(buffer, y));
}

/// Exactly this line, ignoring the pane border and the padding to its right.
fn assert_has_line(buffer: &Buffer, expected: &str) {
    let found = screen(buffer).iter().any(|line| {
        // Drop the pane's left border, then everything from its right one on.
        let body = line.strip_prefix('│').unwrap_or(line);
        body.split('│').next().unwrap_or(body).trim_end() == expected
    });
    assert!(
        found,
        "expected a rendered line {expected:?}, got:\n{}",
        flat(buffer)
    );
}

fn width_of(line: &Line) -> usize {
    line.spans.iter().map(|s| s.content.chars().count()).sum()
}

fn text_of(line: &Line) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Every size the draw has to survive: roomy, narrow, and absurd.
const SIZES: [(u16, u16); 3] = [(120, 40), (40, 12), (8, 3)];

// ------------------------------------------------------------------- draw

#[test]
fn draw_survives_every_view_filter_and_manual_combination_at_every_size() {
    for view in [View::Structure, View::Network] {
        let (_fx, mut app) = watched(view);
        for (width, height) in SIZES {
            for changes_only in [false, true] {
                for help in [false, true] {
                    app.changes_only = changes_only;
                    app.help = help;
                    let terminal = render(&mut app, width, height);
                    let buffer = terminal.backend().buffer();
                    assert_eq!(
                        (buffer.area().width, buffer.area().height),
                        (width, height),
                        "the draw must fill exactly the backend it was given"
                    );
                    assert!(
                        buffer.content().iter().any(|cell| cell.symbol() != " "),
                        "{width}x{height} view={view:?} changes_only={changes_only} \
                         help={help} drew a blank screen"
                    );
                }
            }
        }
    }
}

#[test]
fn structure_view_names_the_target_package_and_every_class() {
    let (_fx, mut app) = watched(View::Structure);
    let terminal = render(&mut app, 120, 24);
    let buffer = terminal.backend().buffer();

    assert!(
        row(buffer, 1).contains("target pkg  (pkg/)"),
        "the header names the package under watch, got {:?}",
        row(buffer, 1)
    );
    assert!(
        row(buffer, 5).contains("INHERITANCE STRUCTURE"),
        "the tree pane is titled for the structure view, got {:?}",
        row(buffer, 5)
    );
    // Channel gained close(), Agent is untouched and hangs off Channel.
    assert_has_line(buffer, "object ⟨external⟩");
    assert_has_line(buffer, "  └─ * Channel  pkg.core:1  AMENDED");
    assert_has_line(buffer, "     └─ · Agent  pkg.core:7");
    // The SITREP explains the one change rather than just naming it.
    assert!(
        flat(buffer).contains("* AMENDED   Channel  pkg.core"),
        "the feed reports the amended class:\n{}",
        flat(buffer)
    );
}

#[test]
fn traffic_view_names_operations_and_tags_the_links_that_moved() {
    let (_fx, mut app) = watched(View::Network);
    let terminal = render(&mut app, 120, 24);
    let buffer = terminal.backend().buffer();

    assert!(
        row(buffer, 5).contains("CALL TRAFFIC · from entry points"),
        "the tree pane is titled for the traffic view, got {:?}",
        row(buffer, 5)
    );
    // sweep is the new entry point; brief swapped send for close.
    assert_has_line(buffer, "+ sweep  pkg.core:15  ACTIVATED");
    assert_has_line(buffer, "  └─ · run  pkg.core:11");
    assert_has_line(buffer, "     └─ ~ Agent.brief ?  pkg.core:8  REROUTED");
    assert_has_line(
        buffer,
        "        ├─ + Channel.close  pkg.core:3  NEW TRAFFIC  ACTIVATED",
    );
    assert_has_line(buffer, "        └─ · Channel.send  pkg.core:4  WENT DARK");
}

#[test]
fn a_changed_link_colours_the_row_even_when_the_operation_itself_is_nominal() {
    let (_fx, mut app) = watched(View::Network);
    let terminal = render(&mut app, 120, 24);
    let buffer = terminal.backend().buffer();

    let y = row_with(buffer, "+ sweep");
    let cell = buffer.cell((col_of(buffer, y, "+ sweep"), y)).unwrap();
    assert_eq!(cell.fg, Color::Green, "ACTIVATED is drawn green");
    assert!(
        cell.modifier.contains(Modifier::BOLD),
        "a changed node is bold"
    );

    // Channel.send is NOMINAL, but the link to it went dark: the edge wins.
    let y = row_with(buffer, "Channel.send");
    let glyph = buffer
        .cell((col_of(buffer, y, "· Channel.send"), y))
        .unwrap();
    assert_eq!(
        glyph.fg,
        Color::Red,
        "WENT DARK recolours the whole row red, not just the tag"
    );
    assert!(
        glyph.modifier.contains(Modifier::BOLD),
        "a changed link is bold even on an unchanged operation"
    );
    let tag = buffer.cell((col_of(buffer, y, "WENT DARK"), y)).unwrap();
    assert_eq!(tag.fg, Color::Red);
}

#[test]
fn changes_only_prunes_branches_that_lead_to_no_change() {
    let (_fx, mut app) = watched(View::Network);
    app.changes_only = true;
    let terminal = render(&mut app, 120, 24);
    let buffer = terminal.backend().buffer();
    let shown = flat(buffer);

    assert!(
        row(buffer, 5).contains("· changes only"),
        "the pane title says the tree is filtered, got {:?}",
        row(buffer, 5)
    );
    // Channel.open is only reachable under Channel.send, and nothing about it
    // moved, so the filter drops it while keeping the rerouted branch.
    assert!(
        !shown.contains("Channel.open"),
        "an untouched leaf survived the filter:\n{shown}"
    );
    assert!(shown.contains("Agent.brief"), "the rerouted branch stays");
    assert!(
        shown.contains("c changes-only [on]"),
        "the footer reports the filter state"
    );
}

#[test]
fn an_unchanged_package_says_so_instead_of_drawing_an_empty_pane() {
    let fx = Fixture::new();
    fx.write("pkg/__init__.py", "");
    fx.write("pkg/core.py", BEFORE);
    fx.commit();
    let mut app = fx.app(View::Structure);
    app.changes_only = true;
    let terminal = render(&mut app, 120, 24);
    let shown = flat(terminal.backend().buffer());

    assert!(
        shown.contains("no deviation from baseline"),
        "the tree pane explains why it is empty:\n{shown}"
    );
    assert!(
        shown.contains("baseline holds — no change detected"),
        "the feed pane explains why it is empty:\n{shown}"
    );
    assert!(
        shown.contains("SITREP · 0 items"),
        "the feed title counts zero items:\n{shown}"
    );
}

#[test]
fn opening_the_manual_replaces_both_panes_and_the_footer() {
    let (_fx, mut app) = watched(View::Structure);
    let closed = flat(render(&mut app, 120, 24).backend().buffer());
    assert!(closed.contains("SITREP"), "the feed is up with help closed");
    assert!(
        !closed.contains("FIELD MANUAL"),
        "the manual is not drawn until it is asked for"
    );

    app.help = true;
    let open = flat(render(&mut app, 120, 24).backend().buffer());
    assert!(
        open.contains("FIELD MANUAL · reading the floor · "),
        "the manual heading appears:\n{open}"
    );
    assert!(
        open.contains("Two snapshots of one Python package, compared, live."),
        "the manual body is drawn from page one:\n{open}"
    );
    assert!(
        !open.contains("SITREP"),
        "the manual takes over the whole body:\n{open}"
    );
    assert!(
        open.contains("? or h close the manual"),
        "the footer switches to the manual's own keys:\n{open}"
    );
    assert!(
        open.contains("target pkg"),
        "the header stays put behind the manual:\n{open}"
    );
    assert_ne!(closed, open, "toggling help must change the screen");
}

#[test]
fn drawing_clamps_a_scroll_that_ran_past_the_end() {
    let (_fx, mut app) = watched(View::Structure);

    // Three rows, plenty of room: the only legal offset is the top.
    app.tree_scroll = usize::MAX;
    let terminal = render(&mut app, 120, 24);
    assert_eq!(
        app.tree_scroll, 0,
        "a tree shorter than the pane cannot be scrolled at all"
    );
    assert_has_line(terminal.backend().buffer(), "object ⟨external⟩");

    // Two visible lines for three rows: the last row lands at the bottom.
    app.tree_scroll = usize::MAX;
    let terminal = render(&mut app, 120, 10);
    assert_eq!(
        app.tree_scroll, 1,
        "the clamp stops with the final row on screen, never past it"
    );
    let buffer = terminal.backend().buffer();
    assert_has_line(buffer, "  └─ * Channel  pkg.core:1  AMENDED");
    assert_has_line(buffer, "     └─ · Agent  pkg.core:7");
    assert!(
        !flat(buffer).contains("⟨external⟩"),
        "the scrolled-past root is gone:\n{}",
        flat(buffer)
    );
}

// ----------------------------------------------------------------- manual

/// The lines `manual::rule` draws; they are the only width-aware ones.
fn rules(width: u16) -> Vec<String> {
    manual::pages(width)
        .iter()
        .map(text_of)
        .filter(|line| line.starts_with("  ── "))
        .collect()
}

#[test]
fn manual_section_rules_stretch_to_the_requested_width() {
    for width in [100u16, 120, 160, 200] {
        let rules = rules(width);
        assert_eq!(
            rules.len(),
            8,
            "every section of the manual is introduced by one rule"
        );
        for rule in &rules {
            assert_eq!(
                rule.chars().count(),
                width as usize - 1,
                "a rule at width {width} fills the line but for the pane's last \
                 column, got {rule:?}"
            );
            assert!(rule.ends_with('─'), "the rule runs out in dashes: {rule:?}");
        }
    }
    assert!(
        rules(100)[0].contains("STATUS · what changed about a thing itself"),
        "the first rule labels the status legend"
    );
}

#[test]
fn manual_lines_fit_inside_the_requested_width() {
    for width in [100u16, 110, 120, 160, 200] {
        for line in manual::pages(width) {
            assert!(
                width_of(&line) <= width as usize,
                "a manual line is {} columns wide at width {width}, so it would be \
                 clipped: {:?}",
                width_of(&line),
                text_of(&line)
            );
        }
    }
}

#[test]
fn manual_section_rules_fit_inside_a_narrow_width() {
    // 40 is the floor manual::pages clamps to, so it is a width the manual
    // claims to support; two of the labels are longer than that on their own.
    for rule in rules(40) {
        assert!(
            rule.chars().count() <= 40,
            "a rule ruled to 40 columns came out {} wide: {rule:?}",
            rule.chars().count()
        );
    }
}

#[test]
fn manual_clamps_the_width_it_rules_to() {
    assert_eq!(
        rules(4),
        rules(40),
        "anything below 40 columns is ruled as 40"
    );
    assert_eq!(
        rules(1000),
        rules(200),
        "anything above 200 columns is ruled as 200"
    );
    assert_ne!(
        rules(40),
        rules(120),
        "between the clamps the width is used"
    );
}

#[test]
fn manual_legend_carries_every_change_status_the_views_can_report() {
    let text = manual::plain(100);
    let statuses: Vec<Status> = Status::STRUCTURE
        .iter()
        .chain(Status::NETWORK.iter())
        .copied()
        .collect();
    assert_eq!(
        statuses.len(),
        Status::STRUCTURE.len() + Status::NETWORK.len(),
        "every status a view can report is checked, however many that is"
    );

    for status in statuses {
        assert!(
            status.is_change(),
            "{status:?} sits in a view legend, so it must count as a change"
        );
        let entry = format!("  {}  {:<11}", status.glyph(), status.tag());
        assert!(
            text.contains(&entry),
            "the manual's legend is missing {:?} — it must be built from the enum \
             so it cannot drift from the code. Looked for {entry:?} in:\n{text}",
            status.tag()
        );
    }
    // NOMINAL is not a change, so no view legend lists it, but the manual still
    // has to explain the dot the tree draws.
    assert!(!Status::Nominal.is_change());
    assert!(
        text.contains(&format!("  {}  {:<11}", Status::Nominal.glyph(), "NOMINAL")),
        "the manual still documents the unchanged marker:\n{text}"
    );
}

#[test]
fn manual_documents_both_traffic_tags() {
    let text = manual::plain(100);
    for edge in [Edge::Live, Edge::Opened, Edge::Closed] {
        if !edge.is_change() {
            assert_eq!(edge.tag(), "", "a live link is drawn with no tag at all");
            continue;
        }
        // Without this, an empty tag would make the `contains` below vacuous.
        assert!(
            !edge.tag().is_empty(),
            "a changed link must carry a tag to document ({edge:?})"
        );
        assert!(
            text.contains(edge.tag()),
            "the manual never mentions the {:?} tag {:?}:\n{text}",
            edge,
            edge.tag()
        );
    }
    assert!(text.contains("NEW TRAFFIC"));
    assert!(text.contains("WENT DARK"));
}

#[test]
fn manual_documents_every_marker_the_floor_draws() {
    let text = manual::plain(100);
    // Exactly the markers floor.rs can put on a row.
    for (marker, meaning) in [
        ("+Log", "a base beyond the one the class hangs off"),
        (
            "⟨external⟩",
            "a name that resolves nowhere inside the package",
        ),
        ("?", "an inferred link"),
        ("⋯", "already expanded further up"),
        ("↻", "the operation calls itself"),
        ("<cycle>", "holds nodes reachable only through a cycle"),
    ] {
        let row = format!("     {marker:<13}");
        let documented = text
            .lines()
            .any(|line| line.starts_with(&row) && line.contains(meaning));
        assert!(
            documented,
            "the marker {marker:?} is drawn on the floor but not explained in the \
             manual's MARKERS table:\n{text}"
        );
    }
}

// ------------------------------------------------------------------- keys

fn press(app: &mut App, code: KeyCode) -> bool {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE))
}

#[test]
fn q_quits() {
    let (_fx, mut app) = watched(View::Structure);
    assert!(press(&mut app, KeyCode::Char('q')), "q asks to leave");
    assert!(
        !press(&mut app, KeyCode::Char('x')),
        "an unbound key does nothing at all"
    );
}

#[test]
fn ctrl_c_quits_rather_than_toggling_the_changes_filter() {
    let (_fx, mut app) = watched(View::Structure);
    let quit = app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(quit, "ctrl-c asks to leave");
    assert!(
        !app.changes_only,
        "ctrl-c must not also flip the changes-only filter"
    );
}

#[test]
fn question_mark_toggles_the_manual_and_rewinds_it() {
    let (_fx, mut app) = watched(View::Structure);
    assert!(!app.help, "the manual starts closed");

    assert!(
        !press(&mut app, KeyCode::Char('?')),
        "opening is not quitting"
    );
    assert!(app.help, "? opens the manual");

    app.help_scroll = 12;
    assert!(!press(&mut app, KeyCode::Char('?')));
    assert!(!app.help, "? closes it again");
    assert_eq!(app.help_scroll, 0, "the manual reopens at the top");

    assert!(!press(&mut app, KeyCode::Char('h')));
    assert!(app.help, "h is the second binding for the manual");
}

#[test]
fn esc_closes_the_manual_when_it_is_open_and_quits_when_it_is_not() {
    let (_fx, mut app) = watched(View::Structure);
    app.help = true;
    assert!(
        !press(&mut app, KeyCode::Esc),
        "esc with the manual open is a close, not a quit"
    );
    assert!(!app.help);
    assert!(
        press(&mut app, KeyCode::Esc),
        "esc with nothing open asks to leave"
    );
}

#[test]
fn digit_keys_switch_view_and_rewind_both_panes() {
    let (_fx, mut app) = watched(View::Structure);
    app.tree_scroll = 7;
    app.feed_scroll = 3;

    assert!(!press(&mut app, KeyCode::Char('2')));
    assert_eq!(app.view, View::Network, "2 is the traffic view");
    assert_eq!(app.tree_scroll, 0, "a new view starts at the top");
    assert_eq!(app.feed_scroll, 0, "and so does its feed");

    app.tree_scroll = 5;
    assert!(!press(&mut app, KeyCode::Char('2')));
    assert_eq!(
        app.tree_scroll, 5,
        "asking for the view already on screen leaves the position alone"
    );

    assert!(!press(&mut app, KeyCode::Char('1')));
    assert_eq!(app.view, View::Structure, "1 is the structure view");
    assert_eq!(app.tree_scroll, 0);

    assert!(!press(&mut app, KeyCode::Char('v')));
    assert_eq!(app.view, View::Network, "v cycles to the other view");
}

#[test]
fn c_toggles_changes_only_and_rewinds_the_tree() {
    let (_fx, mut app) = watched(View::Structure);
    assert!(!app.changes_only, "the filter starts off");
    app.tree_scroll = 4;

    assert!(!press(&mut app, KeyCode::Char('c')));
    assert!(app.changes_only, "c turns the filter on");
    assert_eq!(
        app.tree_scroll, 0,
        "a pruned tree is shorter, so the position is rewound"
    );

    assert!(!press(&mut app, KeyCode::Char('c')));
    assert!(!app.changes_only, "c turns it back off");
}
