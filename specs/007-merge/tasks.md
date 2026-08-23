# Tasks: 007-merge

## Task Writing Rules

- Each task should describe a real unit of progress.
- Each task should name the expected files or areas touched.
- Each task should include explicit verification.
- Prefer behavior-level verification over mock-only checks.

## Phase 1: README cut-down alone (no code)

### 1. Cut README mechanics sections and grow a design-decisions section

- Files or areas: `README.md` only. No file under `src/` touches this task.
- Change:
  - Cut, mechanics only, current (326 lines / 2,404 words) → target (~150
    lines / under 1,200 words):
    - Layout: 57 → ~15 lines — a tree with one short line per file; point the
      reader to module doc comments for the rest, don't restate them.
    - Configuration: 51 → ~15 lines — a table (variable, default, what it
      controls); defaults live in `src/config.rs`, don't re-derive them here.
    - gRPC server: 54 → ~20 lines — the `grpcurl` reflection one-liner, a
      pointer to `proto/orderbook.proto`, and what the stream emits.
    - Docker: 35 → ~12 lines — `docker compose up`, the published port, the
      proxy env vars.
    - Quick start: 25 → ~10 lines.
  - Grow a new design-decisions section, two to four lines per point, no
    essays:
    - `watch` over `broadcast` (snapshot semantics, not an event log).
    - Merging sorted sides rather than sort-the-concat, framed as "don't
      discard ordering the venues already did" — not a speed claim.
    - `f64` newtypes over fixed-point, with the measurement that settled it
      (two million ETHBTC price-pair samples, no observable disagreement at
      8 decimals — see `specs/002-binance-feed/revisions.md` entry 1).
    - One `Exchange` trait, kept synchronous, introduced only once there were
      two implementations.
    - One process per pair, because `BookSummary` takes `Empty`.
    - Tie-break rule: state it as the stated default/intent for the merge
      that lands in Phase 2, **not** as documentation of shipped code — no
      line in this commit may describe `merge()`, crossed-book behaviour, or
      the tie-break rule as running behaviour, because none of it exists
      yet. If mentioned at all here, phrase it as "the merge (landing next)
      will use..." or similar, not "the merge does...".
  - Production section ("what would change for production") may grow
    slightly — schema criticisms, what wasn't built.
  - Do not touch any file under `src/`, `tests/`, `proto/`, or `Cargo.toml`
    in this task.
- Verification:
  - `wc -l README.md` — reports a line count close to ~150 (report the
    actual number observed).
  - `wc -w README.md` — reports under 1,200 words (report the actual number
    observed).
  - Read-through: confirm every cut section still points somewhere a reader
    can find the mechanical detail (module doc comments, `src/config.rs`,
    `proto/orderbook.proto`) — cutting a paragraph without a pointer is not
    acceptable, it's information loss, not a trim.
  - Read-through: confirm no sentence describes `merge()`, the tie-break
    rule, or crossed-book behaviour as already-shipped fact (it isn't yet at
    this point in the branch).
  - `git diff main --stat` — only `README.md` appears in the diff.
  - `git diff main --stat -- src/aggregator.rs` — zero diff (checkpoint 1 of
    3; nothing in this phase touches code, confirm rather than assume).
- Done when:
  - `README.md` is a standalone, reviewable doc diff against `main`: no code
    changed, ~150 lines / under 1,200 words, mechanics sections trimmed with
    pointers preserved, and a new design-decisions section present.
  - This lands as its own commit (e.g. `README: cut mechanics, grow design
    decisions`), before any `src/merge.rs` change.

## Phase 2: `src/merge.rs` rewrite

### 2. Add the `Side` enum with `better()` and `levels()`

- Files or areas: `src/merge.rs` only.
- Change:
  - Add `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Side { Bid,
    Ask }` exactly as specified in `plan.md`/`spec.md` — an enum, never a
    bool, anywhere in the merge path.
  - Implement `Side::better(self, a: &Level, b: &Level) -> Ordering`: price
    comparison direction depends on `self` (`Ask` ascending, `Bid`
    descending), `.then()` on amount descending — the same amount rule on
    both sides, only the price rule inverts.
  - Implement `Side::levels(self, book: &Book) -> &[Level]`: `Bid` reads
    `book.bids`, `Ask` reads `book.asks`.
