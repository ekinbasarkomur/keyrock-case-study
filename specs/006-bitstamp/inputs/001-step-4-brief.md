# Step 4 — Bitstamp, behind an Exchange trait

## Where we are

Step 3 is merged: the Binance feed sends parsed books over an mpsc to an
aggregator task, which calls a pure summarise() and publishes Arc<Summary> into
the watch channel the gRPC server streams from. The parser borrows out of the
message buffer. One venue end to end.

This is step 4 of 11. Add Bitstamp.

## What I want from you first

Write the spec and stop. Don't write code until I've approved it.

Branch: 006-bitstamp. Spec packet is the first commit. Merge with --no-ff.

Keep the spec to two minutes of reading, the way 005's was. The long conceptual
reasoning lives in a handbook outside this repo — the spec carries the conclusion
and one line of why.

## This step is two separate jobs, in this order

They land as separate commits and the order matters.

**Job A — refactor the aggregator to a venue map. No behaviour change.**
**Job B — add Bitstamp behind an Exchange trait.**

Job A is independent of Bitstamp. It could be done today with one venue, and it
should be, before Bitstamp arrives.

The verification is that after Job A's commit, every existing test still passes
unchanged. If a test needs editing, the refactor changed behaviour and I want to
know that without Bitstamp's noise on top of it. Doing both in one commit means
that when something breaks I have to ask whether it was the refactor or the new
venue.

## Job A — the venue map

Today:

    struct Aggregator {
        binance: Option<VenueState>,
    }

    match venue {
        Venue::Binance => self.binance = Some(...),
    }

    summarise(venue, book)

After:

    struct Aggregator {
        venues: BTreeMap<Venue, VenueState>,
    }

    self.venues.insert(venue, VenueState { book, last_update: Instant::now() });

    summarise(&fresh)     // fresh: BTreeMap<Venue, &Book>

The reason is narrow and it's the only one that matters: with named fields, every
new venue changes merge's signature — merge(a, b) becomes merge(a, b, c) becomes
merge(a, b, c, d). With a map the signature is fixed. That's the change that would
otherwise ripple through step 5, which is why it happens before step 5 rather than
during it.

BTreeMap, not HashMap. HashMap's iteration order is unspecified and varies between
runs. Our tie-break rule says equal price ranks by larger amount — but when price
and amount are both equal across two venues, iteration order decides, and with a
HashMap that decision changes run to run. The test would be flaky rather than
wrong, which is worse. Derive PartialOrd and Ord on Venue; variant declaration
order gives the ordering.

### What summarise receives

Pass BTreeMap<Venue, &Book>, not BTreeMap<Venue, VenueState>. The VenueState
carries last_update, which is clock data, and summarise must stay clock-free —
that's decision 6 from step 3 and it holds.

The aggregator builds the borrowed map:

    let fresh: BTreeMap<Venue, &Book> = self.venues.iter()
        .map(|(v, s)| (*v, &s.book))
        .collect();

Step 6 then adds one .filter() to that chain and summarise never learns a clock
exists. Note that in the spec so step 6 doesn't rediscover it.

When you write that filter later, hoist Instant::now() out of the closure so
every venue is judged against the same instant. It's not a performance concern at
two venues — it's that per-element now() calls could in principle judge one venue
fresh and another stale from the same tick.

### What summarise does with more than one venue

Nothing yet. Merging is step 5. After this step the gRPC output still shows a
single venue, and that's correct — I don't want "are both feeds working" and "is
the merge right" being asked at the same time.

Pick the simplest defensible behaviour for the multi-entry case and say in the
spec what you picked. The proof that step 4 worked is the logs showing both
venues' book lines, not the gRPC output.

## Job B — Bitstamp

### The Exchange trait

Two implementations exist now, so the shape is observable rather than guessed —
that's why the trait was deliberately deferred in step 1.

    pub trait Exchange {
        fn venue(&self) -> Venue;
        fn connect_url(&self, pair: &str) -> String;
        fn subscribe_message(&self, pair: &str) -> Option<String>;
        fn parse(&self, raw: &str) -> Option<Book>;
    }

Synchronous, deliberately. The trait describes the differences in the protocol
data, not the control flow. If connect were async on the trait, each venue would
carry its own loop, and step 6's reconnection would then have to land in two
places instead of one — which is the problem the trait exists to prevent.

Generic, not dyn. The set of venues is known at compile time; dynamic dispatch
would buy nothing and cost an indirection.

One driver loop, parameterised:

    async fn run_feed<E: Exchange>(exchange: E, pair: String, tx: Sender<(Venue, Book)>)

with the subscribe as an if-let on subscribe_message — Binance returns None and
skips it, Bitstamp returns Some and sends it. The loop never asks which venue it's
driving.

### What Bitstamp actually does

