# Kraken as a third venue — research spec, not for merging into main

## Request

Add Kraken as a third exchange feed, following the exact same shape the
project already uses for Binance and Bitstamp: a real `Exchange` trait
implementation, wired into the same generic `feed::run_feed<E>` driver
loop, with correct reconnection (backoff, jitter, stability-gated reset,
token bucket) and correct staleness handling (a real per-venue threshold,
measured live against a real connection — not guessed), the same way
`006-bitstamp` and `009-resilience` did it for the first two venues.

**This branch is explicitly for research, not for merging into `main`.**
The existing two-venue system (`main`) stays the deliverable; this is a
separate exploration of what a third venue costs to add, given the
architecture was supposedly designed to make that cheap. Say plainly in
the spec, and repeat in the README if one gets written, that this doesn't
land in `main`.

## What "exactly like Binance and Bitstamp" is expected to mean

- A new `src/exchange/kraken.rs`, structured like `binance.rs`/
  `bitstamp.rs`: a unit struct implementing the `Exchange` trait
  (`venue()`, `connect_url()`, `subscribe_message()`, `parse()`).
- A new `Venue::Kraken` variant in `src/exchange/mod.rs`, with its own
  `connect_rate()` and `staleness_threshold()` — the threshold **measured
  live**, the same discipline used for Bitstamp's 8s figure
  (`specs/006-bitstamp/`), not assumed from the docs.
- Wired into `src/main.rs`'s `JoinSet` the same way the other two feeds
  are — a third `feed::run_feed(Kraken, ...)` spawn.
- Real fixture tests in `kraken.rs`'s own `mod tests`, from an actual
  captured message — not hand-built JSON — same as `binance.rs`/
  `bitstamp.rs`.
- Kraken's connection needs to survive the same disconnect handling
  everything else does: reconnect with backoff, re-subscribe on every
  reconnect (confirmed necessary — Kraken's subscription is
  per-connection, like Bitstamp's), and get excluded from the merge by
  staleness when it goes quiet.

## What came up in discussion before this spec, worth carrying forward

- **`merge()` must not change.** Whatever Kraken needs, the pure merge
  function stays untouched — this has held for every venue added so far
  and is this project's single most emphasized invariant.
- **A prior discussion floated moving `connect_rate()` off `Venue`'s
  central `match` and onto the `Exchange` trait itself**, so a new venue's
  connection-rate data lives entirely in its own file. That's a real,
  separately-approved idea, **not** part of this spec — don't fold it in
  unless asked. (`staleness_threshold()` can't move the same way without a
  bigger change — `src/aggregator.rs` only ever has bare `Venue` tags, not
  a concrete `Exchange` instance, by the time a book reaches it.)
- **The `Exchange` trait was deliberately kept synchronous** and
  `parse(&self, raw: &str) -> Option<Book>` was deliberately pure/stateless
  — this matched Binance and Bitstamp because both send a complete,
  self-contained book on every message. Whether that assumption holds for
  Kraken is an open question raised by the API research (see the other
  input file) — read that before assuming this integration is purely
  mechanical.

## Scope

- Real capture of a Kraken message (via the project's existing proxy
  setup, same as the earlier live Bitstamp probe) before writing any
  parser test, same "investigate, then decide" discipline as every prior
  venue.
- Live measurement of Kraken's actual staleness gap, the same experiment
  shape as `009-resilience`'s Bitstamp measurement (a timed live window,
  reporting the observed max gap, not a guess).
- Whatever `Exchange`/trait-shape change (if any) the incremental-book
  finding forces — decided in the spec, not discovered mid-implementation.
- Out of scope: merging to `main`, changing `proto/orderbook.proto`,
  touching `merge.rs`, building Kraken's checksum verification unless the
  spec's Open Questions decide it's worth it, adding a fourth venue.

## Process

Same packet discipline as every prior step: branch `012-kraken`, spec
packet first, `spec.md` → `plan.md` → `tasks.md`, real verification at
each phase. Synthesize the spec from this brief plus the separate Kraken
API-docs research file, then stop — there are real open questions the API
research surfaced (the incremental-book model, the price/qty type
discrepancy in Kraken's own docs, the pair-format mismatch) that need an
answer before implementation starts, not a guessed default.
