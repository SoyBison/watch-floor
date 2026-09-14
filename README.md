# watch-floor

A terminal dashboard that shows, live, what your edits are doing to a Python
package's structure — the class hierarchy and the call graph — by diffing the
working tree against the baseline committed at git `HEAD`.

It was built to sit in a second pane while an agent edits your codebase, so you
can watch the shape of the thing change instead of reading diffs line by line.

Everything is parsed with [tree-sitter](https://tree-sitter.github.io). Nothing
is imported and nothing is executed, so a file that doesn't parse cleanly yet
still reports whatever the parser could recover.

```
┌──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│WATCH FLOOR  ·  target cipher  (cipher/)    1 STRUCTURE   2 TRAFFIC                                                   │
│  baseline HEAD @ ca2f2b2 (3 files)   current working tree (3 files)   resynced 0s ago in 1ms                         │
│  + 2 ACTIVATED   - 1 BURNED   ~ 2 REROUTED   → 0 RELOCATED   * 1 AMENDED   · 11 operations · 11 links (3 inferred) · │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
┌ CALL TRAFFIC · from entry points ────────────────────────────────────┐┌ SITREP · 6 items ────────────────────────────┐
│+ exfiltrate  cipher.agents:21  ACTIVATED                             ││ + ACTIVATED exfiltrate()  cipher.agents      │
│  └─ · run  cipher.agents:15                                          ││            calls run                         │
│     ├─ ~ Handler.assign ?  cipher.agents:11  REROUTED                ││ + ACTIVATED sweep()  cipher.core             │
│     │  ├─ ~ Agent.brief ?  cipher.agents:5  REROUTED                 ││            calls encode                      │
│     │  │  ├─ · Channel.close  cipher.core:4  NEW TRAFFIC             ││ - BURNED    Handler.log(self)  cipher.agents │
│     │  │  ├─ · Channel.open  cipher.core:2  NEW TRAFFIC              ││            was at cipher/agents.py:14        │
│     │  │  └─ · Channel.send  cipher.core:6  WENT DARK                ││ ~ REROUTED  Agent.brief(self)  cipher.agents │
│     │  │     ├─ · Channel.close ⋯                                    ││            +Channel.close +Channel.open -Chan│
│     │  │     ├─ · Channel.open ⋯                                     ││ ~ REROUTED  Handler.assign(self, agent)  ciph│
│     │  │     └─ * transmit  cipher.core:12  AMENDED                  ││            -Handler.log                      │
│     │  │        └─ · encode  cipher.core:17                          ││ * AMENDED   transmit(payload, priority)  ciph│
└──────────────────────────────────────────────────────────────────────┘└──────────────────────────────────────────────┘
 q quit · r resync · 1/2 view · c changes-only [off] · tab pane · ↑↓/jk scroll · ? help
```

## Install

Requires **Rust 1.90+** (a dependency sets that floor) and `git` on `PATH`.

```sh
cargo build --release
./target/release/watch-floor /path/to/repo
```

## Usage

```sh
watch-floor [PATH]
```

`PATH` defaults to `.`, and can be the package directory itself or a directory
containing exactly one package — the flat layout and the `src/` layout are both
found automatically. If there is more than one candidate, name the one you want.

| flag | effect |
| --- | --- |
| `--once` | print one text report and exit, instead of holding the watch |
| `--view structure\|network` | restrict to one view |
| `-c`, `--changes-only` | open with the tree pruned to changed branches |
| `--manual` | print the field manual to stdout; needs no target |

### Keys

| key | action | key | action |
| --- | --- | --- | --- |
| `1` `2` `v` | switch view | `c` | changes-only filter |
| `tab` | switch pane | `r` | force resync |
| `↑` `↓` `j` `k` | scroll | `g` `G` | top / bottom |
| `?` `h` | field manual | `q` | quit |

The floor refreshes itself: a filesystem watcher on the package and on `.git/refs`
triggers a resync, debounced 150ms. The baseline is only re-read when `HEAD`
actually moves, so committing your work resets the picture to clean.

## The two views

### `1` — Inheritance structure

Classes, hung off their base classes. Each class attaches to the first of its
bases that resolves inside the package; leftover bases ride along as `+Mixin`.
Bases that resolve nowhere become `⟨external⟩` roots, so you can see that
something derives from `BaseModel` without parsing pydantic.

```
INHERITANCE STRUCTURE  + 0 ACTIVATED  - 0 BURNED  ~ 0 REALIGNED  * 1 AMENDED  · 3 classes
object <external>
  └─ · Channel  cipher.core:1
     └─ · Agent  cipher.agents:4
        └─ * Handler  cipher.agents:10  AMENDED
```

### `2` — Call traffic

Functions and methods, rooted at entry points — operations that nothing inside
the package calls — and expanded downward through their callees. A callee
reached from two callers is expanded under the first and marked `⋯` under the
rest; anything reachable only through a cycle is bucketed under `<cycle>`.

Call resolution is best-effort by necessity, since Python dispatches at runtime,
so every edge carries a confidence and the header reports the ratio:

- **Confirmed** — a same-module `def`, a class construction resolving to its
  `__init__`, a qualified name that exists, or `self.m()` / `super().m()` walked
  up the inheritance graph the structure view already built.
- **Probable** (`?`) — exactly one operation in the package carries that name and
  the receiver's type can't be known from syntax alone.
- **Dropped** — ambiguous, or outside the package. Counted as traffic leaving the
  package, never guessed at.

## Vocabulary

Statuses describe **a thing**; traffic tags describe **a link between two things**.
Press `?` in the app (or run `watch-floor --manual`) for the full version, with
worked examples and the exact tree-sitter node each term is read off.

| glyph | status | means |
| --- | --- | --- |
| `+` | ACTIVATED | here now, absent at baseline |
| `-` | BURNED | at baseline, gone now |
| `~` | REALIGNED | a class changed base classes |
| `~` | REROUTED | an operation changed who it calls |
| `→` | RELOCATED | same thing, same links, new module — a move, not a rewrite |
| `*` | AMENDED | contents moved, connections held (method set; params, decorators, async) |
| `·` | NOMINAL | no observed difference |

| tag | means |
| --- | --- |
| NEW TRAFFIC | this caller did not call this callee at baseline |
| WENT DARK | it did at baseline, and no longer does |

Edge tags are only drawn when the caller exists on both sides — every call out
of a brand-new function is new by definition, and flagging all of them tells you
nothing.

| marker | means |
| --- | --- |
| `+Log` | a base beyond the one the class hangs off |
| `⟨external⟩` | a name that resolves nowhere inside the package |
| `?` | an inferred link |
| `⋯` | already expanded further up; the branch stops here |
| `↻` | the operation calls itself |

## How it works

| module | job |
| --- | --- |
| `collection.rs` | locate the package; read both snapshots (disk, and `git ls-tree` + `git show`) |
| `intercept.rs` | tree-sitter extraction: classes, bases, methods, functions, call sites |
| `dossier.rs` | `Subject` and `Operation`; base resolution and call resolution |
| `sitrep.rs` | shared vocabulary: `Status`, `Edge`, `Counts`, tree flattening and pruning |
| `structure.rs` | the class diff and inheritance forest |
| `network.rs` | the call diff and traffic forest |
| `floor.rs` | rendering |
| `manual.rs` | the field manual, for the `?` pane and `--manual` |
| `tripwire.rs` | filesystem watchers |

Neither graph is a tree — inheritance is a DAG and the call graph has cycles —
so each view is a projection, and the projection rules are documented in the
field manual rather than left implicit.

A 201-file / 28.6k-line package resyncs in about **440ms** in release mode: both
snapshots, 8400 operations, 16000 links, both views recompiled.

## Known limits

- **Imports are not followed.** Resolution is structural, so a name that is
  ambiguous across the package resolves to nothing rather than to a guess.
- **Module-level code is not attributed to any operation**, and its calls are not
  recorded. Work done at import time is invisible.
- **A method reached only through its base class has no in-package caller**, so it
  appears as its own entry point. The SITREP notes `overrides Base.m` when it
  applies, but the tree does not reroute virtual dispatch.
- Parameter annotations and defaults are ignored, so a retyped parameter is not
  reported as a change.
