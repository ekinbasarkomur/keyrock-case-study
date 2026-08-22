---
spec_name: "Step 0 — Foundation"
spec_id: "001"
spec_folder: "001-step-0-foundation"
status: "reviewed"
created_at: "2026-08-22"
updated_at: "2026-08-22"
created_by: "spec-synthesizer"
creation_mode: "human-brief"
source_inputs:
  - "inputs/human.md (kept locally, gitignored — raw briefs aren't published)"
source_agents: []
goal: "Land a buildable, containerised, dependency-complete scaffold for the 11-step Keyrock order-book aggregator, adapted from — not layered onto — the pre-existing generic CLI scaffold."
purpose: "Every later step (websocket feeds, merge, gRPC server) needs a crate that already resolves its full dependency graph and already compiles a proto into Rust, so version conflicts and build-pipeline surprises surface once, now, instead of mid-implementation."
parent_request: "step-0 foundation brief, 2026-08-22"
related_paths:
  - "Cargo.toml"
  - "src/main.rs"
  - "src/lib.rs"
  - "src/config.rs"
  - "src/telemetry.rs"
  - "tests/cli.rs"
  - "Dockerfile"
  - "compose.yml"
  - "proto/"
  - "build.rs"
  - ".env.example"
  - "README.md"
verification_level: "mixed"
complexity: "small"
---

# Spec: 001-step-0-foundation

## Problem

The repo has a working scaffold (library/binary split, tests, container) built
*before* the case-study brief was known. It proves the toolchain and test
harness, but its shape — `hello`/`doctor` subcommands, `KEYROCK_`-prefixed
env-var configuration, a Docker image with nothing to run — has nothing to do
with the actual problem: a service that will eventually stream a merged
Binance+Bitstamp order book over gRPC. Step 0 of the project's 11-step build
order is where the crate's dependency graph, its proto build pipeline, and
its container shape get established for good — before any websocket or gRPC
logic exists. This spec is the plan for that step; no code should land until
it's approved.

## Goal

After this step:

- `Cargo.toml` carries the full dependency list the whole 11-step build needs
  (`tokio`, `tokio-tungstenite`, `futures-util`, `serde`/`serde_json`,
  `tonic`/`tonic-prost`/`prost`, `tonic-prost-build`, plus the existing
  `clap`/`tracing`/`tracing-subscriber`/`anyhow`/`thiserror`), added via
  `cargo add` so every version is resolver-picked, not memorised.
- `proto/orderbook.proto` holds the brief's schema verbatim.
- `build.rs` at the crate root compiles it via `tonic_prost_build`, and
  `target/**/orderbook.rs` exists and contains `Summary` and `Level` after a
  build.
- `src/main.rs` parses `--pair`/`--port`, logs one startup line, and exits 0 —
  no subcommands, no business logic.
- The Dockerfile and `compose.yml` build and run that binary end to end,
  still with no websocket or gRPC behaviour.
- `git log` gains one meaningful, reviewable commit for this step.

## Purpose

The reviewer reads the commit history as part of the evaluation. A step-0
commit that only adds dependencies and proves the pipeline compiles — with
nothing pretending to be a feature yet — is the honest foundation for every
step that follows, and it's the cheapest place to debug the container (no
logic to blame it on) and the dependency resolution (no feed code depending
on a version choice made under time pressure later).

## Out of Scope

- Any websocket client code (`src/exchange/*` — arrives at steps 1 and 4).
- Any gRPC service implementation (`src/server.rs` — arrives at step 2).
- `merge.rs`, `book.rs`/`model.rs`, `src/aggregator.rs` — arrive with the
  steps that need them. No stub files, no `todo!()` — they'd only produce
  dead-code warnings for no benefit.
- A `HEALTHCHECK` in the Dockerfile — there is still no long-running server
  to probe; the commented-out block stays commented out.
- Joining a shared external Docker network — this project deliberately stays
  off any such network so it runs for a reviewer with no manual setup;
  `compose.yml` keeps compose's default project network.
- Deciding the fixed-point price/amount scale, the crossed-book behaviour, or
  the sort tie-break — those are step-5 questions for this project's later
  build order; nothing about them needs deciding here.
- `hdrhistogram` — needed for step 8's latency measurement, but not part of
  step 0's dependency list (see Proposed Design). Step 0 adds only what that
  list names; `hdrhistogram` lands when step 8 needs it.

## Current State

Verified by reading the files directly, not assumed:

