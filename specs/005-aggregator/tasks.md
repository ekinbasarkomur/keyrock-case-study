# Tasks: 005-aggregator

Five phases, matching `plan.md`'s phase and commit boundaries exactly. Work
through them in order — each phase's verification gate must be green before
starting the next, since phases 1-3 build on each other's compiling state.

---

## Phase 1: `src/exchange/binance.rs` — borrowed deserialisation

### Task 1.1 — Replace owned `Depth20` with borrowed `Depth20<'a>`

**Files:** `src/exchange/binance.rs`

**Change:**
- Replace:
  ```rust
  #[derive(Deserialize)]
  struct Depth20 {
      #[serde(rename = "lastUpdateId")]
      last_update_id: u64,
      bids: Vec<[String; 2]>,
      asks: Vec<[String; 2]>,
  }
  ```
  with:
  ```rust
  #[derive(Deserialize)]
  struct Depth20<'a> {
      #[serde(rename = "lastUpdateId")]
      last_update_id: u64,
      #[serde(borrow)]
      bids: Vec<[&'a str; 2]>,
      #[serde(borrow)]
      asks: Vec<[&'a str; 2]>,
  }
  ```
- Update `parse(text: &str) -> Option<Book>`'s `serde_json::from_str::<Depth20>(text)`
  call site so the borrowed lifetime is tied to `text`'s borrow (no signature
  change to `parse` itself — it already takes `&str`).
- Update `parse_levels` to take `&[[&str; 2]]` instead of `&[[String; 2]]`;
  body is unchanged (`Price::parse`/`Amount::parse` already take `&str`, so
  destructuring `[price, amount]` still binds `&&str`/`&str` correctly —
  adjust only what the compiler requires, no logic change).
- Do not touch `Book`'s definition in `src/model.rs` — the returned `Book`
  must end up holding only `Price`/`Amount` (`f64`, `Copy`), so the borrow
  ends inside `parse()` by construction, not by an explicit lifetime
  annotation on `Book`.
- If serde cannot actually borrow through this chain (e.g. `cargo build`
  forces an owned `String` somewhere, or a compile error indicates an escape
  that defeats zero-copy), stop and report this rather than falling back to
  owned data silently — per spec.md decision 4 and plan.md's "Expected Drift
  Triggers" — this would require a spec.md update before continuing.

