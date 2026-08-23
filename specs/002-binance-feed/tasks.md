# Tasks: 002-binance-feed

## Task Writing Rules

- Each task should describe a real unit of progress.
- Each task should name the expected files or areas touched.
- Each task should include explicit verification.
- Prefer behavior-level verification over mock-only checks.

## How to Work This List

Work phase by phase, in order. Each phase ends with its own commit — do not
batch multiple phases into one commit (four commits for the four phases,
matching `plan.md`'s commit boundaries). Before committing a phase, its
verification steps must all pass; if one doesn't, fix it inside the phase
before moving on, don't defer it to a later phase's cleanup.

**Standing invariants — reaffirm at every phase's close, not just once (per
`spec.md` Invariants and `plan.md` Cross-Cutting Considerations):** no
`Exchange` trait or cross-exchange abstraction, no `tokio::spawn`, no
`.split()` on the websocket stream, no reconnection/backoff/staleness logic,
no gRPC code (`src/server.rs`, generated `orderbook` types), no stub files or
`todo!()` for anything out of scope (`src/book.rs`, `src/aggregator.rs`), and
`f64` appears in exactly one place across the whole branch — the
`Display` impl in `src/model.rs`.

---

## Phase 1: `src/model.rs` — `Price`, `Amount`, `Book`

No `src/exchange/` or `src/main.rs` change in this phase — the existing
synchronous step-0 `main.rs` is untouched, so a failure here is unambiguously
in the new module.

