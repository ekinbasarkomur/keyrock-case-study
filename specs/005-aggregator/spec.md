---
spec_name: "Step 3 — wire the real feed through to gRPC"
spec_id: "005"
spec_folder: "005-aggregator"
status: "approved"
created_at: "2026-08-23"
updated_at: "2026-08-23"
created_by: "spec-synthesizer"
creation_mode: "human-brief"
source_inputs:
  - "inputs/001-step-3-brief.md"
source_agents: []
goal: "Connect the already-working Binance feed to the already-working gRPC server: feed sends parsed books over an mpsc channel to a new aggregator task, which calls a pure summarise() and publishes into the watch channel (now Option<Arc<Summary>>), replacing step 2's run_fake_writer."
purpose: "Step 2 proved the watch-to-gRPC-stream plumbing with fake data; step 1 proved the feed. Step 3 is the first true end-to-end milestone (real data, real gRPC stream) and lays the mpsc/aggregator shape now so step 4's second venue is a cloned Sender and one match arm, not a redesign."
parent_request: "step-3 brief, 2026-08-23 (specs/005-aggregator/inputs/001-step-3-brief.md)"
related_paths:
  - "src/aggregator.rs"
  - "src/merge.rs"
  - "src/exchange/binance.rs"
  - "src/main.rs"
  - "src/server.rs"
  - "tests/grpc.rs"
  - "README.md"
verification_level: "mixed"
complexity: "small"
---

# Spec: 005-aggregator

## Problem

Step 2 (`004-grpc-server`) shipped a gRPC server streaming a fake `Summary`
once a second. The Binance feed (step 1) runs alongside it in the same
`select!` but only logs to stderr — the two halves aren't wired together.
This is step 3 of 11. Connect them and delete the fake writer.

## Shape

```
Binance feed ──→ mpsc ──→ aggregator ──→ watch ──→ gRPC
```

The feed parses a `Book` and sends it down an `mpsc`. A new aggregator task
owns the last-seen book, calls a pure `summarise()`, and writes the result
into the existing `watch` channel that `src/server.rs` already streams to
clients. `run_fake_writer` is deleted.

## Scope

**IN:** `src/aggregator.rs` (new), `src/merge.rs` (new, `summarise()`),
`src/exchange/binance.rs` (parser switches to borrowed deserialisation),
`src/main.rs` (feed → mpsc, aggregator replaces the fake writer),
`src/server.rs` (delete `run_fake_writer`; watch carries `Arc<Summary>`, so
the `filter_map` adapting the watch stream into the gRPC response stream
changes too — this is where `Arc<Summary>` gets cloned out into an owned
`Summary` for `tonic`, which is the actual point decision 5's "clone moves
outside the lock" claim gets realised), tests, README.

**OUT, with the step each lands in:**
- Bitstamp feed — step 4
- Merging two books (this step summarises one book only) — step 5
- Reconnection and staleness handling — step 6
- Deduplicating identical publishes — step 5, because the comparison has to
  be on the final (merged) output, and that output doesn't exist yet
- An `Exchange` trait — still one implementation; step 4

## Settled decisions

Six decisions, already settled — implement as stated; flag it only if
implementation contradicts one of these, not to relitigate them. (Full
reasoning lives outside this repo; one line of why is recorded here.)

1. **mpsc + aggregator task now, with one venue.** More machinery than one
   exchange strictly needs, but it lands now so step 4's second venue is a
   cloned `Sender` and one new match arm, not a redesign. Channel is
   **bounded at 32** (`mpsc::channel(32)`) — not in the original brief, but
   confirmed with the user during spec review: an unbounded channel hides
   backpressure, and backpressure is the signal that's supposed to stay
   visible. 32 gives the aggregator slack for a brief lag (e.g. a slow
   subscriber wakeup) without hiding a genuinely stuck consumer.
2. **Channel, not a shared mutex.** The aggregator owns its state as a local
   variable in its own task — no lock to hold across an `.await`, no lock to
   forget to take.
3. **`Venue` is an enum (`enum Venue { Binance }`), not a string.** Adding
   Bitstamp in step 4 makes every place that needs updating fail to compile,
   instead of silently doing nothing.
4. **Binance parser switches to borrowed deserialisation** (`Vec<[&'a str; 2]>`
   with `#[serde(borrow)]`, not `Vec<[String; 2]>`) — cuts per-message
   allocations from ~107 to ~27 by not copying strings that are read once and
   dropped. If serde can't actually borrow (escapes forcing an owned
   `String`), report that rather than silently falling back to owned data.
   **The borrow ends inside `parse()`:** `Depth20<'a>` is only valid for the
   lifetime of the websocket message it borrows from, but `Book` outlives
   that message — it goes down the `mpsc` and into the aggregator's state.
   The chain is `message (String) → Depth20<'a> (borrowed) → Price/Amount
   (f64, Copy, independent) → Book (owned, independent)`, and only then can
   the message drop. `Book` must hold only owned or `Copy` data — nothing
   borrowed — and it works today only because `Price`/`Amount` wrap `f64`
   and are `Copy`. Write this down now: the next field added to `Book` (a
   symbol, a venue string) will hit an inexplicable lifetime error without
   this rule on record.
