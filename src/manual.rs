//! The field manual: every piece of jargon on the floor, traced back to the
//! syntax tree it was read off. Rendered into the body area when help is open.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::floor::color;
use crate::sitrep::Status;

const DIM: Color = Color::DarkGray;
const CHROME: Color = Color::Rgb(90, 100, 110);
const PROSE: Color = Color::Gray;
const ART: Color = Color::Rgb(150, 160, 172);

/// The manual as plain text, for piping somewhere else.
pub fn plain(width: u16) -> String {
    pages(width)
        .iter()
        .map(|line| {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            format!("{}\n", text.trim_end())
        })
        .collect()
}

/// Every line of the manual, in order, ruled to fit `width` columns.
pub fn pages(width: u16) -> Vec<Line<'static>> {
    let width = width.clamp(40, 200) as usize;
    let mut out = Vec::new();

    out.extend(prose(OVERVIEW));

    out.extend(rule(width, "STATUS · what changed about a thing itself"));
    out.extend(status_legend());
    out.extend(prose(
        "  Relocation is detected, not inferred twice over: a burned and an activated
  thing with the same name and the same connections are merged into one move.",
    ));

    out.extend(rule(width, "TRAFFIC · what changed about a link between two things"));
    out.extend(edge_legend());
    out.extend(prose(
        "  Only drawn when the caller exists on both sides. Every call out of a brand
  new function is new by definition, and flagging all of them tells you nothing.",
    ));
    out.extend(art(EDGE_EXAMPLE));

    out.extend(rule(width, "VIEW 1 · INHERITANCE STRUCTURE"));
    out.extend(art(CLASS_AST));
    out.extend(prose(
        "  Bases are normalized on the way in: Generic[T] is recorded as Generic, and
  keyword arguments such as metaclass= are dropped. Nested classes keep their
  enclosing class in the name, so Kit inside FieldAgent reads FieldAgent.Kit.",
    ));
    out.extend(art(CLASS_FOREST));

    out.extend(rule(width, "VIEW 2 · CALL TRAFFIC"));
    out.extend(art(CALL_AST));
    out.extend(prose(
        "  Nested def and class bodies are skipped — their calls belong to them, not to
  the function they sit inside.",
    ));
    out.extend(art(RESOLUTION));
    out.extend(art(CALL_FOREST));

    out.extend(rule(width, "THE TRANSLATION, EXACTLY"));
    out.extend(art(NODE_TABLE));

    out.extend(rule(width, "MARKERS"));
    out.extend(markers());

    out.extend(rule(width, "BLIND SPOTS"));
    out.extend(prose(
        "
  Imports are not followed. Resolution is structural, so a name that is
  ambiguous across the package resolves to nothing rather than to a guess.

  Module-level code is not attributed to any operation, and its calls are not
  recorded. Work done at import time is invisible here.

  A method reached only through its base class has no in-package caller, so it
  appears as its own entry point. The SITREP notes `overrides Base.m` when it
  applies, but the tree does not reroute virtual dispatch.",
    ));

    out.extend(rule(width, "KEYS"));
    out.extend(art(KEYS));

    out
}

const OVERVIEW: &str = "
  Two snapshots of one Python package, compared, live.

      baseline   the package as committed at git HEAD    git show HEAD:<path>
      current    the package as it sits on disk right now

  Every .py file on both sides is parsed with tree-sitter. Nothing is imported
  and nothing is executed: every claim on this screen is read off the syntax
  tree, which is why a file that does not yet parse cleanly still reports
  whatever the parser could recover.";

