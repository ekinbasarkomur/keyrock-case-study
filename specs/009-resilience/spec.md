---
spec_name: "Step 7 — Reconnection, Staleness, and Supervision"
spec_id: "009"
spec_folder: "009-resilience"
status: "approved"
created_at: "2026-08-25"
updated_at: "2026-08-25"
created_by: "spec-synthesizer"
creation_mode: "human-brief"
source_inputs:
  - "inputs/001-step-7-brief.md"
source_agents: []
goal: "The process survives venue disconnects indefinitely: it reconnects with backoff/jitter, respects a per-venue connection-rate ceiling, excludes stale venues from the merge, exits with a clear message if a pair never produces data from either venue, and the client shows live per-venue status."
purpose: "Steps 0-6 wired a real two-venue merge, but nothing survives a disconnect today — run_feed returns when a socket closes, the task ends, select! fires, and the process exits. Binance force-closes every connection at the 24h mark (documented behaviour), so today this cannot run for a day."
parent_request: "specs/009-resilience/inputs/001-step-7-brief.md (human brief, step 7 of the project's build order)"
related_paths:
  - "src/feed.rs"
  - "src/exchange/mod.rs"
  - "src/aggregator.rs"
  - "src/main.rs"
  - "src/bin/client.rs"
  - "src/merge.rs"
  - "README.md"
  - "compose.yml"
verification_level: "mixed"
complexity: "large"
---

# Spec: 009-resilience

## Problem

None of the code that exists after step 6 survives a disconnect. `run_feed`
returns as soon as a websocket closes; the owning task ends; the `select!` in
`src/main.rs` fires on whichever task ended first; the whole process exits.
Binance force-closes every connection at the 24-hour mark (documented, not
hypothetical), so as it stands this cannot run for a day, let alone longer.

Bitstamp adds a second failure mode discovered experimentally, not assumed:
subscribing to `order_book_xyzabc` and `order_book_totalgarbage` both receive
`bts:subscription_succeeded` and then silence, with the connection staying
open. Bitstamp does not validate the channel name. There is therefore no
error path that distinguishes a bad pair from a genuinely quiet market — a
fact that removed two mechanisms this step had originally planned (a
give-up-after-N-failures counter, a `bts:error` special case): neither has a
signal to act on. See "What's dropped, and why" below.

## Scope

**IN:**
- `src/feed.rs` — reconnection loop, backoff, jitter, token bucket
- `src/exchange/mod.rs` — per-venue staleness thresholds and connection rate limits
- `src/aggregator.rs` — staleness filter, grace period for never-seen data
- `src/main.rs` — `JoinSet` migration replacing `select!`
- `src/bin/client.rs` — venue status in the terminal header
- Tests (see Testing Strategy)
- README

**OUT — and this is load-bearing:**
- **`src/merge.rs` must not change.** Staleness decides which venues' books
  get handed to `merge()`; `merge()` itself never sees a clock, and this is
  the step that would break that separation if anything did — `merge()` has
  been a pure function since it was first introduced (no clock, no I/O, no
  channel reference) specifically so it stays unit-testable against
  hand-built fixtures. This step is where that separation is under the most
  pressure, since staleness is itself a time-based decision, which is
  exactly why it stays a pre-filter on the map handed to `merge()` rather
  than logic inside it. The scope check for this whole packet is
  `git diff main --stat -- src/merge.rs` showing no diff.
- Latency instrumentation (step 8) — not built here.
- Graceful shutdown on `SIGTERM` — not built here; the spec should note
  where it would slot in, but does not build it.

## Design

### Piece 1 — `JoinSet` migration, behaviour unchanged

`select!` needs one branch per task (a fifth venue needs a fifth branch), and
more importantly it fires once and exits the block — there is no way to say
"that was a feed, keep going," which pieces 5 and 6 both need.

```rust
#[derive(Debug, Clone, Copy)]
enum Component { Feed(Venue), Aggregator, Server }

type TaskResult = (Component, Result<(), anyhow::Error>);
```

Two problems this creates that the migration must solve:

1. Every spawned task must produce the same type — wrap each `tokio::spawn`
   call in an async block that normalises its result into `TaskResult`.
2. `select!` told you which task ended by which branch fired; a `JoinSet`
   does not — the identity has to travel alongside the result. This is not
   just for logging: it is what pieces 5 and 6 decide on (a feed ending is
   not fatal after this step; the aggregator or server ending still is).

