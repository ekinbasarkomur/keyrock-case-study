---
spec_name: "Kraken as a third venue — research spec, not for merging into main"
spec_id: "012"
spec_folder: "012-kraken"
status: "approved"
created_at: "2026-08-26"
updated_at: "2026-08-26"
created_by: "spec-synthesizer"
creation_mode: "synthesized-inputs"
source_inputs:
  - "inputs/001-human-brief.md"
  - "inputs/002-kraken-api-docs.md"
source_agents: []
goal: "Decide, before any code is written, how a third venue whose stream is snapshot-then-incremental (not repeated-full-snapshot like Binance/Bitstamp) fits the existing Exchange trait and run_feed<E> loop — and lay out the real options rather than picking one unilaterally."
purpose: "The brief asks for Kraken 'structured exactly like Binance and Bitstamp,' but Kraken's book channel breaks the assumption every prior venue shared (each message is a complete, self-contained book). Whether that's a trait change, an interior-mutability trick, or a driver-loop change is a real architectural fork, and the wrong unilateral pick here would ripple through the two existing venues for a branch that isn't even merging to main."
parent_request: "specs/012-kraken/inputs/001-human-brief.md, 2026-08-25"
related_paths:
  - "src/exchange/mod.rs"
  - "src/exchange/binance.rs"
  - "src/exchange/bitstamp.rs"
  - "src/exchange/kraken.rs"
  - "src/feed.rs"
  - "src/model.rs"
  - "src/aggregator.rs"
  - "src/merge.rs"
  - "src/main.rs"
verification_level: "mixed"
complexity: "medium"
---

# Spec: 012-kraken

## Problem

This is research, not a step in the main build order. The current deliverable
(`main`) is a two-venue system: Binance and Bitstamp, both behind a
synchronous `Exchange` trait with a stateless `parse(&self, raw: &str) ->
Option<Book>`, driven by one shared `feed::run_feed<E>` loop. The brief for
this branch asks: what does adding a third venue cost, given the
architecture was supposedly designed to make that cheap?

Kraken's WebSocket v2 `book` channel answers that question the hard way. Per
the docs research in `inputs/002-kraken-api-docs.md`: Kraken sends exactly
one `type: "snapshot"` message on subscribe, and every message after that is
`type: "update"` — only the price levels that changed, with `qty: 0` meaning
"remove this level." There is no documented Kraken channel/mode that
re-sends a full snapshot repeatedly the way Binance's `depth20@100ms` or this
project's chosen Bitstamp channel (`order_book_<pair>`) do. Producing a
correct, complete `Book` from Kraken's stream therefore requires holding
local, mutable state across messages — applying each update to a
locally-held copy — which is a capability neither Binance's nor Bitstamp's
`parse` has ever needed, and one the current trait signature (`&self`, not
`&mut self`) has nowhere to put.

**This branch is explicitly research-only and will not be merged into
`main`.** The two-venue system on `main` stays the actual deliverable. This
spec exists to make the architectural fork legible and to surface the real
open questions the docs research found, not to commit to an implementation.

## Goal

A spec that:

1. States plainly, in the Proposed Design, the real options for reconciling
   Kraken's incremental-update model with the existing `Exchange`
   trait/`run_feed<E>` design — without picking one.
2. Carries forward everything else the brief asked for as a genuinely "like
   Binance and Bitstamp" mechanical integration (a `Venue::Kraken` variant, a
   `src/exchange/kraken.rs`, reconnection/re-subscribe wiring, a
   live-measured staleness threshold) as settled design, since none of that
   is in question.
3. Leaves the incremental-book architecture question, and every other
   genuine unresolved fact the docs research surfaced (price/qty type,
   symbol format generality, checksum verification, staleness number), as
   explicit Open Questions for a human to answer next — not a default
   quietly chosen here.

## Purpose

Kraken is the first venue this project has looked at where the "one pure
function per message" assumption baked into `Exchange::parse` doesn't hold.
Getting that decision wrong in a spec that nobody reviews before
implementation starts would either (a) contaminate the two working venues'
trait with machinery only Kraken needs, or (b) produce a Kraken
implementation that silently drops or double-applies updates. Writing the
fork down now, with real tradeoffs on each branch, is what makes "research,
not merged" honest — the point of this exercise is to know what a third
venue costs, not to have quietly paid it already.

## Out of Scope

