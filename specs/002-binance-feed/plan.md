# Plan: 002-binance-feed

## Summary

Land step 1 in four sequential phases plus one late-stage verification gate
that runs alongside (not blocking) the last phase. Ordering follows the
purity gradient: `model.rs` (newtypes, `Book`, the "why not `f64`" argument)
has zero dependency on websockets or async and carries two of the five
required tests, so it lands first and is provably correct in isolation.
`exchange/binance.rs`'s `parse()` function is pure too — a `&str -> Option<Book>`
function — and carries the remaining three tests (the real-fixture 20/20
parse, `serverShutdown` → `None`, malformed JSON → `None`); it lands second,
still with no live socket involved, so a test failure here is unambiguously a
parsing bug, not a networking one. Only once both pure layers are landed and
green does `src/main.rs` get rewired to actually connect, read, and log —
isolating "the parser is wrong" from "the async/websocket wiring is wrong" as
two separate, separately-committed failure modes. `README.md` lands last, once
there is real behavior to document accurately. The 25+ minute live ping/pong
survival check is a distinct, required verification step that can only run
once `main.rs`'s real read loop exists (end of Phase 3 onward); it is not part
of any phase's `cargo test` gate and is called out separately below. All
design decisions (newtype shape, scale, message-variant handling, output
format, test list) are fixed by `spec.md` — this plan only sequences landing
them.

## Phase Breakdown

### Phase 1: `src/model.rs` — `Price`, `Amount`, `Book`

- Objective: Introduce the fixed-point price/amount representation and the
  exchange-agnostic `Book` type, fully proven by pure unit tests, before any
  websocket or exchange-specific code exists.
- Main changes: `src/model.rs` (new) — `Price`/`Amount` newtypes over `i64` at
  scale `1e9`, a string-parsing constructor that converts and rounds to the
  nearest tick, `Display`/`Debug` impls, and `Book` (`bids`/`asks` as ordered
  `Vec<(Price, Amount)>` plus `last_update_id: u64`). `src/lib.rs` gains
  `pub mod model;`. The "why not `f64`" comment lands here in full (modelling
  argument first, measured `~3.9e-13` bound second, accumulation caveat
  third), matching the order `CLAUDE.md` and spec.md both require. No
  `src/exchange/` or `src/main.rs` changes in this phase — the existing
  synchronous `main.rs` from step 0 is untouched, so a failure here is
  unambiguously in the new module, not mixed in with wiring changes.
- Verification:
  - `cargo test` green, including the price round-trip test
    (`"0.03150000"` → `31500000` → `Display`s back as `"0.03150000"`) and the
    named regression test (`0.031505 - 0.031500` in the integer domain
    equals exactly `5000` ticks) — two of the five tests required by
    spec.md's Testing Strategy, both possible without any exchange code.
  - `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` clean.
  - Manual check (by inspection): `f64` appears in exactly one place in this
    file — the `Display` impl — confirming the invariant before more code is
    layered on top of it.
- Done looks like: `Price`, `Amount`, and `Book` exist, are unit-tested in
  isolation, and nothing outside `src/model.rs` references them yet.
- Commit boundary: this phase's diff is `src/model.rs`, `src/lib.rs`.
  Reverting it restores step 0's `lib.rs` module list exactly, with no other
  phase depending on anything reachable only through this commit's absence
  (Phase 2 needs `model` to exist, so revert order runs newest-first).

### Phase 2: `src/exchange/binance.rs` — `parse()`

- Objective: Turn a raw Binance text message into `Option<Book>`, proven by
  the three remaining required tests, still with no live socket or async
  driving loop involved.
- Main changes: `src/exchange/mod.rs` (new, module declaration only) and
  `src/exchange/binance.rs` (new) — `parse(text: &str) -> Option<Book>`:
  `serde_json`-deserializes the `lastUpdateId`/`bids`/`asks` shape, converts
  each price/amount string through `model::Price`/`Amount`'s parser, and
  returns `None` (not `Result`, no `?`) for anything that isn't a recognizable
  book payload. The connect URL constant and the message-variant dispatch
  (`Text`/`Ping`/`Pong`/`Close`/`Binary`) can live in this file too, but the
  read loop itself is not wired into `main` yet — this phase proves `parse`
  is correct as a pure function, deferring "does it actually connect" to
  Phase 3. `src/lib.rs` gains `pub mod exchange;`.
