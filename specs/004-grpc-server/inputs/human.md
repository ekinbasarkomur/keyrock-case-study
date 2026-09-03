# Step 2 — gRPC server with fake data

## Where we are

Step 1 is merged: Binance websocket connecting, parsing, logging top of book.
Step 1 follow-up fixes are on 003-step-1-fixes.

This is step 2 of 11. Stand up the gRPC server and have it stream fake data.
Don't touch the Binance code.

## What I want from you first

Write the spec and stop. Don't write code until I've approved it.

Branch: 004-grpc-server.

**The spec packet is the first commit on the branch.** In 002 it landed after
all the implementation phases, and the commit order contradicts how I've told
the reviewer I work. Spec first, then phases.

Merge back with --no-ff. 002 was fast-forwarded and left no trace of the branch
in main.

## Scope

IN:
- src/server.rs — the OrderbookAggregator service implementation
- A watch channel carrying Option<Summary>, with a small task writing a fake
  Summary once a second
- main.rs: spawn the feed, spawn the fake writer, spawn the server, select! over
  all three
- tonic-reflection, with the file descriptor set emitted from build.rs
- compose.yml: bind address, published port, container now stays up
- tests/grpc.rs
- README

OUT — don't touch:
- src/exchange/binance.rs, src/model.rs, src/proxy.rs
- Any merge logic
- Bitstamp
- Reconnection or staleness

At the end I want `git diff main --stat` to show binance.rs and model.rs
untouched. That's the scope discipline check for this step.

## Why watch already, with no aggregator yet

The watch channel is really an aggregator-to-server concern and doesn't
strictly belong until step 3. I want it here anyway, for the same reason Docker
landed in step 0: it de-risks the fiddly plumbing while there's nothing else
going on.

The awkward part of this step is turning a watch::Receiver into something tonic
will accept as a response stream. I'd rather hit that with a fake producer than
discover it while also wiring up a real feed.

It also makes step 3 small: delete the fake writer, point the watch at the real
aggregator, done. Keeping each step independently deliverable matters more than
keeping each step conceptually pure.

## The stream type — this is the part I want explained

The generated trait has an associated type:

    type BookSummaryStream: Stream<Item = Result<Summary, Status>> + Send + 'static;

The concrete type of a mapped stream includes a closure, and closures have no
nameable type, so it has to be erased:

    type BookSummaryStream = Pin<Box<dyn Stream<Item = Result<Summary, Status>> + Send>>;

I understand this as the same shape as returning unique_ptr<IStream> in C++ —
one allocation per connection, not per message, plus a vtable hop. Tell me if
that mapping is wrong anywhere.

Use tokio_stream::wrappers::WatchStream to adapt the receiver. It's already a
dependency.

Two things to decide and tell me about in the spec:

The watch carries Option<Summary> because there's nothing to publish before the
first value arrives. Publishing an empty Summary would read as "spread is zero",
which is worse than making the client wait. Say how the stream handles the None
case — filtered out, or held until the first Some.

And WatchStream's behaviour on subscribe: whether a new subscriber immediately
gets the current value or waits for the next change. Check the docs rather than
assuming, and tell me which it is, because it decides whether a client that
connects between updates sits there with nothing.

## Fake data

A task that writes a Summary into the watch once a second. Ten bids, ten asks,
plausible ETHBTC numbers around 0.0315, a small positive spread.

Set `exchange` to "fake" on every level, not "binance". Two reasons: I can tell
real data from placeholder at a glance in the terminal, and if step 3 forgets to
replace this, a test can catch the literal.

## Running three tasks

main spawns three now: feed, fake writer, server. Wait on all three with
tokio::select! and shut down cleanly when any one of them ends.

The reasoning, so it goes in the code comment rather than being rediscovered
later: awaiting the JoinHandles in sequence would mean the second one is never
reached while the first is still running, so a dead gRPC server behind a live
feed would go unnoticed. And not awaiting at all means main returns, the runtime
shuts down, and the tasks die silently — no panic, no message, exit 0.

Ending the process when any task ends is the behaviour I want. A server serving
a dead feed publishes stale prices, which is worse than not serving.

## Reflection

Add tonic-reflection. build.rs needs to emit a file descriptor set —
tonic_prost_build::configure() with file_descriptor_set_path — and the server
registers it alongside the aggregator service.

Check docs.rs for the 0.14 API rather than guessing; the builder method names
have changed across versions.

The point is that `grpcurl -plaintext localhost:50051 list` works with no proto
file and no import path. That's a much more inviting line to put in a README.

Note in the README that reflection exposes the full schema, and that on a
public-facing endpoint I'd gate it behind a feature or config flag. Knowing the
limit of a thing you added is worth more than adding it.

## Docker

Three things change together and they'll fail confusingly if only some are done:

- ORDERBOOK_HOST must be 0.0.0.0 in the container, or the server binds to the
  container's own loopback and the published port refuses connections
- compose publishes the port, bound to host loopback only: 127.0.0.1:50051:50051
- The container now stays up instead of exiting, so the compose comments and the
  README that still say "logs and exits 0" are wrong twice over

## Tests

tests/grpc.rs, integration:

- Bind port 0 and read the real port back. Never a fixed port — tests in a
  binary run in parallel and two of them racing for 50051 is a flake that will
  waste an hour.
- Start the server on a task, connect a real tonic client
- Take **two** messages off the stream before asserting

Two, not one. One message proves the call returned. Two proves it's a stream,
which is what the schema actually promises and the thing that could break.

Assert on something meaningful — that bids and asks have ten entries and the
spread is positive, say. Not just that a message arrived.

For every test you add, tell me what bug it catches. If you can't name one, we
drop it.

## README

Updating the README is part of finishing a step now, not an optional last task.
It's currently still describing step 0 even though step 1 shipped, which is the
most visible problem in the repo.

For this step it needs: the server exists and streams placeholder data, the
grpcurl reflection line, the Docker section corrected, and src/server.rs in the
layout.

Keep it short. The README should stay readable in a few minutes.

## At the end

Give me a short list of anything worth recording as a design decision or a
discovery — the WatchStream subscribe behaviour, the reflection API shape,
anything that surprised you. I keep a decision handbook outside the repo and I'll
fold them in.

## Acceptance

- cargo build, cargo test, cargo clippy --all-targets -- -D warnings, cargo fmt --check
- grpcurl -plaintext localhost:50051 list showing the service
- grpcurl streaming fake summaries
- Binance book lines still scrolling alongside
- docker compose up staying up, and grpcurl from the host reaching it
- git diff main --stat showing binance.rs and model.rs untouched
- git log --graph showing the spec as the first commit and a merge commit at the end

## How to work with me

Explain in Turkish, code and docs in English. I come from C and C++ and haven't
shipped Tokio in production — explain the async idioms as they come up,
especially select! and cancellation, JoinHandle semantics, and the Pin/Box
erasure above. C++ comparisons help.

Prefer the explainable option over the clever one. Flag any judgement call I
should know about.

Pace: step 5's merge is the deliverable that gets reviewed. Less polish per
file, more progress per hour.
