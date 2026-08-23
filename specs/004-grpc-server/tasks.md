# Tasks: 004-grpc-server

## Task Writing Rules

- Each task should describe a real unit of progress.
- Each task should name the expected files or areas touched.
- Each task should include explicit verification.
- Prefer behavior-level verification over mock-only checks.

## How to Work This List

Work phase by phase, in order, matching `plan.md`'s six phase boundaries —
six commits, one per phase (this spec packet's own commit, per
`.claude/rules/spec-packet.md` and the standing lesson from
`specs/003-step-1-fixes/`, is the *first* commit on this branch, already
landed before this file existed). Before committing a phase, all of its
verification steps must pass; if one doesn't, fix it inside the phase before
moving on — don't defer it to a later phase's cleanup.

**Standing invariants — reaffirm at every phase's close, not just once (per
`spec.md` Invariants and `plan.md` Cross-Cutting Considerations):**

- `src/exchange/binance.rs`, `src/model.rs`, `src/proxy.rs` show zero diff.
  Check this phase-by-phase (`git diff main --stat -- src/exchange/binance.rs
  src/model.rs src/proxy.rs` after every phase), not only at the end — a
  phase whose diff unexpectedly touches one of these three is a stop-and-flag
  condition, raised immediately, not folded into the commit.
- The watch channel's `Option<Summary>::None` case is always filtered out of
  the outgoing stream, never rendered as a default/zeroed `Summary`.
- `Level.exchange` in the fake writer is always the literal string `"fake"`
  — never `"binance"` or any other value.
- `proto/orderbook.proto` is never edited.
- No merge logic, no Bitstamp, no reconnection/staleness handling, no real
  `aggregator.rs` — all later steps.

---

## Phase 1: Dependencies + `build.rs` + reflection plumbing

No `src/` changes in this phase — isolates the one genuine version-resolution
risk (`tonic-reflection`) from any server logic, so a `cargo build` failure
here is unambiguously a dependency problem.

### 1.1 Add `tonic-reflection` as a dependency
- Files or areas: `Cargo.toml`, `Cargo.lock`
- Change: `cargo add tonic-reflection`. Do not hand-pin a version — let
  cargo resolve it against the already-pinned `tonic`/`tonic-prost` `0.14.6`.
- Verification:
  - `cargo build` succeeds.
  - Read back the resolved version from `Cargo.toml`/`Cargo.lock` and report
    it explicitly (expected `0.14.6` per `spec.md`'s Current State research;
    if it differs, that's a note for the closing design-decisions list, task
    6.4 — not a silent surprise).
- Done when: `tonic-reflection` is a locked, resolved dependency and the
  actual version is reported in this phase's commit message or a scratch
  note carried forward to task 6.4.

### 1.2 Emit a file descriptor set from `build.rs`
- Files or areas: `build.rs`
- Change: Replace the bare `tonic_prost_build::compile_protos("proto/orderbook.proto")`
  call with `tonic_prost_build::configure().file_descriptor_set_path(...)`
  before compiling — path derived from `std::env::var("OUT_DIR")`, not
  hardcoded, matching how the rest of the generated code is emitted. Do not
  touch `proto/orderbook.proto` itself.
- Verification:
  - `cargo build` succeeds.
  - `find target -iname "*.bin"` (or whatever filename the descriptor set
    path actually uses) under `OUT_DIR` shows a real file on disk, confirmed
    by running the `find` and reading its output — not assumed from the code
    alone.
  - Inspection: does this build require `protoc` installed on the host, now
    that `configure()` is used instead of the bare function call? Run
    `cargo clean && cargo build` and report whether it succeeded without a
    system `protoc`, or whether one was already present — state which,
    explicitly, per CLAUDE.md's Trap 7 (this carries into task 6.4's closing
    report either way).
- Done when: a real file descriptor set file exists under `OUT_DIR` after a
  build, proven by `find`, and the `protoc` question above has an explicit
  answer recorded (not skipped).