### 1.1 Add `Price` and `Amount` newtypes
- Files or areas: `src/model.rs` (new)
- Change: Define `Price(i64)` and `Amount(i64)` as distinct newtypes over
  `i64` at fixed scale `1_000_000_000` (1e9). Add a string-parsing
  constructor for each (e.g. `Price::from_str_price` / an equivalent
  associated function) that converts an exchange decimal string
  (`"0.03150000"`) to the integer tick count by multiplying by the scale and
  rounding to the nearest integer — this is the one conversion point where a
  float is permitted transiently during parsing (the *stored* value is
  always the `i64`). Implement `Display` (renders the decimal form at the
  scale's precision, e.g. `0.03150000`) and `Debug` (shows the raw `i64`)
  for both types.
- Change (comment): Add the "why not `f64`" comment above the types, in this
  exact order: (1) the modelling argument — a price is a tick multiple, a
  discrete quantity, not a continuous one, so `f64` is a category error
  before it's a precision one; (2) the measured bound, stated as an honest
  limit on the benefit, not the justification — `0.031505 - 0.031500` in
  `f64` yields a relative error of `~3.9e-13`, invisible at 8 decimals; (3)
  where the error would stop being small — accumulating arithmetic (summing
  many levels, compounding positions) — named explicitly as a case this step
  does not have.
- Verification:
  - `cargo build` succeeds with `src/model.rs` present (even before
    `lib.rs` wires it in, via `cargo check src/model.rs` is not standalone —
    verify after task 1.2 wires the module in).
- Done when:
  - `Price` and `Amount` are distinct types (the compiler rejects passing one
    where the other is expected — confirm by eye, no test needed for this
    specific property since it's a compile-time guarantee).
  - The "why not `f64`" comment is present in the argument order above.

### 1.2 Add `Book` and wire the module into `src/lib.rs`
- Files or areas: `src/model.rs`, `src/lib.rs`
- Change: Define `Book { bids: Vec<(Price, Amount)>, asks: Vec<(Price, Amount)>, last_update_id: u64 }`
  — exchange-agnostic, nothing Binance-specific in this file. Add
  `pub mod model;` to `src/lib.rs`, alongside the existing `pub mod config;
  pub mod telemetry;` (do not reorder or remove the existing two).
- Verification:
  - `cargo build` succeeds.
  - `cargo clippy --all-targets -- -D warnings` clean.
- Done when:
  - `Book` compiles and is reachable as `keyrock_case_study::model::Book`
    from outside the crate (confirmed in task 1.3's test module via
    `use crate::model::...`).

### 1.3 Add the price round-trip test and the precision regression test
- Files or areas: `src/model.rs` (`mod tests`)
- Change: Add two unit tests, per `spec.md`'s Testing Strategy:
  1. Price round-trip: `"0.03150000"` parses to the integer `31500000` and
     `Display`s back as `"0.03150000"`.
  2. A named regression test (e.g. `f64_would_lose_this_precision` or an
     equivalent name that states what it guards against) asserting that
     `0.031505 - 0.031500`, computed entirely in the integer domain (parse
     both strings to `Price`, subtract the underlying `i64`s), equals exactly
     `5000` ticks — so a future change that "simplifies" `Price` to `f64`
     fails this test rather than silently reintroducing drift.
- Verification:
  - `cargo test --lib model::` — both tests pass.
- Done when:
  - Both tests exist in `src/model.rs`'s `mod tests`, are named to state
    intent, and pass.

### 1.4 Full green check for Phase 1
- Files or areas: none — verification-only task
- Change: none.
- Verification:
  - `cargo test` — full suite green, including the two new `model` tests
    (test count is step 0's 8 plus these 2 = 10 at this point).
  - `cargo clippy --all-targets -- -D warnings` clean.
  - `cargo fmt --check` clean.
  - Inspection: `grep -n "f64" src/model.rs` shows `f64` only inside the
    parsing-conversion step and/or the `Display` impl, and `grep -rn "f64"
    src/` outside `src/model.rs` returns nothing (no other file touches
    `f64` yet, since `exchange`/`main` haven't landed).
  - Inspection: no `Exchange` trait, no `tokio::spawn`, no `.split()`, no
    gRPC code, and no `src/exchange/`, `src/book.rs`, or `src/aggregator.rs`
    exist in this phase's diff.
- Done when:
  - All checks above pass with zero warnings and zero invariant violations.

**Commit boundary:** `src/model.rs`, `src/lib.rs`. Nothing else.

---

## Phase 2: `src/exchange/binance.rs` — `parse()`

No live socket involved anywhere in this phase — `parse` is proven as a pure
function against fixtures before Phase 3 ever opens a connection.

### 2.1 Add `src/exchange/mod.rs`
- Files or areas: `src/exchange/mod.rs` (new), `src/lib.rs`
- Change: Create `src/exchange/mod.rs` with a module declaration for
  `binance` (`pub mod binance;`) and nothing else — no trait, no shared
  types beyond re-exporting what `binance.rs` defines if convenient. Add
  `pub mod exchange;` to `src/lib.rs`.
- Verification:
  - `cargo build` succeeds (will fail until task 2.2 creates
    `binance.rs` — acceptable to land 2.1/2.2 together in one working
    tree state before committing).
- Done when:
  - `src/exchange/mod.rs` exists and declares exactly one submodule
    (`binance`) — no `Exchange` trait, no cross-exchange abstraction.

### 2.2 Add the connect URL, message-variant dispatch shape, and `parse()`
- Files or areas: `src/exchange/binance.rs` (new)
- Change:
  - A connect-URL constant or builder:
    `wss://stream.binance.com:9443/ws/{pair}@depth20@100ms`, pair lowercased.
  - `pub fn parse(text: &str) -> Option<model::Book>`: deserializes the
    Binance payload (`lastUpdateId`, `bids`, `asks` as `Vec<[String; 2]>`)
    via `serde_json`, converts each price/amount string through
    `model::Price`/`Amount`'s string-parsing constructor, and returns
    `Some(Book)`. Any message that isn't a recognizable book payload
    (missing fields, `{"e":"serverShutdown",...}`, malformed JSON) returns
    `None` — logged at `tracing::debug!`, never a hard error. `parse` must
    not use `?` on a `Result` that would propagate past this function — no
    panics, no error propagation to a caller that expects every message to
    be a book.
  - The message-variant dispatch (`Message::Text` → `parse`;
    `Message::Ping`/`Pong` → ignored; `Message::Close` → signal to break the
    read loop; `Message::Binary` → `debug`-logged and skipped) can be
    written here as a function taking a `Message` and returning the
    dispatch decision, even though it isn't driven by a live loop until
    Phase 3 — this task proves the shape compiles and is unit-testable
    where relevant, not that it runs against a socket.
- Verification:
  - `cargo build` succeeds.
  - `cargo clippy --all-targets -- -D warnings` clean.
- Done when:
  - `parse(text: &str) -> Option<model::Book>` exists, returns `Option`
    (never `Result`), and is the only place in this file that touches
    `model::Price`/`Amount` parsing.

### 2.3 Add the real-fixture 20/20 parse test
- Files or areas: `src/exchange/binance.rs` (`mod tests`)
- Change: Embed a real captured Binance `depth20` payload as a string
  literal fixture (capture one manually, e.g. via `websocat` or a short
  throwaway script against the live endpoint, or from Binance's published
  API docs sample if a genuine capture isn't practical — state which source
  was used in the test's doc comment). Add a test asserting `parse(fixture)`
  returns `Some(Book)` with exactly 20 bids and 20 asks, and assert actual
  converted `Price`/`Amount` values for at least the first bid and first ask
  against the source strings in the fixture (not just lengths/counts).
- Verification:
  - `cargo test --lib exchange::binance::` — this test passes.
- Done when:
  - The test is present, uses a real (not synthetic/hand-typed) Binance
    payload shape, and asserts on actual values, not just counts.

### 2.4 Add the `serverShutdown` and malformed-JSON tests
- Files or areas: `src/exchange/binance.rs` (`mod tests`)
- Change: Add two tests:
  1. `parse(r#"{"e":"serverShutdown","E":1234567890}"#)` returns `None`
     without panicking.
  2. `parse("not valid json {{{")` (or equivalent malformed input) returns
     `None` without panicking.
- Verification:
  - `cargo test --lib exchange::binance::` — both tests pass, bringing this
    file's test count to 3 (with task 2.3), which is 5 of 5 required tests
    across Phases 1-2 (2 in `model.rs` + 3 here).
- Done when:
  - Both tests exist and pass; between them and task 2.3, all five tests
    from `spec.md`'s Testing Strategy are landed and green.

### 2.5 Full green check for Phase 2
- Files or areas: none — verification-only task
- Change: none.
- Verification:
  - `cargo test` — full suite green, all 5 required parser/conversion tests
    present and passing (running count: step 0's 8 + Phase 1's 2 + Phase
    2's 3 = 13).
  - `cargo clippy --all-targets -- -D warnings` clean.
  - `cargo fmt --check` clean.
  - Inspection: `parse` returns `Option<Book>`, not `Result` — confirm no
    `?`-propagated error path exists in `src/exchange/binance.rs` that could
    kill a future read loop on a non-book message.
  - Inspection: no `Exchange` trait, no `tokio::spawn`, no `.split()`, no
    reconnection/backoff/staleness, no gRPC code exist anywhere in this
    phase's diff. No live websocket connection is opened anywhere during
    `cargo test` (confirm by eye — no `connect_async` call inside a test).
- Done when:
  - All checks above pass with zero warnings and zero invariant violations.

**Commit boundary:** `src/exchange/mod.rs`, `src/exchange/binance.rs`,
`src/lib.rs`. Nothing in `src/main.rs` changes in this phase.

---

## Phase 3: `src/main.rs` — real connect-and-read loop

This is the first phase in this packet that opens a live socket. A failure
here is async/websocket plumbing, not parsing logic (already proven green in
Phases 1-2).

### 3.1 Convert `main()` to async and connect to the live Binance endpoint
- Files or areas: `src/main.rs`
- Change: `#[tokio::main] async fn main() -> anyhow::Result<()>`. After
  building `Config` (unchanged) and calling `telemetry::init(...)` (unchanged
  from step 0), connect via `tokio_tungstenite::connect_async` to
  `wss://stream.binance.com:9443/ws/{pair}@depth20@100ms` using
  `config.pair.to_lowercase()`. Drive the read loop directly in the `main`
  task with a single `StreamExt::next()` loop over the returned
  `WebSocketStream` — no `tokio::spawn`, no `.split()` anywhere in this file.
- Verification: covered by task 3.3 (running it is the real proof for this
  task — connecting alone produces no visible output until 3.2 lands the
  logging).
- Done when:
  - `main` is `async`, connects to the real endpoint, and contains no
    `tokio::spawn` or `.split()` call.

### 3.2 Dispatch messages through `exchange::binance` and log each parsed `Book`
- Files or areas: `src/main.rs`
- Change: In the read loop, match each incoming `Message`:
  `Message::Text(t)` → `exchange::binance::parse(&t)`; on `Some(book)`, emit
  one `tracing::info!` line with best bid, best ask, and `last_update_id` in
  readable decimal (e.g.
  `binance ethbtc | bid 0.03150000 x 5.00000000 | ask 0.03151000 x 12.50000000 | id 7723441`),
  reading best bid/ask as `book.bids.first()`/`book.asks.first()` (already
  best-first per Binance's sorted snapshot — no full-book dump, one line per
  update). `Message::Ping`/`Message::Pong` → no action (tungstenite answers
  automatically). `Message::Close(_)` → `break` the loop. `Message::Binary(_)`
  → `tracing::debug!` and continue. No `println!` anywhere.
- Verification: covered by task 3.3.
- Done when:
  - Every `Message` variant is matched explicitly (no wildcard `_ =>`
    swallowing an unhandled variant silently), and the log line uses
    `tracing::info!`, never `println!`.

### 3.3 Manually run against the live endpoint and confirm output
- Files or areas: none — verification-only task
- Change: none.
- Verification:
  - `cargo run -- --pair ethbtc` — observe live output for at least a
    couple of minutes; confirm the printed lines show readable decimal
    prices in a sane range for ETHBTC (~0.03), one line per update, on
    stderr (not stdout — confirm via `cargo run -- --pair ethbtc 1>/dev/null`
    still shows the log lines).
  - `cargo test` still green — no new tests added this phase per
    `plan.md`/`spec.md` (integration wiring, verified by running it).
- Done when:
  - The manual run produces the documented log format against the real
    endpoint, and `cargo test`'s count is unchanged from Phase 2's close
    (13).

### 3.4 Confirm the container path runs the same binary the same way
- Files or areas: none — verification-only task
- Change: none (no `Dockerfile`/`compose.yml` edits expected in this phase —
  step 0 already produced a working container shape for this binary).
- Verification:
  - `docker compose up --build` — container builds and starts.
  - `docker compose logs` — shows the same live book-update log lines seen
    in task 3.3, confirming the container path matches the host path.
- Done when:
  - The container produces the same log format as the host run, with no
    `Dockerfile`/`compose.yml` changes required to get there (if a change
    turns out to be required, that's a plan drift trigger — flag it rather
    than silently patching around it).

### 3.5 Full green check and invariant inspection for Phase 3
- Files or areas: none — verification-only task
- Change: none.
- Verification:
  - `cargo clippy --all-targets -- -D warnings` clean.
  - `cargo fmt --check` clean.
  - Inspection: `grep -n "tokio::spawn\|\.split(" src/main.rs` returns
    nothing.
  - Inspection: no reconnection/backoff/staleness logic, no gRPC code,
    no `Exchange` trait exist anywhere in `src/main.rs`'s diff — a `Close`
    frame ends the loop and the program exits; there is no retry.
  - Inspection: `f64` still appears in exactly one place across the whole
    branch (`src/model.rs`'s `Display` impl) — confirm `grep -rn "f64" src/`
    shows only that one file.
- Done when:
  - All checks above pass with zero warnings and zero invariant violations.

**Commit boundary:** `src/main.rs` only.

---

## Phase 4: `README.md` — Price representation section

### 4.1 Add the "Price representation" section
- Files or areas: `README.md`
- Change: Add a new section under design decisions containing, in this
  order: (1) the modelling argument (price is a tick multiple, not a
  continuous quantity); (2) the measured `~3.9e-13` bound, stated explicitly
  as an honest limit on the precision benefit, not the justification; (3)
  the accumulation case (summing many levels, compounding positions) named
  as what this step does not do; (4) the fixed `1e9`/`i64` scale as a
  documented assumption, with both bounds stated —
  largest representable price `i64::MAX / 1e9 ≈ 9.22 × 10^9` in the pair's
  quote unit, smallest representable tick `1e-9`; (5) a "what would change
  for production" note naming `exchangeInfo`-derived per-symbol tick sizes
  as the production answer, not implemented here.
- Verification: covered by task 4.2.
- Done when:
  - The section exists with all five elements above, in the specified
    order, matching `spec.md`'s required argument ordering exactly (no
    leading with "`f64` can't represent decimals").

### 4.2 Run every command the README's Quick Start section shows
- Files or areas: none — verification-only task
- Change: none.
- Verification:
  - Run `cargo run -- --pair ethbtc` (or whatever the README's Quick Start
    literally shows) and confirm actual output matches what's documented.
  - Run the README's `docker compose` example(s) and confirm actual output
    matches what's documented.
- Done when:
  - Every command shown in the README produces output consistent with what
    is documented — no drift between what's written and what actually runs.

**Commit boundary:** `README.md` only.

---

## Cross-Phase (unblocked by Phase 3, not gated by any single phase's commit)

### X.1 25+ minute live ping/pong survival verification
- Files or areas: none — verification-only task, run against the code
  landed by Phase 3 (can run in parallel with or after Phase 4's README
  work, per `plan.md`, but must complete and be reported before this branch
  is considered ready to hand off).
- Change: none.
- Verification:
  - Set `tungstenite`/`tokio-tungstenite` logging to `trace` level for this
    run specifically (e.g. `RUST_LOG=keyrock_case_study=info,tungstenite=trace,tokio_tungstenite=trace`
    or the project's equivalent env-var convention) and run
    `cargo run -- --pair ethbtc` against the real Binance endpoint
    continuously for at least 25 minutes, without restarting the process.
  - Capture the trace-level log output for the full run (redirect stderr to
    a file, since logs are on stderr per the existing telemetry split).
  - From the captured log, extract and report concrete evidence that:
    (a) the connection survives past the 60-second pong-timeout window at
    least once (i.e. more than 60 seconds of continuous connection with no
    self-initiated write beyond what `tokio-tungstenite` sends
    automatically);
    (b) ping frames from Binance are visible in the trace log
    (approximately every 20 seconds);
    (c) pong frames are sent in response, confirming `tokio-tungstenite`'s
    automatic pong-reply behavior actually fired during this run, not just
    in theory;
    (d) the process is still running and still printing book-update lines
    at the 25-minute mark (or beyond), i.e. the connection never dropped.
  - Report the actual evidence — timestamps of at least one ping/pong pair
    near the 20s/60s boundaries, and the wall-clock duration actually
    achieved — not an assumption that it worked. If the connection drops
    before 25 minutes without a self-initiated write, that is a real bug per
    `spec.md`'s Risks (`tokio-tungstenite` only sends queued pongs when the
    write half makes progress, and this loop never writes) — do not mark
    this task done; flag it as a plan drift trigger and fix before
    proceeding, per `plan.md`'s Expected Drift Triggers.
- Done when:
  - A single continuous run reached at least 25 minutes wall-clock time
    against the real endpoint, the trace log contains identifiable
    ping/pong frame evidence spanning at least one full 60-second window
    with no self-initiated write, and that evidence (with approximate
    timestamps and total duration) is reported alongside this task rather
    than merely asserted.

---

## Final Verification

Before closing the packet, run, from the crate root:

- `cargo run -- --pair ethbtc` — shows live book lines with readable decimal
  prices in a sane range for ETHBTC (~0.03). This is the most representative
  real behavior path for this step: a reviewer connecting to the real
  exchange and watching a live single-venue book (Binance only, at this
  step) print to the terminal.
- `cargo test` — green, including all 5 parser/conversion tests from
  `spec.md`'s Testing Strategy (2 in `src/model.rs`, 3 in
  `src/exchange/binance.rs`).
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo fmt --check` — clean.
- `docker compose up --build` — runs the same binary the same way, logs
  visible via `docker compose logs`.
- The 25+ minute live ping/pong survival check (task X.1) has been run to
  completion, with ping/pong frame evidence from the trace log reported —
  not assumed — per `spec.md`'s Acceptance Criteria.
- Inspection, across the full branch diff, confirms none of the following
  were introduced anywhere: an `Exchange` trait or cross-exchange
  abstraction, `tokio::spawn`, `.split()` on the websocket stream,
  reconnection/backoff/staleness logic, any gRPC code (`src/server.rs`, use
  of generated `orderbook` types), or stub files for `merge.rs`/
  `src/aggregator.rs`/`src/book.rs`.
- Inspection: `f64` appears in exactly one place across the whole branch —
  the `Display` impl in `src/model.rs`.
- `git log` on this branch shows one commit per phase (four total), each
  independently buildable at the point it was made (Phase 1's commit builds
  and tests green on its own; Phase 2's commit builds/tests/clippy pass on
  top of Phase 1; Phase 3's commit adds the live read loop on top of both;
  Phase 4 is docs-only) — confirm by reviewing the diff boundaries, and
  re-running `cargo build`/`cargo test` at each commit if there's any doubt.
</content>
