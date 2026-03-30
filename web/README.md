# camdl web editor

Browser-based visual editor for the camdl DSL. **Not part of the core
compiler/simulator pipeline** — this is a separate UI layer that wraps the core
tools.

## What this is

A React/TypeScript single-page app that provides:

- DSL text editor with live compilation (via a local `camdlc` HTTP proxy)
- Compartment/transition canvas visualizer
- Parameter editor with live override sliders
- In-browser simulation via WASM (no server needed for sim)
- Scenario comparison panel (multiple param sets, multiple replicates)
- Claude agent panel for assisted model editing

## How it connects to the core

| Component                                         | Role                                                                                                                                               |
| ------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| `web/compiler-server/`                            | Small Express server that spawns `camdlc` and proxies compile requests to the browser; also proxies Claude API                                     |
| `rust/crates/wasm/`                               | WASM build of the Rust sim backends — compiled to `web/public/camdl_sim_bg.wasm`; exposes `validate(ir_json)` and `simulate(ir_json, config_json)` |
| Core OCaml (`ocaml/`)                             | Not touched by the web app directly; reached only via `camdlc` subprocess                                                                          |
| Core Rust (`rust/crates/{ir,sim,observe,io,cli}`) | Sim backends live here; the WASM crate re-exports them                                                                                             |

## Setup

```bash
# Build WASM (from repo root)
cd rust/crates/wasm
wasm-pack build --target web --out-dir ../../web/public/wasm

# Start compiler server (needs camdlc built: cd ocaml && dune build)
cd web/compiler-server
npm install && npm run dev

# Start web app
cd web
npm install && npm run dev
```

## For agents working on the core compiler/simulator

You do not need to touch anything in `web/` or `rust/crates/wasm/` to work on:

- The DSL (`ocaml/lib/compiler/`)
- The IR (`ocaml/lib/ir/`, `rust/crates/ir/`)
- The simulator backends (`rust/crates/sim/`)
- The CLI (`rust/crates/cli/`)
- Golden tests (`ocaml/golden/`, `ir/golden/`)

The web app tracks the IR schema automatically as long as:

1. `rust/crates/ir/` types stay serde-compatible
2. The WASM crate is rebuilt after backend changes (`wasm-pack build`)
3. `web/src/types/ir.ts` is kept in sync with `ir/schema.json` if the schema
   changes

See `CLAUDE.md` at the repo root for the full architecture and build commands.