Endpoint is wss://ws.bitstamp.net with nothing in the path. After connecting,
send:

    {"event":"bts:subscribe","data":{"channel":"order_book_<pair>"}}

Messages arrive wrapped, unlike Binance's flat payload:

    {"event":"data",
     "channel":"order_book_ethbtc",
     "data":{"timestamp":"...","microtimestamp":"...","bids":[["0.031","36.3"],...],"asks":[...]}}

The bids and asks inside are the same string-pair shape Binance uses, so
Price::parse and Amount::parse work unchanged. Keep the borrowed deserialisation —
the envelope struct borrows out of the message the same way Depth20<'a> does, and
Book still holds nothing borrowed.

Four event types to handle, and only the first is a book:

- "data" — parse it
- "bts:subscription_succeeded" — None, log at info
- "bts:request_reconnect" — None. Step 6 turns this into a reconnect trigger; for
  now log at info and note in the spec that step 6 owns it
- "bts:error" — None, but log at warn. It's the one that means something is
  actually wrong, and it shouldn't be swallowed at the same level as the others

Everything non-book returning None is exactly why parse returns Option rather than
Result — a stray control message must not kill the read loop.

### Don't split() the socket

The subscribe is a single write before the read loop starts, so the socket is
never written to while a read is in flight. split() costs a mutex on the shared
socket and buys nothing here. It would be necessary if we sent our own periodic
pings or subscribed dynamically at runtime — we don't, and tungstenite answers
Binance's pings automatically, which a 25-minute live run already confirmed.

Say that in the spec, so the question is answered rather than left open.

### Bitstamp differs in ways step 6 will care about

Record these now, don't act on them:

Bitstamp sends 100 levels; Binance sends 20. Doesn't matter — we take ten.

Bitstamp publishes only on change; Binance publishes every 100ms whether or not
the book moved. So silence means different things: on Binance it means failure,
on Bitstamp it may just mean a quiet market. Step 6's staleness thresholds have
to differ per venue, and this is where that requirement comes from.

Bitstamp carries a microtimestamp; Binance carries no event time at all. Don't add
it to Book this step — Book is venue-agnostic and I don't want a venue-specific
field in it before there's a use. Log it if useful.

### Symbol formatting stays inside the trait

--pair ethbtc becomes ethbtc@depth20@100ms in a URL path for one venue and
order_book_ethbtc in a channel name for the other. Both are currently lowercase so
it's simple, but the conversion belongs in connect_url and subscribe_message. When
a venue eventually wants ETH-BTC, one implementation changes and nothing else does.

## Tests

Bitstamp parse, all pure:
- a real captured "data" message parses to the right levels and prices
- bts:subscription_succeeded returns None, no panic
- bts:request_reconnect returns None, no panic
- bts:error returns None, no panic
- malformed JSON returns None, no panic

Trait behaviour:
- Binance's subscribe_message is None
- Bitstamp's contains the right channel name for the pair

That second one catches a real bug with a nasty symptom: a wrong channel name
means Bitstamp accepts the subscription and then silently sends nothing. Silent
failure is the category we've been designing against all the way through.

**The fixture must be real.** Step 1 shipped a fabricated one and it was visibly
synthetic — perfectly regular price steps that no real book has. Bitstamp is
generally reachable from Turkey without the proxy, so capture an actual message,
trim it if it's unwieldy, and comment where and when it was captured. If you can't
reach it, leave a TODO naming me rather than inventing something plausible.

For every test tell me the bug it catches. If you can't name one, drop it.

## Test hazard worth knowing before you write them

Any test that drives the watch channel has to interleave sends with reads. watch
only holds the latest value, so publishing two updates before the client
subscribes collapses them into one and the second read never wakes. That was a
real deadlock in step 3, not a flaky test — found by running it and killing the
stuck process.

## Acceptance

- both venues' book lines scrolling in the logs
- grpcurl still streaming (single venue — merge is step 5)
- Job A's commit passing every existing test unchanged
- the Bitstamp fixture real, with capture provenance in the comment
- cargo build, test, clippy --all-targets -- -D warnings, fmt --check
- docker compose up with both feeds connecting
- git diff main --stat showing merge.rs changed only by Job A's signature change,
  and not at all by Job B

That last one is the scope check for this step.

## At the end

The short list for my handbook: anything that surprised you, anything where
implementation contradicted the plan, and specifically whether the Exchange trait
as specified actually absorbed both venues cleanly or whether something leaked
into the driver loop. If it leaked, that's the interesting finding, not a failure.

Tell me plainly what you couldn't verify in your environment rather than reporting
it as passing.

## How to work with me

Explain in Turkish, code and docs in English. I come from C and C++ and haven't
shipped Tokio in production. Explain idioms as they come up — particularly what
the generic monomorphisation actually produces here, and why BTreeMap's ordering
comes from the enum's declaration order.

Step 5's merge is the deliverable that gets reviewed. Less polish per file, more
progress per hour.
