# Step 5 — the merge

## Where we are

Step 4 is merged: both feeds live behind the Exchange trait, one generic
run_feed driving them, aggregator holding a BTreeMap<Venue, VenueState>, and
summarise() taking &BTreeMap<Venue, &Book> but only reading the first entry.

This is step 5 of 11, and it's the deliverable. Two of the brief's four numbered
requirements close here — "merges and sorts the order books to create a combined
order book" and the combined book reaching the gRPC stream. Everything after this
is the brief's "feel free to show off" territory.

## What I want from you first

Write the spec and stop. Don't write code until I've approved it.

Branch: 007-merge. Spec packet is the first commit. Merge with --no-ff.

Two minutes of reading. The design below is settled — implement it, flag it only
if implementation contradicts something, don't re-derive the reasoning.

## Scope

IN: src/merge.rs — summarise() becomes merge(), a Side enum, merge_side(), and
the tests. README, cut down as described at the end.

OUT: reconnection and staleness (step 6), latency instrumentation (step 8), the
example client (step 9).

Deduplicating identical publishes is a judgement call. The output is final after
this step, so it becomes possible here — but it lives in the aggregator, not the
merge, and it pairs naturally with step 6's other aggregator work. Say in the
spec which you'd prefer and why, and I'll decide.

src/aggregator.rs should not change otherwise. That's this step's scope check:
the signature was fixed in step 4 precisely so adding the real merge wouldn't
ripple outward.

## The design

### Signature — unchanged from step 4

    pub fn merge(venues: &BTreeMap<Venue, &Book>) -> Option<Summary>

Same shape summarise() already has. Rename and fill in the body; no file move, no
caller change.

### Three layers

    merge()            edge cases, spread, Summary
      merge_side()     the algorithm, N cursors, top ten
        Side::better() the ordering rule
        Side::levels() which list to read

Each does one thing and each is separately testable.

### Side, an enum rather than a bool

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

A bool would read as merge_side(&books, true) at the call site, and a bool passed
in from elsewhere and accidentally inverted sorts the bid list by the ask rule —
which produces plausible-looking numbers and no error. Same silent failure
category we've been designing against since step 1.

The a/b swap in the Bid arm is the entire difference between the two sides.
Keeping it in one place keeps the mistake surface at one place.

.then() is where the tie-break lives: price first, amount only when price ties.
Note that the amount rule is the same on both sides — only the price rule
inverts.

### merge — the edge cases fall out of ?

    pub fn merge(venues: &BTreeMap<Venue, &Book>) -> Option<Summary> {
        let bids = merge_side(venues, Side::Bid);
        let asks = merge_side(venues, Side::Ask);

        let (best_bid, best_ask) = (bids.first()?, asks.first()?);

        Some(Summary {
            spread: best_ask.price.into() - best_bid.price.into(),
            bids, asks,
        })
    }

No venues, one side empty, and a single live venue are all handled by that one
line — no explicit branches. Publishing a spread of 0.0 for a one-sided book
would be a false claim about the market, which is why step 3 removed the 0.0
fallback.

### merge_side — peekable cursors, not manual indices

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

peek() plus filter_map drops exhausted cursors with no bounds checks and no usize
arithmetic — manual index juggling is where this algorithm usually goes wrong.

while out.len() < TOP_N is what makes the cost independent of book depth. Ten
elements come out and the remaining levels are never touched, whether each venue
sent twenty or a thousand.

A min-heap would be right past four or five venues — k·log N rather than k·N —
but at two it's more machinery than the problem has. The signature takes a map,
so that's an internal change later, not a caller-visible one. Note it in the spec
rather than building it.

### The comment merge_side needs

min_by returns the first of equal elements, and cursors is built from a BTreeMap,
so it's in Venue's Ord order. When price and amount both tie across venues,
Binance wins deterministically.

That determinism is the whole reason step 4 chose BTreeMap over HashMap, and it's
invisible from reading this function. Put a comment on the cursors line saying so
— otherwise someone reasonably concludes a HashMap would be faster and quietly
reintroduces a flaky test.

### merge keeps returning the proto type

An internal MergedBook decoupling the merge from the wire format is the textbook
answer, but with one consumer it's indirection without a payoff. Leave it. It goes
in the README's production section instead.

## Tests

Eight, each naming the bug it catches. No test beyond this list.

At the Side level, where the ordering rule can be tested without running a merge:

1. Ask prefers the lower price; Bid prefers the higher — catches an inverted
   comparison, the easiest mistake here