- **`Cargo.toml`** already has `anyhow`, `clap` (derive), `thiserror`,
  `tracing`, `tracing-subscriber` (env-filter). None of the brief's
  case-study-specific crates (`tokio`, `tokio-tungstenite`, `futures-util`,
  `serde`/`serde_json`, `tonic`/`tonic-prost`/`prost`,
  `tonic-prost-build`) are present yet.
- **`src/lib.rs`** — `pub mod config; pub mod telemetry;`, a `VERSION` const,
  a placeholder `greeting()` fn with one unit test.
- **`src/config.rs`** — `Config { log_level, host, port }`, built by
  `Config::from_env()` reading `KEYROCK_`-prefixed env vars, with
  `ConfigError::InvalidPort` for an unparseable `KEYROCK_PORT`. No `pair`
  field exists.
- **`src/telemetry.rs`** — installs a `tracing_subscriber` with the writer
  pinned to stderr, `RUST_LOG` overrides the passed-in filter. Reusable
  as-is; nothing about it is tied to the old CLI shape.
- **`src/main.rs`** — clap `Cli` with `Hello { name }` / `Doctor`
  subcommands; calls `Config::from_env()`, then `telemetry::init(...)`.
- **`tests/cli.rs`** — 4 integration tests: `hello` greets and exits 0,
  `doctor` reports a `KEYROCK_PORT` override, an invalid `KEYROCK_PORT` fails
  loudly, and logs stay off stdout. All 4 assert behaviour that goes away
  with the old CLI shape.
- **`Dockerfile`** — two-stage build, `rust:1.97-slim-bookworm` builder +
  `debian:bookworm-slim` runtime. Dependency-cache stub stage does
  `COPY Cargo.toml Cargo.lock ./` then stubs `src/main.rs`/`src/lib.rs` only
  — no `build.rs`, no `proto/`. `RUN touch src/main.rs src/lib.rs` before the
  real build is the documented load-bearing trap fix. Installs
  `ca-certificates` via apt for TLS. `ENTRYPOINT ["keyrock-case-study"]`,
  `CMD ["--help"]`.
- **`compose.yml`** — CLI-runner shape (`docker compose run --rm app hello
  Keyrock`), sets `environment: KEYROCK_LOG_LEVEL: ${KEYROCK_LOG_LEVEL:-info}`,
  no `ports:`, comments explain why no external Docker network is joined.
- **`.env.example`** — templates `KEYROCK_LOG_LEVEL`/`KEYROCK_HOST`/`KEYROCK_PORT`.
- **`README.md`** — documents the `hello`/`doctor` commands and the
  `KEYROCK_*` env vars; no proto/gRPC mention yet.
- **`.dockerignore`** — already excludes `target/`, `.claude/`, `CLAUDE.md`,
  `specs/`, `docs/`. Correct as-is; no change needed here.
- **git** — this is not a first commit. There is existing history (currently
  one commit, "Scaffold Rust project with tests and container"); step 0 lands
  as one new commit on top of it, not a fresh `git init`.

## Proposed Design

### Dependencies (`Cargo.toml`)

Run the brief's `cargo add` list as-is, layered onto the existing four
dependencies (do not re-run `cargo new`):

```
cargo add tokio --features full
cargo add tokio-tungstenite --features rustls-tls-webpki-roots
cargo add futures-util serde_json tracing tracing-subscriber
cargo add serde --features derive
cargo add clap --features derive
cargo add tonic tonic-prost prost
cargo add --build tonic-prost-build
```

`clap` and `tracing`/`tracing-subscriber` are already dependencies; re-running
`cargo add` for them is a no-op unless it bumps a version — read `Cargo.toml`
back after running the list, as the brief asks, rather than assuming nothing
changed. Every version is whatever the resolver picks; none are hand-pinned
from memory, per the brief's explicit constraint and the tonic 0.12/0.13 vs
0.14 API-split risk it calls out.

### `proto/orderbook.proto`

The brief's schema, copied verbatim, byte-for-byte. Never hand-edited after
this — any opinion about the schema goes in the README's "what would change
for production" section, not into the `.proto` file itself, because the
reviewer tests against their own copy of it.

