//! Plain-text rendering of the same picture the floor draws, for `--once`.
//! Returns lines rather than printing them, so it can be asserted against.

use crate::app::{App, View};
use crate::sitrep::{leaf, NodeKey, Status};

/// The full report. `only` restricts it to a single view.
pub fn render(app: &App, only: Option<View>) -> String {
    let mut out = vec![
        format!(
            "WATCH FLOOR · target {} ({}/)",
            app.target.pkg_name, app.target.pkg_rel
        ),
        format!(
            "  baseline {} ({} files)   current {} ({} files)   {}ms",
            app.baseline.label,
            app.baseline.files,
            app.current.label,
            app.current.files,
            app.scan_ms
        ),
    ];

    if only != Some(View::Network) {
        out.extend(structure_lines(app));
    }
    if only != Some(View::Structure) {
        out.extend(network_lines(app));
    }

    out.iter()
        .map(|line| format!("{line}\n"))
        .collect::<String>()
}

fn structure_lines(app: &App) -> Vec<String> {
    let counts = app.structure.counts;
    let mut head = String::from("INHERITANCE STRUCTURE  ");
    for status in Status::STRUCTURE {
        head.push_str(&format!(
            "{} {} {}  ",
            status.glyph(),
            counts.of(status),
            status.tag()
        ));
    }
    head.push_str(&format!("· {} classes", counts.total()));

    let mut out = vec![String::new(), head];

    for row in app.structure.rows(app.changes_only) {
        match &row.node.key {
            NodeKey::External(name) => out.push(format!("{}{name} <external>", row.prefix)),
            NodeKey::Subject(qualname) => {
                let Some(entry) = app.structure.entries.get(qualname) else {
                    continue;
                };
                let mixins = if entry.mixins.is_empty() {
                    String::new()
                } else {
                    format!(" +{}", entry.mixins.join(" +"))
                };
                out.push(format!(
                    "{}{} {}{mixins}  {}{}",
                    row.prefix,
                    entry.status.glyph(),
                    row.node.label,
                    entry.subject.location(),
                    tag(entry.status)
                ));
            }
            NodeKey::Operation(_) => {}
        }
    }

    let feed = app.structure.feed();
    out.push(String::new());
    out.push(format!("SITREP · structure · {} items", feed.len()));
    for entry in feed {
        out.push(format!(
            " {} {:<9} {}  ({})",
            entry.status.glyph(),
            entry.status.tag(),
            entry.subject.display_name(),
            entry.subject.module
        ));
        match entry.status {
            Status::Realigned => {
                out.push(format!("            was  {}", bases(&entry.prior_bases)));
                out.push(format!("            now  {}", bases(&entry.subject.bases)));
            }
            Status::Relocated => out.push(format!(
                "            moved {} → {}",
                entry.prior_module.clone().unwrap_or_default(),
                entry.subject.module
            )),
            Status::Activated => out.push(format!(
                "            inherits {}",
                bases(&entry.subject.bases)
            )),
            Status::Burned => out.push(format!(
                "            held {} at {}",
                bases(&entry.subject.bases),
                entry.subject.location()
            )),
            _ => {}
        }
        out.extend(delta_line(&entry.gained, &entry.lost));
    }
    out
}

fn network_lines(app: &App) -> Vec<String> {
    let counts = app.network.counts;
    let mut head = String::from("CALL TRAFFIC  ");
    for status in Status::NETWORK {
        head.push_str(&format!(
            "{} {} {}  ",
            status.glyph(),
            counts.of(status),
            status.tag()
        ));
    }
    head.push_str(&format!(
        "· {} operations · {} links ({} inferred) · {} calls leave the package",
        counts.total(),
        app.network.links,
        app.network.probable,
        app.network.external
    ));

    let mut out = vec![String::new(), head];

    for row in app.network.rows(app.changes_only) {
        match &row.node.key {
            NodeKey::External(name) => out.push(format!("{}{name}", row.prefix)),
            NodeKey::Operation(callsign) => {
                let Some(entry) = app.network.entries.get(callsign) else {
                    continue;
                };
                let note = row.node.note.clone().unwrap_or_default();
                let place = if row.node.repeat {
                    "⋯".to_string()
                } else {
                    entry.operation.location()
                };
                let edge = if row.node.edge.is_change() {
                    format!("  {}", row.node.edge.tag())
                } else {
                    String::new()
                };
                out.push(format!(
                    "{}{} {}{}{note}  {place}{edge}{}",
                    row.prefix,
                    entry.status.glyph(),
                    row.node.label,
                    if entry.operation.recursive {
                        " ↻"
                    } else {
                        ""
                    },
                    tag(entry.status)
                ));
            }
            NodeKey::Subject(_) => {}
        }
    }

    let feed = app.network.feed();
    out.push(String::new());
    out.push(format!("SITREP · traffic · {} items", feed.len()));
    for entry in feed {
        let op = &entry.operation;
        out.push(format!(
            " {} {:<9} {}{}  ({})",
            entry.status.glyph(),
            entry.status.tag(),
            op.display_name(),
            op.signature(),
            op.module
        ));
        match entry.status {
            Status::Activated => {
                let calls: Vec<&str> = op.links.iter().map(|l| leaf(&l.target)).collect();
                out.push(format!(
                    "            calls {}",
                    if calls.is_empty() {
                        format!("nothing in-package, {} out", op.external)
                    } else {
                        calls.join(", ")
                    }
                ));
            }
            Status::Burned => out.push(format!("            was at {}", op.source())),
            Status::Wake => out.extend(entry.moved.iter().map(|m| format!("            {m}"))),
            Status::Amended => out.push(format!(
                "            ({}) → {}",
                entry.prior_params.join(", "),
                op.signature()
            )),
            Status::Relocated => out.push(format!(
                "            moved {} → {}",
                entry.prior_module.clone().unwrap_or_default(),
                op.module
            )),
            _ => {}
        }
        out.extend(delta_line(&entry.gained, &entry.lost));
        if let Some(base) = &entry.overrides {
            out.push(format!("            overrides {base}"));
        }
    }
    out
}

fn delta_line(gained: &[String], lost: &[String]) -> Option<String> {
    if gained.is_empty() && lost.is_empty() {
        return None;
    }
    let marks: Vec<String> = gained
        .iter()
        .map(|m| format!("+{m}"))
        .chain(lost.iter().map(|m| format!("-{m}")))
        .collect();
    Some(format!("            {}", marks.join(" ")))
}

fn tag(status: Status) -> String {
    if status.is_change() {
        format!("  {}", status.tag())
    } else {
        String::new()
    }
}

fn bases(list: &[String]) -> String {
    if list.is_empty() {
        "object".to_string()
    } else {
        list.join(", ")
    }
}
