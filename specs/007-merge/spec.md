---
spec_name: "Step 5 — the merge"
spec_id: "007"
spec_folder: "007-merge"
status: "approved"
created_at: "2026-08-24"
updated_at: "2026-08-24"
created_by: "spec-synthesizer"
creation_mode: "human-brief"
source_inputs:
  - "inputs/001-step-5-brief.md"
source_agents: []
goal: "Replace src/merge.rs's single-venue summarise() with a real two-book merge() that walks both venues' sorted sides with peekable cursors, producing the top-10 bids/asks and the spread the brief's two core requirements (combined book, published stream) close on."
purpose: "This is the case study's core deliverable — the last piece before the gRPC stream carries genuinely merged, two-venue data instead of Binance-only output."
parent_request: "step-5 brief, 2026-08-24 (specs/007-merge/inputs/001-step-5-brief.md)"
related_paths:
  - "src/merge.rs"
  - "src/aggregator.rs"
  - "README.md"
verification_level: "unit"
complexity: "medium"
---

# Spec: 007-merge

## Problem

Step 4 left `src/merge.rs`'s `summarise()` reading only the first
(lowest-ordered, i.e. Binance) entry of `venues: &BTreeMap<Venue, &Book>` —
gRPC output is still single-venue. This step replaces it with a real merge
across all venues in the map, sorted so the best deals are first on each
side, with the correct tie-break and spread. Two of the brief's four
numbered requirements close here: the combined order book, and that book
reaching the gRPC stream.