5. **`watch` carries `Option<Arc<Summary>>`, not `Option<Summary>`.** The
   `watch`'s internal lock is held while a subscriber reads; cloning a
   `Summary` under that lock is ~22 allocations, `Arc::clone` is one atomic
   increment. This moves the deep clone (still required — `tonic` wants a
   `Summary` by value) outside the lock; it reduces waiting under many
   subscribers, not total work done.
6. **`summarise(book: Option<&Book>) -> Option<Summary>` is pure, and lives
   in `src/merge.rs`.** No network, clock, or channels. It lives in
   `merge.rs` rather than `aggregator.rs` because step 5 widens the same
   function to take two books — right file now means a signature change
   later, not a file move. `Option` in the return carries "nothing to
   publish," consistent with step 2's `None`-before-first-value handling.

**Forward note for step 6:** because `summarise()` is pure, the staleness
check (which needs `Instant::now()`) belongs in the aggregator — deciding
*which* books to hand to `summarise()` — never inside `summarise()` itself.

## Aggregator state

```rust
struct VenueState {
    book: Book,
    last_update: Instant,
}

struct Aggregator {
    binance: Option<VenueState>,
}
```

`last_update` is unused this step — added now so step 6 adds a check, not a
field. `Option` because there's no data before the first message.

When the feed's `Sender` is dropped, the aggregator's `recv()` returns
`None`: the feed died, so the aggregator task ends too, which ends the
`select!` and the process — the same supervision behaviour step 2 already
established for the three tasks.

## Collecting on the "fake" label

Step 2 deliberately labelled placeholder levels `exchange == "fake"` so a
forgotten deletion of `run_fake_writer` couldn't look like working software.
This step adds a test asserting no `Level` anywhere carries `exchange ==
"fake"` — it catches exactly that forgotten deletion, with "the service
looks like it works" as the failure symptom.

## Tests

Each earns its place by naming the bug it catches, per this project's
testing convention — no test beyond this list:

- `summarise`, a 20-level book → 10 bids, 10 asks, correct spread — catches
  a truncation or sort bug in the top-10 selection.
- `summarise`, bids come back descending by price and asks ascending —
  currently defensible without this test, since Binance already sends sorted
  books and `summarise` only takes the first ten. Added now because step 5's
  `merge()` does real ordering work; this is the test that catches a merge
  that ruins it, not a bug summarise can introduce today.
- `summarise`, a 6-level book → 6, not padded to 10 — catches accidental
  zero-padding of a short book.
- `summarise(None)` → `None` — catches a panic or a synthesized-empty-book
  bug on the "no data yet" path.
- No `Level` anywhere carries `exchange == "fake"` — catches a forgotten
  deletion of `run_fake_writer` (see above).
- Update `tests/grpc.rs`'s existing assertion from `exchange == "fake"` to
  `exchange == "binance"` — it's currently pinned to the placeholder and
  would otherwise pass against dead code.

## Acceptance Criteria

- `grpcurl` against the running server streams real Binance data with
  `exchange == "binance"`.
- No `"fake"` literal anywhere in `src/`.
- The parser struct holds `&'a str`, not `String` — verify by inspection of
  `src/exchange/binance.rs`.
- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` all pass.
- `docker compose up` works through the proxy.
- README updated as part of this step, not deferred.

## Open Questions

None blocking. The six decisions above are settled by the input brief;
implementation should flag a contradiction rather than re-litigate them. The
one real gap in the brief — the mpsc channel's capacity — was resolved during
spec review (decision 1: bounded at 32). Borrowed deserialisation's
interplay with `Price::parse`/`Amount::parse` (both already take `&str`) is
expected to just work; decision 4 already carries the brief's own
instruction to report rather than silently fall back if it doesn't.
