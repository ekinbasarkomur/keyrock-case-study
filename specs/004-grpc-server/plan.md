# Plan: 004-grpc-server

## Summary

Six phases, following the natural dependency order the spec's own Proposed
Design already implies: the version-resolution risk (`tonic-reflection`,
`file_descriptor_set_path`) lands alone first so a `cargo build` failure
there is never entangled with server logic; `src/server.rs` — the actual
core of this step — lands second, built so its server-construction path is
callable both from `main.rs` and from a test, since phase 4's integration
test needs to start a real server on port 0 without going through the CLI;
`src/main.rs`'s three-task `select!` supervision lands third, once there is
a real server and fake writer to spawn; `tests/grpc.rs` lands fourth, once
phase 2/3 give it something real to drive; Docker lands fifth, once the
server that `grpcurl` and `docker compose up` are meant to prove actually
exists and streams something; `README.md` lands last, describing what
phases 1-5 actually shipped, not what was planned — this is a standing rule
from `specs/003-step-1-fixes/revisions.md` entry 1, not new for this step.
Every phase's diff is checked against the three files this step must not
touch (`src/exchange/binance.rs`, `src/model.rs`, `src/proxy.rs`); if any
phase's diff needs to touch one of those, that's a stop-and-flag condition,
not something to fold in silently. Two deliverables specific to this branch
sit outside the phase-by-phase code changes and are called out in their own
sections below: the closing design-decisions list the human owner asked
for, and honest reporting of what could not be verified live in this
environment (Docker daemon availability, real Binance reachability).

## Phase Breakdown

### Phase 1: Dependencies + build.rs + reflection plumbing

- Objective: Land the one genuine version-resolution risk this step
  carries — `tonic-reflection` as a new dependency, and `build.rs` emitting
  a file descriptor set — in isolation, before any server code depends on
  either existing, so a `cargo build` failure here is unambiguously a
  dependency-resolution problem, not mixed in with new server logic.
- Main changes: `Cargo.toml` gains `tonic-reflection` (expected to resolve
  to `0.14.6` per spec.md's Current State research, but not yet proven by
  an actual `cargo add`/`cargo build`). `build.rs` gains
  `tonic_prost_build::configure().file_descriptor_set_path(...)` ahead of
  `compile_protos`, writing the descriptor set under `OUT_DIR` (via
  `std::env::var("OUT_DIR")`) rather than a hardcoded path, matching how
  the rest of the generated code is already emitted. No `src/` changes in
  this phase — nothing consumes the descriptor set yet.
- Verification:
  - `cargo build` succeeds with the new dependency actually resolved —
    report the version `Cargo.lock` actually pins, not the version
    predicted in spec.md; if it differs, that's worth a note in the
    closing design-decisions list, not a silent surprise.
  - `find target -name "*.bin"` (or whatever filename the descriptor set
    path uses) under `OUT_DIR` confirms the file descriptor set is actually
    emitted, not just that the build.rs change compiles.
  - `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`
    clean.
  - Inspection: does `cargo build` need `protoc` installed on the host for
    this to succeed, now that `configure()` is used instead of the bare
    `compile_protos` call? Re-confirm CLAUDE.md's Trap 7 hasn't flipped —
    report whether protoc was needed here, since a `configure()`-based
    build can behave differently from the plain function call.
- Done looks like: the crate builds with `tonic-reflection` as a resolved,
  locked dependency, and a real file descriptor set exists on disk after a
  build — proven by inspection, not assumed from the code alone.
- Commit boundary: `Cargo.toml`, `Cargo.lock`, `build.rs`. Reverting this
  phase alone restores step 1's pre-reflection build pipeline exactly, with
  no other file depending on its absence yet.

### Phase 2: `src/server.rs` — `OrderbookAggregator`, watch channel, fake writer

- Objective: The core of this step. Implement the generated
  `OrderbookAggregator` trait against a `watch::Receiver<Option<Summary>>`,
  register the reflection service alongside it, and add the once-a-second
  fake-data writer task — all provably correct in isolation from `main.rs`,
  which still drives only the Binance feed at the end of this phase.