- Verification:
  - `cargo test` green, including: a real captured Binance `depth20` payload
    (embedded as a string fixture) parses to exactly 20 bids and 20 asks with
    asserted `Price`/`Amount` values (not just counts); `{"e":"serverShutdown","E":1234567890}`
    parses to `None` without panicking; malformed JSON parses to `None`
    without panicking. That is five of five required tests green across
    Phases 1-2.
  - `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` clean.
  - Inspection check: `parse` returns `Option<Book>`, not `Result`, and no
    `?`-propagated error path exists that could kill a future read loop on a
    non-book message.
- Done looks like: `binance::parse` is fully tested against real and
  adversarial fixtures with no websocket connection ever opened during
  `cargo test`.
- Commit boundary: this phase's diff is `src/exchange/mod.rs`,
  `src/exchange/binance.rs`, `src/lib.rs`. Reverting it (with Phase 1 still in
  place) leaves `model.rs` landed and tested but unused — a safe, buildable
  intermediate state, since nothing else references `exchange` yet.

### Phase 3: `src/main.rs` — real connect-and-read loop

- Objective: Wire the proven parser into an actual live connection — the
  phase that turns this from "pure logic, no I/O" into "the real thing
  running" — isolating any bug here as async/websocket plumbing, not parsing
  logic (already proven in Phases 1-2).
- Main changes: `src/main.rs` becomes `#[tokio::main] async fn main() ->
  anyhow::Result<()>`. After building `Config` (unchanged) and initializing
  telemetry, it connects to
  `wss://stream.binance.com:9443/ws/{pair}@depth20@100ms` (pair lowercased)
  and drives the read loop directly in the `main` task — no `tokio::spawn`,
  no `.split()`, a single `StreamExt::next()` loop over the
  `WebSocketStream`. On each `Message::Text` that `binance::parse` turns into
  `Some(Book)`, logs one `tracing::info!` line with best bid/ask and
  `last_update_id`; `Ping`/`Pong` are ignored (tungstenite answers
  automatically), `Close` breaks the loop, `Binary` is logged at `debug` and
  skipped.
- Verification:
  - `cargo run -- --pair ethbtc` shows live book lines with readable decimal
    prices in the ~0.03 range for ETHBTC — run and read the actual output,
    don't assume the format matches what was written.
  - `cargo test` still green (no new tests this phase per spec.md's build
    order — this is integration wiring, verified by running it, same pattern
    as step 0's spec for wiring-only phases).
  - `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` clean.
  - `docker compose up --build` runs the same binary the same way, logs
    visible via `docker compose logs`.
  - Inspection check: no `tokio::spawn`, no `.split()` anywhere in the diff.
- Done looks like: running the binary against the real Binance endpoint
  prints one readable line per update, sustained for at least a short manual
  run, with the container path producing the same behavior.
- Commit boundary: this phase's diff is `src/main.rs`. Reverting it (with
  Phases 1-2 still in place) restores step 0's synchronous `main.rs` on top
  of fully-tested, unused `model`/`exchange` modules — buildable, but with no
  running feed, which is why the 25-minute verification gate below cannot
  start until this commit lands.

### Phase 4: `README.md` — Price representation section

- Objective: Document the price-representation decision and its bounds
  accurately, once the behavior it describes is proven, not before.
- Main changes: `README.md` only — new "Price representation" section under
  design decisions: the modelling argument stated first (a price is a tick
  multiple, not a continuous quantity), the measured `~3.9e-13` bound stated
  second as an honest limit on the precision benefit (not the justification),
  the accumulation case named third as what this step does not do, the fixed
  `1e9`/`i64` scale stated as a documented assumption with both bounds
  (`i64::MAX / 1e9 ≈ 9.22e9`, smallest tick `1e-9`), and `exchangeInfo`-derived
  tick sizes named under a "what would change for production" note.
- Verification: manually run every command the README's Quick Start section
  shows (`cargo run -- --pair ethbtc`, the `docker compose` example) and
  confirm actual output matches what's documented — a README edited without
  re-running its own examples is how docs drift starts.
- Done looks like: no claim in the new section contradicts what Phases 1-3
  already proved, and the argument order (modelling → measured bound →
  accumulation caveat) matches spec.md's required ordering exactly.
- Commit boundary: this phase's diff is `README.md` alone. Reverting it has
  no effect on build or test state.

## Cross-Cutting Considerations

- **Invariant check applies to every phase, not just once.** Before each
  phase's commit, confirm by inspection of that phase's diff (not just the
  final state) that none of the following were introduced: an `Exchange`
  trait or cross-exchange abstraction, `tokio::spawn`, `.split()` on the
  websocket stream, reconnection/backoff/staleness logic, any gRPC code
  (`src/server.rs`, use of generated `orderbook` types), or stub files for
  `merge.rs`/`src/aggregator.rs`. These are all explicitly deferred to later
  steps per spec.md's Scope and Invariants sections — a phase that
  accidentally reaches for one of them is scope creep, not a shortcut.