- Merging any of this work into `main`.
- Changing `proto/orderbook.proto` (the wire schema is fixed by the brief).
- Changing `src/merge.rs` — see Invariants below; this holds regardless of
  which architecture option gets picked.
- Adding a fourth venue.
- Picking a fixed-point or `Decimal` representation for Kraken's
  price/qty — `Price`/`Amount` stay `f64` newtypes project-wide, per the
  already-settled step-1 decision; nothing here re-opens that.
- Generalizing the pair-format converter (Open Question 3, resolved below)
  beyond what a `"ethbtc"`-shaped token needs — no support for pairs with
  ambiguous base/quote splits (e.g. 4-letter quote currencies) unless a
  real need for one shows up.

## Current State

Verified by reading the source, not assumed:

- **`Venue`** (`src/exchange/mod.rs`) is a two-variant enum (`Binance`,
  `Bitstamp`), declaration order load-bearing for `BTreeMap<Venue, _>`
  iteration and `merge()`'s tie-break. Two `match`es live on `Venue`:
  `connect_rate()` (token-bucket capacity/refill) and
  `staleness_threshold()` (a `Duration` per venue, Binance 1.5s measured
  from its 100ms cadence, Bitstamp 8s measured live on 2026-08-24).
- **`Exchange`** (`src/exchange/mod.rs`) is a synchronous trait with four
  methods: `venue(&self) -> Venue`, `connect_url(&self, pair: &str) ->
  String`, `subscribe_message(&self, pair: &str) -> Option<String>`, and
  `parse(&self, raw: &str) -> Option<Book>`. All four take `&self`. Every
  call site uses a concrete generic `E: Exchange`, never `dyn Exchange`.
- **`Binance`** and **`Bitstamp`** (`src/exchange/binance.rs`,
  `src/exchange/bitstamp.rs`) are unit structs (`pub struct Binance;` /
  `pub struct Bitstamp;`) carrying only the trait impl — no fields, no
  state. Each `parse` is a pure function of its input string alone: for
  Binance, a flat `depth20` payload with 20 already-sorted bid/ask levels
  on every message; for Bitstamp, an enveloped `"data"` event with the same
  property, or one of three lifecycle events routed to `None`
  (`bts:subscription_succeeded`, `bts:request_reconnect`, `bts:error`).
  Both are self-contained per message — nothing carried from the previous
  call.