2. Equal prices prefer the larger amount, on both sides — catches someone
   inverting the amount rule along with the price rule in the Bid arm

At the merge level:

3. Two books produce the right top ten and the right spread
4. Equal price and equal amount across venues resolves deterministically —
   catches a return to HashMap
5. A crossed book produces a negative spread and doesn't panic — catches an abs()
   or a clamp being added by someone who thinks it's a bug
6. A single venue works — catches N=1 being treated as a special case
7. No venues returns None — catches a fabricated empty Summary
8. Six levels returns six, not padded to ten — catches invented price levels

Four and five matter most. Four locks in a decision made a step earlier that this
code depends on but doesn't state. Five locks in behaviour most candidates would
classify as a defect.

### Test hazard

Any test driving the watch channel must interleave sends with reads. watch holds
only the latest value, so two sends before a read collapse into one and the second
read never wakes. That was a real deadlock in step 3, and this step's scenarios
involve more multi-update cases than any step so far.

## README — cut it roughly in half, as its own commit first

Do this before the merge work, so the diff is reviewable on its own rather than
tangled with the algorithm.

It's 326 lines and 2,268 words, around ten minutes of reading, and the proportions
are wrong. Layout is 57 lines, gRPC server 54, Configuration 51, Docker 35 —
197 lines, sixty percent of the file, on mechanics. Price representation gets 11.
What would change for production gets 11.

So the design decisions, the thing the brief says they'll ask me about, have less
space than the environment variable table. The reader is evaluating an engineer,
not operating a service. Every line explaining ORDERBOOK_LOG_LEVEL is a line they
don't spend on why the merge walks two sorted sides instead of sorting.

Target about 150 lines, under 1,200 words. Three or four minutes.

Cut hard:

- Layout, 57 to about 15. A tree with one short line per file. No paragraph per
  module — the module docs already say that, and anyone curious opens the file.
- Configuration, 51 to about 15. A table: variable, default, what it controls.
  Nothing else; defaults are visible in config.rs.
- gRPC server, 54 to about 20. The grpcurl reflection one-liner, a pointer to
  proto/orderbook.proto, and what the stream emits. Drop anything restating the
  schema, since the schema is in the repo.
- Docker, 35 to about 12. docker compose up, the published port, the proxy
  variables. The Dockerfile's comments carry the rest.
- Quick start, 25 to about 10.

Grow: pull the design decisions into one section and give them the room the
mechanics lose. Two to four lines each, the decision and why, no essays.

- watch rather than broadcast, because a book summary is a snapshot and a slow
  client wants the current book rather than a backlog
- merging two sorted sides rather than sorting the combined set, framed honestly
  — not a speed claim, but that a sort discards ordering the venues already did
  and allocates an intermediate we throw away
- newtypes over f64, with the measurement that made fixed-point not worth it
- the tie-break rule, and that a crossed book produces a negative spread on
  purpose
- one Exchange trait, kept synchronous, introduced only once there were two
  implementations
- one process per pair, because BookSummary takes Empty

The production section can take a few more lines too. It's where the schema
criticisms live, and it's the section that shows I know the limits of what I built.

The test: a reader should skim it in three minutes and come away knowing what it
does, how to run it, and the four or five choices worth asking me about. Right now
they'd come away knowing the environment variables.

Then, as part of the merge commits, the README needs the merge described, the
tie-break rule stated, and the crossed-book behaviour called out explicitly — a
reader seeing a negative spread should find the explanation there rather than
filing a bug. Add the internal-type decoupling to the production section.

## Acceptance

- grpcurl output shows levels from both venues, with different exchange labels
- the spread is computed from the two venues' best prices, not one venue's
- eight tests green
- merge stays pure — no network, no clock, no channels
- Side is an enum, no bool parameter anywhere
- git diff main --stat shows src/aggregator.rs unchanged
- README under 1,200 words
- cargo build, test, clippy --all-targets -- -D warnings, fmt --check
- docker compose up working through the proxy

## At the end

The short list for my handbook: anything that surprised you, anything where
implementation contradicted the design above, and specifically whether the
peekable-cursor shape held up or fought you somewhere.

Tell me plainly what you couldn't verify rather than reporting it as passing.

## How to work with me

Explain in Turkish, code and docs in English. Explain the idioms as they come up —
particularly what min_by does with ties, and what Peekable actually costs.

This is the step that gets reviewed. It's worth more care per line than steps 3
and 4 were.