### 1.3 Full green check for Phase 1
- Files or areas: none — verification-only task
- Change: none.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`,
    `cargo fmt --check` — all clean.
  - `cargo test` — green, same count as `main`'s tip (this phase adds no
    tests; nothing yet consumes the reflection plumbing).
  - `git diff main --stat -- src/exchange/binance.rs src/model.rs src/proxy.rs`
    — empty.
- Done when: all checks above pass with zero warnings and zero invariant
  violations.

**Commit boundary:** `Cargo.toml`, `Cargo.lock`, `build.rs`. Nothing in `src/`.

---

## Phase 2: `src/server.rs` — `OrderbookAggregator`, watch channel, fake writer

The core of this step. `main.rs` still drives only the Binance feed at the
end of this phase — `server.rs` is built and proven to compile in isolation.

### 2.1 Add the `BookSummaryStream` type and the trait impl
- Files or areas: `src/server.rs` (new)
- Change: Define
  `type BookSummaryStream = Pin<Box<dyn Stream<Item = Result<Summary, Status>> + Send>>;`
  and a struct implementing the generated `OrderbookAggregator` trait's
  `book_summary` method, built on
  `WatchStream::new(rx).filter_map(|opt| opt.map(Ok))` (or an exactly
  equivalent adapter chain) over a `watch::Receiver<Option<Summary>>` held by
  the struct. The `None` case must be discarded by this `filter_map` (or
  equivalent combinator) — never by a hand-rolled poll loop reimplementing
  `Stream`, and never substituted with a default/zeroed `Summary`.
- Verification:
  - `cargo build` succeeds once `src/lib.rs` gains `pub mod server;` (task
    2.4) — acceptable to land 2.1-2.4 together in one working tree state
    before committing.
  - Inspection: `grep -n "filter_map\|WatchStream" src/server.rs` shows the
    combinator-based approach; no hand-rolled `impl Stream for ...` exists
    anywhere in this file.
- Done when: the trait impl compiles against the generated
  `OrderbookAggregator` trait and its associated `BookSummaryStream` type,
  and the `None`-filtering is implemented via `filter_map`/`WatchStream`, not
  a manual poll loop.

### 2.2 Add a standalone, reusable `serve` entry point
- Files or areas: `src/server.rs`
- Change: Add an async function (e.g.
  `pub async fn serve(addr: SocketAddr, rx: watch::Receiver<Option<Summary>>) -> anyhow::Result<()>`
  or an equivalent signature) that builds a `tonic::transport::Server` with
  both the `OrderbookAggregatorServer` (wrapping task 2.1's struct) and the
  reflection service (via
  `tonic_reflection::server::Builder::configure().register_encoded_file_descriptor_set(...)`
  reading phase 1's file descriptor bytes through
  `include_bytes!(concat!(env!("OUT_DIR"), "/<name>.bin"))`, then
  `.build_v1()`) registered, and calls `.serve(addr)`. This function must be
  the *only* place server construction happens — `main.rs` (phase 3) and
  `tests/grpc.rs` (phase 4) both call this same function with different
  addresses, per `plan.md`'s explicit seam requirement. If this split turns
  out not to be cleanly achievable (e.g. the fake writer's lifecycle can't be
  separated from the server's), stop and flag it per `plan.md`'s Expected
  Drift Triggers rather than writing divergent test-only server-construction
  code.
- Verification:
  - `cargo build` succeeds.
  - Inspection: exactly one function in the crate builds a
    `tonic::transport::Server` for this service — confirm no duplicate
    construction logic exists or is planned for `tests/grpc.rs`.
- Done when: `serve(addr, rx)` exists, is `pub`, and is the single seam both
  `main.rs` and the integration test will call.

### 2.3 Add the fake-data generator task
- Files or areas: `src/server.rs`
- Change: Add a function (called via `tokio::spawn` from wherever the watch
  channel's sender lives — `main.rs` in phase 3, but the generator function
  itself belongs here) using `tokio::time::interval` at 1 second. Each tick,
  build a `Summary` with exactly 10 `Level`s per side, prices clustered
  around the `0.0315` ETHBTC scale, a small positive spread consistent with
  best-bid < best-ask, and `Level.exchange` set to the literal string
  `"fake"` at every construction site — never `"binance"` or any other
  value. Send `Some(summary)` into the `watch::Sender<Option<Summary>>`; the
  task never reads the receiver back.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings` clean.
  - Inspection: `grep -n '"fake"' src/server.rs` shows the literal at every
    `Level` construction site in this generator; `grep -n '"binance"'
    src/server.rs` returns nothing.
