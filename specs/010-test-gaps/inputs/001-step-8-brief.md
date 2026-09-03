# Step 8 — the tests the earlier steps didn't cover

## Where we are

Steps 0 through 7 are merged. 42 tests, all passing, and each one names a bug it
catches — the per-step discipline held and there's no coverage padding to clean
up.

So this step isn't "write the tests." It's the narrower question of which
behaviours aren't covered and whether breaking them is plausible.

## What I want from you first

Write the spec and stop. Branch: 010-test-gaps. Spec packet is the first commit.
Merge with --no-ff. Keep it short — this is a small step with a clear list.

## The gap

The existing tests cover mechanisms well and composition not at all.

Backoff is tested. The token bucket is tested. The staleness filter is tested.
But the loop in run_feed that uses backoff and the bucket together isn't, and
neither is the aggregator loop that applies the staleness filter and publishes
the result.

That matters because the mechanisms are the easy part. What breaks in a refactor
is the wiring — a call moved outside a loop, a clone that stops being a clone —
and none of that shows up in a unit test of the piece in isolation.

## The tests

Eight, in priority order. If time runs short, the first four are the ones that
matter.

### 1. Re-subscribe on every reconnect

The most dangerous gap in the codebase.

Bitstamp's subscription is per-connection. If a refactor moves the
subscribe_message call outside the reconnect loop, the socket opens, no error
appears, and no data ever arrives. Silent, and staleness would hide the cause by
just marking the venue dead.

The 009 spec said to "confirm this is actually true during implementation." A
test is a better confirmation than a reading, because it stays true.

Count subscribe_message calls across two forced disconnects and assert three.

### 2. Two clients, and one of them leaving

watch over broadcast is the decision I'll be asked about most, and its entire
justification is fan-out to N subscribers. It's tested with N=1.

A BookSummary implementation that moves the Receiver rather than cloning it, or
hands every caller the same one, works perfectly with one client and breaks with
two. Nothing currently catches that.

Connect two clients, assert both receive, drop one, assert the other keeps
streaming.

### 3. The read loop retries rather than returning

Reconnection's whole premise is that run_feed never returns. Backoff's numbers
are tested; the loop that consumes them isn't.

Bind a local TcpListener, accept and immediately close, and assert the feed comes
back for another attempt.

I'd rather do it this way than abstract connect() behind the trait. The trait
describes protocol data, not control flow, and adding a connect method to make it
mockable would undo that — see step 4's reasoning. A local listener is less
invasive and exercises real socket behaviour.

### 4. A stale venue actually narrows the published Summary

The filter is tested in isolation. What isn't tested is that a venue going quiet
changes what comes out the other end.

The aggregator already takes Instant as a parameter, so drive it with a fixed
clock: send from both venues, advance the clock past one threshold, send from one
venue only, assert the published Summary carries a single venue's levels.

**Watch out for the deadlock.** watch holds only the latest value, so sends
batched before a read collapse into one and the second read never wakes. That was
a real hang in step 3, not a flaky test. Interleave sends with reads.

### 5. A level that won't parse

Currently untested, and I don't actually know what the code does. Find out first
and tell me before writing the test, because this is a behaviour decision as much
as a coverage one.

Three possibilities: the whole message is rejected, that level is skipped, or it
panics.

Skipping is the dangerous one — a silently short book, a gap in the top ten,
nothing in the logs. If that's what it does now, I'd rather reject the whole
message: a missing tick is honest and staleness picks up the slack, while a
quietly incomplete book is the failure mode this codebase has been designed
against since step 1.

Tell me what it does, then we decide, then the test locks it in.

### 6. A negative price

Same shape: an untested behaviour that's also an undecided one.

A negative ask sorts to the front and takes the spread with it, so the blast
radius is larger than the input looks. No venue would send one, but a corrupted
frame or a proxy fault could.

Reject or accept — either is defensible, silently mishandling isn't. Tell me
what it does now.

### 7. Venue::Display is a wire contract

Display produces "binance", that string goes into Level.exchange, and the brief's
example output shows it lowercase. Nothing asserts it.

Change Display to "Binance" and everything compiles, 42 tests pass, and clients
get a different string.

Two assertions.

### 8. An empty side parses without panicking

"bids": [] is legal JSON and plausible from a venue in a strange state. merge
covers the empty case; the parser doesn't.

## Not doing

No coverage tooling and no coverage target. The bar stays "name the bug it
catches" — a percentage would just invite tests for the lines nobody would
break.

No mock websocket framework. Test 3's local listener is the whole harness.

No property-based testing. Interesting, and it would want a day I'd rather spend
on the latency numbers.

## For every test

Tell me the bug it catches. If you can't name one, drop it and say so.

Tests 5 and 6 need a decision from me before they're written. Report what the
code does today, don't pick for me.

## Acceptance

- cargo build, test, clippy --all-targets -- -D warnings, fmt --check
- no production code changes except whatever tests 5 and 6 turn out to need
- git diff main --stat shows src/merge.rs unchanged
- for tests 5 and 6, the current behaviour reported before any test is written

## At the end

The short list for my handbook: what the parser actually does with a bad level
and a negative price, whether the local-listener approach for test 3 held up or
fought you, and anything else that surprised you.

Tell me plainly what you couldn't verify rather than reporting it as passing.

## How to work with me

Explain in Turkish, code and docs in English. This step is small — don't let it
grow. The remaining work after it is latency numbers and a delivery pass, and
those matter more than an eighth test.
