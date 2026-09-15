//! The watch floor itself: everything the operator sees.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{App, Pane, View};
use crate::manual;
use crate::network;
use crate::sitrep::{leaf, Counts, Edge, NodeKey, Row, Status};
use crate::structure;

const DIM: Color = Color::DarkGray;
const EXTERNAL: Color = Color::Blue;
const CHROME: Color = Color::Rgb(90, 100, 110);
const WAKE: Color = Color::Rgb(112, 130, 152);

pub fn color(status: Status) -> Color {
    match status {
        Status::Activated => Color::Green,
        Status::Burned => Color::Red,
        Status::Realigned | Status::Rerouted => Color::Yellow,
        Status::Relocated => Color::Magenta,
        Status::Amended => Color::Cyan,
        // Softer than any real change: present, but never the loudest thing.
        Status::Wake => WAKE,
        Status::Nominal => Color::Gray,
    }
}

fn edge_color(edge: Edge) -> Color {
    match edge {
        Edge::Live => Color::Gray,
        Edge::Opened => Color::Green,
        Edge::Closed => Color::Red,
    }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    header(frame, chunks[0], app);

    if app.help {
        pane_manual(frame, chunks[1], app);
        footer(frame, chunks[2], app);
        return;
    }

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(40), Constraint::Length(48)])
        .split(chunks[1]);

    let (tree, feed, items) = match app.view {
        View::Structure => {
            let feed = app.structure.feed();
            let lines = feed.iter().flat_map(|e| structure_report(e)).collect();
            (structure_rows(app), lines, feed.len())
        }
        View::Network => {
            let feed = app.network.feed();
            let lines = feed.iter().flat_map(|e| network_report(e)).collect();
            (network_rows(app), lines, feed.len())
        }
    };

    pane_tree(frame, body[0], app, tree);
    pane_feed(frame, body[1], app, feed, items);
    footer(frame, chunks[2], app);
}