- Done when: the generator compiles, produces a 10/10-level `Summary` with a
  positive spread every tick, and every `Level.exchange` field is the
  literal `"fake"`.

### 2.4 Wire `src/lib.rs`
- Files or areas: `src/lib.rs`
- Change: Add `pub mod server;` alongside the existing module declarations —
  do not reorder or remove any existing `pub mod` line.
- Verification: `cargo build` succeeds with `src/server.rs` reachable as
  `keyrock_case_study::server::...`.
- Done when: `src/server.rs` is a compiling, reachable module.

### 2.5 Full green check for Phase 2
- Files or areas: none — verification-only task
- Change: none.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`,
    `cargo fmt --check` — all clean.
  - `cargo test` — green, same count as Phase 1's close (no new tests this
    phase; `tests/grpc.rs` lands in phase 4).
  - `git diff main --stat -- src/exchange/binance.rs src/model.rs src/proxy.rs`
    — empty.
  - Manual check, if practical without a running `main`: is there any way to
    exercise `serve()` live at the end of this phase? There is no running
    binary yet (main.rs still only drives the feed) — if a manual
    `grpcurl` check is deferred to phase 3/4, say so explicitly in this
    phase's report rather than claiming a check that didn't happen.
- Done when: all checks above pass, and the manual-verification status is
  reported honestly (deferred vs. actually run).

**Commit boundary:** `src/server.rs`, `src/lib.rs`. Reverting this phase
(with phase 1 still in place) leaves the reflection plumbing landed but
unused — still buildable.

---

## Phase 3: `src/main.rs` — three-task supervision

### 3.1 Move the existing feed loop into its own spawned task
- Files or areas: `src/main.rs`
- Change: Extract the existing Binance connect-and-read loop (unchanged in
  behavior) into its own `async fn`, and spawn it via `tokio::spawn`,
  capturing the returned `JoinHandle`. Do not change what it connects to,
  parses, or logs.
- Verification: covered by task 3.4 (running it end to end is the real
  proof).
- Done when: the feed loop runs inside a spawned task with its `JoinHandle`
  retained (not dropped), and its logic is otherwise byte-for-byte the same
  as before this task.

### 3.2 Create the watch channel and spawn the fake writer and the server
- Files or areas: `src/main.rs`
- Change: Create the `watch::channel::<Option<Summary>>(None)` pair once in
  `main`. Spawn phase 2.3's fake-writer function with the sender half,
  capturing its `JoinHandle`. Spawn phase 2.2's `serve(addr, rx)` with the
  receiver half and a `SocketAddr` built from `Config`'s `host`/`port`,
  capturing its `JoinHandle`. Three `tokio::spawn` call sites total in this
  file, no more, no fewer.
- Verification: covered by task 3.4.
- Done when: exactly three `tokio::spawn` calls exist in `src/main.rs`, each
  with its `JoinHandle` bound to a named variable (not dropped/discarded).

### 3.3 Add the `select!` supervision with an inline rationale comment
- Files or areas: `src/main.rs`
- Change: `tokio::select!` over all three `JoinHandle`s from 3.1/3.2;
  whichever completes first, the process exits (propagating that task's
  `Result`/panic). Add an inline comment at the `select!` site stating why:
  a live server behind a dead feed is worse than no server, since it would
  keep answering `BookSummary` with stale data under a "still live"
  appearance — this reasoning must live in code, not only in `spec.md`.
- Verification: covered by task 3.4.
- Done when: `main` contains exactly one `select!` covering all three
  handles, with the rationale comment present; no sequential `.await`s on
  the handles exist anywhere else in the file.

### 3.4 Run it end to end and confirm concurrent behavior
- Files or areas: none — verification-only task
- Change: none.
- Verification:
  - `cargo run -- --pair ethbtc` — network permitting (report plainly if
    this environment can't reach Binance directly or via a configured
    proxy, per the honesty requirement carried from phase 5/6) — confirm
    Binance book-update log lines and the server accepting connections
    happen concurrently in the same process's logs. If a `grpcurl` binary is
    available, run `grpcurl -plaintext localhost:50051 list` against this
    live run and report actual output.
  - `cargo test` — green, same count as Phase 2's close (no new tests this
    phase per the build order's "wiring-only phases are verified by
    running them" convention).
  - `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` —
    clean.
  - Inspection: exactly three `tokio::spawn` call sites and one `select!`
    covering all three `JoinHandle`s — confirm by reading the file, not by
    assumption.
- Done when: the live run (or its explicitly reported network limitation)
  and all static checks are done, and — if a live kill is practical to
  stage locally (e.g. temporarily returning early from one spawned task
  during a local-only test run, then reverting) — confirm that ending any
  one task ends the whole process; otherwise confirm this by inspection of
  the `select!` structure and say so explicitly.

### 3.5 Confirm the untouched-files invariant for Phase 3
- Files or areas: none — verification-only task
- Change: none.
- Verification: `git diff main --stat -- src/exchange/binance.rs src/model.rs
  src/proxy.rs` — empty.
- Done when: the diff is confirmed empty.

**Commit boundary:** `src/main.rs`. Reverting this phase (with phases 1-2
still in place) restores the single-task `main.rs` on top of a fully built
but unused `server.rs`.

---

## Phase 4: `tests/grpc.rs`

### 4.1 Add the real-server, real-client, two-message streaming test
- Files or areas: `tests/grpc.rs` (new)
- Change: Bind to an OS-assigned port (`127.0.0.1:0`, reading back the
  actual bound port rather than a fixed one — required because `cargo test`
  runs tests in the same binary concurrently and a fixed port is a flake
  waiting to happen, per the standing convention recorded in
  `specs/002-binance-feed/revisions.md` entry 3). Spawn phase 2.2's `serve`
  on that address with a fake-writer (phase 2.3) feeding its watch channel.
  Connect a real generated `tonic` client to `http://127.0.0.1:{port}`. Call
  `BookSummary(Empty {})`, then take **exactly two** messages off the
  returned stream before asserting anything — one message only proves the
  RPC call returned; two proves it's actually streaming, which is what the
  schema's `returns (stream Summary)` promises. Assert on real content from
  both messages: `bids.len() == 10`, `asks.len() == 10`, `spread > 0.0` — not
  merely "a message arrived." Name the test after the bug it catches (e.g.
  `book_summary_streams_multiple_updates_not_a_single_shot_response`).