- Main changes: `src/server.rs` (new) —
  - The `BookSummaryStream` type alias
    (`Pin<Box<dyn Stream<Item = Result<Summary, Status>> + Send>>`) and the
    trait impl, built on `WatchStream::new(rx).filter_map(|opt| opt.map(Ok))`
    per spec.md's `None`-filtering decision — never a fabricated
    zero-value `Summary` for the pre-first-tick case.
  - A function that builds a `tonic::transport::Server` (with the
    `OrderbookAggregatorServer` and the reflection service both registered)
    given a `watch::Receiver<Option<Summary>>` and a bind address, callable
    on its own — this is the seam phase 4's test needs, so the plan
    specifies it explicitly here: server construction/serving should be a
    standalone async function (e.g. `pub async fn serve(addr: SocketAddr,
    rx: watch::Receiver<Option<Summary>>) -> anyhow::Result<()>` or
    equivalent), not logic inlined only inside `main`. `main.rs` (phase 3)
    and `tests/grpc.rs` (phase 4) both call this same function with
    different addresses (a real config-derived one vs. `127.0.0.1:0`).
  - The fake-data generator task: `tokio::spawn`ed, `tokio::time::interval`
    at 1 second, 10 `Level`s per side, prices clustered around the
    `0.0315` ETHBTC scale, small positive spread, `Level.exchange ==
    "fake"` unconditionally (literal string, never `"binance"`) — sends
    into the `watch::Sender<Option<Summary>>`, never reads it back.
  - `src/lib.rs` gains `pub mod server;`.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`,
    `cargo fmt --check` clean.
  - Manual `grpcurl -plaintext localhost:PORT list` and
    `.../BookSummary` against a small throwaway `main`-less run (or wait
    for phase 3/4 to exercise this live — if this phase's own verification
    is deferred to phase 3/4 because there's no running binary yet at the
    end of phase 2, say so explicitly rather than claiming a manual check
    that didn't happen).
  - Inspection: confirm `filter_map` (or equivalent) is what discards
    `None`, not a hand-rolled poll loop reimplementing `Stream`; confirm
    the fake writer's `Level.exchange` field is the literal `"fake"` at
    every call site that constructs one.
- Done looks like: `src/server.rs` compiles, exposes a single reusable
  `serve`-style entry point that both `main.rs` and the integration test
  can call, and no other module changes yet.
- Commit boundary: `src/server.rs`, `src/lib.rs`. Reverting this phase
  (with phase 1 still in place) leaves the reflection plumbing landed but
  unused — buildable, since nothing yet calls into `server.rs`.

### Phase 3: `src/main.rs` — three-task supervision

- Objective: Wire the existing Binance feed loop, the new fake writer, and
  the new server into three `tokio::spawn`ed tasks under one
  `tokio::select!`, so the process exits the instant any one of them ends —
  per spec.md's explicit design reasoning (a live server behind a dead feed
  is worse than no server).
- Main changes: `src/main.rs` — the existing feed read loop is moved,
  unchanged in behavior, into its own `async fn` and spawned rather than
  awaited directly in `main`'s own task. The fake writer (constructed in
  phase 2's `server.rs`, or spawned here calling into a function `server.rs`
  exposes — the plan leaves the exact split to implementation, but the
  `watch` channel's sender/receiver pair is created once in `main` and
  passed to both the fake-writer spawn and the server-serve spawn) and the
  server's `serve` function (from phase 2) each become their own spawned
  task. `main` then `tokio::select!`s over all three `JoinHandle`s and
  returns/propagates on whichever completes first — with an inline comment
  at the `select!` site explaining why (per spec.md's Proposed Design
  requirement that this reasoning live in code, not only in the spec).
- Verification:
  - `cargo run -- --pair ethbtc` (network permitting — see the honesty
    note in Verification Gates below) shows Binance book lines and the
    server accepting connections concurrently in the same process's logs.
  - `cargo test` still green — no new tests this phase per the build
    order's own convention (wiring-only phases are verified by running
    them, not by new unit tests).
  - `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`
    clean.
  - Inspection: exactly three `tokio::spawn` call sites, one `select!`
    covering all three `JoinHandle`s, no sequential `.await`s and no
    dropped/detached handles.
- Done looks like: killing any one of the three tasks (e.g. by having the
  feed disconnect, or manually returning early from one task during a local
  test) ends the whole process — confirmed by observation if practical, or
  by inspection of the `select!` structure if a live kill isn't practical
  to stage.
- Commit boundary: `src/main.rs`. Reverting this phase (with phases 1-2
  still in place) restores the single-task `main.rs` from step 1, on top of
  a fully built but unused `server.rs`.

### Phase 4: `tests/grpc.rs`

- Objective: Prove the `stream` contract is actually honored — not just
  that one message arrives — via a real server, real client, real TCP/HTTP2
  connection, on an OS-assigned port.
- Main changes: `tests/grpc.rs` (new) — binds to port 0 (or lets
  `tonic::transport::Server` bind and reads back the OS-assigned port),
  starts phase 2's `serve` function on a spawned task with a fake-writer
  feeding its watch channel, connects a real generated `tonic` client to
  `http://127.0.0.1:{port}`, calls `BookSummary(Empty {})`, and takes
  **exactly two** messages off the stream before asserting — per spec.md's
  explicit reasoning that one message only proves the call returned, two
  proves it's actually streaming. Assertions: `bids.len() == 10`,
  `asks.len() == 10`, `spread > 0.0` (or whatever positive-spread
  assertion matches the fake generator's actual output) — not merely "a
  message arrived."
- Verification:
  - `cargo test` green, with this test's pass/fail reported explicitly
    (test name, assertion outcomes) rather than folded into an aggregate
    "tests pass" claim.
  - Confirm this test does not depend on network access to Binance or any
    external service — it only exercises the fake-data path, so it should
    be runnable in any environment including one with no outbound network.
  - Inspection: the test takes exactly two messages, not one, off the
    stream before its assertions — this is the one line in the test file
    worth calling out by name when reporting phase completion, since it's
    the whole point of the test.
- Done looks like: `cargo test` includes this test, it passes deterministically
  (no fixed port, no timing-dependent sleep racing the 1-second fake-writer
  interval — the `WatchStream` subscribe-on-first-poll behavior researched
  in spec.md means the test doesn't need to wait for the first tick if the
  watch already holds a value by the time it connects, but if the fake
  writer hasn't ticked yet, the test may need to tolerate a short wait for
  the first `Some` — call out during implementation which case applies).
- Commit boundary: `tests/grpc.rs`. Reverting this phase alone removes the
  integration test without touching any shipped code — a safe, independent
  revert.

### Phase 5: Docker + compose.yml

- Objective: Make the container reachable from the host — `ORDERBOOK_HOST`
  bound to all interfaces inside the container, the port published on
  loopback only, and the "logs one line and exits 0" description corrected
  to "stays running" — landing after the server actually exists and works,
  so this phase's own verification (`grpcurl` reaching the container) has
  something real to hit rather than testing plumbing around an empty
  server.
- Main changes: `compose.yml` — set `ORDERBOOK_HOST=0.0.0.0` inside the
  container's environment; add `127.0.0.1:50051:50051` under `ports:`
  (host-loopback only, never `0.0.0.0:50051:50051`); correct the header
  comment describing "defaults, logs, exits 0" to describe the new
  stays-running behavior. Explicitly verify — not re-add — that the
  existing `PROXY_HOST`/`PROXY_PORT` → `HTTP_PROXY`/`HTTPS_PROXY`
  pass-through block (landed in step 1's proxy revision) is still present
  and unshadowed by this phase's edits.
- Verification:
  - `docker compose build` succeeds.
  - `docker compose up` — **report actual, observed behavior in this
    environment, not the intended behavior.** If this environment has a
    working Docker daemon and outbound network access to Binance (directly
    or via a configured `PROXY_HOST`/`PROXY_PORT`), confirm the container
    stays up and `grpcurl -plaintext 127.0.0.1:50051 list` from the host
    succeeds — quote the actual `grpcurl` output. If Docker is unavailable
    in this environment, or the container can reach Binance but the
    three-task `select!` design means it exits shortly after because the
    feed can't connect (per spec.md's explicit "expected, not a defect"
    framing), report exactly which of those happened and why `grpcurl`
    could or couldn't be run — do not report this gate as passed if it
    wasn't actually exercised.
  - `grep` (or read) `compose.yml` to confirm the proxy pass-through block
    survived this phase's edits unmodified.
- Done looks like: `compose.yml` reflects the three changes; the actual
  Docker verification outcome (pass, or specifically what couldn't be
  checked and why) is reported honestly rather than assumed from the code
  change alone.
- Commit boundary: `compose.yml`. Reverting this phase alone restores the
  no-`ports:`, loopback-bound container from step 1, on top of a fully
  working server the container-internal binary would still run — just
  unreachable from the host.

### Phase 6: `README.md`

- Objective: Describe what phases 1-5 actually shipped, once there is real
  behavior to document — per the standing rule from
  `specs/003-step-1-fixes/revisions.md` entry 1 that a step's README lands
  with the step, not after it.
- Main changes: `README.md` — build-order table's step 2 row moves to
  "Done"; a new/extended section describing the server (streams
  placeholder/fake data, `Level.exchange == "fake"`, not yet wired to the
  real feed — that's step 3); the `grpcurl -plaintext localhost:50051
  list` one-liner with a note that reflection means no local `.proto` file
  is needed; the Docker section corrected to describe the actual observed
  behavior from phase 5 (stays running and reachable at
  `127.0.0.1:50051`, or — if phase 5 could only be verified by
  inspection/partial run in this environment — say that plainly rather
  than asserting it was confirmed); `src/server.rs` added to the Layout
  tree; a short limitation note that reflection is unconditionally on in
  this build and should be gated behind a flag for a production-facing
  deployment.
- Verification:
  - Manually run every command the README's Quick Start / gRPC sections
    show and confirm actual output matches what's documented, to the
    extent this environment allows (see phase 5's honesty note — the
    README should not claim a check succeeded that this environment
    couldn't actually run).
  - Read-through: no leftover "logs one line and exits 0" language
    remains anywhere in the README once this phase lands.
- Done looks like: every claim in the updated sections matches what phases
  1-5 actually produced and actually verified in this environment — not
  what was planned.
- Commit boundary: `README.md` alone. Reverting it has no effect on build
  or test state.

## Cross-Cutting Considerations

- **Untouched-files invariant, checked per phase and again at the end.**
  `src/exchange/binance.rs`, `src/model.rs`, and `src/proxy.rs` must show
  zero diff throughout this branch. This is checked via `git diff main
  --stat` at the end of the branch, but each phase's own diff should also
  be eyeballed before its commit — a phase whose diff unexpectedly touches
  one of these three files is a stop-and-flag condition to raise
  immediately, not something to fold into the commit and explain later.
- **The `None`-filtering decision is fixed, not re-litigated.** Every phase
  that touches the watch channel (phases 2-4) must filter `Option::None`
  out of the outgoing stream rather than substituting a
  default/zero-valued `Summary`. This is the same decision spec.md
  establishes as the precedent for step 5/6's all-venues-stale case — a
  phase encountering the `None` case again (e.g. while writing the
  integration test's wait-for-first-tick logic in phase 4) should treat
  the existing decision as settled, not reopen it.
- **The container-exits-on-feed-failure behavior is expected, not a bug.**
  Phase 5's own verification section is where this shows up in practice —
  if `docker compose up` exits shortly after starting because the
  container can't reach Binance, that is the `select!` design in phase 3
  working correctly, not a regression to chase down or fix by adding
  resilience out of scope for this step.
- **Scope discipline.** No phase in this plan adds merge logic, Bitstamp,
  reconnection/staleness handling, or a real `aggregator.rs` — all later
  steps per spec.md's Out of Scope. No phase edits `proto/orderbook.proto`
  — reflection is additive only (a new dependency, a `build.rs` change, and
  server-side registration), never a schema change.
- **`f64` boundary discipline carries forward from step 1.** The fake
  writer in phase 2 constructs `Summary`/`Level` values directly with
  `f64` fields, per the wire format — this is the schema's own boundary,
  not a violation of the money-is-never-a-float rule, since there's no
  internal fixed-point representation being bypassed here (the fake data
  never came from a parsed exchange string). No phase should introduce
  fixed-point arithmetic into the fake generator; that machinery belongs to
  step 5's real merge logic, not this step's placeholder.

## Verification Gates

Before this branch is considered ready to hand off:

- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` all clean at the tip of the branch.
- `tests/grpc.rs` passes, with its two-message, content-asserting shape
  confirmed by reading the test file, not just by a green `cargo test`
  summary line.
- `grpcurl -plaintext localhost:50051 list` shows
  `orderbook.OrderbookAggregator` when run against a live local instance
  (`cargo run`), with no local `.proto` file present — report the actual
  command output.
- `git diff main --stat` at the tip of the branch shows zero diff for
  `src/exchange/binance.rs`, `src/model.rs`, `src/proxy.rs`.
- `docker compose build` succeeds; `docker compose up` + `grpcurl` from the
  host is either genuinely exercised with real output reported, or its
  environment limitation (no Docker daemon, no route to Binance, or the
  container exiting per the `select!` design before `grpcurl` could run) is
  stated plainly rather than reported as passed. This is a named,
  non-negotiable reporting requirement for this branch, per the human
  owner's explicit ask for the same honesty given in the step 1 ping/pong
  report.
- The closing **design decisions / discoveries list** is produced as part
  of this branch's own deliverables (not deferred past merge): at minimum,
  `WatchStream`'s actual subscribe-on-first-poll behavior and the
  `tonic-reflection` API's exact shape (both already researched in
  spec.md and reusable verbatim), plus the actual `tonic-reflection`
  version `Cargo.lock` resolved to (phase 1), whether `protoc` was needed
  in the builder stage (phase 1's inspection check), and anything else
  discovered while implementing that spec.md flagged as unverified-until-
  build (e.g. the version-resolution risk in spec.md's Risks section).
- `git log --graph` shows this spec packet's commit as the first commit on
  the branch, and a `--no-ff` merge commit at the end.

## Expected Drift Triggers

If any of the following becomes true while implementing, update `spec.md`
before continuing rather than improvising past it:

- `cargo add tonic-reflection` in phase 1 resolves to a version other than
  `0.14.6`, or fails to resolve at all against the already-pinned
  `tonic`/`tonic-prost` `0.14.6` — this is the one risk spec.md names as
  verified against metadata but not yet proven by an actual build; a
  resolution conflict here is a real design gap, not a detail to patch
  around silently.
- `protoc` turns out to be required in the Docker builder stage once
  `build.rs` uses `configure().file_descriptor_set_path(...)` instead of
  the bare `compile_protos` call, and it isn't already installed there —
  per CLAUDE.md's Trap 7, this must be confirmed locally, not assumed
  either way.
- Phase 2's server-construction seam turns out not to be cleanly callable
  from both `main.rs` and `tests/grpc.rs` with the same function (e.g. the
  fake-writer's lifecycle can't be cleanly separated from the server's) —
  that's a structural surprise worth flagging rather than working around
  with divergent test-only server-construction code, since the whole point
  of specifying a shared `serve` entry point is that phase 4's test proves
  the same code path `main.rs` runs.
- Phase 5's Docker verification cannot be run at all in this environment
  (no Docker daemon available) — this must be reported as "not verified
  here" in the closing deliverables, not silently omitted or reported as
  passed by inference from the code change alone.
- Any phase discovers a genuine third message-shape or edge case in the
  watch/stream plumbing that spec.md's `None`-filtering and
  `WatchStream`-subscribe-behavior research didn't anticipate — that's a
  design gap to raise, not to guess through, per this step's own emphasis
  on landing the tricky plumbing correctly before step 3 depends on it.