**The design below is settled — implement it, don't re-derive the
reasoning.** Full rationale for each decision lives in the user's own
handbook, per this project's established "two minutes of reading"
convention (matching specs 005/006, not 004's longer style).

## Scope

**IN:** `src/merge.rs` — `summarise()` becomes `merge()`, a `Side` enum,
`merge_side()`, and the eight tests below. README, cut down as described
below, landed as its own commit before the merge-algorithm commit.

**OUT, with the step each lands in:**
- Reconnection and staleness — step 6.
- Latency instrumentation — step 8.
- The example client binary — step 9.
- Dedup of identical publishes — see Open Questions; belongs in
  `src/aggregator.rs` alongside step 6's other aggregator work, not this
  step's `merge.rs`.

**`src/aggregator.rs` should not change.** This step's scope check —
`merge`'s signature was fixed in step 4 precisely so the real merge
wouldn't ripple outward.

## Design

### Signature — unchanged from step 4

```rust
pub fn merge(venues: &BTreeMap<Venue, &Book>) -> Option<Summary>
```

Same shape `summarise()` already has. Rename and fill in the body; no file
move, no caller change.

### Three layers, each separately testable

```
merge()            edge cases, spread, Summary
  merge_side()     the algorithm, N cursors, top ten
    Side::better() the ordering rule
    Side::levels() which list to read
```

### `Side` — an enum, not a bool

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side { Bid, Ask }

impl Side {
    /// Returns Less when `a` should come before `b` in this side's ordering.
    fn better(self, a: &Level, b: &Level) -> Ordering {
        let by_price = match self {
            Side::Ask => a.price.cmp(&b.price),
            Side::Bid => b.price.cmp(&a.price),
        };
        by_price.then(b.amount.cmp(&a.amount))
    }

    fn levels(self, book: &Book) -> &[Level] {
        match self {
            Side::Bid => &book.bids,
            Side::Ask => &book.asks,
        }
    }
}
```

A bool parameter is silently invertible with no compile error and produces
plausible-looking wrong numbers — same silent-failure category this project
has been designing against since step 1. `.then()` is where the tie-break
lives (price first, amount only on a price tie); the amount rule is
identical on both sides, only the price rule inverts.

### `merge` — edge cases fall out of `?`

```rust
pub fn merge(venues: &BTreeMap<Venue, &Book>) -> Option<Summary> {
    let bids = merge_side(venues, Side::Bid);
    let asks = merge_side(venues, Side::Ask);

    let (best_bid, best_ask) = (bids.first()?, asks.first()?);

    Some(Summary {
        spread: best_ask.price.into() - best_bid.price.into(),
        bids, asks,
    })
}
```

No venues, one side empty, and a single live venue are all handled by that
one line, no explicit branches. No `0.0` spread fallback — a fabricated
spread is a false claim about the market (same reasoning step 3 already
applied to the empty case).

### `merge_side` — peekable cursors, not manual indices

```rust
fn merge_side(venues: &BTreeMap<Venue, &Book>, side: Side) -> Vec<Level> {
    let mut cursors: Vec<_> = venues.iter()
        .map(|(venue, book)| (*venue, side.levels(book).iter().peekable()))
        .collect();

    let mut out = Vec::with_capacity(TOP_N);

    while out.len() < TOP_N {
        let best = cursors.iter_mut()
            .enumerate()
            .filter_map(|(i, (v, it))| it.peek().map(|lvl| (i, *v, *lvl)))
            .min_by(|(_, _, a), (_, _, b)| side.better(a, b));

        match best {
            None => break,
            Some((i, venue, level)) => {
                out.push(to_level(venue, level));
                cursors[i].1.next();
            }
        }
    }
    out
}
```

`peek()` + `filter_map` drops exhausted cursors with no bounds checks or
`usize` arithmetic. `while out.len() < TOP_N` makes the cost independent of
book depth — ten elements come out, whatever each venue sent past that is
never touched. A min-heap (k·log N) would beat this past four or five
venues; at two, it's more machinery than the problem has — note it here as
a documented tradeoff, don't build it.

**Required comment on the `cursors` line:** `min_by` returns the first of
equal elements, and `cursors` is built by iterating the `BTreeMap`, so ties
resolve in `Venue`'s `Ord` order — Binance wins deterministically on a full
price+amount tie. That determinism is the entire reason step 4 chose
`BTreeMap` over `HashMap`, and it is invisible from reading the function
without this comment — omit it and a future reader reasonably "fixes" it to
`HashMap`, reintroducing a flaky test.

### `merge` keeps returning the proto type directly

An internal `MergedBook` decoupled from the wire format is the textbook
answer, but with one consumer it's indirection without payoff. Leave it;
document the tradeoff in the README's production section instead.

## Tests

Eight, each naming the bug it catches. No test beyond this list.

**`Side` level** (ordering rule tested without running a merge):
1. Ask prefers the lower price; Bid prefers the higher — catches an
   inverted comparison.
2. Equal prices prefer the larger amount, on both sides — catches someone
   inverting the amount rule along with the price rule in the `Bid` arm.

**`merge` level:**
3. Two books produce the right top ten and the right spread.
4. Equal price and equal amount across venues resolves deterministically —
   catches a regression back to `HashMap`.
5. A crossed book produces a negative spread and doesn't panic — catches an
   `abs()` or a clamp added by someone who mistakes this for a bug.
6. A single venue works — catches N=1 being treated as a special case.
7. No venues returns `None` — catches a fabricated empty `Summary`.
8. Six levels returns six, not padded to ten — catches invented price
   levels.

Tests 4 and 5 matter most: 4 locks in a decision (`BTreeMap` for
determinism) made a step earlier that this code depends on but doesn't
restate; 5 locks in behaviour most candidates would misclassify as a
defect.

**Test hazard, binding, not new:** any test driving the `watch` channel
must interleave sends with reads, never batch sends up front — `watch`
holds only the latest value, so two sends before a read collapse into one
and the second read never wakes. Real deadlock in step 3
(`specs/005-aggregator/revisions.md`, entry 3); this step's scenarios
involve more multi-update cases than any step so far, so the hazard is more
likely to bite here than previously.

## README — cut roughly in half, as its own commit first

Land this commit before the merge-algorithm commit, so the diff is
reviewable on its own rather than tangled with the algorithm.

Current state: 326 lines / 2,268 words (~10 minutes), and the proportions
are wrong — Layout (57 lines), gRPC server (54), Configuration (51), Docker
(35) is 197 lines (60%) on mechanics, while price representation and
"what would change for production" get 11 lines each. The reader is
evaluating an engineer, not operating a service.

**Target: ~150 lines, under 1,200 words (~3-4 minutes).**

Cut:
- Layout: 57 → ~15 lines. A tree with one short line per file; module docs
  already carry the rest.
- Configuration: 51 → ~15 lines. A table (variable, default, what it
  controls); defaults are visible in `config.rs`.
- gRPC server: 54 → ~20 lines. `grpcurl` reflection one-liner, a pointer to
  `proto/orderbook.proto`, what the stream emits.
- Docker: 35 → ~12 lines. `docker compose up`, the published port, the
  proxy variables.
- Quick start: 25 → ~10 lines.

Grow: pull the design decisions into one section, two to four lines each,
no essays:
- `watch` over `broadcast` (snapshot semantics, not an event log).
- Merging two sorted sides rather than sorting the combined set — framed
  honestly as "don't discard ordering the venues already did," not a speed
  claim.
- Newtypes over `f64`, with the measurement that made fixed-point not worth
  it.
- The tie-break rule, and that a crossed book produces a negative spread on
  purpose.
- One `Exchange` trait, kept synchronous, introduced only once there were
  two implementations.
- One process per pair, because `BookSummary` takes `Empty`.

The production section can grow a little too — it's where the schema
criticisms live and shows awareness of what wasn't built.

**Then, as part of the merge commits** (not the README-cut commit), the
README needs: the merge described, the tie-break rule stated, the
crossed-book behaviour called out explicitly (a reader seeing a negative
spread should find the explanation there, not file a bug), and the
internal-type decoupling tradeoff added to the production section.

## Open Question — dedup of identical publishes (pending user decision)

The brief raises this explicitly and asks for a recommendation, not a
resolution: once the merge is real, the output is final, which is what
makes deduping repeated identical publishes possible for the first time.
It belongs in `src/aggregator.rs` (comparing the freshly merged `Summary`
against the last published one before writing to `watch`), not in
`merge.rs` itself — `merge()` must stay pure, with no notion of "last
published."

**This step does not implement dedup.** It's out of scope here regardless
of the answer, because it touches `src/aggregator.rs`, and this step's own
scope check is that file staying unchanged. The question is *when* to do
it, not whether — recorded here so it isn't silently dropped.

**Recommendation: defer it to step 6**, bundled with that step's other
`src/aggregator.rs` work (staleness filtering), rather than adding a
same-step exception to this step's "aggregator unchanged" scope check —
one aggregator-touching commit for both concerns is cleaner than carving
out a special case here for a change that's already explicitly documented
in `CLAUDE.md`'s Architecture section as belonging to the aggregator.

**Decided: recommendation confirmed by user, 2026-08-24.** Dedup lands in
step 6 with the rest of the aggregator work. This step proceeds with
`src/aggregator.rs` untouched.

## Acceptance Criteria

- `grpcurl` output shows levels from both venues, with different `exchange`
  labels.
- The spread is computed from the two venues' best prices, not one venue's.
- All eight tests above pass.
- `merge` stays pure — no network, no clock, no channels.
- `Side` is an enum; no bool parameter anywhere in the merge path.
- **Scope check:** `git diff main --stat -- src/aggregator.rs` shows no
  logic change. The rename `summarise()` → `merge()` forces a one-line
  call-site edit and two doc-comment references there — that mechanical
  edit is not a scope violation (no wrapper function; matches step 4's
  Job A precedent: "call sites may change, no assertion may change").
  Nothing else in `src/aggregator.rs` may change.
- **README commit sequencing:** the README cut-down lands as its own
  commit, under 1,200 words, before the merge-algorithm commit.
- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` all pass.
- `docker compose up` works through the proxy.

## Risks

- The `min_by`/`Peekable` shape is new to this codebase — if it fights the
  borrow checker or the tie-break comment is skipped, the determinism
  guarantee (test 4) silently degrades to "usually passes." Flag rather
  than work around if implementation contradicts the design above.
- Cutting the README to word-count risks losing information a reviewer
  needs — cut mechanics only (layout/config/docker/quick-start), not the
  design-decision content the brief explicitly wants preserved.
