# Step 9 — measurement

## Where we are

Steps 0 through 8 are merged, 52 tests, everything passing. What's missing is any
number. The brief says speed is one of the most important factors operationally,
and so far everything I can say about it is a design argument — merged instead of
sorted, borrowed the parse, `Arc` in the watch. All true, none of it measured.

This step produces numbers. It also settles one thing I've deliberately left
undone, and turns one estimate into a measurement.

## What I want from you first

Write the spec and stop. Branch: 011-measurement. Spec packet is the first
commit. Merge with --no-ff.

## Write your prediction down before you measure

Do this first, in the spec, before any instrumentation exists.

Based on the allocation counts we already know — roughly 27 allocations per tick
in the parse after the borrowed deserialisation, roughly 22 building the Summary,
about 20 comparisons in the merge — say what you expect p50 to be and which stage
you expect to dominate.

My own guess is 10 to 50 microseconds, with parsing the largest share.

Both outcomes are useful. If the prediction holds, the mental model of where the
work goes is correct. If it doesn't, that's the more interesting result and the
one worth writing up. What I don't want is a measurement with no prediction to
compare it against, because then it's just a number.

## Piece 1 — ingest to publish

Timestamp when a message is parsed, timestamp when the aggregator publishes,
record the difference in an hdrhistogram.

`Book` needs a `received_at: Instant`. The recording happens in the aggregator.

**merge stays pure.** No clock reaches it — that rule has held for eight steps and
this is the step most likely to break it out of convenience.

hdrhistogram is currently a dev-dependency. It has to move to [dependencies],
which was noted back in step 3 as a thing that would eventually be needed.

Report p50, p99, p99.9 and the sustained update rate on a periodic log line,
every thirty seconds or so.

Percentiles rather than a mean, because latency distributions are skewed and a
mean hides the tail. One 50ms outlier in a thousand 10µs samples produces a
mean of 60µs, which reads as fine and isn't. For a market maker the tail is the
part that costs money.

### What this metric is not

Binance's partial depth stream carries no event time. Bitstamp's carries a
microtimestamp. So exchange-to-us latency is measurable on one venue and not the
other, and ingest-to-publish is the only figure comparable across both.

Say that in the README next to the numbers, not as a footnote. A number that gets
read as wire latency is worse than no number.

If you want to report Bitstamp's wire latency separately as a secondary figure,
fine — but make clear it has no Binance counterpart.

## Piece 2 — how often the top ten actually repeats

Same instrumentation, and this one settles a question I've been holding open.

Binance publishes every 100ms whether or not the book changed, so some ticks
produce a merged top ten identical to the last one published. Publishing those
wakes every subscriber and re-encodes identical bytes for each.

Count them. Compare the newly merged Summary against the last published one and
track the percentage that are identical.

Then act on the number rather than on my guess:

If it's above about 30%, implement the deduplication — keep the last published
Summary, skip the send when equal. Two lines, and it now has a measured
justification rather than an assumed one. It's also closer to what the brief asks
for, which is a stream on every change of any order book.

If it's below that, report the measurement and don't implement it. A measurement
that talks you out of a change is as good a result as one that talks you into it.

Either way the number goes in the README.

**The comparison is on the published Summary, not on lastUpdateId.** A venue's
fifteenth level can change while the merged top ten doesn't.

## Piece 3 — release profile

Currently only `strip = true`. Add:

    [profile.release]
    strip = true
    lto = "fat"
    codegen-units = 1

Measure p50 before and after and report both. Typically five to fifteen percent,
sometimes nothing, and either result is worth a line.

**Do not add `panic = "abort"`.** It would break `JoinSet`'s `is_panic()`
distinction, and step 7's supervision policy depends on being able to tell a panic
from a cancellation and log them differently.

## Piece 4 — load test, and this is the one I care most about

The README currently estimates that the per-subscriber encode cost saturates a
core somewhere in the low thousands of subscribers. That's arithmetic, not
measurement, and it's labelled as such — but it doesn't have to stay an estimate.