`JoinSet::join_next()` has one more layer of error handling than `select!`
had:

```rust
match tasks.join_next().await {
    Some(Ok((c, Ok(()))))          => // ended cleanly
    Some(Ok((c, Err(e))))          => // returned an error
    Some(Err(je)) if je.is_panic() => // panicked
    Some(Err(je))                  => // cancelled
    None                           => // set is empty
}
```

`is_panic()` distinguishes a bug from a cancellation and the two must not be
logged identically.

**This commit must not change behaviour.** Keep today's policy: any task
ending exits the process. Existing tests must pass unchanged (call sites may
move where a signature forces it, but no assertion should need to change).
Later commits in this packet change the exit policy; if something breaks
during implementation, this separation is what tells you whether it was the
migration itself or a new rule from a later piece.

### Piece 2 — reconnection with backoff and jitter

Wrap connect-and-read in an outer loop inside `run_feed` so it never returns
on a closed socket.

- Backoff sequence: 1s, 2s, 4s, 8s, 16s, capped at 30s.
- Jitter every wait by a random multiplier in `0.5x`-`1.5x`. Without jitter,
  when a venue comes back, every client that was disconnected follows the
  same schedule and all retry in the same second — a synchronised herd is
  how a shared outage turns into a self-inflicted rate-limit hit. Binance
  allows 300 connection attempts per five minutes per IP.
- `rand` is already a dependency, resolved to `0.10` — recent enough that
  most examples found online are `0.8`/`0.9` syntax and will not compile.
  Work from the `docs.rs` page for the resolved version, not memory.
- Bitstamp must re-subscribe after every reconnect — its subscription is
  per-connection, and without re-sending it the socket opens, no error
  appears, and no data ever arrives. `run_feed` already calls
  `subscribe_message`; wrapping that call inside the reconnect loop gets
  this for free. **Confirm this is actually true during implementation**
  (i.e. that the existing call site sits inside the new loop, not before
  it) rather than assuming the refactor preserved it.

### Piece 3 — reset the backoff on stability, not on connect

The subtle piece. Resetting the backoff the moment `connect()` succeeds looks
correct and is not: if a venue accepts the connection and then drops it
immediately (overloaded, half-banned, or a stream name that yields nothing
until it times out), the loop becomes:

```
connect ok -> reset -> wait 1s -> connect ok -> drop -> reset -> wait 1s -> ...
```

One attempt per second is 300 attempts in five minutes — exactly Binance's
stated limit — and the backoff never actually engages because it is reset
every cycle.

Fix: the reset only fires once the connection has proven itself stable, not
merely established:

```rust
if connected_at.elapsed() > Duration::from_secs(30) {
    backoff.reset();
}
```

Five lines, and it closes the hole at the root rather than papering over the
symptom.

### Piece 4 — a token bucket per venue

Piece 3 fixes the specific thrash pattern that was identified. The token
bucket is a ceiling for the connection-rate failure modes that were not
thought of — backoff answers "when do I try next" (reactive); the bucket
answers "am I allowed to try" (absolute). They compose:

```rust
backoff.wait().await;
bucket.acquire().await;
connect().await
```

- Binance's documented limit is 300 attempts per 5 minutes → one token per
  second refill rate.
- Bucket capacity should be small (five, not three hundred) — a capacity of
  300 would let the very first burst spend the entire five-minute allowance
  in one go.
- Put the rate next to the staleness threshold on `Venue`, so every
  per-venue fact lives in one place and both `match`es stay exhaustive:

```rust
impl Venue {
    fn connect_rate(self) -> (f64, f64);      // (capacity, tokens per second)
    fn staleness_threshold(self) -> Duration;
}
```

- Bitstamp does not publish a documented connection-rate limit. Pick a
  conservative number and say plainly in the README that it is not a
  documented figure — stating "we don't know, so here's a conservative
  guess" is more honest than inventing a precise number and presenting it
  as fact.
- Frame the bucket's purpose correctly in its code comment: under normal
  backoff behaviour, a venue that is reconnecting continuously produces
  roughly fourteen attempts in five minutes against Binance's limit of
  three hundred — the bucket is essentially never reached in the common
  case. It exists not because the backoff is insufficient in practice, but
  because there is a *documented* ceiling and expressing it directly in
  code is preferable to relying on a heuristic (the backoff curve) staying
  correct forever.

### Piece 5 — staleness

