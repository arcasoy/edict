# cargo-edict

**Deterministic architecture laws for Rust crates.**

Clippy checks your code. `cargo edict` checks your *structure*: which modules may
depend on which, and what the core of your system is allowed to touch. Every rule is
a yes/no question answered from the syntax tree. No heuristics, no scores, no
judgment calls. That makes it a hard gate in CI and a guardrail for coding agents,
which follow whatever patterns the codebase already has.

```
$ cargo edict
error[cycles]: module cycle: billing::invoice -> billing::tax -> billing::invoice
  --> src/billing/invoice.rs:1: `billing::invoice` uses `crate::billing::tax::rate`
  --> src/billing/tax.rs:2: `billing::tax` uses `crate::billing::invoice::Invoice`

error[layers]: `domain` (layer `domain`) depends on `infra` (layer `infra`), but `domain` may not depend on `infra`
  --> src/domain/mod.rs:1: `domain` uses `crate::infra::Db`

error[purity]: `domain` is in pure layer `domain` but uses `std::fs::read_to_string` (denied: `std::fs`)
  --> src/domain/mod.rs:5: pass this in from an outer layer instead

edict: `shop`: 3 errors, 0 warnings (8 modules, 41 edges)
```

## The laws

| # | Law | Rule id | Default |
|---|-----|---------|---------|
| 1 | **No cycles.** The module graph is a DAG, down to individual `.rs` files. | `cycles` | error |
| 2 | **Layers point down.** Modules depend only on their own layer or layers declared below. | `layers` | error (when layers are declared) |
| 3 | **Facades only.** Outside a top-level module, you may use only what it exports at its root: `crate::billing::Invoice` yes, `crate::billing::tax::rate` no. | `facade` | error |
| 4 | **Pure core.** Layers marked `pure` may not touch fs, net, env, processes, threads, clocks, randomness, or async runtimes. | `purity` | error (for `pure` layers) |
| 5 | **No global mutable state.** No `static mut`, no statics holding `Mutex`/`RwLock`/`RefCell`/`Cell`/atomics, no `thread_local!`, outside an allowlist. | `global-state` | error |
| – | **Soft limits.** File length and fan-out. | `limits` | warning |

Why these? Each one is a property that's cheap to keep and expensive to recover.
A cycle forces modules to change together. A layer violation drags I/O into
logic. A facade breach freezes a module's internals. Hidden global state is
coupling that doesn't show up in any import graph. File length is a cost, not a
correctness problem, so it only warns.

## Install

```sh
cargo install --git https://github.com/arcasoy/edict   # not on crates.io yet
rustup component add rust-analyzer                     # only needed for --full
cargo edict init     # writes a commented edict.toml
cargo edict          # fast check (exit 1 on errors)
```

Options: `--path <dir>` (crate or workspace), `--config <file>`, `--format json`,
`--full`, `--scip <index>`, `--hook`. Subcommands: `check` (default), `init`, `graph`. Workspaces are expanded automatically. Each
member uses its own `edict.toml`, falling back to the workspace root's.

## Two modes

| | Fast (default) | Full (`--full`) |
|---|---|---|
| Engine | Paths as written, parsed with `syn` | Fast mode **plus** rust-analyzer's SCIP index |
| Sees | `use`, inline paths, re-exports, globs, macro args | Also method calls on inferred types, macro-generated code, impls in other modules |
| Speed | Milliseconds (0.3s on `syn`) | Seconds (≈10s on this repo, ≈17s on `syn`) |
| Needs | Nothing | `rustup component add rust-analyzer` |
| Use for | Every agent edit / end of turn | Pre-push, CI |

Full mode is a strict superset: it unions both engines' edges, so it can only add
findings, never drop one that fast mode reports. The summary line counts the
file-to-file dependencies only rust-analyzer found, so you can see how much fast
mode misses on your codebase:

```
edict (full): `syn`: 40 errors, 29 warnings (98 modules, 14038 edges, 38 found only by rust-analyzer)
```

The `facade` and `global-state` laws stay syntactic in both modes: facades are
about how a path is *written*, and global state is a property of declarations.

If your repo already maintains SCIP indexes, skip the indexing step:
`cargo edict --scip path/to/index.scip`.

## Seeing the graph

```sh
cargo edict graph            # writes edict-graph.html
cargo edict graph --full     # include dependencies only rust-analyzer finds
```

One self-contained HTML file, no server or network needed. Drag to pan,
scroll or pinch to zoom, tap a module to see what it depends on and what uses
it, with the exact paths and lines behind each arrow.