- Verification:
  - `cargo test` — this test passes; report its name and each assertion's
    outcome explicitly, not folded into an aggregate "tests pass" summary
    line.
  - Confirm this test requires no network access to Binance or any external
    service — it only drives the fake-data path. Run it with network
    disabled (e.g. `cargo test --offline` for the dependency-resolution
    sense, plus manually confirming no outbound connection attempt in the
    test itself by inspection) and confirm it still passes.
  - Inspection: `grep -n "next()\|\.take(2)\|for _ in 0\.\.2" tests/grpc.rs`
    (or equivalent) confirms exactly two messages are pulled before the
    first assertion — call this line out by name in the phase report, since
    it's the whole point of the test.
- Done when: the test exists, passes deterministically (no fixed port, no
  arbitrary sleep racing the 1-second fake-writer interval — if a short wait
  for the first `Some` value turns out to be necessary because the watch
  channel starts empty, state explicitly which case applied: connecting
  after the first tick already landed, vs. needing to wait for it), and its
  content assertions are on real values, not just presence/count.

### 4.2 Full green check for Phase 4
- Files or areas: none — verification-only task
- Change: none.
- Verification:
  - `cargo test` — full suite green; report the new total test count.
  - `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` —
    clean.
  - `git diff main --stat -- src/exchange/binance.rs src/model.rs src/proxy.rs`
    — empty.
- Done when: all checks pass and the new total test count is reported.

**Commit boundary:** `tests/grpc.rs` only. Reverting this phase alone removes
the integration test without touching any shipped code.

---

## Phase 5: Docker + `compose.yml`