const EDGE_EXAMPLE: &str = "
  baseline                        current
  ────────                        ───────
  def brief(self):                def brief(self):
      self.send(\"orders\")             self.open()
                                      self.close()";

const CLASS_AST: &str = "
  class Handler(Agent, Log):        class_definition
      def assign(self):               ├── name .......... identifier Handler
          self.log()                  ├── superclasses .. argument_list
                                      │     ├── identifier Agent
                                      │     └── identifier Log
                                      └── body .......... block
                                            └── function_definition

  becomes   Subject  qualname  pkg.mod.Handler
                     bases     Agent, Log
                     methods   assign";

const CLASS_FOREST: &str = "
  class Channel: ...                object ⟨external⟩
  class Log: ...                      ├─ Channel
  class Agent(Channel, Log):          │    └─ Agent +Log
      ...                             └─ Log

  A class hangs off the first of its bases that resolves inside the package.
  Any base left over rides along as +Log. Bases that resolve nowhere become
  ⟨external⟩ roots, so third-party hierarchies stay visible without being
  parsed — you see that something derives from BaseModel without reading
  pydantic.";

const CALL_AST: &str = "
  def send(self, payload):          function_definition
      self.open()                     ├── name ........ identifier send
      transmit(payload)               ├── parameters .. parameters
                                      └── body ........ block
                                            ├── call
                                            │     └── function
                                            │           attribute self.open
                                            └── call
                                                  └── function
                                                        identifier transmit

  becomes   Operation  callsign  pkg.mod.Channel.send
                       params    self, payload
                       calls     self.open, transmit   (text, resolved later)";

const RESOLUTION: &str = "
  Python dispatches at run time, so every resolved call carries a grade and
  the header reports the ratio.

  CONFIRMED     read straight off the syntax
                  transmit()       a def in this same module
                  Channel()        a class in this module → its __init__
                  self.open()      walked up the inheritance graph of view 1
                  super().open()   the same walk, starting one level up
                  pkg.mod.fn()     that qualified name exists

  PROBABLE  ?   exactly one operation in the package carries the name, and the
                receiver's type cannot be known from syntax alone
                  agent.brief()    → pkg.agents.Agent.brief

  DROPPED       ambiguous, or outside the package entirely. Counted as traffic
                leaving the package, never guessed at.
                  print()   len(x)   requests.get()   two classes with .run()";

const CALL_FOREST: &str = "
  def run():                        run
      handler.assign()                └─ Handler.assign ?
                                           ├─ Agent.brief ?
  def assign(self, agent):                 │    └─ Channel.send
      agent.brief()                        └─ Handler.log
      self.log()

  Roots are entry points: operations that nothing inside the package calls.
  Both graphs are directed and acyclic only by luck, and the screen is a tree,
  so a callee reached from two callers is expanded under the first one and
  marked ⋯ under the rest. Anything reachable only through a cycle is bucketed
  under <cycle> rather than dropped.";

const NODE_TABLE: &str = "
  tree-sitter node        fields read              becomes
  ────────────────        ───────────              ───────
  class_definition        name, superclasses, body Subject
  function_definition     name, parameters, body   Operation
  decorated_definition    definition               carries decorators down
  argument_list           identifier | attribute   a base class
                          subscript → value        Generic[T] → Generic
                          keyword_argument         skipped (metaclass=...)
  parameters              name of each parameter   the signature
                          annotations and defaults ignored, so a retyped
                                                   parameter is not a change
  call                    function                 one raw call target
                          identifier | attribute   anything else ignored";

const KEYS: &str = "
  1 2 v    switch view                c      changes-only filter
  tab      switch pane                r      force resync
  ↑ ↓ j k  scroll                     g G    top / bottom
  ? h      this page                  q      quit";

/// The status legend, built from the enum so it cannot drift from the code.
fn status_legend() -> Vec<Line<'static>> {
    let rows: [(Status, &str, &str); 6] = [
        (
            Status::Activated,
            "here now, absent at baseline",
            "no node with this qualified name existed in the baseline tree",
        ),
        (
            Status::Burned,
            "at baseline, gone now",
            "the definition was deleted, or renamed into something new",
        ),
        (
            Status::Realigned,
            "a class changed base classes",
            "class A(Base) → class A(Other): the argument_list differs",
        ),
        (
            Status::Rerouted,
            "an operation changed who it calls",
            "a call node entered or left the body, changing the resolved set",
        ),
        (
            Status::Relocated,
            "same thing, same links, new module",
            "a move rather than a rewrite; the burned/activated pair is merged",
        ),
        (
            Status::Amended,
            "contents moved, connections held",
            "a class's method set, or an operation's params, decorators or async",
        ),
    ];

    let mut out = vec![Line::raw("")];
    for (status, headline, detail) in rows {
        out.push(Line::from(vec![
            Span::styled(
                format!("  {}  ", status.glyph()),
                Style::default().fg(color(status)).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:<11}", status.tag()),
                Style::default().fg(color(status)).add_modifier(Modifier::BOLD),
            ),
            Span::styled(headline.to_string(), Style::default().fg(PROSE)),
        ]));
        out.push(Line::from(Span::styled(
            format!("                 {detail}"),
            Style::default().fg(DIM),
        )));
    }
    out.push(Line::from(vec![
        Span::styled(
            format!("  {}  {:<11}", Status::Nominal.glyph(), "NOMINAL"),
            Style::default().fg(color(Status::Nominal)),
        ),
        Span::styled("no observed difference", Style::default().fg(PROSE)),
    ]));
    out.push(Line::raw(""));
    out
}