A venue that has been reconnecting for eight seconds still has its last
known book sitting in the aggregator, and `merge()` is still reading it. The
gRPC output would carry eight-second-old prices with nothing to distinguish
them from live ones. The dangerous concrete case: the market moves, Binance
follows, Bitstamp is frozen mid-reconnect, and `merge()` produces a crossed
book that looks like a real arbitrage opportunity and is not.

```rust
let now = Instant::now();
let fresh: BTreeMap<Venue, &Book> = self.venues.iter()
    .filter(|(v, s)| now.duration_since(s.last_update) < v.staleness_threshold())
    .map(|(v, s)| (*v, &s.book))
    .collect();
```

`Instant::now()` is hoisted out of the filter closure deliberately — not for
the nanoseconds saved, but so every venue in the same pass is judged against
the identical instant, rather than risking one venue reading as fresh and
another as stale within what should be the same tick.

Thresholds differ per venue, and that difference is the entire point:
Binance publishes a full snapshot every 100ms whether or not the book
changed, so silence itself means failure and a tight threshold is safe.
Bitstamp only publishes on change, so silence can mean a genuinely quiet
market rather than a dead connection — it needs a more generous threshold.

**Bitstamp's threshold: 8 seconds, measured live** — see Open Questions for
the measurement (792 messages over ~5.25 minutes, max observed gap 1.795s,
threshold set to ~4x that). `merge()` is not touched by this piece; only the
`BTreeMap<Venue, &Book>` handed to it changes, by pre-filtering out stale
entries before the call.

### Piece 6 — never-had-data is not the same as went quiet

Because Bitstamp accepts any channel name without validation, `--pair
xyzabc` produces two connected feeds, two permanent silences, two stale
venues, `merge()` returning `None` forever, and a process that runs
indefinitely doing nothing observable. This is exactly the silent-failure
shape this codebase has been designed against everywhere else in the build
order.

The distinguishable fact is: has a venue produced data at least once? An
aggregator whose venue map is still empty after a grace period means nothing
ever arrived from either venue.

```rust
if started_at.elapsed() > GRACE && self.venues.is_empty() {
    error!(pair = %pair, "no data from any venue after {GRACE:?} — check the pair name");
    return;   // aggregator ends, which is fatal
}
```

This grace-period exit is what makes the aggregator's own return fatal again
under the new `JoinSet` policy — a feed ending is not fatal after this step,
but the aggregator ending still is (piece 1's `TaskResult`/`Component`
carries this distinction).

This also revises an earlier answer given to the client about unsupported
pairs: the previous framing was "run on whichever venue has it, exit only if
neither does." Neither support nor non-support is detectable given
Bitstamp's behaviour — but "neither venue has ever produced anything" is
detectable, and is the honest version of the same intent.

**Grace period: 60 seconds, confirmed** — see Open Questions for the
reasoning (roughly one full backoff cycle: `1+2+4+8+16+30 = 61s`).

### Piece 7 — venue status in the client header

```
ETHBTC    binance ●  bitstamp ○ stale 4.2s        14:32:07
```

This is the verification surface for pieces 5 and 6 in practice: kill the
proxy, watch Binance go stale and the combined book narrow to Bitstamp-only
levels, restore the proxy, watch Binance come back — without reading logs
for it.

`Summary` carries no venue-health field and the `.proto` is fixed (must not
be touched — see Invariants), so the client infers venue health from which
venues actually appear in the streamed levels, rather than from an explicit
signal. This is a deliberately crude inference that avoids touching the
wire schema. The header already takes a venue list rather than a fixed
string — that shape was set up in step 6 specifically for this piece, so
this should be filling in a field rather than restructuring the header.

**Where "stale 4.2s" comes from — this must be stated explicitly, not left
implicit.** Presence/absence in the streamed levels only tells the client a
venue is currently missing; it gives no duration. The client must keep its
own per-venue last-seen timestamp — updated whenever a frame contains at
least one level from that venue — and compute the displayed duration from
that, client-side, on every redraw.

Two consequences of this, both worth writing into the README rather than
leaving implicit:

