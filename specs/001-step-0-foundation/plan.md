# Plan: 001-step-0-foundation

## Summary

Land step 0 in four sequential phases, each ending at a state where
`cargo build`/`cargo test` is either green or explicitly not expected to
compile yet, with a commit after each phase. Ordering follows the risk
gradient: the build-pipeline unknown (does `tonic-prost-build` need
`protoc` on this machine?) gets resolved first, before any source file
depends on it; the CLI/Config rewrite — the part with actual behavior to
get wrong — comes once the build pipeline is proven; the container and
docs come last, since they only wrap a binary that already works on the
host. All design decisions (dependency list, `Config` shape, Dockerfile
edits, `.env.example`, README changes) are fixed by `spec.md` — this plan
only sequences landing them.

## Phase Breakdown

### Phase 1: Dependency set + proto + build pipeline
- Objective: Prove the crate resolves its full step-0 dependency graph and
  compiles the brief's proto into Rust, without touching any existing
  source file's behavior yet.
- Main changes: `Cargo.toml`/`Cargo.lock` (via the `cargo add` list in
  spec.md's Proposed Design, run as-is), `proto/orderbook.proto` (copied
  verbatim from the brief), `build.rs` (new, crate root).
  `src/main.rs`/`src/lib.rs`/`src/config.rs`/`tests/cli.rs` are untouched
  in this phase — the existing `hello`/`doctor` scaffold still builds and
  passes exactly as it does today, so a failure here is unambiguously the
  new dependency/proto/build.rs work, not a regression mixed in with it.
- Verification:
  - `cargo build` succeeds.
  - `find target -name orderbook.rs` returns a file whose contents mention
    `Summary` and `Level`.
  - `cargo test` still passes with the original 7 tests (proves this phase
    added nothing that broke the pre-existing scaffold).
  - If `cargo build` fails on a missing `protoc`, resolve per spec.md's
    documented risk (add `protobuf-compiler` to the builder stage in
    Phase 3, not here) and report which case applied — do not guess ahead
    of the failure.
- Done looks like: `Cargo.toml` lists the full dependency set, generated
  code exists under `target/`, and nothing in `src/` references it yet.
- Commit boundary: this phase's diff is `Cargo.toml`, `Cargo.lock`,
  `proto/orderbook.proto`, `build.rs`. Reverting it restores a crate with
  the original four dependencies and no proto pipeline.

### Phase 2: Config, CLI entry point, and test rework
- Objective: Replace the `hello`/`doctor` scaffold with the `--pair`/`--port`
  entry point and the env-var/CLI precedence rule, with full test coverage
  for the new contract.
- Main changes: `src/config.rs` (`pair` field, `ORDERBOOK_PAIR`, default port
  `50051`), `src/main.rs` (flat `Cli` struct, `Option<T>` flags, merge over
  `Config::from_env()`, single `info!` startup line), `src/lib.rs`
  (`greeting()`, its test, and `VERSION` removed), `tests/cli.rs` (replace
  the 4 subcommand-era tests with the 6 cases spec.md's Testing Strategy
  lists: default run, flags-only, env-only, flag-overrides-env precedence,
  invalid `--port`, invalid `ORDERBOOK_PORT`).
- Verification:
  - `cargo test` green against the new test set (exact count per spec.md,
    but zero tests still asserting `hello`/`doctor` behavior).
  - `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`
    clean.
  - Manually run `cargo run --` and `cargo run -- --pair btcusd --port
    12345` once each and read the stderr line, to confirm the logged
    format matches what the tests assert (belt-and-braces before trusting
    the assertions alone).
- Done looks like: the binary has no subcommands, `cargo run -- --pair X
  --port Y` logs a line containing both values on stderr with empty
  stdout, and the precedence test explicitly proves flags beat env vars
  (not just that each input source works alone).
- Commit boundary: this phase's diff is `src/config.rs`, `src/main.rs`,
  `src/lib.rs`, `tests/cli.rs`. Reverting it (with Phase 1 still in place)
  restores the `hello`/`doctor` CLI on top of the new dependency set and
  proto pipeline — a safe, buildable intermediate state.

### Phase 3: Container shape
- Objective: Make the Dockerfile and `compose.yml` build and run the new
  binary end to end, with the dependency-cache stub stage correctly
  covering `build.rs`/`proto/`.
- Main changes: `Dockerfile` (stub stage gains `COPY build.rs Cargo.toml
  Cargo.lock ./` and `COPY proto/ ./proto/` before the stub build; drop the
  `ca-certificates` apt install; `CMD` changes from `["--help"]` to `[]`),
  `compose.yml` (header comment update only — no behavioral change),
  `.env.example` (`ORDERBOOK_PAIR=ethbtc` line added, `ORDERBOOK_PORT` default
  shown as `50051`).