**Verification:**
- `cargo build` — clean.
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo fmt --check` — clean.
- `cargo test` — the three existing tests in this file pass **unchanged**
  (no test body or assertion edits):
  - `parses_depth20_into_twenty_bids_and_twenty_asks_with_correct_values`
  - `server_shutdown_message_parses_to_none_without_panicking`
  - `malformed_json_parses_to_none_without_panicking`
- Inspection: `grep -n "bids:" src/exchange/binance.rs` shows `Vec<[&'a str; 2]>`,
  not `Vec<[String; 2]>`, on the `Depth20` struct.

**Done when:** the crate builds, all three parser tests pass without any
edits to their bodies, and the struct fields are genuinely `&'a str` (not a
`.to_string()`'d string that defeats the borrow).

---

### Task 1.2 — Add `enum Venue` to `src/exchange/mod.rs`

**Files:** `src/exchange/mod.rs`

**Change:**
- Add:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum Venue {
      Binance,
  }
  ```
- Add a label impl yielding `"binance"` for `Venue::Binance` — `impl
  fmt::Display for Venue` (preferred, since `merge.rs` in phase 2 needs
  `venue.to_string()` per plan.md's "Plan Review Notes") or an equivalent
  `as_str(&self) -> &'static str` method, whichever reads more idiomatically
  once written; either satisfies phase 2's need for a label, but `Display`
  is what plan.md's Phase 2 description names (`venue.to_string()`), so
  prefer it unless it fights the borrow checker.
- No other module content changes; `pub mod binance;` stays as-is.

**Verification:**
- `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` — clean.
- `cargo test` — no new tests required for `Venue` itself at this phase (it
  has no behaviour yet beyond a label); it becomes exercised by phase 2's
  and phase 3's tests instead. Do not add a standalone `Venue` test here —
  per the testing convention, a test needs a bug it would catch, and an enum
  with one variant and a `Display` impl has none worth isolating yet.

**Done when:** `Venue::Binance.to_string()` (or the chosen equivalent)
compiles and returns `"binance"`, verified by a quick `cargo build` — no
committed test needed for this alone, per the note above.

**Commit boundary for Phase 1:** `src/exchange/binance.rs`,
`src/exchange/mod.rs`. Reverting this phase restores the owned-`String`
parser and removes `Venue`, with nothing downstream yet depending on either.

---

## Phase 2: `src/merge.rs` — `summarise()`

### Task 2.1 — Create `src/merge.rs` with `summarise()`

**Files:** `src/merge.rs` (new), `src/lib.rs`

**Change:**
- `src/lib.rs`: add `pub mod merge;` alongside the existing `pub mod`
  declarations (after `pub mod model;`, matching alphabetical order used
  today: `config`, `exchange`, `merge`, `model`, `proxy`, `server`,
  `telemetry`).
- `src/merge.rs` (new file):
  ```rust
  pub fn summarise(venue: Venue, book: Option<&Book>) -> Option<Summary>
  ```
  - No clock, no I/O, no channel reference anywhere in this file — pure
    function only, per spec.md decision 6.
  - `book: None` → returns `None` immediately.
  - `book: Some(book)` → takes the first 10 entries of `book.bids` and the
    first 10 of `book.asks` (Binance already sends sorted, truncation only —
    no sort call here; that's step 5's job). Each level maps to a
    `crate::orderbook::Level { exchange: venue.to_string(), price:
    <Price as f64>, amount: <Amount as f64> }` — conversion to `f64` happens
    only here, at the `Summary`/`Level` boundary, per the money-boundary
    rule.
  - `spread` = best ask price minus best bid price (`f64` subtraction at
    this boundary is expected and accepted — this is the one arithmetic
    step CLAUDE.md's rules call out as where the fixed-point-vs-float
    tension is visible; `src/model.rs`'s existing doc comment already
    documents the project's chosen `f64`-with-`total_cmp` stance, so this
    is not a new decision, just its application). **Resolved gap (not
    covered by spec.md's test list, decided here rather than left to guess
    mid-implementation):** use `book.bids.first()`/`book.asks.first()`
    (both `Option`) to source the spread; if either side is empty, spread
    is `0.0` with a `//` comment stating this is a defensive fallback never
    expected to trigger against real Binance `depth20` data (which always
    returns 20/20), not a claim about market state — a genuinely empty
    single-venue book is step 5's "one venue's book empty" test territory,
    not this step's, so this step only needs to not panic.
  - No hardcoded `"binance"` string literal anywhere in this file — the
    `exchange` label always comes from `venue`.

**Tests (co-located `#[cfg(test)] mod tests` in `src/merge.rs`, hand-built
`Book` and `Venue::Binance` fixtures, no `test_` prefix, behaviour-sentence
names):**

- `summarise` on a 20-level book returns 10 bids, 10 asks, and the correct
  spread — catches a truncation or off-by-one bug in top-10 selection.
- `summarise` returns bids descending by price and asks ascending — catches
  a future merge (step 5) breaking sort order; not currently reachable as a
  bug in this step's code, but locks the contract in before step 5 changes
  this function's body.
- `summarise` on a 6-level book returns 6 levels per side, not padded to 10
  — catches accidental zero-padding.
- `summarise(Venue::Binance, None)` returns `None` — catches a panic or a
  synthesized-empty-`Summary` bug on the "no data yet" path.

**Verification:**
- `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` — clean.
- `cargo test` — all four tests above pass, each named as a behaviour
  sentence (no `test_` prefix), each individually reported (not folded into
  an aggregate "tests pass" claim).
- Inspection: `grep -n '"binance"' src/merge.rs` returns nothing — the label
  must come from `venue.to_string()`, never a literal in this file.

**Done when:** `summarise()` compiles, all four tests pass against hand-built
`Book` fixtures, and nothing in the running binary calls it yet.

**Commit boundary:** `src/merge.rs`, `src/lib.rs`. Reverting this phase
(with phase 1 still in place) leaves the borrowed parser landed but unused —
independently buildable.

---

## Phase 3: Wire the pipeline — `src/aggregator.rs`, `src/server.rs`, `src/main.rs`

This phase is one atomic commit-sized unit — the crate does not compile in
any partial state between these three files (plus `src/lib.rs`'s new module
declaration). Implement all four file changes together before running any
verification.

### Task 3.1 — `src/aggregator.rs` (new)

**Files:** `src/aggregator.rs` (new), `src/lib.rs`

**Change:**
- `src/lib.rs`: add `pub mod aggregator;`.
- `src/aggregator.rs`:
  ```rust
  struct VenueState {
      book: Book,
      last_update: Instant,
  }

  struct Aggregator {
      binance: Option<VenueState>,
  }
  ```
  - `last_update` is written on every update but not read this step — leave
    a `//` comment on the field stating that step 6 adds the staleness
    check that reads it, so it doesn't read as dead code.
  - An async task function (e.g. `pub async fn run(mut rx: mpsc::Receiver<(Venue, Book)>, tx: watch::Sender<Option<Arc<Summary>>>)`)
    that:
    - Owns an `Aggregator { binance: None }` as a local variable (no
      `Arc<Mutex<_>>` — single-owner task-local state, per CLAUDE.md's
      architecture section).
    - Loops on `rx.recv().await`; on `Some((venue, book))`, does a real
      `match venue { Venue::Binance => { self.binance = Some(VenueState {
      book, last_update: Instant::now() }); } }` — a genuine match, not a
      single-arm shortcut, so the compiler forces a new arm when Bitstamp
      lands in step 4.
    - Calls `merge::summarise(venue, self.binance.as_ref().map(|s| &s.book))`
      (or equivalent — the exact book reference must come from the
      just-updated venue slot).
    - Wraps a `Some(summary)` result in `Arc::new` and writes
      `Some(Arc::new(summary))` into `tx`; if `summarise` returns `None`,
      do not write to `tx` (nothing new to publish).
    - On `rx.recv()` returning `None` (feed's `Sender` dropped), the loop
      ends and the function returns — no explicit shutdown signalling
      needed, matching step 2's task-ends-propagates-through-`select!`
      pattern.

**Verification (folded into Task 3.4's full-pipeline verification below,
since this file alone does not compile without the other two changes in
this phase):**
- Inspection: confirm the `match venue { Venue::Binance => ... }` arm is a
  real match (not `_ => ...` alone), confirm `last_update`'s comment exists,
  confirm no `Arc::clone` happens under any lock held across an `.await`.

---

### Task 3.2 — `src/server.rs`: delete `run_fake_writer`, migrate watch type to `Option<Arc<Summary>>`

**Files:** `src/server.rs`

**Change:**
- Delete `run_fake_writer` in its entirety, including its doc comment.
  Update or remove `AggregatorService`'s and `router`'s doc comments that
  forward-reference "step 3 of the build order deletes it" — that
  forward-reference is now stale once this phase lands; either delete the
  sentence or replace it with a past-tense note that this step did the
  deletion.
- `AggregatorService.rx` type changes from `watch::Receiver<Option<Summary>>`
  to `watch::Receiver<Option<Arc<Summary>>>`.
- `router()`'s parameter type changes from `watch::Receiver<Option<Summary>>`
  to `watch::Receiver<Option<Arc<Summary>>>`.
- `book_summary`'s `filter_map` changes from:
  ```rust
  let stream = WatchStream::new(self.rx.clone()).filter_map(|opt| opt.map(Ok));
  ```
  to something that clones the `Arc`'s contents into an owned `Summary`
  after the `Arc::clone` leaves the watch's internal lock, e.g.:
  ```rust
  let stream = WatchStream::new(self.rx.clone())
      .filter_map(|opt| opt.map(|arc| Ok((*arc).clone())));
  ```
  This is the line spec.md's decision 5 is actually about — add a `//`
  comment here explaining that the `Arc::clone` (cheap, atomic) happens
  under `WatchStream`'s lock, and the deep `Summary` clone (`tonic` needs
  a `Summary` by value) happens after, outside it.
- Add `use std::sync::Arc;` to this file's imports.

**Verification:** folded into Task 3.4.

---

### Task 3.3 — `src/main.rs`: mpsc wiring, aggregator task, watch type change

**Files:** `src/main.rs`

**Change:**
- `use keyrock_case_study::aggregator;`, `use keyrock_case_study::merge;` (if
  referenced directly here — likely not, since `aggregator::run` wraps the
  `summarise` call; add only what's actually referenced), `use
  keyrock_case_study::exchange::Venue;`, `use tokio::sync::mpsc;`, `use
  std::sync::Arc;` added to imports as needed.
- `let (tx, rx) = watch::channel(None);` — type now infers as
  `Option<Arc<Summary>>` because of `server::router`'s new signature; no
  explicit turbofish needed unless type inference fails, in which case
  annotate explicitly.
- Add `let (feed_tx, feed_rx) = mpsc::channel::<(Venue, Book)>(32);` —
  bounded at 32 per spec.md decision 1 (not unbounded — a `//` comment
  should note this is a deliberate backpressure choice, not an arbitrary
  number, matching decision 1's stated reasoning).
- `run_feed`'s signature changes to accept the mpsc `Sender<(Venue, Book)>`
  (e.g. `async fn run_feed(pair: String, tx: mpsc::Sender<(Venue, Book)>) -> Result<()>`).
  Inside the `Message::Text(text)` arm, in addition to (or replacing) the
  existing `info!` logging line, send the parsed book down the channel:
  ```rust
  if let Some(book) = binance::parse(&text) {
      let _ = tx.send((Venue::Binance, book)).await;
      // existing info! logging line stays, or is adapted — see note below
  }
  ```
  Note: sending is a bounded `.send(...).await`, which naturally blocks
  (applies backpressure) if the aggregator falls behind — this is decision
  1's intended behaviour, not a bug. Do not use `try_send` (which would
  drop messages silently instead of backpressuring) — flag this in review
  if `try_send` is used instead of `send`, since it changes the semantics
  decision 1 relies on. A dropped `send` error (aggregator's `Receiver`
  gone) should not kill the feed loop mid-stream — matching the existing
  `if let Some(book) = ...` pattern's non-panicking style; log it at
  `debug!` or let it silently end (the aggregator ending should end the
  whole process via `select!` regardless, so the feed noticing separately
  is not required — decide the simplest correct option here, do not overengineer).
- Replace `let fake_writer_handle = tokio::spawn(server::run_fake_writer(tx));`
  with `let aggregator_handle = tokio::spawn(aggregator::run(feed_rx, tx));`.
- Update `feed_handle`'s spawn to pass `feed_tx`:
  `tokio::spawn(async move { run_feed(pair, feed_tx).await })`.
- Update the `select!` block: rename the `fake_writer_handle` arm to
  `aggregator_handle`, matching its new task and log message (e.g.
  `"aggregator task ended"` instead of `"fake writer task ended"`). Keep the
  existing three-arm shape and error-propagation pattern exactly — only the
  task identity behind the middle arm changes.
- Update this file's module-level doc comment (currently describes "the
  fake-data writer (`server::run_fake_writer`)") to describe the real
  aggregator task instead.

**Verification (Tasks 3.1-3.3 verified together, since the crate only
compiles once all three land):**
- `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` — clean.
- `cargo run -- --pair ethbtc --port 50051`, then in a second terminal:
  `grpcurl -plaintext 127.0.0.1:50051 orderbook.OrderbookAggregator/BookSummary`
  (network permitting — report honestly if this environment cannot reach
  Binance, per the project's standing honesty requirement). Confirm streamed
  `Level.exchange` reads `"binance"`, never `"fake"`, and that spread/price
  values look like plausible real ETHBTC market data (not zeros, not a
  static repeated value).
- `grep -rn '"fake"' src/` returns nothing — direct check, not inferred from
  the diff.
- `docker compose up` (or `docker compose up --build` if the image needs
  rebuilding) — report actual observed behaviour in this environment
  (Docker daemon availability, Binance reachability through the configured
  proxy), not intended behaviour.
- `git diff main --stat -- proto/orderbook.proto` shows zero diff.
- `git diff main --stat -- src/config.rs src/proxy.rs src/telemetry.rs`
  shows zero diff — this phase should not need to touch any of these three
  files; if it does, stop and flag rather than continuing.

**Done when:** killing/disconnecting the feed still ends the whole process
via the same three-task `select!` supervision step 2 established;
`run_fake_writer` no longer exists anywhere in `src/`; a live `grpcurl`
session shows real Binance data end to end with `exchange == "binance"`.

**Commit boundary for Phase 3:** `src/aggregator.rs`, `src/server.rs`,
`src/main.rs`, `src/lib.rs`. Reverting this phase (with phases 1-2 still in
place) restores step 2's fake-writer pipeline exactly, on top of an unused
borrowed parser and an unused `summarise()` — both still independently
buildable and tested.

---

## Phase 4: `tests/grpc.rs`

### Task 4.1 — Replace the `run_fake_writer`-based test setup with a real pipeline drive

**Files:** `tests/grpc.rs`

**Change:**
- Remove the `tokio::spawn(server::run_fake_writer(tx));` line (that
  function no longer exists after phase 3).
- Construct the mpsc channel from phase 3:
  `let (feed_tx, feed_rx) = tokio::sync::mpsc::channel::<(Venue, Book)>(32);`
  (import `keyrock_case_study::exchange::Venue` and
  `keyrock_case_study::model::Book`).
- Spawn the real aggregator task on it:
  `tokio::spawn(keyrock_case_study::aggregator::run(feed_rx, tx));` — real
  code path, not a mock: `aggregator` → `summarise` → `watch`.
- Send **two distinct hand-built `Book` values** down `feed_tx` (e.g. two
  20-level books with different prices, so the two published `Summary`
  values are provably distinct — not two sends of the identical book, which
  would not prove the "two consecutive messages" guarantee this test
  exists to protect, per spec.md's Tests section and this file's own
  existing doc comment about why two reads matter).
- Update the `watch::channel(None)` construction's inferred type to
  `Option<Arc<Summary>>` (implicit via `server::router`'s new signature —
  add `use std::sync::Arc;` if needed for explicit typing).
- Update both content assertions from `exchange == "fake"` to
  `exchange == "binance"` in the `for summary in [&first, &second]` loop.
  This single assertion, applied to every level of both messages, is
  treated as satisfying spec.md's second "fake"-related test bullet ("no
  `Level` anywhere carries `exchange == \"fake\"`") as well — per plan.md's
  Phase 4 note, asserting `== "binance"` on every level is equivalent to
  asserting `!= "fake"` given there is exactly one venue this step. Do not
  add a second, separate assertion/test for this — it would be redundant
  given the current one-venue state.
- Update the test's own doc comment (currently says "the content assertions
  ... catch a fake-generator regression") to describe the real pipeline
  instead of the fake writer.
- Keep the exact existing structure otherwise: OS-assigned port
  (`TcpListener::bind("127.0.0.1:0")`), `serve_with_incoming`, exactly two
  `stream.message().await` reads before any assertion, 10-bids/10-asks/
  positive-spread assertions unchanged in shape (values will differ per the
  hand-built fixtures used).

**Verification:**
- `cargo test book_summary_streams_multiple_updates_not_a_single_shot_response`
  — passes; report this test's name and pass/fail explicitly, not folded
  into an aggregate "tests pass" summary.
- Confirm no outbound network dependency: the test drives the pipeline with
  hand-built `Book` values sent down the mpsc, never a real Binance
  connection — must remain runnable with no network access, same as today.
- Inspection: confirm exactly two `stream.message().await` calls occur
  before any assertion (unchanged from the existing structure).
- Full suite: `cargo test` — all tests green (this test plus phase 1's
  three, phase 2's four, and any pre-existing tests in `src/model.rs`/
  `src/config.rs`/`src/proxy.rs`/`tests/cli.rs`).

**Done when:** `cargo test` passes deterministically (no fixed port), and
the test would fail if `exchange` reverted to `"fake"` or if the aggregator
collapsed two distinct `Book` sends into one published `Summary` (dedupe is
out of scope this step, so two sends must yield two distinct stream reads —
confirm the two hand-built books differ enough that `summarise`'s output
differs between them, or dedupe logic added by mistake would silently pass
this test).

**Commit boundary:** `tests/grpc.rs` alone. Per plan.md, this phase should
not ship independently of phase 3 landing correctly first (reverting phase 4
alone with phases 1-3 still in place leaves the pipeline shipped but its own
integration test stale — not a safe end state).

---

## Phase 5: `README.md`

### Task 5.1 — Update README to describe what phases 1-4 shipped

**Files:** `README.md`

**Change:**
- Build-order table: move step 3's row to "Done" (matching whatever
  status-column convention the existing table uses for steps 0-2).
- gRPC section: correct any language describing the stream as fake/
  placeholder data to state it carries real Binance data
  (`exchange == "binance"`) end to end.
- Confirm (do not silently rewrite) the existing `f64`-vs-fixed-point
  documentation for `Price`/`Amount` still reads accurately — it should
  already be covered per `src/model.rs`'s own doc comment and
  `specs/002-binance-feed/revisions.md`; only touch it if this step's
  changes actually made it inaccurate (they should not have).
- Layout/file-tree section: add `src/aggregator.rs` and `src/merge.rs` as
  entries, with a one-line description matching this step's actual role for
  each (aggregator: owns per-venue state, drives `summarise`, publishes to
  `watch`; merge: pure `summarise()`, entry point for step 5's later
  two-book `merge()`).
- Wherever the mpsc channel or backpressure design is already discussed (or
  add a short new note if it isn't yet discussed at all), mention the
  bound of 32 explicitly as a design decision confirmed during spec review,
  not just an implementation detail — one sentence, matching this repo's
  "spec length" convention of conclusion-plus-one-sentence-why rather than
  a full derivation.
- Read through the whole file for any remaining reference to
  `run_fake_writer` or `"fake"`-labelled example `grpcurl` output and remove
  or correct it.

**Verification:**
- Manually run every command shown in the README's Quick Start / gRPC
  sections (`cargo run -- --pair ...`, `grpcurl ...`, `docker compose up
  ...`) and confirm actual output matches what's documented, to the extent
  this environment allows — report honestly where it doesn't (no Docker
  daemon, no Binance reachability), matching phase 3's same standard.
- Read-through: `grep -n "fake" README.md` returns nothing (or only
  historical/step-2-context references clearly marked as past-tense, if
  any remain intentionally — prefer removing entirely).

**Done when:** every claim in the updated sections matches what phases 1-4
actually produced and were actually verified to do in this environment.

**Commit boundary:** `README.md` alone. Reverting it has no effect on build
or test state.

---

## Final Verification Gate (run at the tip of the branch, after all 5 phases)

- `cargo build` — clean.
- `cargo test` — clean; explicitly confirm at least these behaviour-named
  tests are present and passing (not folded into an aggregate count):
  - `parses_depth20_into_twenty_bids_and_twenty_asks_with_correct_values`
  - `server_shutdown_message_parses_to_none_without_panicking`
  - `malformed_json_parses_to_none_without_panicking`
  - the 20-level truncation+spread test in `src/merge.rs`
  - the sort-order (bids descending, asks ascending) test in `src/merge.rs`
  - the 6-level-not-padded test in `src/merge.rs`
  - the `summarise(None) -> None` test in `src/merge.rs`
  - `book_summary_streams_multiple_updates_not_a_single_shot_response` in
    `tests/grpc.rs`
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo fmt --check` — clean.
- `grep -rn '"fake"' src/` — no output.
- `grpcurl -plaintext 127.0.0.1:50051 orderbook.OrderbookAggregator/BookSummary`
  against a live `cargo run -- --pair ethbtc --port 50051` — real output
  quoted in the handoff, `exchange == "binance"` confirmed, network
  permitting (state plainly if this environment cannot reach Binance).
- `docker compose up` through the configured proxy — genuinely exercised
  with real output reported, or the environment limitation stated plainly
  (no Docker daemon / no route to Binance even through the proxy).
- `git diff main --stat` — zero diff for `proto/orderbook.proto`,
  `src/config.rs`, `src/proxy.rs`, `src/telemetry.rs`.
- README.md's claims verified against what was actually shipped and
  actually run in this environment, not what was planned.