- **Modules / Files** toggle between top-level modules and individual files.
- Rows are dependency depth: things that depend on nothing sit at the bottom.
- Nodes carry their layer's color; red borders are modules in a cycle.
- Red arrows are cycle edges, orange are layer violations, dashed are
  dependencies only rust-analyzer found. **Problems only** fades everything else.

Useful for drafting layers: put each row-band in a layer, then fix the red and
orange.

## Hooking it up to an agent

`--hook` is built for agent hooks: silent on success; on errors it writes the
report to stderr and exits 2, which Claude Code feeds back to the model.

Fast mode after every edit and at end of turn (`.claude/settings.json`):

```json
{
  "hooks": {
    "PostToolUse": [
      { "matcher": "Edit|Write|MultiEdit", "hooks": [{ "type": "command", "command": "cargo edict --hook" }] }
    ],
    "Stop": [
      { "hooks": [{ "type": "command", "command": "cargo edict --hook" }] }
    ]
  }
}
```

A failing `Stop` hook keeps the agent working until the laws pass, so on a
codebase with existing violations it will keep cleaning them up turn after turn.
That is the point, but it can loop for a long time; scope the laws (set rules to
`warn`, or declare fewer layers) to control how much it takes on at once.

Full mode before every push (`.git/hooks/pre-push`, executable):

```sh
#!/bin/sh
exec cargo edict --full
```

## Configuration

```toml
# edict.toml
[cycles]
granularity = "file"     # "file": no cycles between any .rs files. "top": only between top-level modules.

[global_state]
allow = ["infra::runtime"]

[purity]
extra_deny = ["sqlx"]    # added to the default deny list

[limits]
level = "warn"
max_fan_out = 10
max_lines = 1500

# Layers, bottom-up. `depends_on` is transitive.
# The crate root (lib.rs / main.rs) is the composition root and is exempt.
[[layer]]
name = "domain"
modules = ["domain"]
pure = true

[[layer]]
name = "app"
modules = ["app"]
depends_on = ["domain"]

[[layer]]
name = "infra"
modules = ["db", "http"]
depends_on = ["app"]

[layers]
require_all = true       # every top-level module must be in a layer
```

Every rule takes `level = "error" | "warn" | "off"`. Unknown keys are rejected,
so typos fail loudly.

## How it works

1. Follows `mod` declarations from `lib.rs`/`main.rs` (including `mod.rs`, inline
   modules and `#[path]`) to build the module tree.
2. Parses every file with `syn` and collects every path: `use` trees, inline
   paths like `crate::a::b()`, `super::`/`self::`, and paths inside macro calls
   (parsed as expressions where possible, and scanned token-by-token for
   `a::b::c` sequences otherwise, which also catches `$crate::` in `macro_rules!`).
3. Names that arrive through in-crate glob imports (`use crate::foo::*`) are
   resolved against what `foo` actually defines or re-exports. To avoid
   mistaking local variables for items, bare lowercase names are only resolved
   this way when called (`helper()`); capitalized names (types, consts,
   variants) always are.
4. Resolves each path to the module that **defines** the item, following named
   and glob `pub use` re-exports. A parent's `pub use child::Thing` is treated as
   a barrel, not as a dependency, so re-export hubs don't create fake cycles.
5. Runs Tarjan's SCC algorithm for cycles; checks every edge against layers,
   facades, and deny lists.

Code under `#[cfg(test)]` and `#[test]` functions is ignored.

### Known limits

Fast mode:
- No macro expansion and no `cfg` evaluation (other than `cfg(test)`): code behind
  every feature flag counts, which errs toward reporting, never hiding.
- Can't see dependencies through inferred types, e.g. calling a method that
  another module defines on a value whose type you never named. Full mode can.
- Bare lowercase names from a glob import only resolve when called.

Full mode:
- Re-indexes the whole workspace on every run; `rust-analyzer scip` has no
  incremental mode. Use `--scip` to reuse an index you already maintain.
- rust-analyzer's CLI subcommands carry no stability guarantee; if `scip`
  output changes, full mode may need an update.

Both: one crate at a time; cross-crate dependencies are already a DAG (Cargo
enforces it).

## Telling the agent about the laws

Put a line in `AGENTS.md` / `CLAUDE.md` so the agent knows what the hook enforces:

> `cargo edict` must pass. Shared types go in lower layers; never add a cycle, a
> deep cross-module path, or I/O in a pure layer. Run `cargo edict --full` before
> pushing.

## Dogfooding

This repo checks itself: [`edict.toml`](edict.toml) declares five layers
(`core` → `config` → `analysis` → `io` → `cli`), with the analysis pure, and the
test suite fails if `cargo edict` or `cargo edict --full` reports anything.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
