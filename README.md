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
cargo install cargo-edict
cargo edict init     # writes a commented edict.toml
cargo edict          # check (exit 1 on errors)
```

Options: `--path <dir>` (crate or workspace), `--config <file>`, `--format json`.
Workspaces are expanded automatically. Each member uses its own `edict.toml`,
falling back to the workspace root's.

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
   paths like `crate::a::b()`, `super::`/`self::`, and paths inside
   comma-separated macro arguments (`format!`, `assert!`, …).
3. Resolves each path to the module that **defines** the item, following named
   and glob `pub use` re-exports. A parent's `pub use child::Thing` is treated as
   a barrel, not as a dependency, so re-export hubs don't create fake cycles.
4. Runs Tarjan's SCC algorithm for cycles; checks every edge against layers,
   facades, and deny lists.

Code under `#[cfg(test)]` and `#[test]` functions is ignored.

### Known limits

- Name resolution is syntactic: no macro expansion, no `cfg` evaluation (other
  than `cfg(test)`), and glob imports (`use foo::*`) only record the dependency on
  `foo`, not on individual items.
- Paths inside macros whose arguments aren't plain expressions are not seen.
- One crate at a time; cross-crate dependencies are already a DAG (Cargo enforces it).

## Using with coding agents

Run `cargo edict --format json` in your agent's check loop or in CI. Violations
carry file and line, and every message says what to do instead. Put a line in
`AGENTS.md` / `CLAUDE.md`:

> Run `cargo edict` before finishing. It must pass. Shared types go in lower
> layers; never add a cycle, a deep cross-module path, or I/O in `domain`.

## Dogfooding

This repo checks itself: [`edict.toml`](edict.toml) declares five layers
(`core` → `config` → `analysis` → `io` → `cli`), with the analysis pure, and the
test suite fails if `cargo edict` reports anything.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