### 5.1 Bind the container to all interfaces and publish the port
- Files or areas: `compose.yml`
- Change: Set `KEYROCK_HOST=0.0.0.0` in the container's `environment:` block.
  Add `127.0.0.1:50051:50051` under `ports:` — host-loopback only, never
  `0.0.0.0:50051:50051`. Confirm — do not re-add — that the existing
  `PROXY_HOST`/`PROXY_PORT` → `HTTP_PROXY`/`HTTPS_PROXY` pass-through block
  (landed in step 1's proxy revision, `003-step-1-fixes`) is still present
  and unshadowed by this edit; read the file after editing and quote the
  surviving block.
- Verification:
  - `docker compose build` succeeds.
  - `grep -n "PROXY_HOST\|PROXY_PORT\|HTTP_PROXY\|HTTPS_PROXY" compose.yml`
    shows the pass-through block unchanged from before this task.
- Done when: `compose.yml` sets `KEYROCK_HOST=0.0.0.0`, publishes
  `127.0.0.1:50051:50051`, and the proxy pass-through block is confirmed
  intact by the grep above.

### 5.2 Correct the header comment describing container behavior
- Files or areas: `compose.yml`
- Change: Replace any comment describing "defaults, logs, exits 0" with one
  describing the actual new behavior — the gRPC server keeps the container
  running; it does not exit promptly.
- Verification: manual read-through of `compose.yml`'s header/inline
  comments — no "exits 0" language remains.
- Done when: `grep -n "exits 0\|exit 0" compose.yml` returns nothing.

### 5.3 Run and honestly report the live Docker verification
- Files or areas: none — verification-only task
- Change: none.
- Verification:
  - `docker compose build` — succeeds; report actual output.
  - `docker compose up` — **report exactly what was observed in this
    environment, not the intended behavior.** Three possible honest
    outcomes, and the report must state which one actually happened:
    (a) Docker daemon unavailable in this environment — state this plainly,
    do not attempt to infer a pass from the code change alone.
    (b) Docker available, container starts, but exits shortly after because
    the feed task can't reach Binance (no route, no proxy configured) — this
    is the `select!` design in phase 3 working as intended, not a defect;
    report it as such, and report that `grpcurl` therefore could not be
    exercised against a live container in this environment.
    (c) Docker available, container reaches Binance (directly or via a
    configured `PROXY_HOST`/`PROXY_PORT`), stays up — run
    `grpcurl -plaintext 127.0.0.1:50051 list` from the host and quote its
    actual output (expect `orderbook.OrderbookAggregator` and the
    reflection service listed).
  - This is a named, non-negotiable reporting requirement for this branch —
    do not mark this task done by inference; mark it done by stating which
    of (a)/(b)/(c) actually happened, with the actual command output or
    error quoted.
- Done when: the actual observed outcome — one of (a), (b), or (c) above —
  is reported with real command output/errors, not assumed.

### 5.4 Full green check for Phase 5
- Files or areas: none — verification-only task
- Change: none.
- Verification:
  - `docker compose build` succeeds.
  - `git diff main --stat -- src/exchange/binance.rs src/model.rs src/proxy.rs`
    — empty.
- Done when: build succeeds and the diff is confirmed empty.

**Commit boundary:** `compose.yml` only.

---

## Phase 6: `README.md`

### 6.1 Update the build-order table and add the server section
- Files or areas: `README.md`
- Change: Build-order table's step 2 row moves from "Not started" to "Done."
  Add or extend a section describing the server: streams placeholder/fake
  data (`Level.exchange == "fake"`), not yet wired to the real feed (that's
  step 3 of the build order). Add `src/server.rs` to the Layout tree.
- Verification: manual read-through — the build-order table and Layout tree
  match the actual repository state (`find src -name "*.rs" | sort` against
  the Layout tree).
- Done when: both the table and Layout tree are accurate.

### 6.2 Add the `grpcurl` reflection one-liner and correct the Docker section
- Files or areas: `README.md`
- Change: Add `grpcurl -plaintext localhost:50051 list` with a note that
  reflection means no local `.proto` file or import path is needed. Correct
  the Docker section to describe what phase 5's task 5.3 actually observed —
  if the container was confirmed staying up and reachable, say so with the
  real command; if it could only be partially verified in this environment
  (Docker unavailable, or the container exits per the `select!` design),
  state that plainly rather than asserting a full pass. Remove every
  remaining "logs one line and exits 0" phrase.
- Verification:
  - `grep -n "logs.*exits 0\|exits 0" README.md` returns nothing.
  - Manual read-through: confirm the Docker section's claims match exactly
    what task 5.3 reported, not what was merely intended.
- Done when: the `grpcurl` one-liner is present, the Docker section matches
  phase 5's actual reported outcome, and no stale "exits 0" language remains.

### 6.3 Add the reflection limitation note
- Files or areas: `README.md`
- Change: Add a short note that `tonic-reflection` is unconditionally
  enabled in this build and should be gated behind a feature flag or config
  toggle for a production-facing deployment — this is a documented
  limitation, not a code change made here.
- Verification: manual read-through — the note is present and named as a
  limitation, not silently omitted.
- Done when: the note exists in the README.

### 6.4 Produce the closing design-decisions / discoveries list
- Files or areas: none — this is a reporting deliverable, not a file edit
  (record it in the phase 6 commit message, or as a short section in
  `README.md`'s design-decisions area if one already exists — implementer's
  call, but it must be produced and shown to the human owner, not silently
  dropped).
- Change: none (or a README addition, per the note above).
- Verification: produce, at minimum:
  - `WatchStream::new(rx)`'s actual subscribe-on-first-poll behavior
    (reusable verbatim from `spec.md`'s Proposed Design — confirmed against
    `docs.rs` for `tokio-stream` `0.1.19`).
  - `tonic_reflection::server::Builder`'s exact API shape used
    (`configure()`, `register_encoded_file_descriptor_set(&[u8])`,
    `build_v1()` — reusable verbatim from `spec.md`).
  - The actual `tonic-reflection` version `Cargo.lock` resolved to (from
    task 1.1).
  - Whether `protoc` was needed in the builder stage / on the host once
    `configure()` replaced the bare `compile_protos` call (from task 1.2).
  - Anything else discovered during implementation that `spec.md` flagged as
    unverified-until-build (its Risks section), or any genuine surprise in
    the watch/stream plumbing not anticipated by the `None`-filtering or
    `WatchStream` research.
- Done when: this list is produced and delivered to the human owner as part
  of this branch's own deliverables — not deferred past merge.

### 6.5 Run every command README's relevant sections show
- Files or areas: none — verification-only task
- Change: none.
- Verification:
  - Run `cargo run -- --pair ethbtc` and the `grpcurl` one-liner (if
    `grpcurl` is available in this environment; if not, state that
    plainly) and confirm actual output matches what's documented.
  - Run the README's Docker commands and confirm actual output matches
    exactly what section 6.2 now documents — including any partial-only
    verification it states.
- Done when: every command shown in the updated sections produces output
  consistent with what's documented, with no claim of a check that wasn't
  actually run in this environment.

**Commit boundary:** `README.md` only.

---

## Final Verification

Before considering this branch ready to hand off:

- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` — all clean at the tip of the branch; report the final
  test count.
- `tests/grpc.rs` passes, and its two-message, content-asserting shape is
  confirmed by reading the test file (task 4.1), not just by a green
  `cargo test` summary line.
- `grpcurl -plaintext localhost:50051 list` shows `orderbook.OrderbookAggregator`
  when run against a live local `cargo run` instance, with no local `.proto`
  file present — quote the actual command output. This is the most
  representative real behavior path for this step: a reviewer starting the
  server and discovering its schema purely through reflection, then
  streaming fake `Summary` values with no client code of their own.
- `git diff main --stat` at the tip of the branch shows zero diff for
  `src/exchange/binance.rs`, `src/model.rs`, `src/proxy.rs`.
- `docker compose build` succeeds; `docker compose up` + `grpcurl` from the
  host is either genuinely exercised with real output reported, or its
  environment limitation (no Docker daemon, no route to Binance, or the
  container exiting per the `select!` design before `grpcurl` could run) is
  stated plainly — this is a named, non-negotiable reporting requirement per
  the human owner's explicit ask for the same honesty given in the step 1
  ping/pong report (`specs/002-binance-feed/tasks.md` task X.1).
- The closing design-decisions/discoveries list (task 6.4) has been produced
  and delivered, not deferred past merge.
- Any item this environment genuinely could not verify (Docker daemon
  availability, Binance/proxy reachability from inside the container for the
  live `grpcurl` check) is listed explicitly here, one line each, stating
  what was and wasn't checked — do not let a partial verification silently
  read as a full pass anywhere in this document or the README.
- `git log --graph` shows this spec packet's commit as the first commit on
  the branch, six phase commits after it in order, and — once merged — a
  `--no-ff` merge commit on `main` (`git merge --no-ff 004-grpc-server`; do
  not `git merge` without `--no-ff`).