- **`feed::run_feed<E>`** (`src/feed.rs`) is the one shared driver loop:
  connects (direct or via the project's HTTP CONNECT proxy support),
  sends `subscribe_message` if `Some`, then loops on `ws.next()`, calling
  `exchange.parse(&text)` once per text message and sending the resulting
  `Book` down an `mpsc::Sender<(Venue, Book)>`. It owns reconnection: a
  jittered exponential backoff (1s/2s/4s/8s/16s/30s-capped, reset only
  after `STABILITY_WINDOW` = 30s held connected) composed with a per-venue
  `TokenBucket`. `exchange: E` is taken by value into `run_once<E>(exchange:
  &E, ...)` — the loop holds one `&E` for the duration of a connect cycle,
  and re-enters `subscribe_message`/`parse` fresh on every reconnect
  attempt (a new `run_once_inner` call). There is no facility today for a
  `parse` call to carry state forward to the *next* call except via
  `&mut self` on the trait method (not present) or interior mutability
  inside a concrete `Exchange` impl (nothing prevents this, but nothing
  today uses it either).
- **`model::Book`** (`src/model.rs`) is `{ bids: Vec<(Price, Amount)>, asks:
  Vec<(Price, Amount)>, last_update_id: u64, parse_started_at: Instant,
  parsed_at: Instant }` — always assumed, everywhere it's consumed, to be a
  **full** book (both `Binance::parse` and `Bitstamp::parse` always return
  every level the venue sent, not a delta).
- **`aggregator::record_and_publish`** (`src/aggregator.rs`) does
  `aggregator.venues.insert(venue, VenueState { book, last_update: now })`
  on every received `(Venue, Book)` — a full **replace** of the venue's
  prior state, not a merge or patch. There is no concept anywhere in this
  codebase of "apply a partial update to the existing book." If a
  `Kraken::parse` call ever returned only the *changed* levels as a `Book`,
  the aggregator would silently treat that partial set as the venue's
  *entire* book — a state-corrupting bug, not a crash, and not caught by
  any existing test.
- **`merge::merge`** (`src/merge.rs`) exports one public function:
  `pub fn merge(venues: &BTreeMap<Venue, &Book>) -> Option<Summary>`, plus
  the `Side` enum (not exported outside the module's own use). It is pure —
  no clock, no I/O, no channel — and reads only `Book.bids`/`Book.asks`. It
  has no notion of "venue count" hardcoded anywhere; a third `BTreeMap`
  entry flows through `merge_side`'s `Vec<Peekable<_>>` cursor construction
  with no code change required, *provided* the `Book` handed to it is
  already a correct, complete top-N snapshot at that instant — same
  precondition Binance and Bitstamp already satisfy.
- No `specs/006-bitstamp/spec.md`-equivalent architecture note exists yet
  for a venue whose wire protocol is snapshot-then-delta; this is the first
  one.

## Proposed Design

### What's settled — genuinely mechanical, matches Binance/Bitstamp precedent

- **`Venue::Kraken`**, added as the third variant (after `Bitstamp`, per
  declaration-order dependence noted above — appending, not inserting,
  keeps the existing two venues' `BTreeMap` ordering/tie-break behavior
  unchanged). Gets its own `connect_rate()` arm (no documented Kraken
  connection-rate limit was found for public spot WebSocket connects, per
  the docs research — same "stated guess, not fact" treatment Bitstamp's
  entry already uses) and its own `staleness_threshold()` arm, the value
  for which is an Open Question below (must be measured live, not
  transcribed from docs).
- **`src/exchange/kraken.rs`**, structured like `bitstamp.rs`: a unit
  struct `pub struct Kraken;`, `connect_url` returning `wss://ws.kraken.com/v2`
  (`pair` unused, same as Bitstamp — nothing pair-specific in the URL),
  `subscribe_message` returning `Some(...)` with the `{"method":"subscribe",
  "params":{"channel":"book","symbol":[...],"depth":10}}` JSON (per-connection
  subscription, confirmed necessary — Kraken does not restore subscriptions
  across a reconnect, per the docs research's Reconnection section).
- **Re-subscribe on every reconnect** is already `run_feed`'s existing
  behavior for any `Exchange` whose `subscribe_message` returns `Some` — no
  new mechanism needed, Kraken gets this for free from the current loop
  structure, same as Bitstamp does today.
- **Control/lifecycle messages routed to `None`**: `heartbeat`
  (`{"channel":"heartbeat"}`, sent ~1/sec whenever no book update is due)
  and `status` (sent on connect and on trading-engine status changes). The
  parser must branch on the top-level `"channel"` field (`"book"` vs
  `"heartbeat"` vs `"status"`) before looking at `"type"` — unlike Binance
  (flat payload, one shape) or Bitstamp (one `"event"` field to branch on),
  this is a two-level dispatch. Subscribe acks (`"method":"subscribe",
  "success":true/false`) are a third message shape on the same connection
  and also route to `None`; a `"success": false` ack should log at `warn`,
  matching the precedent Bitstamp's `bts:error` set (the one lifecycle
  message that means something is actually wrong gets a different log
  level than the benign ones).
- **Reconnect-then-rebuild, not reconnect-then-resume**: per the docs
  research's Reconnection section, Kraken's own guidance is explicit —
  discard any locally-held book state on reconnect, wait for the fresh
  snapshot, never attempt to merge deltas across a reconnect gap. Whatever
  architecture option below gets picked, this rule applies to it: local
  book state (if any) is scoped to one connection's lifetime, not carried
  across `run_feed`'s reconnect loop.
- **Real fixture tests**, same discipline as `binance.rs`/`bitstamp.rs`:
  a real captured `snapshot` message, a real captured `update` message, and
  each recognized control message (`heartbeat`, `status`,
  `success: false`) parsing to `None` without panicking. Per this project's
  fixture convention, these must come from an actual capture during
  implementation, not be hand-built — this spec does not include fixture
  text because no capture has happened yet.

### Decided — the three smaller Open Questions

- **Symbol format: build a general converter**, not a hardcoded
  `"ETH/BTC"`. Given `--pair`/`KEYROCK_PAIR`'s existing shape (a lowercase
  concatenated token, e.g. `"ethbtc"`), and that Binance/Bitstamp both
  already lowercase/reuse that same token directly with no base/quote
  split needed, Kraken's converter needs a real rule for where the split
  falls. The simplest general rule this project's existing pair set
  supports: split on a known quote-currency suffix (`btc`, `usd`, `eur`,
  etc., matching what Binance/Bitstamp already assume are the only quote
  currencies this project's `--pair` ever names), uppercase both halves,
  join with `/`. Implemented and tested in `kraken.rs`, not a shared
  utility — nothing else needs it.
- **CRC32 checksum verification: build it.** The first per-message
  integrity check in this codebase. Computed per the algorithm in
  `inputs/002-kraken-api-docs.md` (top 10 asks low→high, then top 10 bids
  high→low, digit-strings with `.` and leading zeros stripped,
  concatenated, CRC32'd) after every `Kraken::parse` call that produces a
  book, compared against the message's own `checksum` field. On mismatch:
  log a warning and — per Kraken's own documented remedy — clear the held
  `RefCell` state so the *next* message is forced to wait for a fresh
  `snapshot` rather than continuing to accumulate from a state already
  known to have diverged. Does not unsubscribe/reconnect on its own; the
  next `snapshot` arrives only via reconnect or an explicit
  re-subscribe, so a checksum failure without a reconnect would otherwise
  leave the venue silently producing no further books until one occurs —
  worth confirming live during implementation, not asserted here.
- **Client-side ping: build proactive handling.** `run_feed`'s read loop
  tracks the time of the last message received on the Kraken connection;
  if idle past a threshold comfortably under Kraken's documented 60s
  (e.g. 30s, mirroring this project's existing "well under the real
  limit" margins elsewhere, such as the staleness thresholds), send
  `{"method":"ping"}`. Kraken-specific — Binance needs no client-initiated
  ping (server-initiated, already auto-answered by `tokio-tungstenite`)
  and Bitstamp documents no ping requirement at all, so this is not a
  change to the shared `run_feed` behavior for every venue, only a
  Kraken-triggered branch (exact mechanism — a per-venue opt-in on the
  `Exchange` trait vs. Kraken-specific logic living inside option (a)'s
  own connection handling — decided during implementation).

### Decided — the central architectural fork

**Resolved: option (a), interior-mutable state inside `Kraken`'s own
struct.** `pub struct Kraken { book: RefCell<Option<Book>> }`.
`parse(&self, raw: &str) -> Option<Book>` keeps its exact current
signature — zero change to the `Exchange` trait, `Binance`, `Bitstamp`, or
`run_feed`. A `snapshot` message replaces the cell's contents wholesale; an
`update` message mutates the held book (applying each changed level,
removing any level whose `qty` is `0`) and returns a clone of the resulting
full state. `RefCell`, not `Mutex` — nothing here is genuinely concurrent;
one `&Kraken` is held for the duration of a single-threaded `run_once` call
in `src/feed.rs`.

Accepted, going in with eyes open, per the tradeoffs already laid out
below: this makes `Kraken::parse` order-dependent in a way `Binance`'s and
`Bitstamp`'s aren't — calling it twice with the same `update` message would
silently double-apply that delta, and nothing in the trait signature
signals this to a future reader. Worth a loud comment on the struct and the
`impl Exchange for Kraken` block saying so explicitly, since the trait
itself won't.

The other three options, kept below for the record (not chosen, but the
reasoning that ruled them out):

**(a) — CHOSEN, see "Decided" above. Interior-mutable state inside `Kraken`'s own struct** — e.g.
`pub struct Kraken { book: RefCell<Option<Book>> }` (or `Mutex`, though
`RefCell` suffices since nothing here is genuinely concurrent — one
`&Kraken` is held for the duration of a single-threaded `run_once` call).
`parse(&self, raw: &str) -> Option<Book>` keeps its exact current
signature; internally, a `snapshot` message replaces the cell's contents
wholesale, an `update` message mutates the held book and returns a clone of
the current full state.
- *Blast radius*: zero on `Exchange`'s signature and zero on
  `Binance`/`Bitstamp` — both keep being pure unit structs with no
  behavior change. Confined entirely to `kraken.rs`.
- *Fits the trait's stated philosophy?* Arguably not — the trait's own doc
  comment and the project's convention frame `parse` as describing
  protocol *data*, and this makes one implementation secretly stateful
  behind a `&self` signature that looks pure everywhere else it's called.
  A reader of `run_feed` has no way to see, from the trait alone, that one
  venue's `parse` is order-dependent (calling it twice with the same
  `update` message a second time would silently double-apply that delta) —
  that's a real correctness trap for anyone extending or testing this
  implementation later, distinct from `Binance`/`Bitstamp::parse`, which are
  safe to call with the same input any number of times.
- *Testability*: `#[cfg(test)] mod tests` in `kraken.rs` would need to
  construct a fresh `Kraken` per test (state doesn't reset itself) and
  drive it through `snapshot` → `update` → `update` in sequence to test the
  accumulation — a different test shape from Binance/Bitstamp's
  call-once-per-fixture pattern, though still confined to unit tests per
  this project's filing convention (internal, no `pub` surface added).

**(b) Change the trait's `parse` to `&mut self`** — touches every
implementation, even though only Kraken needs the mutability.
- *Blast radius*: every call site of `parse` across `binance.rs`,
  `bitstamp.rs`, `feed.rs`, and every existing unit test that calls
  `Binance.parse(...)`/`Bitstamp.parse(...)` as an immutable value (most of
  them do, per the fixture tests read above) needs `mut` added, and
  `run_feed`'s `exchange: &E` parameter would need to become `&mut E`
  through the whole call chain (`run_once`, `run_once_inner`).
- *Fits the trait's philosophy?* Explicitly makes mutability part of the
  trait's data-description contract for every venue, not just the one that
  needs it — a real widening of what every future venue's `parse` is
  allowed/expected to do, for a capability two of three current venues
  never use.
- *Testability*: existing Binance/Bitstamp tests would compile with a
  trivial `mut` addition and behave identically (they never actually
  mutate); no meaningful change to their test shape.

**(c) Maintain incremental book state inside `src/feed.rs`'s `run_feed`
loop itself**, generically — e.g. a new trait method like
`fn apply_update(&self, book: &mut Book, raw: &str) -> bool` (or an enum
return from `parse` distinguishing "here is a full snapshot" from "here is
a delta to apply to state you're holding"), with `run_feed` owning a
`Option<Book>` local variable across loop iterations and calling the right
method depending on message type.
- *Blast radius*: touches the trait (a new method, or a changed return
  type on `parse`) and `run_feed`'s loop body for every venue, even though
  Binance and Bitstamp would implement the new method/branch as a no-op
  (every message is already a full snapshot for them). Every existing
  `Exchange` impl needs to satisfy whatever new trait shape gets chosen.
- *Fits the trait's philosophy?* Closer to the stated goal than (a) or (b)
  in one sense — the *loop*, not an individual venue implementation, ends
  up owning the "state carried across messages" concern, which is at least
  consistent with the existing split between "what varies per venue" (the
  trait) and "what's shared" (`run_feed`). But it also means `run_feed` is
  no longer venue-agnostic in the way it is today — it would need to know
  the difference between "this venue hands me complete books" and "this
  venue hands me deltas I must accumulate," which is closer to control
  flow than the trait's stated "abstract over data, not control flow"
  design principle explicitly warns against.
- *Testability*: the accumulation logic would live in `src/feed.rs`, which
  today has no book-construction logic at all (it only routes `Book`s
  already produced by `parse`) — testing it would need either a new unit
  test surface in `feed.rs` or an integration-style test driving
  `run_feed` end-to-end, a different testing shape from every other
  parse-level unit test in this project.

**(d) Something else** — e.g. a free function outside the trait entirely
(`kraken::apply_update(state: &mut Book, raw: &str)`) called directly from
a Kraken-specific wrapper around `run_feed` rather than through the shared
generic loop at all, effectively opting Kraken out of the "one loop drives
every venue" design. Not fully explored here — flagged because ruling it
out ("no, Kraken must go through the same generic loop") is itself a
decision, not a given, and the brief's request to be "exactly like Binance
and Bitstamp" is itself in tension with a protocol that doesn't share their
per-message-completeness property.

**No option above is chosen in this spec.** Each has a genuine, different
cost on the codebase's existing trait/loop contract; the tradeoff is not
close enough to call without a human weighing the blast radius against how
much this branch's research is meant to prove. See Open Questions.

## Acceptance Criteria

These apply only once an Open Question below picks an architecture and
implementation proceeds — restated here as the bar a later plan/tasks
packet on this same branch would need to hit, not as claims about work
already done:

- `src/exchange/kraken.rs` exists, implementing whichever `Exchange`-family
  shape the architecture Open Question resolves to, with `Venue::Kraken`
  wired into `src/exchange/mod.rs`'s `connect_rate()`/`staleness_threshold()`
  matches.
- A third `feed::run_feed(Kraken, ...)` spawn exists in `src/main.rs`'s
  `JoinSet`, alongside the existing Binance/Bitstamp spawns.
- Real fixture tests exist in `kraken.rs`'s own `mod tests` for: a captured
  `snapshot` message, a captured `update` message (asserting the
  accumulated book reflects the update correctly, not just that parsing
  didn't panic), and each recognized control message type parsing to
  `None`.
- `git diff main --stat -- src/merge.rs` shows zero diff — the same scope
  check step 5 used for the Binance/Bitstamp merge, restated here since
  it's this project's single most emphasized cross-venue invariant.
- Kraken's connection survives a forced disconnect: re-subscribes on
  reconnect, and a quiet Kraken feed is excluded from the merge once its
  measured staleness threshold elapses (verified live, not by unit test
  alone — same as `009-resilience`'s Bitstamp staleness verification).
- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` all pass.

## Invariants and Critical Don'ts

- **`merge()` does not change.** Whatever Kraken's `Exchange`
  implementation ends up looking like, it must hand `merge()` the same
  `BTreeMap<Venue, &Book>` shape it already accepts, with each `Book` a
  correct, complete top-N snapshot at that instant — `merge()` itself gains
  no venue-count-specific logic, no Kraken-specific branch, nothing. This
  has held for every venue added so far and is this project's single most
  emphasized invariant; it applies here even though this branch won't
  merge.
- **Never present Kraken's price/qty type or the incremental-book
  architecture as settled from docs alone.** The docs research explicitly
  found a real inconsistency (float vs. string across two different Kraken
  doc pages) and could not resolve it without a live capture — do not
  guess a `f64`-vs-`&str` parsing shape for `kraken.rs` before that capture
  happens.
- **Reconnect discards local book state, per Kraken's own documented
  guidance** — do not attempt to carry accumulated book state across a
  reconnect gap regardless of which architecture option is chosen.
- **This branch does not merge to `main`.** Nothing here should be read as
  authorizing a merge; say so again in any later `plan.md`/`tasks.md` or
  README note this branch produces.

## Risks and Tradeoffs

- **The central risk is picking the wrong architecture option under
  research-branch time pressure.** Options (b) and (c) both touch code that
  currently works and is tested for two production venues; a mistake there
  is a real regression risk to `main`-adjacent code even on a branch that
  won't merge, if the branch is later cherry-picked from.
- **Kraken's ping requirement is a directional reversal from both existing
  venues** — Binance's server pings the client (already auto-answered by
  `tokio-tungstenite`); Bitstamp documents no ping requirement at all;
  Kraken expects *this project's client* to send one. Building this (see
  "Decided" above) means `run_feed`'s read loop needs an idle-timer branch
  that's Kraken-specific, not shared — a small but real change to a loop
  that's otherwise identical for every venue today; keep the branch scoped
  narrowly so Binance/Bitstamp's read path is unaffected.
- **The symbol converter (see "Decided" above) is only as correct as its
  quote-currency suffix list.** A pair whose quote currency isn't in the
  hardcoded suffix set fails to convert — acceptable for this branch's
  scope (this project's own default and test pairs), a real limitation if
  this were ever generalized further.

## Testing Strategy

Required real verification, once an architecture option is chosen:

- Real fixture tests in `kraken.rs`'s own `mod tests`, from actual captured
  `snapshot` and `update` messages (via the project's existing HTTP CONNECT
  proxy setup) — not hand-built JSON, matching this project's binding
  fixture convention.
- A test proving the *accumulation* behavior specifically (whichever
  architecture option is picked): feed a captured snapshot, then a captured
  update, and assert the resulting book reflects the update's changed
  levels correctly — not just that each message individually parses
  without panicking. This is the one test shape that's genuinely new here;
  every other venue's parse tests are single-message, and that alone isn't
  sufficient evidence Kraken's stateful path works.
- Recognized control messages (`heartbeat`, `status`, a `"success":false`
  subscribe ack) each parse to `None` without panicking — same shape as
  `server_shutdown_message_parses_to_none_without_panicking` /
  `bts_request_reconnect_parses_to_none_without_panicking`.
- A live-run verification that a forced Kraken disconnect triggers
  re-subscribe on reconnect and that the reconnect discards prior local
  book state rather than resuming it (per the Reconnection invariant
  above) — this is a live/manual check, not a unit test, matching how
  `009-resilience` verified Bitstamp's `bts:request_reconnect` handling.
- A live-measured staleness window for Kraken (see Open Questions) —
  report the observed max gap from a real timed connection, the same
  experiment shape `009-resilience` used for Bitstamp's 8s figure, before
  setting `Venue::Kraken`'s `staleness_threshold()`.
- `git diff main --stat -- src/merge.rs` showing zero diff, as a scope
  check on every commit that touches this packet.

Optional supporting checks:

- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`, same
  as every other step in this project.

## Rollback Plan

This branch is research-only and is not intended to merge into `main`. If
the architecture question resolves in a direction that turns out to be a
dead end mid-implementation, the rollback is simply not merging this
branch — `main`'s two-venue system is unaffected regardless of how far this
branch gets. No production rollback plan is needed because there is no
production change; if any part of this work is later judged worth
porting to `main`, that would be its own separate spec, not this one.

## Open Questions

Four of the original six were real choices and are now resolved — the
human weighed the tradeoffs above and answered directly, not a default
picked here. The remaining two aren't choices at all; they're facts that
don't exist yet because nothing has been captured or measured. Both block
implementation just as hard as an unresolved choice would.

### Resolved

1. **Architecture: option (a), interior-mutable state inside `Kraken`'s own
   struct.** See "Decided — the central architectural fork" above.
2. **Symbol format: build a general converter**, not a hardcoded
   `"ETH/BTC"`. See "Decided — the three smaller Open Questions" above.
3. **CRC32 checksum: build it.** See "Decided — the three smaller Open
   Questions" above.
4. **Client-side ping: build proactive handling.** See "Decided — the
   three smaller Open Questions" above.

### Resolved by live capture (2026-08-26)

5. **Price/qty representation: bare JSON floats, confirmed.** Connected
   live to `wss://ws.kraken.com/v2` (via this project's HTTP CONNECT proxy)
   and subscribed to the `book` channel for `ETH/BTC`. A real captured
   snapshot: `{"price":0.031348,"qty":0.03740000}` — no quotes. A real
   captured update: `{"price":0.031347,"qty":0.00000000}` — same, and
   confirms `qty: 0` (unquoted zero) is the real remove-this-level signal,
   not a string `"0"` or `"0.00000000"`. The checksum guide's string
   example was that guide's own illustrative formatting for explaining the
   digit-stripping algorithm, not the wire format. `kraken.rs`'s struct
   shape is therefore `Vec<Level<'a>>` with typed `f64` `price`/`qty`
   fields (via `serde`'s numeric deserialization), not a borrowed
   `Vec<[&str; 2]>` the way Binance/Bitstamp's levels are — the one place
   Kraken's parsing shape genuinely differs from the other two venues'.
   Also confirmed live in the same session: `status` arrives once,
   immediately after connecting (`{"channel":"status","type":"update",
   "data":[{"version":"2.0.10","system":"online","api_version":"v2",
   "connection_id":...}]}`), the subscribe ack arrives before the
   snapshot, and `heartbeat` is exactly `{"channel":"heartbeat"}` with no
   other fields — all matching the docs research, no surprises there.

### Resolved by live measurement (2026-08-26)

6. **Staleness threshold: 12s.** Held a live `wss://ws.kraken.com/v2` `book`
   connection open for 300.6s (ETH/BTC, via this project's proxy), counting
   gaps between real `book`-channel messages only (`snapshot`/`update` —
   `heartbeat`/`status` don't carry book state and were excluded from the
   gap measurement, the same reasoning `009-resilience` used for Bitstamp).
   16,444 book messages in the window (~54.7/s — a much higher natural
   cadence than Bitstamp's ~2.5/s, since Kraken's incremental model means
   even a single-level change is its own message), max observed gap
   2.914s, median 0.000s (updates arrive in bursts), mean 0.018s. Threshold
   set at ~4x the observed max, rounded up — same rule Bitstamp's 8s figure
   used (1.795s max → 8s) — giving `2.914 * 4 ≈ 11.66` → **12s**.