### `build.rs` (crate root, not `src/`)

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::compile_protos("proto/orderbook.proto")?;
    Ok(())
}
```

Verify this shape against the actual `tonic-prost-build` 0.14 API on docs.rs
for whatever version `cargo add` resolved — the brief flags that most
training-era knowledge describes the older `tonic-build` API, which will not
compile against 0.14's split.

### `src/config.rs` — **decided 2026-08-22: keep env-var config, layer CLI flags on top**

The env-var design in `Config::from_env()` stays — it was a deliberate,
already-tested choice (config has a working default for every field), and
this spec extends it rather than replacing it. `Config` gains a fifth
field, `pair`, read from a new `KEYROCK_PAIR` env var, defaulting to
`"ethbtc"`. The default `port` changes from `8080` to `50051` — the brief's
own default, and the port this service
will actually bind to once the gRPC server exists in a later step, so there's
no reason for step 0's default to disagree with it. `host`/`log_level` are
unchanged; `ConfigError::InvalidPort` stays, since `KEYROCK_PORT` is still a
possible source of an unparseable value that clap never sees.

```rust
pub struct Config {
    pub log_level: String,
    pub host: String,
    pub port: u16,
    pub pair: String,
}
```

**CLI flags override env vars, not the other way round** — `--pair`/`--port`
are the more specific, closer-to-the-call-site input, so they win when both
are set. Mechanically: `Cli`'s `pair`/`port` fields are `Option<T>` with *no*
clap-level default (so "not given" is representable), `Config::from_env()`
runs first to get the env-or-default values, then any `Some(_)` from `Cli`
overwrites the matching `Config` field. This is more moving parts than a
CLI-only design — two input sources instead of one, plus a merge step — but
it's what was asked for, and the merge itself is a few lines, not a new
abstraction.

### `src/main.rs`

Flat CLI, no subcommands, flags optional so they can be distinguished from
"unset":

```rust
#[derive(Parser)]
struct Cli {
    #[arg(long)]
    pair: Option<String>,
    #[arg(long)]
    port: Option<u16>,
}
```

`main()`: parse `Cli`, build `Config::from_env()?`, then apply
`cli.pair`/`cli.port` over it if `Some`, call `telemetry::init(&config.log_level)`
(unchanged from today — this is exactly what the existing scaffold already
does, so step 0 doesn't touch it), log one `info!` line in the form
`starting, pair=ethbtc port=50051`, and return `Ok(())`. The existing
`--verbose` flag is dropped — it wasn't part of the brief's two-flag CLI
surface, and `RUST_LOG` already covers the same need without adding a flag
nothing asked for. `Command::Hello`/`Command::Doctor`, `greeting()` in
`src/lib.rs`, and its unit test are deleted — placeholder code is meant to
disappear when the real problem lands, not grow neighbours. The unused
`VERSION` const in `src/lib.rs` goes with it: its only two callers
(`greeting()` and the `doctor` printout) are both gone, and clap's own
`#[command(version)]` already reads the crate version independently.

### `tests/cli.rs`

The 4 existing tests assert `hello`/`doctor` subcommand behaviour that no
longer exists; replace them with a set that covers both input sources and
their precedence, since that's the actual new contract:

- Default run (no flags, no env vars) exits 0, stderr contains
  `pair=ethbtc port=50051`, stdout is empty (no answer to print yet — this
  step has no output beyond logs).
- `--pair btcusd --port 12345` (no env vars) overrides both defaults, logged
  line reflects it.
- `KEYROCK_PAIR=btcusd KEYROCK_PORT=12345` (no flags) overrides both
  defaults via the env path — proves `Config::from_env()` still works, not
  just the new flag-merge code.
- `KEYROCK_PORT=1` with `--port 12345` given at the same time → the logged
  line shows `12345`, not `1` — proves the flag wins over the env var, which
  is the actual point of this design.
- An invalid `--port` (e.g. `--port not-a-number`) is rejected by clap with a
  non-zero exit, before `Config` is even built.
- An invalid `KEYROCK_PORT` with no `--port` flag given still fails loudly
  via `ConfigError::InvalidPort` and mentions `KEYROCK_PORT` on stderr —
  this is the one existing test that carries over almost unchanged, since
  that code path didn't change.

`src/config.rs`'s two existing unit tests (defaults, invalid-port-is-an-error)
stay, updated only for the new default port (`50051`) and the new `pair`
field (`Config::default().pair == "ethbtc"`); no test is deleted here, since
`Config::from_env()` itself wasn't replaced.

### `Dockerfile`

Three changes:

1. **Dependency-cache stub stage must see `build.rs` and `proto/`.**
   `build.rs` runs on every `cargo build` invocation, including the
   stub-source dependency-only compile — it is not conditional on `src/`
   being real. Add `COPY build.rs Cargo.toml Cargo.lock ./` and
   `COPY proto/ ./proto/` before the stub build. The existing
   `RUN touch src/main.rs src/lib.rs` trap-fix stays scoped to `src/` only —
   `build.rs` doesn't change between the stub and real build, so it doesn't
   need touching, and touching it would just force `tonic-prost-build` to
   re-run for no reason.