fn header(frame: &mut Frame, area: Rect, app: &App) {
    let (counts, legend): (Counts, &[Status]) = match app.view {
        View::Structure => (app.structure.counts, &Status::STRUCTURE),
        View::Network => (app.network.counts, &Status::NETWORK),
    };
    let age = app.last_sync.elapsed().as_secs();

    let mut chips = vec![Span::raw("  ")];
    for status in legend {
        let n = counts.of(*status);
        let style = if n > 0 {
            Style::default()
                .fg(color(*status))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(DIM)
        };
        chips.push(Span::styled(
            format!("{} {n} {}   ", status.glyph(), status.tag()),
            style,
        ));
    }
    chips.push(Span::styled(
        match app.view {
            View::Structure => format!("· {} classes", counts.total()),
            View::Network => format!(
                "· {} operations · {} links ({} inferred) · {} calls leave the package",
                counts.total(),
                app.network.links,
                app.network.probable,
                app.network.external
            ),
        },
        Style::default().fg(DIM),
    ));

    let tab = |which: View, label: &str, key: &str| {
        Span::styled(
            format!(" {key} {label} "),
            if app.view == which {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(DIM)
            },
        )
    };

    let lines = vec![
        Line::from(vec![
            Span::styled(
                "WATCH FLOOR",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  target ", Style::default().fg(DIM)),
            Span::styled(
                app.target.pkg_name.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  ({}/)   ", app.target.pkg_rel),
                Style::default().fg(DIM),
            ),
            tab(View::Structure, "STRUCTURE", "1"),
            Span::raw(" "),
            tab(View::Network, "TRAFFIC", "2"),
        ]),
        Line::from(vec![
            Span::styled("  baseline ", Style::default().fg(DIM)),
            Span::styled(
                app.baseline.label.clone(),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(
                format!(" ({} files)", app.baseline.files),
                Style::default().fg(DIM),
            ),
            Span::styled("   current ", Style::default().fg(DIM)),
            Span::styled(app.current.label.clone(), Style::default().fg(Color::Green)),
            Span::styled(
                format!(" ({} files)", app.current.files),
                Style::default().fg(DIM),
            ),
            Span::styled(
                format!("   resynced {age}s ago in {}ms", app.scan_ms),
                Style::default().fg(DIM),
            ),
        ]),
        Line::from(chips),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CHROME));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// One line per node of the inheritance forest.
fn structure_rows(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for row in app.structure.rows(app.changes_only) {
        let mut spans = chrome(&row);
        match &row.node.key {
            NodeKey::External(name) => spans.extend(external(name)),
            NodeKey::Subject(qualname) => {
                let Some(entry) = app.structure.entries.get(qualname) else {
                    continue;
                };
                let hue = color(entry.status);
                let style = emphasis(hue, entry.status.is_change());
                spans.push(Span::styled(format!("{} ", entry.status.glyph()), style));
                spans.push(Span::styled(row.node.label.clone(), style));
                if !entry.mixins.is_empty() {
                    spans.push(Span::styled(
                        format!(" +{}", entry.mixins.join(" +")),
                        Style::default().fg(EXTERNAL),
                    ));
                }
                spans.push(Span::styled(
                    format!("  {}", entry.subject.location()),
                    Style::default().fg(DIM),
                ));
                if entry.status.is_change() {
                    spans.push(tag(entry.status.tag(), hue));
                }
            }
            NodeKey::Operation(_) => continue,
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// One line per node of the call graph.
fn network_rows(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for row in app.network.rows(app.changes_only) {
        let mut spans = chrome(&row);
        match &row.node.key {
            NodeKey::External(name) => spans.extend(external(name)),
            NodeKey::Operation(callsign) => {
                let Some(entry) = app.network.entries.get(callsign) else {
                    continue;
                };
                // A changed link is the headline; otherwise the node speaks.
                let hue = if row.node.edge.is_change() {
                    edge_color(row.node.edge)
                } else {
                    color(entry.status)
                };
                let style = emphasis(hue, entry.status.is_change() || row.node.edge.is_change());
                spans.push(Span::styled(format!("{} ", entry.status.glyph()), style));
                spans.push(Span::styled(row.node.label.clone(), style));
                if entry.operation.recursive {
                    spans.push(Span::styled(" ↻", Style::default().fg(EXTERNAL)));
                }
                if let Some(note) = &row.node.note {
                    spans.push(Span::styled(format!(" {note}"), Style::default().fg(DIM)));
                }
                if row.node.repeat {
                    spans.push(Span::styled(" ⋯", Style::default().fg(DIM)));
                } else {
                    spans.push(Span::styled(
                        format!("  {}", entry.operation.location()),
                        Style::default().fg(DIM),
                    ));
                }
                if row.node.edge.is_change() {
                    spans.push(tag(row.node.edge.tag(), hue));
                }
                if entry.status.is_change() {
                    spans.push(tag(entry.status.tag(), color(entry.status)));
                }
            }
            NodeKey::Subject(_) => continue,
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn chrome(row: &Row) -> Vec<Span<'static>> {
    vec![Span::styled(
        row.prefix.clone(),
        Style::default().fg(CHROME),
    )]
}

fn external(name: &str) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            name.to_string(),
            Style::default().fg(EXTERNAL).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ⟨external⟩", Style::default().fg(DIM)),
    ]
}

fn emphasis(hue: Color, bold: bool) -> Style {
    let style = Style::default().fg(hue);
    if bold {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

fn tag(text: &str, hue: Color) -> Span<'static> {
    Span::styled(
        format!("  {text}"),
        Style::default().fg(hue).add_modifier(Modifier::DIM),
    )
}

fn pane_tree(frame: &mut Frame, area: Rect, app: &mut App, mut lines: Vec<Line<'static>>) {
    let height = area.height.saturating_sub(2) as usize;
    app.tree_scroll = app
        .tree_scroll
        .min(lines.len().saturating_sub(height.max(1)));

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            if app.changes_only {
                "  no deviation from baseline"
            } else {
                "  nothing to show"
            },
            Style::default().fg(DIM),
        )));
    }
    let view: Vec<Line> = lines
        .into_iter()
        .skip(app.tree_scroll)
        .take(height)
        .collect();

    let title = format!(
        " {} {}",
        match app.view {
            View::Structure => "INHERITANCE STRUCTURE",
            View::Network => "CALL TRAFFIC · from entry points",
        },
        if app.changes_only {
            "· changes only "
        } else {
            ""
        }
    );
    frame.render_widget(
        Paragraph::new(view).block(pane(title, app.focus == Pane::Tree)),
        area,
    );
}

fn pane_feed(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    mut lines: Vec<Line<'static>>,
    items: usize,
) {
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            " baseline holds — no change detected",
            Style::default().fg(DIM),
        )));
    }

    let height = area.height.saturating_sub(2) as usize;
    app.feed_scroll = app
        .feed_scroll
        .min(lines.len().saturating_sub(height.max(1)));
    let view: Vec<Line> = lines
        .into_iter()
        .skip(app.feed_scroll)
        .take(height)
        .collect();

    let title = format!(" SITREP · {items} items ");
    frame.render_widget(
        Paragraph::new(view).block(pane(title, app.focus == Pane::Feed)),
        area,
    );
}