- Verification:
  - Unit tests (see task 5 below) exercise `Side::better` directly, no
    `merge()` call needed to test the ordering rule in isolation.
  - `cargo build` — compiles.
- Done when:
  - `Side` exists as an enum with both methods, matching the signatures in
    `spec.md`'s Design section verbatim.

### 3. Implement `merge_side()`

- Files or areas: `src/merge.rs` only.
- Change:
  - Implement `fn merge_side(venues: &BTreeMap<Venue, &Book>, side: Side) ->
    Vec<Level>` using one `Peekable` cursor per venue (`venues.iter().map(...
    side.levels(book).iter().peekable())`), a loop bounded by `while
    out.len() < TOP_N`, `filter_map` to drop exhausted cursors, `min_by`
    with `side.better(a, b)` to pick the next element each iteration.
  - Add the required comment on the `cursors` line (or immediately above the
    `min_by` call) explaining: `min_by` returns the first of equal elements,
    `cursors` is built by iterating the `BTreeMap`, so a full price+amount
    tie resolves in `Venue`'s `Ord` order (Binance wins deterministically).
    This comment is load-bearing per `spec.md` — its omission is explicitly
    called out as the way a future reader "fixes" this to `HashMap` and
    silently reintroduces a flaky test. Do not skip it.
- Verification:
  - Unit tests (task 5, tests 3, 4, 6, 8) exercise this function indirectly
    through `merge()`.
  - `cargo build` — compiles with no borrow-checker workaround that
    contradicts the peekable-cursor shape in `spec.md`.
- Done when:
  - `merge_side` returns up to `TOP_N` levels for a given side, drawn from
    all venues in the map, correctly ordered per `Side::better`, and the
    determinism comment is present on the line `spec.md` specifies.

### 4. Implement `merge()` and delete `summarise()`

- Files or areas: `src/merge.rs` only.
- Change:
  - Replace `summarise(venues: &BTreeMap<Venue, &Book>) -> Option<Summary>`
    with `merge(venues: &BTreeMap<Venue, &Book>) -> Option<Summary>` —
    **same signature**, just the renamed function and a new body. No caller
    elsewhere in the crate should need a signature change; only the call
    site's function name changes (in `src/aggregator.rs` — see explicit note
    below).
  - Body: `let bids = merge_side(venues, Side::Bid); let asks =
    merge_side(venues, Side::Ask); let (best_bid, best_ask) =
    (bids.first()?, asks.first()?); Some(Summary { spread: best_ask.price -
    best_bid.price, bids, asks })` — no explicit branch for "no venues,"
    "one side empty," or "single venue"; all three fall out of the `?`
    operator, per `spec.md`'s "edge cases fall out of `?`" framing. No
    fabricated `0.0` spread fallback.
  - Do not introduce an internal `MergedBook` type — `merge()` keeps
    returning the proto `Summary`/`Level` types directly, per `spec.md`'s
    explicit "leave it, document the tradeoff in the README instead" call.
    That documentation is Phase 3's job, not this task's.
  - **Resolved (spec.md updated accordingly):** the rename is fine. Update
    `src/aggregator.rs`'s one call site (`merge::summarise(&venues)` →
    `merge::merge(&venues)`) and its two doc-comment references
    (`crate::merge::summarise` / `merge::summarise`) as a mechanical
    consequence of the rename — this is not a scope violation, since no
    logic or behavior changes. No pass-through wrapper; that would be
    indirection with no payoff, which this project's working style
    explicitly avoids. Matches step 4's Job A precedent: "call sites may
    change, no assertion may change." Nothing else in `src/aggregator.rs`
    (its logic, its structure, any other doc comment) may change.
- Verification:
  - `cargo build` — compiles, no remaining reference to `summarise` anywhere
    in the crate (`grep -rn summarise src/` returns nothing).
  - `cargo clippy --all-targets -- -D warnings` — clean.
- Done when:
  - `merge()` exists with the exact signature
    `pub fn merge(venues: &BTreeMap<Venue, &Book>) -> Option<Summary>`,
    `summarise` no longer exists anywhere in the crate, and the only change
    outside `src/merge.rs` is the one-line call-site rename in
    `src/aggregator.rs`.

### 5. Write the eight named tests

