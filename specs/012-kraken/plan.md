# Plan: 012-kraken

## Summary

**This branch (`012-kraken`) is research-only and is not intended to merge
into `main`.** `main`'s two-venue system (Binance, Bitstamp) is the actual
deliverable; nothing below authorizes merging this work into it, and every
phase restates that rather than treating it as implied once. The purpose of
implementing anyway is to make the real cost of a third, incremental-update
venue legible — spec.md already resolved the one genuine architectural fork
(interior-mutable state inside `Kraken`'s own struct, `RefCell<Option<Book>>`)
and three smaller open questions (symbol converter, CRC32 checksum,
client-side ping), so this plan sequences *implementation*, not more
decision-making.

Two facts spec.md explicitly could not resolve from docs alone — Kraken's
price/qty wire type (float vs. decimal string, the docs disagree with
themselves) and Kraken's staleness threshold — block real parser and
`Venue::Kraken` work respectively. Both need a live capture/measurement
before the code that depends on them is written, the same "investigate
first" discipline `006-bitstamp`'s plan used for its own fixture. Phase 1
does the first of these; Phase 3 does the second. Nothing in Phases 2, 4, or
5 should be started with an invented price/qty shape or a guessed staleness
number standing in for the real one.

Six phases, sized so each has one clear regression surface and one clear
rollback boundary, mirroring `009-resilience`'s per-file phase shape:

- Phase 1 — live capture: a real Kraken `snapshot`, `update`, `heartbeat`,
  `status`, and subscribe-ack, resolving the price/qty type question before
  any parser code exists.
- Phase 2 — `src/exchange/kraken.rs`: the `Exchange` impl, `RefCell`-based
  accumulation, the symbol converter, CRC32 checksum, fixture tests built
  from Phase 1's capture.
- Phase 3 — `src/exchange/mod.rs`: `Venue::Kraken`, `connect_rate()`, and a
  live-measured `staleness_threshold()` (its own timed sub-step, same shape
  as Phase 1 but measuring silence over a longer window, not guessed).
- Phase 4 — `src/feed.rs`: the Kraken-specific proactive-ping idle-timer
  branch, scoped to Kraken alone.
- Phase 5 — `src/main.rs`: the third `feed::run_feed(Kraken, ...)` spawn in
  the `JoinSet`.
- Phase 6 — live verification: forced disconnect and re-subscribe, staleness
  exclusion, and the full three-venue merge producing real
  binance/bitstamp/kraken labels in one `Summary`.

Every phase that touches `src/aggregator.rs` or `src/model.rs` (Phase 3 and,
if a `Book`-shape gap surfaces during Phase 2, potentially Phase 2) includes
`git diff main --stat -- src/merge.rs` in its verification, expecting zero
diff — this project's single most emphasized cross-venue invariant, unaffected
by this being a research branch. `merge()` must keep receiving the same
`BTreeMap<Venue, &Book>` shape it already accepts; nothing about a third,
incremental venue is allowed to leak into it.

## Phase Breakdown

### Phase 1: live capture — resolve price/qty type and confirm message shapes

- Objective: Capture real Kraken WebSocket v2 traffic (via this project's
  existing HTTP CONNECT proxy setup, same mechanism `bitstamp.rs`'s fixture
  used) and use it, not the docs, to settle the one fact spec.md flags as
  genuinely unresolved: whether `book` channel prices/quantities arrive as
  JSON numbers or decimal strings. Also confirm the exact shapes of
  `heartbeat`, `status`, and a subscribe ack (`success: true`/`false`)
  against what the server actually sends, not just the docs' description of
  them.
- Main changes: none to `src/` — this phase produces captured fixture text
  (a scratch file or inline notes) to hand to Phase 2, plus a short record of
  what was observed (message shapes, the resolved price/qty type) that
  Phase 2's doc comments and fixtures will cite. No code changes yet.
- Verification:
  - A working connection to `wss://ws.kraken.com/v2`, subscribed to
    `{"method":"subscribe","params":{"channel":"book","symbol":["ETH/BTC"],"depth":10}}`
    (or the project's configured pair), captured with a throwaway script or
    `websocat`/similar — report the actual tool used.
  - At least one real `type: "snapshot"` message and at least one real
    `type: "update"` message captured verbatim, long enough to inspect
    `bids`/`asks`/`checksum` field types directly rather than inferring them.
  - At least one real `heartbeat` message and one real `status` message
    captured, and — if triggerable without waiting for a real failed
    subscribe — a `success: false` ack; if not practically triggerable live,
    say so plainly and note the fixture for that case will be a documented
    best-effort construction rather than a live capture, per this project's
    honesty convention.
  - Explicit written conclusion: "prices/quantities are numbers" or
    "prices/quantities are decimal strings," backed by quoting the actual
    captured field values — not a restatement of either doc page.
- Done looks like: real captured JSON for `snapshot`, `update`, `heartbeat`,
  and `status`, plus a plain statement of the resolved price/qty type with
  the evidence for it, ready to hand to Phase 2 as-is.
- Rollback boundary: nothing to roll back — this phase produces no source
  diff.

### Phase 2: `src/exchange/kraken.rs` — the `Exchange` impl

- Objective: A real, testable Kraken `Exchange` implementation matching the
  architecture spec.md already resolved (`pub struct Kraken { book:
  RefCell<Option<Book>> }`), using Phase 1's captured price/qty shape rather
  than a guess.
- Main changes: `src/exchange/kraken.rs` (new file), structured like
  `bitstamp.rs`:
  - `connect_url` returning `wss://ws.kraken.com/v2` (`pair` unused, same as
    Bitstamp).
  - `subscribe_message` building the `book` channel subscribe JSON from
    `pair`, using the symbol converter below.
  - A general lowercase-token-to-slash-separated-uppercase converter (e.g.
    `"ethbtc" -> "ETH/BTC"`), split on a known quote-currency suffix
    (`btc`, `usd`, `eur`, ...), implemented and tested in this file only —
    not a shared utility, per spec.md's explicit scope note.
  - `parse(&self, raw: &str) -> Option<Book>`: two-level dispatch on the
    top-level `"channel"` field (`"book"` vs `"heartbeat"` vs `"status"`)
    before looking at `"type"`, plus a third branch for
    `{"method":"subscribe", "success": ...}` acks (`false` logged at `warn`,
    matching the `bts:error` precedent). A `snapshot` message replaces the
    `RefCell`'s contents wholesale; an `update` message mutates the held book
    (applying each changed level, removing any level whose qty is `0`) and
    returns a clone of the resulting full state.
  - CRC32 checksum: computed after every `parse` call that produces a book
    (top 10 asks low→high, then top 10 bids high→low, digit-strings with `.`
    and leading zeros stripped, concatenated, CRC32'd), compared against the
    message's own `checksum` field. On mismatch: log a warning and clear the
    `RefCell` (forcing the next message to wait for a fresh `snapshot`).
  - A loud doc comment on the struct and the `impl Exchange for Kraken`
    block, per spec.md's explicit instruction, stating that `parse` is
    order-dependent here — calling it twice with the same `update` message
    would silently double-apply that delta — unlike `Binance`/`Bitstamp`.
  - Real fixture tests, from Phase 1's capture (not hand-built): a captured
    `snapshot` parses correctly; a captured `snapshot` followed by a
    captured `update` produces an accumulated book reflecting the update's
    changed levels (not just "doesn't panic" — the accumulation itself,
    per spec.md's Testing Strategy); a `qty: 0` level in an `update` removes
    that price level; `heartbeat`, `status`, and `success: false` each parse
    to `None` without panicking; the symbol converter round-trips the
    project's default pair; a deliberately corrupted checksum triggers the
    documented clear-and-warn path (constructible without a live server —
    this is pure state-machine behavior, testable with a hand-modified copy
    of the real snapshot/update fixtures).
- Cargo.toml: add a CRC32 crate. Per the task brief's suggestion, `crc32fast`
  is the common minimal choice — confirm nothing already in the dependency
  tree provides this (checked against the current `Cargo.toml`, which has
  none) before adding it, and record the version `cargo add` actually
  resolves rather than assuming one.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo test` — every new test in `kraken.rs`'s `mod tests` individually
    named per this project's behaviour-sentence convention; report the exact
    new count against the pre-Phase-2 baseline.
  - Manual trace: feed the accumulation test's `update` fixture through
    `parse` a second time and confirm (by inspection or an explicit test) it
    double-applies the delta — the concrete evidence for the loud doc
    comment's claim, not just an assertion that it's true.
  - `git diff main --stat -- src/merge.rs` and `git diff main --stat --
    src/aggregator.rs` — both expected zero diff; `Kraken` stays entirely
    self-contained in its own file with no change to how the aggregator or
    merge consume a `Book`.
  - `git diff main --stat` overall — confirm only `src/exchange/kraken.rs`
    (new) and `Cargo.toml`/`Cargo.lock` touched.
- Done looks like: `kraken.rs` compiles, its fixture tests (built from real
  captured data) pass, the checksum and accumulation logic are both
  exercised by a real update sequence rather than a single-message smoke
  test, and `src/merge.rs`/`src/aggregator.rs` show zero diff.
- Rollback boundary: `src/exchange/kraken.rs` and the `Cargo.toml`/
  `Cargo.lock` CRC32 addition. Reverting both returns to a two-venue
  codebase with no trace of Kraken — nothing else depends on this phase yet
  since it's unwired.

### Phase 3: `src/exchange/mod.rs` — `Venue::Kraken`, `connect_rate`, staleness

- Objective: Wire `Venue::Kraken` into the two per-venue `match`es
  (`connect_rate()`, `staleness_threshold()`), with the staleness number
  coming from a real live measurement, not a guess — the second of spec.md's
  two facts that need a capture/measurement rather than a decision.
- Main changes: `src/exchange/mod.rs`.
  - `Venue::Kraken` appended as the **third** variant, after `Bitstamp` —
    appending, not inserting, preserves the existing two venues'
    `BTreeMap`/tie-break ordering, per spec.md's explicit instruction.
  - `connect_rate()` arm: no documented Kraken public-WebSocket
    connection-rate limit was found in spec.md's docs research — same
    "stated guess, not fact" treatment `Venue::Bitstamp`'s entry already
    uses, with the comment saying so plainly.
  - `staleness_threshold()` arm: a real number from a live-timed connection
    (see sub-step below), not transcribed from docs — matching the
    discipline `Venue::Bitstamp`'s 8s figure used in `009-resilience`.
  - Live-measurement sub-step (its own small piece of this phase, run before
    the number goes into code): hold a live Kraken `book` connection open
    for a window comparable in spirit to Bitstamp's ~5.25-minute, 792
    -message measurement, record the maximum observed gap between messages
    (`update` or `heartbeat`, whichever actually indicates liveness — decide
    based on what Phase 1's capture showed about `heartbeat` cadence), and
    set the threshold at a documented multiple of that observed max, with
    the reasoning stated in the code comment the same way Bitstamp's is.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo test` — extend the existing `thresholds_differ_per_venue` test (or
    add a Kraken-specific variant) to assert Kraken's threshold differs from
    both existing venues'; extend
    `venue_display_matches_the_wire_contracts_lowercase_strings` to cover
    `Venue::Kraken.to_string() == "kraken"`.
  - The live-measurement sub-step's actual observed numbers (message count,
    window length, max gap) reported in the commit or a scratch note before
    the threshold constant is set — same evidentiary bar `009-resilience`'s
    README section used for Bitstamp's 8s.
  - `git diff main --stat -- src/merge.rs` — zero diff.
  - `git diff main --stat` overall — confirm only `src/exchange/mod.rs`
    touched since Phase 2.
- Done looks like: `Venue::Kraken` compiles into both exhaustive `match`es,
  its staleness threshold is backed by a reported real measurement rather
  than an assumption, and the two extended per-venue tests pass.
- Rollback boundary: `src/exchange/mod.rs`. Reverting it (with Phase 2 still
  in place) leaves `kraken.rs` compiling but with no `Venue::Kraken` to
  attach it to — a safe, if inert, intermediate state.

### Phase 4: `src/feed.rs` — Kraken-specific proactive ping

- Objective: `run_feed`'s read loop sends `{"method":"ping"}` on a Kraken
  connection that's gone idle past ~30s, without changing the shared
  behavior for Binance or Bitstamp — a scoped, venue-specific branch, not a
  change to the loop's general shape.
- Main changes: `src/feed.rs`. Per spec.md, the exact mechanism (a per-venue
  opt-in on the `Exchange` trait vs. logic scoped inside the loop by
  checking `exchange.venue() == Venue::Kraken`) is left to implementation —
  this plan does not pre-decide it, but flags the tradeoff to resolve here:
  a trait-level opt-in keeps the loop venue-agnostic in spirit but touches
  the trait every future venue implements; a `venue()`-checked branch inside
  `run_once_inner`'s read loop keeps the trait untouched but puts one
  `if exchange.venue() == Venue::Kraken` line into otherwise-shared code.
  Given spec.md's explicit framing ("Kraken-specific — not a change to the
  shared `run_feed` behavior for every venue"), prefer the narrower option
  (a scoped branch inside the loop, gated on `venue()`) over widening the
  trait, unless implementation finds a concrete reason the trait-level
  version is meaningfully simpler.
  - An idle timer tracking the time of the last message received on the
    connection; if idle past the threshold, send `Message::Text(r#"{"method":"ping"}"#.into())`.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo test` — existing `src/feed.rs` `mod tests` (backoff, jitter,
    token bucket) pass unedited, confirming this phase didn't touch shared
    reconnect logic; a new unit test for the idle-timer decision itself if
    it can be expressed as a pure function of "time since last message" vs.
    threshold (matching this project's injected-clock testing pattern
    elsewhere in this file), rather than requiring a live socket.
  - `git diff main --stat -- src/merge.rs` and `-- src/exchange/binance.rs`
    and `-- src/exchange/bitstamp.rs` — all zero diff, confirming the branch
    is genuinely Kraken-scoped.
  - `git diff main --stat` overall — confirm only `src/feed.rs` (and,
    if the trait-opt-in path was chosen instead, `src/exchange/mod.rs`)
    touched since Phase 3.
- Done looks like: a Kraken connection held idle past the threshold sends a
  client-initiated ping (confirmed live in Phase 6, not just by a unit
  test), and Binance/Bitstamp's read paths show no behavioral change.
- Rollback boundary: `src/feed.rs`. Reverting it (Phases 1-3 in place)
  leaves Kraken without proactive pings — a real risk of Kraken-side
  disconnects under its documented 60s limit, but not a build break.

### Phase 5: `src/main.rs` — wire the third feed spawn

- Objective: A third `feed::run_feed(Kraken, ...)` task in the `JoinSet`,
  alongside the existing Binance and Bitstamp spawns, sharing the same
  `mpsc::Sender<(Venue, Book)>`.
- Main changes: `src/main.rs`. Clone `feed_tx` a third time, clone `pair` a
  third time, add one more `tasks.spawn(async move { ... (Component::Feed(Venue::Kraken), res) })`
  block — mechanically identical to the existing two, per the file's own
  doc comment describing this as the pattern to extend.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo test` — full suite passes unedited (this phase adds no new pure
    logic to unit test; it's wiring).
  - `cargo run -- --pair ethbtc --port 50051` starts, and `RUST_LOG=debug`
    shows real Kraken log lines (connect, subscribe, book updates)
    alongside Binance/Bitstamp's existing ones — report the actual observed
    log lines, not just "it started."
  - `grpcurl -plaintext 127.0.0.1:50051 orderbook.OrderbookAggregator/BookSummary`
    against a live run — confirm at least one streamed `Summary` contains a
    `"kraken"`-labelled level (once Kraken's book is populated), alongside
    `"binance"`/`"bitstamp"` ones.
  - `git diff main --stat -- src/merge.rs` — zero diff.
  - `git diff main --stat` overall — confirm only `src/main.rs` touched
    since Phase 4.
- Done looks like: `docker compose`-free `cargo run` streams a real
  three-venue combined book over gRPC, observed directly via `grpcurl`.
- Rollback boundary: `src/main.rs`. Reverting it (Phases 1-4 in place)
  returns to the two-venue running system with Kraken's code present but
  unreachable — the safest possible partial-revert point in this plan.

### Phase 6: live verification — reconnect, staleness, three-venue merge

- Objective: Prove, against the real Kraken endpoint, the three behaviors
  spec.md's Acceptance Criteria name as live-only (not unit-testable): a
  forced disconnect triggers re-subscribe and discards prior `RefCell` state
  rather than resuming it; a quiet Kraken feed is excluded from the merge
  once its measured staleness threshold elapses; and the full three-venue
  merge produces a real combined book with `binance`, `bitstamp`, and
  `kraken` labels together.
- Main changes: none — this phase is observation only, no source diff
  expected. If it surfaces a bug (e.g. reconnect resuming stale state, or a
  checksum mismatch loop that never recovers, per spec.md's own flagged
  uncertainty about that path), fix it here as a small, scoped diff to
  whichever of Phases 2-4's files is responsible, and re-run the specific
  check that failed.
- Verification:
  - Forced disconnect (kill the proxy, if this environment tunnels through
    one, or interrupt the socket another way) mid-run: confirm Kraken
    re-subscribes on reconnect (log line), and that the first book published
    after reconnect only reflects data from the fresh `snapshot` onward —
    not a resumed pre-disconnect state. Report the actual observed sequence.
  - Quiet-Kraken staleness check: block Kraken's connection specifically
    (not Binance/Bitstamp) for longer than Phase 3's measured threshold,
    confirm via `grpcurl` that the streamed `Summary` narrows to
    binance/bitstamp-only levels, then restore and confirm `kraken` levels
    reappear.
  - Full three-venue merge: with all three feeds live and fresh, confirm at
    least one streamed `Summary` contains all three exchange labels among
    its top-10 bids/asks — report the actual observed labels and a sample
    of the levels, not just "it worked."
  - Checksum-mismatch path, if triggerable live (or otherwise via Phase 2's
    corrupted-fixture unit test, cited here as the fallback evidence):
    confirm a mismatch logs a warning and that the venue produces no further
    books until the next `snapshot` (via reconnect or re-subscribe) — per
    spec.md's own flagged uncertainty about whether this leaves Kraken
    silently stuck; report which behavior was actually observed.
  - `git diff main --stat -- src/merge.rs` — zero diff, final confirmation
    at the tip of the branch.
  - `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D
    warnings`, `cargo fmt --check` all clean at the tip.
- Done looks like: every live behavior spec.md's Acceptance Criteria names
  is actually observed and reported (not inferred from unit tests alone),
  and the checksum-mismatch recovery question spec.md flagged as unresolved
  has a real, reported answer.
- Rollback boundary: none (no source changes expected beyond any bugfix this
  phase's observation surfaces, which would carry its own small, described
  diff). This phase is the branch's final gate, not a further build-out.

## Cross-Cutting Considerations

- **This branch does not merge to `main`, regardless of outcome.** Every
  phase's done-criteria is "the researched behavior works and is reported,"
  not "this is ready to ship." If any part of this work is later judged
  worth porting to `main`, that is its own separate spec, not an extension
  of this one.
- **`src/merge.rs` zero-diff is checked at every phase**, not just the end —
  same discipline `007-merge` and `009-resilience` used. The temptation to
  fold Kraken-specific accounting (e.g. checksum state, accumulation) into
  `merge()` is real given how central it is to the combined-book output;
  spec.md is explicit this must not happen.
- **Two facts are measured, not assumed, before the code that depends on
  them is written**: price/qty type (Phase 1, blocks Phase 2) and staleness
  threshold (Phase 3's live-measurement sub-step, blocks the
  `staleness_threshold()` arm). Neither should be filled in with a
  placeholder "to be confirmed later" value that then quietly becomes
  permanent.
- **The `RefCell` order-dependence is a real correctness trap, flagged
  loudly in code, not just in this plan.** Phase 2's doc comment on
  `Kraken` and `impl Exchange for Kraken` is not optional polish — it's the
  one piece of documentation standing in for the trait signature's own
  inability to express "this implementation is stateful."
- **Reconnect discards local book state.** This applies regardless of which
  phase touches reconnect handling — Phase 6 verifies it live, but the
  underlying rule (a `Kraken::parse` cycle after a fresh connect always
  starts from `None`, never resumes a `RefCell`'s prior contents across a
  `run_once` boundary) should already be true by construction from Phase 2's
  implementation, since `run_feed` constructs a fresh `Kraken` per
  `run_feed` call but the same `&Kraken` is reused across `run_once`
  attempts within one `run_feed` call — confirm during Phase 2 whether the
  `RefCell` needs an explicit reset on reconnect (i.e. at the top of
  `run_once_inner`, not just relying on process/task restart) since
  `run_feed` does **not** construct a new `Kraken` per reconnect attempt,
  only per process start. This is a concrete implementation detail worth
  resolving explicitly in Phase 2, not assumed to fall out for free.
- **Untouched-files discipline.** `src/exchange/binance.rs`,
  `src/exchange/bitstamp.rs`, `src/merge.rs`, `src/model.rs`,
  `src/server.rs`, `proto/orderbook.proto` should all show zero diff at the
  tip of this branch. A phase whose diff unexpectedly touches any of these
  is a stop-and-flag condition, same as `009-resilience`'s equivalent rule.
- **Every citation in this plan points at `spec.md`, source files, or
  other tracked `specs/` packets** — nothing here is sourced from a file a
  reader of the pushed repo can't open.

## Verification Gates

Before this branch is considered ready to hand off (still not to merge):

- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` all clean at the tip of the branch.
- `cargo test` reports the pre-Phase-1 baseline plus every new Kraken test
  named across Phases 2-3, each individually identifiable by a
  behaviour-sentence name — report the actual observed count.
- Phase 1's captured `snapshot`/`update`/`heartbeat`/`status` messages, and
  the resolved price/qty type conclusion, are recorded and cited by Phase
  2's fixtures — not re-guessed.
- Phase 3's staleness threshold is backed by a reported live measurement
  (message count, window, max observed gap), not a transcribed or assumed
  number.
- The live three-venue merge (Phase 6) shows a real `grpcurl`-observed
  `Summary` containing `binance`, `bitstamp`, and `kraken` labels together.
- The live disconnect/re-subscribe check (Phase 6) shows Kraken discarding
  prior `RefCell` state on reconnect, not resuming it.
- `git diff main --stat -- src/merge.rs` shows zero diff — checked
  identically at every phase above, not just here.
- `git diff main --stat` at the tip shows only `src/exchange/kraken.rs`
  (new), `src/exchange/mod.rs`, `src/feed.rs`, `src/main.rs`,
  `Cargo.toml`/`Cargo.lock`, and `specs/012-kraken/` — no other path.
- This plan and every downstream commit message on this branch states
  plainly that the branch is not intended to merge into `main`.

## Expected Drift Triggers

If any of the following becomes true while implementing, update spec.md
before continuing rather than improvising past it:

- Phase 1's capture shows a price/qty shape, or a control-message shape,
  materially different from what spec.md's docs research anticipated (e.g.
  a third message type not covered by `book`/`heartbeat`/`status`/subscribe
  ack) — this is new information the spec's Proposed Design didn't have;
  record it before Phase 2 encodes an assumption around it.
- The checksum-mismatch recovery question (does a mismatch without a
  reconnect leave Kraken silently producing no further books?) resolves in
  the "yes, it silently stalls" direction during Phase 6 — spec.md already
  flags this as worth confirming rather than asserting; a confirmed stall
  is a real gap worth naming explicitly, not silently accepted.
- `run_feed`'s reuse of one `&Kraken` across reconnect attempts inside a
  single `run_feed` call (see Cross-Cutting above) turns out to require a
  `RefCell` reset mechanism spec.md's Proposed Design didn't anticipate
  (i.e. more than "the next `snapshot` naturally replaces the cell's
  contents wholesale") — if a genuinely new mechanism is needed, that's a
  design change to record, not a bugfix to fold in silently.
- The Kraken-specific ping branch (Phase 4) turns out to need more than a
  narrow, venue-scoped change to `run_feed`'s loop — e.g. if it can't be
  expressed without touching the `Exchange` trait's signature for every
  venue — that contradicts spec.md's explicit framing and is worth a
  human decision before proceeding.
- No live Kraken connectivity is reachable at all in this environment (no
  route through the configured proxy, or Kraken blocks the origin) — report
  this as "not verified here" for every affected phase, per this project's
  standing honesty convention, rather than silently substituting
  hand-built fixtures for what spec.md requires to be real captures.
- Any phase's `git diff main --stat` touches a file outside its declared
  scope (especially `src/merge.rs`, `src/model.rs`, or `src/server.rs`) —
  stop and reconcile before continuing.