2. **`ca-certificates` apt install — decided 2026-08-22: drop it.** Now that
   `tokio-tungstenite`'s `rustls-tls-webpki-roots` feature is a dependency,
   it bundles its own root certs, so the runtime image needs no system CA
   package once TLS connections actually happen in a later step.
   Step 0 makes no TLS connection itself, but there's no reason to carry a
   package the design has already committed to not needing — remove the
   `apt-get install ca-certificates` line and the `apt-get update`/cleanup
   around it entirely, since nothing else in the runtime stage needs apt.
3. **`CMD` changes from `["--help"]` to `[]`.** `ENTRYPOINT` stays
   `["keyrock-case-study"]`. With `CMD ["--help"]`, `docker compose up
   --build` with no override would print help text and exit 0 without ever
   logging the startup line — that would technically exit cleanly but would
   not satisfy "logs the startup line" in acceptance criterion 3. `CMD []`
   runs the binary with no arguments, which resolves to the CLI's own
   defaults (`--pair ethbtc --port 50051`) and produces the real log line.

### `compose.yml`

**Unchanged from the current file** — since `Config::from_env()` is being
kept (see the config-shape decision above), `environment: KEYROCK_LOG_LEVEL:
${KEYROCK_LOG_LEVEL:-info}` still wires a variable `Config` actually reads;
it was only slated for removal under the rejected "replace Config" option.
The one edit that still applies: update the file's header comment, which
currently says "there is no long-running service yet, so there is nothing to
`up -d`" — that framing still holds (this binary starts, logs, and exits; it
is not a server), but the comment should mention `docker compose up --build`
as the command step 0's acceptance criterion actually runs, alongside the
existing `docker compose run --rm app ...` examples.

### `.env.example` — **decided 2026-08-22: keep, and extend it**

Since `Config::from_env()` survives, `.env.example` still documents real,
read variables. Add a `KEYROCK_PAIR=ethbtc` line alongside the existing
three, and update the shown `KEYROCK_PORT` default from `8080` to `50051` to
match the new `Config` default.

### `README.md`

Update "Quick start" for the new flags (`cargo run -- --pair ethbtc --port
50051`, and plain `cargo run --` since both have defaults). Update the
"Configuration" table: keep the `KEYROCK_*` env var rows (still real), add
`KEYROCK_PAIR` (default `ethbtc`), update `KEYROCK_PORT`'s shown default to
`50051`, and add one line stating the precedence decided above — `--pair`/
`--port` override the matching env var when both are given. Add a
`proto/orderbook.proto` mention under "Layout." Add a placeholder heading —
no invented content — "What would change for production," per the brief's
own instruction that any opinion about the schema belongs there, not in the
`.proto` file.

### Git

One new commit on the existing history (not a fresh `git init` — the repo
already has one commit). Proposed message, adapted from the brief's suggested
text to reflect what's actually landing on top of the pre-existing scaffold
rather than a from-scratch `cargo init`:

```
Step 0: add proto schema, tonic build pipeline, and full dependency set

Replace the hello/doctor CLI scaffold with a --pair/--port entry point,
layered as overrides on the existing KEYROCK_ env-var Config; the real
websocket and gRPC logic arrive in later steps.
```

## Acceptance Criteria

- `cargo build` succeeds.
- `find target -name "orderbook.rs"` returns a file containing `Summary` and
  `Level`.
- `docker compose up --build` brings the container up, logs the startup
  line, and exits cleanly.
- `cargo test` is green against the new test set described above (the exact
  count depends on how many cases land, but every existing test tied to
  `hello`/`doctor`/`KEYROCK_*` behaviour is either replaced or removed, not
  left failing).

## Invariants and Critical Don'ts

- The `.proto` schema is never hand-edited after being copied in — the
  reviewer tests against their own copy.
- No websocket, gRPC-service, merge, or model code lands in this step —
  see Out of Scope.
- No dependency-caching Docker tricks (`cargo-chef` or similar) — the brief
  is explicit that the reviewer builds once, so that complexity has no
  payoff here.
- Dependency versions come from `cargo add`'s resolver, never pinned from
  memory.
- The toolchain stays pinned in both `rust-toolchain.toml` and the
  `Dockerfile`'s `FROM` line — this step does not touch either, and nothing
  here should cause them to drift apart.