- **The client's timer is not the server's staleness state — they measure
  different things.** The server's threshold (piece 5, 8s for Bitstamp)
  starts counting from the last message *it* received from the venue. The
  client's timer starts from the last *frame* in which that venue's levels
  appeared, which is downstream of the server's own publish cadence. The
  client's number will therefore run slightly behind the server's — by
  roughly one publish interval — not because either is wrong, but because
  they're two different clocks measuring two different events. The README
  should say plainly which one the header shows (the client's own
  observation, not the server's internal staleness state).
- **The inference has a concrete blind spot, not just a theoretical one.**
  A venue that is publishing completely normally but whose levels never
  make the top 10 (e.g. consistently the worse-priced side of two liquid
  venues) would look identical to a genuinely stale venue — absent from
  the levels either way. Unlikely with only two venues in this project,
  but it is the kind of failure that goes wrong quietly rather than
  loudly. The README's note that a real system would carry venue health on
  the wire (rather than a fixed-schema client inferring it) should name
  this blind spot as the concrete reason, not as a general principle.

### What's dropped, and why

These were part of the original plan for this step and are deliberately not
being built, because the Bitstamp experiment (see Problem) removed the
signal each of them would have needed to act on:

- **No give-up-after-N-failures counter.** Nothing in the available signals
  distinguishes a permanent failure from a transient one, so a feed that
  gives up and stops trying is strictly worse than one that stays patient
  forever — a venue can legitimately be down for an hour and come back.
- **No `live_feeds` counter.** Feeds no longer end under the new reconnect
  policy, so the supervisor (`JoinSet` loop in `src/main.rs`) only needs to
  care about the aggregator ending, the server ending, and panics.
- **No `bts:error` special path.** The existing match arm for
  `bts:error` in `src/exchange/bitstamp.rs` stays, since it may still fire
  on a genuinely malformed request — but its comment should be updated to
  say plainly that it was never observed for an invalid channel name, so
  nobody later infers a behaviour from it that does not exist.
- **Panics stay fatal.** A panic is a bug, not a transient condition, and
  restarting a task into a repeating panic is a crash loop dressed up as
  resilience. Dying loudly is the correct response and this step does not
  add panic-recovery.

## Testing Strategy

Every test in this packet names the bug it catches — no coverage-for-its-own-sake.

Required real verification:

- `backoff grows and caps` — catches unbounded growth, or a missing cap
- `a short-lived connection does NOT reset the thrash loop (piece 3)` — the
  single most valuable test in this packet; it is the only thing standing
  between the shipped code and actually hitting Binance's rate limit
- `a stable connection does reset` — catches a 30s stability window that
  effectively never fires (e.g. a bug that waits two days into a run)
- `jitter stays within range` — catches randomness applied incorrectly (out
  of the 0.5x-1.5x band, or not applied at all)
- `bucket empties and refills` — catches a limit that never actually applies
- `bucket doesn't exceed capacity` — catches a burst of up to three hundred
  attempts slipping through unthrottled
- `stale venue excluded from merge` — catches staleness not being wired into
  the aggregator at all
- `fresh venue included` — catches a threshold set too tight, dropping
  everything
- `thresholds differ per venue` — catches the two thresholds collapsing to
  one shared value
- `empty map past grace exits` — catches the silent-nothing-forever case
  (piece 6's core guarantee)

All of the above are pure unit tests: pass `Instant` in as a parameter
rather than calling `Instant::now()` inside the function under test, so
tests can supply a fake/fixed clock without needing `tokio::time::pause`.
Filed alongside the code under test in `src/feed.rs`, `src/aggregator.rs`,
and `src/exchange/mod.rs` — a unit test needs access to internal items
(backoff state, the token bucket, per-venue thresholds) that aren't public,
so it belongs with the code, not in `tests/`, which only reaches the public
surface.

Integration-level verification (acceptance criteria, below) exercises the
real binary and a real proxy interruption — this is the truth anchor for
"reconnect actually works end to end," which the pure unit tests above
cannot prove by themselves.

Optional supporting checks:

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test`, `docker compose up --build` against a live or proxied venue

## Acceptance Criteria

- Kill the proxy mid-run: the client shows Binance going stale, the combined
  book narrows to Bitstamp-only levels, and the process keeps serving
  (does not exit).
- Restore the proxy: Binance comes back without a process restart.
- `--pair xyzabc` exits within the grace period, with a log message naming
  the pair.
- `git diff main --stat -- src/merge.rs` shows no diff.
- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` all pass.
- `docker compose up` survives a proxy interruption without the container
  exiting.
- README and `compose.yml` no longer claim the container exits shortly
  after startup if it cannot reach Binance — check the Docker section and
  the top-of-file summary specifically, since this claim currently appears
  in two places and both describe the old `select!` exit-on-failure
  behaviour that this step removes.
- README gains: reconnection with jittered backoff and why the reset waits
  for stability (piece 3's reasoning, not just the mechanism); staleness
  with the per-venue thresholds and where Bitstamp's number came from;
  the grace period and its chosen value; the connection rate limits
  (including that Bitstamp's is an undocumented, conservative guess).
  Production section gains a note about venue health belonging on the wire
  in a real system. Keep the README tight — it was 1,157 words before this
  step; this step has a lot to add, so cut elsewhere if it grows past
  roughly 1,400 words.

## Invariants and Critical Don'ts

- **`src/merge.rs` must not change.** This is the scope check's centerpiece
  and the rule that has held since step 3: staleness filtering happens
  before the `BTreeMap<Venue, &Book>` reaches `merge()`; `merge()` itself
  stays pure, with no clock and no notion of "stale."
- `proto/orderbook.proto` is not touched — the venue-status inference in
  piece 7 stays entirely client-side.
- Commit 1 (the `JoinSet` migration) changes no behaviour and no existing
  test assertion — only call sites may move where a signature forces it.
  This is what makes later commits debuggable in isolation.
- Panics remain fatal — no panic-recovery is introduced by this packet.

## Risks and Tradeoffs

- Choosing the wrong Bitstamp staleness threshold either causes false
  "stale" flapping during normal quiet markets (too tight) or lets a truly
  dead Bitstamp feed pollute the combined book for too long (too loose) —
  mitigated by measuring rather than guessing (see Open Questions).
- The grace-period duration for "never produced data" trades false-positive
  early exits (a legitimately slow-to-start valid pair) against a longer
  hang on a genuinely bad pair name — see Open Questions.
- Client-side venue-status inference (piece 7) is a workaround for a fixed
  wire schema, not a real signal; it can be wrong in edge cases the README
  should acknowledge (e.g. a venue publishing but every level filtered out
  for unrelated reasons would look identically absent to a stale venue).
- Bitstamp's connection-rate limit is genuinely unknown; the chosen bucket
  capacity/refill is a conservative guess, not a documented ceiling — if
  wrong, it either throttles Bitstamp reconnects too aggressively or offers
  no real protection.

## Rollback Plan

Each of the six pieces lands as its own commit in a defined order (see
below), on branch `009-resilience`, merged into `main` with `--no-ff`. Any
piece found to be wrong post-merge can be reverted independently as long as
later commits in the sequence that depend on it are reverted alongside it —
piece 1 (JoinSet) is the foundation every later piece builds on, so it is
the one piece that cannot be reverted without reverting everything after it.

Commit order:

1. `JoinSet` migration, behaviour unchanged
2. Backoff, jitter, stable reset
3. Token bucket
4. Staleness filter and per-venue thresholds
5. Grace period
6. Client header

## Open Questions — resolved

Both were explicitly flagged by the brief as needing a real answer, not a
silent default. Both are now resolved, before implementation starts:

1. **Bitstamp's staleness threshold — measured, not guessed. Resolved: 8
   seconds.** Live measurement, 2026-08-24, ETHBTC, `RUST_LOG=debug` against
   the real Bitstamp feed for 314.8s (~5.25 minutes): 792 messages, max
   observed gap **1.795s**, median 0.213s, mean 0.398s — no long tail, the
   ten largest gaps all clustered 1.4s-1.8s. Per the brief's own rule
   (3-4x the observed max), that's 5.4s-7.2s; chosen **8s** (4x the
   observed max, rounded up slightly) rather than the low end, since a
   5-minute sample is short and a genuinely quiet moment could plausibly
   produce a longer natural gap than this window happened to catch. Record
   this measurement (the actual numbers, not just the chosen threshold) in
   the README per the brief's explicit request.

2. **Grace period for "never produced any data" — confirmed at 60s, with
   reasoning beyond the brief's own guess.** 60s lines up almost exactly
   with one full lap of the backoff curve itself:
   `1 + 2 + 4 + 8 + 16 + 30 = 61s` for the first six reconnect attempts.
   That is the actual justification, not "it seemed reasonable": the grace
   period should cover at least one full backoff cycle, or a venue that is
   genuinely mid-reconnect (not an invalid pair) risks being killed before
   it ever gets a fair shot. 60s is long enough to absorb startup slowness
   and short enough to fail fast on a genuinely bad pair name. Record this
   reasoning in the README, not just the number.

Both are recorded here and must also land in the README (piece 6/piece 5
sections) and `revisions.md` during implementation, per this project's
convention for measured/confirmed decisions.
