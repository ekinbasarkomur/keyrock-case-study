# Step 3 — wire the real feed through to gRPC

## Where we are

Step 2 is merged: the gRPC server streams a fake Summary once a second over a
watch channel, three tasks supervised under select!, reflection on, container
publishing the port. The Binance feed runs alongside it but only logs to stderr —
the two halves aren't connected.

This is step 3 of 11. Connect them, and delete the fake writer.

## What I want from you first

Write the spec and stop. Don't write code until I've approved it.

Branch: 005-aggregator. Spec packet is the first commit. Merge with --no-ff.

Keep the spec to two minutes of reading. 004's was fifteen. Say what's being
built and why that shape, then stop — the long conceptual justification for these
decisions already lives in my handbook outside the repo, and repeating it in the
spec makes the spec worse, not better.

## Scope

IN:
- src/aggregator.rs — the task that owns the merged state
- src/merge.rs — summarise(), a pure function
- src/exchange/binance.rs — switch the parser to borrowed deserialisation
- src/main.rs — feed sends into an mpsc, aggregator task replaces the fake writer
- src/server.rs — delete run_fake_writer, watch now carries Arc<Summary>
- Tests, README

OUT:
- Bitstamp — step 4
- The merge of two books — step 5. This step summarises one book.
- Reconnection, staleness — step 6
- Deduplicating identical publishes — step 5, because the comparison has to be on
  the final output and the output isn't final yet
- Any Exchange trait — still one implementation, still step 4

## The shape

    Binance feed ──→ mpsc ──→ aggregator ──→ watch ──→ gRPC

Six decisions, all settled. Implement them, don't relitigate them — but tell me
if implementation contradicts any of them.

### 1. mpsc now, with one venue

An mpsc channel and a separate aggregator task are more machinery than one
exchange needs. The feed could summarise and publish directly in three lines.

They land now because step 4 otherwise has to introduce a second venue and this
architecture at the same time. With the channel in place, adding Bitstamp is a
cloned Sender and one new match arm; the aggregator doesn't change at all.

Same reasoning as putting the watch channel in step 2 and Docker in step 0: do
the awkward part while nothing else is going on.

### 2. Channel, not a shared mutex

The aggregator owns its state as a local variable in its own task. No Arc, no
Mutex, no lock to reason about.

This is not a contention argument — two writers at ten messages a second wouldn't
contend on a mutex either, and I don't want the code comment or the README
claiming otherwise. The reasons are:

A lock held across an .await is held for an unbounded time, and .await is five
characters that don't look like a suspension point. With message passing there's
nothing to hold.

And correctness becomes structural rather than a discipline: there's no lock to
forget to take, because there is no lock.

### 3. Venue is an enum, not a string

    enum Venue { Binance }

When Bitstamp arrives in step 4, every place that needs updating stops compiling.
A string would let it fail silently.

### 4. Borrowed deserialisation in the parser

Currently:

    struct Depth20 { bids: Vec<[String; 2]>, ... }

Change to:

    struct Depth20<'a> {
        #[serde(borrow)]
        bids: Vec<[&'a str; 2]>,
        ...
    }

Measured against the real fixture in the repo: 20 bids plus 20 asks with two
fields each is 80 String allocations per message, made and dropped microseconds
later once the numbers are parsed out. Total allocations per tick go from about
107 to about 27.

Frame it as removing work that shouldn't happen, not as an optimisation. The
strings are already in the message buffer; copying each one to read a float out
of it is a job that doesn't need doing.

If serde can't borrow — escapes in the input would force an owned String — tell
me rather than silently falling back. Price strings won't contain escapes, so I
expect this to work, but I want to know if it doesn't.

### 5. The watch carries Arc<Summary>

Currently watch::channel(Option<Summary>). Change to Option<Arc<Summary>>.

The watch has an internal lock, held while a subscriber reads the value. Cloning
a Summary means about 22 allocations — two Vecs and twenty Level strings — and
right now that happens inside the lock. With fifty subscribers waking at once
they queue behind each other doing it.

Arc::clone is one atomic increment. The critical section goes from 22 allocations
to one operation.

Be accurate about what this doesn't do: tonic still wants a Summary by value, so
the deep clone still happens. It just happens outside the lock. The win is less
waiting, not less work — and it scales with subscriber count, which is the same
family as the deduplication decision coming in step 5.

### 6. summarise() is a pure function, and it lives in merge.rs

    pub fn summarise(book: Option<&Book>) -> Option<Summary>

No network, no clock, no channels. Input to output.

It goes in merge.rs rather than aggregator.rs because step 5 widens the same
function to take two books. Put it in the wrong file now and step 5 starts with a
file move; put it in the right one and step 5 is a signature change.

Purity has a consequence I want respected in step 6: the staleness check needs
Instant::now(), so it belongs in the aggregator, deciding which books to hand to
merge. merge itself never sees a clock. Note that in the spec so step 6 doesn't
have to rediscover it.

Option in the return position carries "there is nothing to publish", consistent
with the None filtering in step 2.

### The aggregator's state

    struct VenueState {
        book: Book,
        last_update: Instant,
    }

    struct Aggregator {
        binance: Option<VenueState>,
    }

last_update is unused this step. Add the field now so step 6 adds a check rather
than a field.

Option because there's no data before the first message.

When recv() returns None every sender is gone, which means the feed died. The
aggregator should end too, and select! should end the process — same reasoning as
the three-task supervision in step 2.

## Collecting on the "fake" label

Step 2 labelled the placeholder levels "fake" specifically so that forgetting to
delete the writer couldn't look like working software. Collect on that now: a
test asserting no Level carries exchange == "fake".

The bug it catches is a forgotten deletion, and the symptom of that bug would be
a service that appears to work.

## Tests

summarise, pure and cheap to test:
- a 20-level book gives 10 bids, 10 asks, and the right spread
- a 6-level book gives 6, not 6 padded to 10
- None gives None

Plus the "fake" assertion above, and update the existing gRPC integration test —
it currently expects exchange == "fake" and should now expect "binance".

For each test tell me the bug it catches. If you can't name one, drop it.

## Acceptance

- grpcurl streams real Binance data with exchange "binance"
- no "fake" literal anywhere in src/
- the parser holds &'a str, not String — show me the struct
- cargo build, cargo test, cargo clippy --all-targets -- -D warnings, cargo fmt --check
- docker compose up works through the proxy
- README updated as part of this step, not deferred

## At the end

The short list for my handbook: anything that surprised you, anything where
implementation contradicted the plan above, and the actual allocation figures if
you can observe them.

Tell me plainly what you couldn't verify in your environment rather than
reporting it as passing.

## How to work with me

Explain in Turkish, code and docs in English. I come from C and C++ and haven't
shipped Tokio in production. Explain the idioms as they come up — particularly
what mpsc's Sender clone actually costs, and what happens to the borrow when the
message buffer is dropped.

Step 5's merge is the deliverable that gets reviewed. Less polish per file, more
progress per hour.