fn edge_legend() -> Vec<Line<'static>> {
    let rows = [
        (Color::Green, "NEW TRAFFIC", "this caller did not call this callee at baseline"),
        (Color::Red, "WENT DARK", "it did at baseline, and no longer does"),
    ];
    let mut out = vec![Line::raw("")];
    for (hue, tag, detail) in rows {
        out.push(Line::from(vec![
            Span::styled(
                format!("     {tag:<13}"),
                Style::default().fg(hue).add_modifier(Modifier::BOLD),
            ),
            Span::styled(detail.to_string(), Style::default().fg(PROSE)),
        ]));
    }
    out.push(Line::raw(""));
    out
}

fn markers() -> Vec<Line<'static>> {
    let rows = [
        ("+Log", Color::Blue, "a base beyond the one the class hangs off"),
        ("⟨external⟩", Color::Blue, "a name that resolves nowhere inside the package"),
        ("?", Color::Gray, "an inferred link — see PROBABLE above"),
        ("⋯", Color::Gray, "already expanded further up; the branch stops here"),
        ("↻", Color::Blue, "the operation calls itself"),
        ("<cycle>", Color::Gray, "holds nodes reachable only through a cycle"),
    ];
    let mut out = vec![Line::raw("")];
    for (marker, hue, detail) in rows {
        out.push(Line::from(vec![
            Span::styled(
                format!("     {marker:<13}"),
                Style::default().fg(hue).add_modifier(Modifier::BOLD),
            ),
            Span::styled(detail.to_string(), Style::default().fg(PROSE)),
        ]));
    }
    out.push(Line::raw(""));
    out
}

/// The worked traffic example, coloured the way the real tree colours it.
fn traffic_example() -> Vec<Line<'static>> {
    let row = |glyph: &str, name: &str, tag: &str, hue: Color| {
        let left = format!("  {glyph} {name}");
        Line::from(vec![
            Span::styled(
                format!("{left:<36}"),
                Style::default().fg(hue).add_modifier(Modifier::BOLD),
            ),
            Span::styled(tag.to_string(), Style::default().fg(hue)),
        ])
    };
    vec![
        Line::raw(""),
        row("~", "Agent.brief", "REROUTED", Color::Yellow),
        row("  ├─ ·", "Channel.open", "NEW TRAFFIC", Color::Green),
        row("  ├─ ·", "Channel.close", "NEW TRAFFIC", Color::Green),
        row("  └─ ·", "Channel.send", "WENT DARK", Color::Red),
        Line::raw(""),
    ]
}

fn rule(width: usize, label: &str) -> Vec<Line<'static>> {
    let width = width.saturating_sub(label.chars().count() + 7);
    vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled("  ── ", Style::default().fg(CHROME)),
            Span::styled(
                label.to_string(),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {}", "─".repeat(width)), Style::default().fg(CHROME)),
        ]),
    ]
}

fn prose(text: &str) -> Vec<Line<'static>> {
    block(text, Style::default().fg(PROSE))
}

fn art(text: &str) -> Vec<Line<'static>> {
    let mut lines = block(text, Style::default().fg(ART));
    // The edge example is the one place where colour carries the meaning.
    if text == EDGE_EXAMPLE {
        lines.extend(traffic_example());
    }
    lines
}

fn block(text: &str, style: Style) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = text
        .lines()
        .map(|line| Line::from(Span::styled(line.to_string(), style)))
        .collect();
    lines.push(Line::raw(""));
    lines
}