- Verification:
  - `docker compose build` succeeds.
  - `docker compose up --build` brings the container up, `docker compose
    logs` shows the startup line, and the container exits with status 0.
  - If Phase 1 surfaced a missing-`protoc` issue on the host, confirm here
    whether the same issue exists in the `rust:1.97-slim-bookworm` builder
    image, and add `protobuf-compiler` to the builder stage's apt install
    if so — this is the phase where that risk gets closed out either way.
  - Re-run `cargo test` once more (no source changes in this phase, but
    confirms nothing about the Docker edits reached back into `src/`).
- Done looks like: a clean `docker compose up --build` prints the same
  kind of startup log line proven in Phase 2, from inside the container,
  and exits 0 without a `--help` fallback.
- Commit boundary: this phase's diff is `Dockerfile`, `compose.yml`,
  `.env.example`. Reverting it leaves a working host binary (Phases 1-2)
  with the pre-existing container shape still pointed at the old CLI —
  buildable but stale, which is why this phase should not be skipped even
  though it doesn't touch `src/`.

### Phase 4: Documentation
- Objective: Bring `README.md` in line with the shipped CLI, config
  surface, and layout, so it's accurate at the point this step is
  considered done.
- Main changes: `README.md` only — quick start commands, configuration
  table (`ORDERBOOK_PAIR` row added, `ORDERBOOK_PORT` default corrected,
  precedence rule stated), `proto/orderbook.proto` mention under Layout,
  placeholder "What would change for production" heading.
- Verification: manually run every command shown in the updated "Quick
  start" section (`cargo run --`, `cargo run -- --pair ethbtc --port
  50051`, the `docker compose` examples) and confirm the actual output
  matches what the README claims — a README edited without re-running its
  own examples is how docs drift starts.
- Done looks like: no command in the README produces output that
  contradicts what Phases 2-3 already proved.
- Commit boundary: this phase's diff is `README.md` alone. Reverting it
  has no effect on build or test state.

## Cross-Cutting Considerations

- **Commit cadence vs. spec.md's rollback plan.** spec.md's Rollback Plan
  describes step 0 landing as a single commit; the pipeline-level process
  for this packet is to commit after each phase instead, so intermediate
  history has four commits, not one. Whether those get squashed before
  this branch merges is a decision for whoever lands the branch, not this
  plan — each phase's commit message should stand on its own regardless
  (state what changed and why it's safe to commit at that point), since
  squashing is easy but reconstructing intent from a squashed diff is not.
- **`protoc` availability is the one real unknown** and is deliberately
  resolved twice — once for the host in Phase 1, once for the container in
  Phase 3 — because a working host build says nothing about the builder
  image having the same tool available.
- **No stub files.** Per spec.md's Out of Scope, no phase should add
  `src/exchange/`, `src/book.rs`, `src/aggregator.rs`, or `src/server.rs`
  even as empty placeholders — the four phases above are the entire scope
  of this branch.
- **Toolchain pin.** No phase in this plan touches `rust-toolchain.toml` or
  the Dockerfile's `FROM` line; if either turns out to need a change to
  make a phase pass, that's drift (see below), not something to patch
  silently mid-phase.

## Verification Gates

Before this branch is considered ready to hand off:

- `cargo build` succeeds and `find target -name orderbook.rs` contains
  `Summary` and `Level`.
- `cargo test` is green, `cargo clippy --all-targets -- -D warnings` is
  clean, `cargo fmt --check` is clean.
- `docker compose up --build` starts the container, logs the startup line
  (visible via `docker compose logs`), and exits 0.
- Every command shown in the updated `README.md` Quick Start section
  produces output consistent with what's documented.
- `git log` on this branch shows one commit per phase, each independently
  buildable at the point it was made (Phase 1's commit builds and tests
  green on its own; Phase 2's commit builds, tests, and clippy pass on top
  of Phase 1; and so on).

## Expected Drift Triggers

If any of the following becomes true while implementing, update `spec.md`
before continuing rather than improvising past it:

- `tonic-prost-build`'s actual 0.14 API doesn't match the `build.rs` shape
  spec.md proposes (the spec already flags this as a real risk to verify,
  not assume).
- `protoc` is unavailable on the host or in the builder image and needs a
  new system dependency spec.md didn't account for.
- The `cargo add` list resolves a `tokio`/`tonic`/`prost`/
  `tokio-tungstenite` version combination that conflicts, forcing a
  dependency choice spec.md didn't make.
- Any phase turns out to need touching `rust-toolchain.toml`, the
  Dockerfile's `FROM` line, joining the `echo` network, or adding a
  `HEALTHCHECK` — all explicitly out of scope per spec.md.
- The precedence test (flag overrides env var) fails in a way that
  suggests the merge design itself (not just an implementation bug) is
  wrong — that's a config-shape question spec.md already closed, and
  reopening it needs the human, not a silent workaround.