Write a load harness: spawn N clients that connect, stream, and discard. Run it at
100, 500 and 1000 subscribers and record CPU and the sustained publish rate at
each.

This matters more to me than any optimisation would.

If the curve is linear as predicted, the estimate becomes a measurement and the
README's claim about where the ceiling sits stops being arithmetic.

If it isn't linear, that's the more interesting outcome — something is behaving
differently from the model and I'd want to know what.

Either way, it's what tells us whether the per-subscriber encode is worth fixing.

Keep the harness simple. A separate binary or an ignored test, not a benchmarking
framework. It doesn't need to be precise, it needs to be honest about the shape of
the curve.

## What we're not doing, and why

**No custom tonic codec for pre-encoded bytes.** This is the real fix for the
per-subscriber encode and I'm deliberately leaving it out. It means implementing
tonic's Codec trait, which is an under-documented corner of the API that has
changed across versions, and the reviewer will run one client where the gain is
zero. Piece 4 gives us the number that says how much it would be worth, and the
README says we scoped it and chose not to. That reads better than not having
considered it.

**No simd-json unless piece 1 justifies it.** If parsing turns out to be over
sixty percent of p50, mention it as the next thing to try. Below that, don't. And
even above it, note that a 2-3x parse speedup on microsecond absolute numbers
wouldn't change anything operationally — considering it and declining is a fine
outcome to report.

**No buffer pooling.** It'd save two allocations a tick and cost merge's purity,
which the eight merge tests depend on. That trade was rejected earlier and having
time doesn't change it.

**No metrics endpoint, no Prometheus, no flamegraphs.** Log lines and a README
table are the deliverable.

## Piece 5 — the 24-hour run

Start this **after** everything above is merged, so it exercises the shipped
build rather than an intermediate one.

Binance force-closes connections at the 24 hour mark, documented, so a run of that
length is the only thing that actually exercises the reconnection path against the
condition it was built for.

Record:

- how many reconnects happened, per venue, and whether Binance's 24h close shows
  up as one of them
- p50 and p99 at the start versus at the end — drift would suggest a leak or
  something accumulating
- peak RSS
- how many times a venue was excluded as stale, and for how long
- the deduplication rate over the full run against the short-run figure

Start it, then write the README while it runs — don't sit and wait for it.

The README line at the end should be something like: ran for 24 hours, N
reconnects including Binance's scheduled close, p50 stable at X, no drift. That
one sentence is the proof for all of step 7's reconnection work, which currently
rests on a proxy interruption lasting a few seconds.

## Order

    1. prediction written down
    2. latency instrumentation, dedupe counting
    3. release profile, measured before and after
    4. dedupe implemented, if the number says so
    5. load test at 100 / 500 / 1000
    6. README
    7. start the 24-hour run, then finish the README while it runs

## README

A measurement section with the numbers, how they were taken, and what they aren't
— specifically that ingest-to-publish is not wire latency and why it can't be.

The prediction against the result, in one sentence.

Update the production notes: the load test replaces the estimated figures with
measured ones, and the deduplication entry either disappears because it's now
built, or gains the measurement that says it wasn't worth building.

It's around 1,900 words. This adds a section; trim mechanics elsewhere if it goes
past 2,100.

## Acceptance

- periodic log line with p50, p99, p99.9, update rate, dedupe percentage
- hdrhistogram in [dependencies]
- git diff main --stat shows src/merge.rs unchanged
- before-and-after p50 for the release profile change
- load test results at all three subscriber counts
- prediction and result both in the README
- 24-hour run started, with a note of when

## At the end

The short list for my handbook: the prediction versus the result and what you make
of the difference, the dedupe percentage, the load curve, and anything that
surprised you.

Tell me plainly what you couldn't verify rather than reporting it as passing.

## How to work with me

Explain in Turkish, code and docs in English.

Measure first, then decide. The only change in this packet that's allowed to
precede its measurement is the release profile, because it's free — and even that
gets measured before and after.