- The stdout/answer vs. stderr/logs split (`src/telemetry.rs`) stays intact
  even though there's no "answer" yet in this step — logs still go to
  stderr only.
- The project stays off any shared external Docker network and adds no
  `HEALTHCHECK` — neither applies until a real server exists.

## Risks and Tradeoffs

- **`protoc` availability is unverified until this step actually runs.**
  Recent `prost`/`tonic-prost-build` versions may bundle `protoc`; if the
  local `cargo build` fails without it, the Docker builder stage needs
  `apt-get install -y protobuf-compiler` added, and the outcome should be
  reported back (which case applied), not assumed either way.
- **`Config` now has two ways to set the same value** (env var and CLI flag)
  with a precedence rule between them — more surface area than either input
  alone, and precedence rules are exactly the kind of thing that's easy to
  get backwards without a test for it (see the dedicated precedence test in
  Testing Strategy). This was a deliberate choice (keep the existing,
  already-tested `Config::from_env()`, extend rather than replace) over the
  narrower CLI-only design that was recommended but not chosen.
- **Two dependency ecosystems land in one pass** (async runtime + websocket
  + gRPC/protobuf) specifically to catch version conflicts early, per the
  brief's own reasoning — the tradeoff is that this step's `Cargo.lock` diff
  is large and mostly unrelated to anything this step's own code exercises
  yet, which could look like scope creep to a reviewer skimming the diff
  without the project's step-by-step build-order context.

## Testing Strategy

Required real verification:

- Run the real binary (`tests/cli.rs`, via `env!("CARGO_BIN_EXE_...")`) with
  no arguments and no relevant env vars set; confirm exit code 0, the
  startup line on stderr containing `pair=ethbtc port=50051`, and empty
  stdout.
- Run the real binary with `--pair btcusd --port 12345` and confirm the
  logged line reflects both overrides.
- Run the real binary with `KEYROCK_PAIR`/`KEYROCK_PORT` set and no flags;
  confirm the env values show up in the logged line.
- Run the real binary with both a `KEYROCK_PORT` env var and a `--port`
  flag set to different values; confirm the flag's value wins — this is the
  regression test for the actual precedence decision, not just that each
  input source works in isolation.
- Run the real binary with an invalid `--port` value and confirm a non-zero
  exit via clap's own type parser.
- Run the real binary with an invalid `KEYROCK_PORT` and no `--port` flag;
  confirm a non-zero exit and a stderr message naming `KEYROCK_PORT`, via
  `ConfigError::InvalidPort` (unchanged code path).
- `docker compose up --build` end to end: image builds, container starts,
  the startup log line appears in `docker compose logs`, and the container
  exits with status 0.
- `find target -name orderbook.rs` after a build, asserting the generated
  file's contents mention `Summary` and `Level` (can be a `cargo test`
  integration check or a manual step reported back per the brief — either
  way it must be run, not assumed).

Optional supporting checks:

- Unit tests on `src/config.rs`'s new struct (defaults resolve to
  `ethbtc`/`50051` when no CLI overrides are given at the construction call
  site used by the test).
- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`, per
  the repo's standing verification rule — necessary but not sufficient on
  their own; they don't prove the behavior above actually works.

## Rollback Plan

This step only touches `Cargo.toml`/`Cargo.lock`, `src/`, `tests/cli.rs`,
`Dockerfile`, `compose.yml`, `.env.example`, `README.md`, and adds
`proto/orderbook.proto` + `build.rs`. If something breaks acceptance
criteria after landing, `git revert` the single commit this step produces —
there is nothing else layered on top of it yet, so a clean revert restores
the pre-existing `hello`/`doctor` scaffold exactly as it was.

## Open Questions

None currently open. All four raised during drafting were answered on
2026-08-22 and are reflected directly in Proposed Design above:

- **Config shape** → keep `Config::from_env()`, add a `pair` field
  (`KEYROCK_PAIR`), CLI flags (`--pair`/`--port`, both `Option<T>`) override
  the env-sourced value when given. Default `port` becomes `50051`.
- **`ca-certificates`** → drop it from the Dockerfile runtime stage now.
- **`.env.example`** → keep it, extended with `KEYROCK_PAIR` and the new
  `50051` default (this follows mechanically once Config-shape was decided
  the way it was — `.env.example` documents variables that are still real).
- **Timeline** → no hard deadline; proceed through all 11 steps of this
  project's build order at a normal pace, latency measurement and polish
  included. No effect on step 0's own scope.