fn pane_manual(frame: &mut Frame, area: Rect, app: &mut App) {
    let lines = manual::pages(area.width.saturating_sub(2));
    let total = lines.len();
    let height = area.height.saturating_sub(2) as usize;
    app.help_scroll = app.help_scroll.min(total.saturating_sub(height.max(1)));
    let shown = (app.help_scroll + height).min(total);
    let view: Vec<Line> = lines
        .into_iter()
        .skip(app.help_scroll)
        .take(height)
        .collect();

    let title = format!(" FIELD MANUAL · reading the floor · {shown}/{total} ");
    frame.render_widget(Paragraph::new(view).block(pane(title, true)), area);
}

fn headline(status: Status, name: String, module: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!(" {} {:<9} ", status.glyph(), status.tag()),
            Style::default()
                .fg(color(status))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(name, Style::default().fg(Color::White)),
        Span::styled(format!("  {module}"), Style::default().fg(DIM)),
    ])
}

fn detail(text: String) -> Line<'static> {
    Line::from(Span::styled(
        format!("            {text}"),
        Style::default().fg(Color::Gray),
    ))
}

fn gained_lost(gained: &[String], lost: &[String]) -> Option<Line<'static>> {
    if gained.is_empty() && lost.is_empty() {
        return None;
    }
    let mut spans = vec![Span::raw("            ")];
    for item in gained {
        spans.push(Span::styled(
            format!("+{item} "),
            Style::default().fg(Color::Green),
        ));
    }
    for item in lost {
        spans.push(Span::styled(
            format!("-{item} "),
            Style::default().fg(Color::Red),
        ));
    }
    Some(Line::from(spans))
}

fn bases(list: &[String]) -> String {
    if list.is_empty() {
        "object".to_string()
    } else {
        list.join(", ")
    }
}

/// The change log entry for one class.
fn structure_report(entry: &structure::Entry) -> Vec<Line<'static>> {
    let mut lines = vec![headline(
        entry.status,
        entry.subject.display_name(),
        entry.subject.module.clone(),
    )];
    match entry.status {
        Status::Activated => {
            lines.push(detail(format!("inherits {}", bases(&entry.subject.bases))))
        }
        Status::Burned => lines.push(detail(format!(
            "held {} at {}",
            bases(&entry.subject.bases),
            entry.subject.location()
        ))),
        Status::Realigned => {
            lines.push(detail(format!("was  {}", bases(&entry.prior_bases))));
            lines.push(detail(format!("now  {}", bases(&entry.subject.bases))));
        }
        Status::Relocated => lines.push(detail(format!(
            "moved {} → {}",
            entry.prior_module.clone().unwrap_or_default(),
            entry.subject.module
        ))),
        _ => {}
    }
    lines.extend(gained_lost(&entry.gained, &entry.lost));
    lines
}

/// The change log entry for one operation.
fn network_report(entry: &network::Entry) -> Vec<Line<'static>> {
    let op = &entry.operation;
    let mut lines = vec![headline(
        entry.status,
        format!("{}{}", op.display_name(), op.signature()),
        op.module.clone(),
    )];
    match entry.status {
        Status::Activated => {
            let calls: Vec<String> = op
                .links
                .iter()
                .map(|l| leaf(&l.target).to_string())
                .collect();
            lines.push(detail(if calls.is_empty() {
                format!("calls nothing in-package, {} out", op.external)
            } else {
                format!("calls {}", calls.join(", "))
            }));
        }
        Status::Burned => lines.push(detail(format!("was at {}", op.source()))),
        Status::Wake => lines.extend(entry.moved.iter().map(|m| detail(m.clone()))),
        Status::Amended => lines.push(detail(format!(
            "({}) → {}",
            entry.prior_params.join(", "),
            op.signature()
        ))),
        Status::Relocated => lines.push(detail(format!(
            "moved {} → {}",
            entry.prior_module.clone().unwrap_or_default(),
            op.module
        ))),
        _ => {}
    }
    lines.extend(gained_lost(&entry.gained, &entry.lost));
    if let Some(base) = &entry.overrides {
        lines.push(detail(format!("overrides {base}")));
    }
    lines
}

fn footer(frame: &mut Frame, area: Rect, app: &App) {
    let line = if let Some(error) = &app.error {
        Line::from(Span::styled(
            format!(" ! {error}"),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ))
    } else if app.help {
        Line::from(Span::styled(
            " ↑↓/jk scroll · g/G top/bottom · ? or h close the manual · q quit",
            Style::default().fg(DIM),
        ))
    } else {
        Line::from(Span::styled(
            format!(
                " q quit · r resync · 1/2 view · c changes-only [{}] · tab pane · ↑↓/jk scroll · ? help",
                if app.changes_only { "on" } else { "off" }
            ),
            Style::default().fg(DIM),
        ))
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn pane(title: String, focused: bool) -> Block<'static> {
    let border = if focused { Color::White } else { CHROME };
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
        .title(Span::styled(
            title,
            Style::default().fg(if focused { Color::White } else { Color::Gray }),
        ))
}