- Files or areas: `src/merge.rs`, `#[cfg(test)] mod tests` block only.
- Change: add exactly these eight tests, each named as a behaviour sentence
  (no `test_` prefix), each carrying the bug-caught framing from
  `spec.md`'s Tests section as a comment or doc line above the test:
  1. `ask_prefers_lower_price_bid_prefers_higher` — catches an inverted
     comparison in `Side::better`.
  2. `equal_price_prefers_larger_amount_on_both_sides` — catches someone
     inverting the amount rule along with the price rule in the `Bid` arm.
  3. `two_books_merge_into_correct_top_ten_and_spread` — two hand-built
     `Book`s across both venues produce the right top-10 on each side and
     the right spread.
  4. `equal_price_and_amount_across_venues_resolves_deterministically` —
     catches a regression back to `HashMap`; construct two venues with an
     identical price+amount level and assert the winning `Level.exchange`
     is always `"binance"` (or whichever venue sorts first under `Venue`'s
     `Ord`), run this assertion, not just once but in a way that would catch
     nondeterminism if `BTreeMap` were replaced with `HashMap` (e.g. assert
     the exact expected exchange label, not just "some exchange").
  5. `crossed_book_produces_negative_spread_without_panicking` — construct a
     book where one venue's best ask sits below the other venue's best bid;
     assert `spread < 0.0` and that the call does not panic. Catches an
     `abs()` or a clamp added by someone who mistakes this for a bug.
  6. `single_venue_still_merges` — only one venue present in the map;
     catches N=1 being treated as a special case that produces wrong or
     panicking output.
  7. `no_venues_returns_none` — an empty `BTreeMap`; catches a fabricated
     empty `Summary` being returned instead of `None`.
  8. `six_levels_returns_six_not_padded_to_ten` — a book with fewer than 10
     levels on a side; catches invented price levels padding the output.
  - No test beyond this list of eight in this file — per `spec.md`, "No test
    beyond this list."
  - Confirm by inspection that none of these eight tests touches
    `tokio::sync::watch` (all are pure, calling `merge`/`merge_side`/
    `Side::better` directly) — the interleave-sends-with-reads hazard from
    `specs/005-aggregator/revisions.md` entry 3 does not apply here, and
    this must stay true; do not introduce a `watch`-driven test in this
    file.