- **`f64` boundary discipline.** `f64` should appear in exactly one place
  across the whole branch: the `Display` impl in `src/model.rs`. Phase 1
  establishes this; Phases 2-3 should not introduce a second occurrence (e.g.
  no float arithmetic creeping into `parse` or the logging line).
- **Commit cadence.** One commit per phase, matching 001's pattern — each
  commit message should stand on its own (what changed, why it's safe to
  commit at that point), since squashing before merge is a later decision,
  not something this plan assumes.
- **No stub files.** Per spec.md's Out of Scope, no phase should add
  `src/book.rs`, `src/aggregator.rs`, or `src/server.rs` even as empty
  placeholders.
- **Toolchain and container shape are untouched.** No phase in this plan
  touches `rust-toolchain.toml`, the Dockerfile's `FROM` line, or joins the
  external `echo` network. `Cargo.toml` is also untouched — every dependency
  this step needs (`tokio`, `tokio-tungstenite`, `futures-util`,
  `serde`/`serde_json`, `tracing`) already landed in step 0.

## Verification Gates

Before this branch is considered ready to hand off:

- `cargo test` is green across all four source phases, with the five tests
  spec.md's Testing Strategy requires all present and passing (2 in
  `model.rs`'s tests, 3 in `exchange/binance.rs`'s tests).
- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are
  clean.
- `cargo run -- --pair ethbtc` produces live, readable book lines in a sane
  price range, confirmed by actually running it and reading the output.
- `docker compose up --build` runs the same binary the same way.
- **The 25+ minute live ping/pong survival check** (see Risks in spec.md):
  run the binary with `tokio-tungstenite`/`tungstenite` logging at `trace`
  level for at least 25 minutes against the real Binance endpoint, and
  confirm from the trace log that the connection survives past the 60s
  pong-timeout without the read loop ever writing anything itself — report
  the actual ping/pong frame evidence from the log, not an assumption that it
  worked. This check depends only on Phase 3 landing (the real read loop must
  exist) and does not block Phase 4 — it can run in parallel with or after
  the README phase — but it must be run and its evidence reported before this
  branch is considered ready to hand off, since it is the only verification
  of the connection-survival claim in spec.md's Goal and Acceptance Criteria,
  and it cannot be satisfied by `cargo test` or any short dev-loop command.
- `git log` on this branch shows one commit per phase, each independently
  buildable at the point it was made (Phase 1's commit builds and tests green
  on its own; Phase 2's commit builds/tests/clippy pass on top of Phase 1;
  and so on).
- Inspection confirms none of the Scope "Out" items (`Exchange` trait,
  `tokio::spawn`, `.split()`, reconnection/backoff/staleness, gRPC code) were
  added anywhere in the branch's diff.

## Expected Drift Triggers

If any of the following becomes true while implementing, update `spec.md`
before continuing rather than improvising past it:

- The 25-minute ping/pong check fails — the connection drops before 60s
  without a self-initiated write. This is flagged in spec.md's Risks as a
  real possibility (`tokio-tungstenite` only sends queued pongs when the
  write half makes progress, and this loop never writes); if it happens, that
  is a genuine bug to fix before this step can be considered done, not
  something to note and move past.
- The real captured Binance `depth20` fixture used in Phase 2's tests turns
  out to have a shape spec.md didn't anticipate (e.g. a field spec.md doesn't
  mention), forcing a `parse` design change beyond what spec.md's Proposed
  Design describes.
- Any phase turns out to need a new dependency, touching `Cargo.toml`,
  `rust-toolchain.toml`, or the Dockerfile's `FROM` line — all out of scope
  per spec.md, since every dependency this step needs already landed in step
  0.
- The fixed `1e9`/`i64` scale turns out to be insufficient for the configured
  pair during the live run (e.g. a price or amount that doesn't round-trip
  cleanly) — that's a scale-decision question spec.md already closed as a
  documented assumption, and reopening it needs a design sign-off, not a
  silent workaround.
- Implementing `parse` reveals a Binance message shape that isn't cleanly
  `Option<Book>`-shaped (neither a full book nor a recognizable non-book
  control frame) — spec.md's message-variant handling assumes exhaustive,
  simple categories; a genuine third category is a design gap to raise, not
  to guess through.