- Verification:
  - `cargo test merge::` — all eight tests run and pass, each identifiable
    individually by name in the output (not folded into an aggregate count).
  - `cargo test merge::equal_price_and_amount_across_venues_resolves_deterministically`
    and `cargo test merge::crossed_book_produces_negative_spread_without_panicking`
    run individually and pass — these are the two `spec.md` calls out as
    mattering most; confirm each by name, don't infer from the full-suite
    pass.
  - `cargo test` (full suite) — quote the actual total test count observed
    (baseline before this branch: 29 tests, 22 unit + 6 + 1 integration;
    report the actual new total, don't assume the arithmetic).
- Done when:
  - All eight tests exist with the names above, each passes, and `cargo
    test` output for this module reads as a list of the merge's actual
    guarantees (per the project's testing convention on names-as-sentences).

### 6. Full-gate verification for Phase 2 and the aggregator scope check

- Files or areas: verification only, no new file changes beyond tasks 2-5.
- Change: none — this task is pure verification, run after tasks 2-5 land.
- Verification:
  - `cargo build` — clean.
  - `cargo clippy --all-targets -- -D warnings` — clean.
  - `cargo fmt --check` — clean.
  - `cargo test` — full suite passes; quote the actual count.
  - `cargo run -- --pair ethbtc --port 50051` in one terminal, then
    `grpcurl -plaintext 127.0.0.1:50051
    orderbook.OrderbookAggregator/BookSummary` in another (network
    permitting) — confirm levels with `exchange == "binance"` and
    `exchange == "bitstamp"` both appear in the same stream response. Quote
    the actual observed output; if the environment can't reach one or both
    exchanges, report that honestly rather than skipping silently.
  - `git diff main --stat -- src/aggregator.rs` — diff limited to exactly
    the one call-site line and the two doc-comment references identified in
    task 4, nothing else. If the diff goes beyond that, stop and flag it —
    do not patch around it by widening the aggregator further.
  - `git diff main --stat` overall — confirm `src/merge.rs` and the minimal
    `src/aggregator.rs` change are the only files touched by this phase;
    `README.md` from Phase 1 also shows, nothing else.
- Done when:
  - `merge()` genuinely combines both venues' sorted sides into a correctly
    ordered top-10 with the right tie-break and honest crossed-book
    behaviour; all eight tests pass individually; the `src/aggregator.rs`
    diff matches exactly the minimal rename described in task 4, nothing
    more; this lands as its own commit (e.g. `merge: real two-venue merge
    with tie-break and crossed-book handling`).

## Phase 3: second README pass and full verification gate

### 7. Describe the shipped merge in the README

- Files or areas: `README.md` only.
- Change:
  - In the design-decisions section grown in Phase 1, update the merge-
    related points from "stated intent" to shipped fact now that Phase 2
    landed:
    - Describe the merge algorithm as peekable cursors walking both venues'
      already-sorted sides, not sort-the-concatenation — framed as "don't
      discard ordering the venues already did," not a speed claim (matching
      `spec.md`'s own framing verbatim in spirit).
    - State the tie-break rule as shipped fact: equal price on the same
      side resolves to the larger amount first, same rule on both sides.
    - Call out crossed-book behaviour explicitly enough that a reader
      seeing a negative spread in `grpcurl` output finds the explanation
      here rather than filing a bug — state that it's intentional, not a
      defect, and why (two independently-matched venues can legitimately
      cross even though a single exchange's own book cannot).
  - In the production section, add the internal-type decoupling tradeoff:
    `merge()` returns the `Summary`/`Level` proto types directly rather than
    through an internal `MergedBook`, and why that's fine with one consumer
    but would be worth revisiting with more (an added transport, a second
    output format, etc.).
  - Re-check word count: additions here should not regrow the README past
    Phase 1's target. If the merge description needs more room, trim
    elsewhere in the doc first rather than letting the total creep back
    toward 326 lines / 2,400 words.
  - Do not touch any file under `src/`, `tests/`, `proto/`, or `Cargo.toml`
    in this task.
- Verification:
  - `wc -l README.md` and `wc -w README.md` — confirm the total stays close
    to Phase 1's ~150 lines / under 1,200 words; report the actual numbers.
  - Read-through: no leftover references to single-venue output, no stale
    "step 5: not yet implemented" language anywhere in the README.
  - `git diff main --stat -- src/aggregator.rs` — still limited to the
    minimal rename from task 4, nothing more (checkpoint 3 of 3, final
    confirmation).
  - `git diff main --stat` overall — confirm the full branch touches only
    `README.md` and `src/merge.rs` (plus the one-line aggregator rename),
    nothing else.
- Done when:
  - Every claim in the README about the merge, the tie-break rule, and
    crossed-book behaviour matches what Phase 2 actually shipped, word
    count stays within target, and this lands as its own commit, distinct
    from Phase 1's (e.g. `README: describe the merge, tie-break, and
    crossed-book behaviour`).

## Final Verification

Before closing the packet, run the following once at the tip of the branch
— this is the representative real-behaviour path for this step (real
gRPC output, real two-venue data, real container run), not a rerun of the
per-phase checks alone:

- `cargo build`
- `cargo clippy --all-targets -- -D warnings`
- `cargo fmt --check`
- `cargo test` — quote the full actual test count and confirm all eight
  `merge.rs` tests appear individually by name, with tests 4
  (`equal_price_and_amount_across_venues_resolves_deterministically`) and 5
  (`crossed_book_produces_negative_spread_without_panicking`) specifically
  called out in the report.
- `cargo run -- --pair ethbtc --port 50051`, then in a second terminal
  `grpcurl -plaintext 127.0.0.1:50051
  orderbook.OrderbookAggregator/BookSummary` — quote the actual response,
  confirming both `"binance"` and `"bitstamp"` appear as `exchange` values
  in the same stream and the spread is computed across the two venues' best
  prices, not one venue alone. Report honestly if the environment cannot
  reach one or both exchanges.
- `docker compose up --build` — confirm it comes up through the configured
  proxy, both feeds connect, and the gRPC output through the container
  matches the direct-`cargo run` check above. Report actual observed
  behaviour, including any environment limitation.
- `git diff main --stat -- src/aggregator.rs` — matches exactly the minimal
  rename from task 4 (one call-site line plus two doc-comment references),
  nothing beyond it (checkpoint, final confirmation).
- `git diff main --stat` at the tip — confirm the whole branch touches only
  `README.md` and `src/merge.rs` (plus that one aggregator line).
