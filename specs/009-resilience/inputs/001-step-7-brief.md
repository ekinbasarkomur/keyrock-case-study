# Step 7 — reconnection, staleness, and supervision

## Where we are

Steps 0 through 6 are merged. Both feeds live behind the Exchange trait, one
generic run_feed driving them, aggregator keyed on a BTreeMap, a real two-venue
merge, and a terminal client that renders the book.

None of it survives a disconnect. run_feed returns when the socket closes, the
task ends, select! fires, the process exits. Binance force-closes at the 24 hour
mark, documented, so today this can't run for a day.

This is the biggest step so far — six pieces, one branch, separate commits.

## What I want from you first

Write the spec and stop. Branch: 009-resilience. Spec packet is the first commit.
Merge with --no-ff. Two minutes of reading.

## An experiment that shaped this

I probed Bitstamp before designing it. Subscribing to order_book_xyzabc and
order_book_totalgarbage both get bts:subscription_succeeded and then silence,
with the connection staying open. Bitstamp doesn't validate the channel name.

So there is no error path distinguishing a bad pair from a quiet market. That
removed two things I'd planned — a give-up-after-N-failures counter and a
bts:error special case — because neither has a signal to act on.

Don't reintroduce failure classification. The symptoms genuinely overlap.

## Scope

IN: src/feed.rs (reconnection, backoff, token bucket), src/exchange/mod.rs
(per-venue thresholds and rate limits), src/aggregator.rs (staleness filter,
grace period), src/main.rs (JoinSet), src/bin/client.rs (venue status in the
header), tests, README.

OUT: src/merge.rs must not change. Staleness decides which books to hand to
merge; merge itself never sees a clock. That's been the rule since step 3 and
this is the step that would break it if anything did.

Also out: latency instrumentation (step 8), graceful shutdown on SIGTERM —
mention where it would slot in but don't build it.

## Piece 1 — JoinSet, behaviour unchanged

select! needs a branch per task, so a fifth venue needs a fifth branch. More
importantly it fires once and you leave the block — you can't say "that was a
feed, keep going," which pieces 5 and 6 need.

    #[derive(Debug, Clone, Copy)]
    enum Component { Feed(Venue), Aggregator, Server }

    type TaskResult = (Component, Result<(), anyhow::Error>);

Two problems to solve. Every task has to produce the same type — wrap each spawn
in an async block that normalises it. And select! told you which task ended by
which branch fired; a JoinSet doesn't, so the identity travels alongside the
result. That's not just for logging, it's what pieces 5 and 6 decide on.

Error handling has three layers, one more than select! had:

    match tasks.join_next().await {
        Some(Ok((c, Ok(()))))          => // ended cleanly
        Some(Ok((c, Err(e))))          => // returned an error
        Some(Err(je)) if je.is_panic() => // panicked
        Some(Err(je))                  => // cancelled
        None                           => // set is empty
    }

is_panic distinguishes a bug from a cancellation and they shouldn't log the same.

**This commit must not change behaviour.** Keep today's policy — any task ending
exits the process. Existing tests pass unchanged. Later commits change the policy;
if something breaks I want to know whether it was the migration or the new rules.

## Piece 2 — reconnection with backoff and jitter

Wrap connect-and-read in an outer loop so run_feed never returns.

1s, 2s, 4s, 8s, 16s, capped at 30s.

Jitter every wait by a random 0.5x to 1.5x. Without it, when a venue comes back
every client that was disconnected is following the same schedule and they all
retry in the same second. Binance allows 300 connection attempts per five minutes
per IP; a synchronised herd is how you find that limit.

rand is already a dependency and resolved to 0.10, which is recent enough that
most examples online are 0.8 or 0.9 syntax and won't compile. Work from docs.rs
for the resolved version.

Bitstamp has to re-subscribe after every reconnect — the subscription is
per-connection, and without it the socket is open, no error appears, and no data
ever arrives. run_feed already calls subscribe_message, so wrapping it in the
loop gets this for free. Confirm that it actually does rather than assuming.

## Piece 3 — reset the backoff on stability, not on connect

This is the subtle one and I want it right.

Resetting the backoff when connect() succeeds looks correct and isn't. If the
venue accepts the connection and drops it immediately — overloaded, half-banned,
a stream name that yields nothing until it times out — the loop becomes:

    connect ok -> reset -> wait 1s -> connect ok -> drop -> reset -> wait 1s -> ...

One attempt per second is 300 in five minutes, which is exactly Binance's limit.
The backoff never engages because it's reset every cycle.

So the reset waits for the connection to have been stable:

    if connected_at.elapsed() > Duration::from_secs(30) {
        backoff.reset();
    }

Connecting isn't the same as working. Five lines, and it closes the hole at the
root.

## Piece 4 — a token bucket per venue

Piece 3 fixes the known thrash. The bucket is a ceiling for the ones I haven't
thought of.

Backoff answers "when do I try next" and is reactive. The bucket answers "am I
allowed to try" and is absolute. They compose:

    backoff.wait().await;
    bucket.acquire().await;
    connect().await

Binance's limit is 300 per 5 minutes, so one token per second. Capacity small —
five, not three hundred, or the first burst is three hundred attempts.

Put the rate on Venue, next to the staleness threshold, so both per-venue facts
live together and both matches are exhaustive:

    impl Venue {
        fn connect_rate(self) -> (f64, f64);      // capacity, per second
        fn staleness_threshold(self) -> Duration;
    }

Bitstamp doesn't publish a connection limit. Pick something conservative and say
in the README that it isn't a documented figure. Saying you don't know beats
inventing a number.

Frame this correctly in the comment. Under normal backoff it's about fourteen
attempts in five minutes against a limit of three hundred, so the bucket is never
reached. It isn't there because the backoff is insufficient — it's there because
there's a documented limit and I'd rather express the ceiling in code than rely
on a heuristic staying correct.

## Piece 5 — staleness

A venue that's been reconnecting for eight seconds still has its last book in the
aggregator, and merge is still using it. The gRPC output carries eight-second-old
prices that a client can't distinguish from live ones.

The dangerous version: the market moves, Binance follows, Bitstamp is frozen, and
merge produces a crossed book that looks like arbitrage and isn't.

    let now = Instant::now();
    let fresh: BTreeMap<Venue, &Book> = self.venues.iter()
        .filter(|(v, s)| now.duration_since(s.last_update) < v.staleness_threshold())
        .map(|(v, s)| (*v, &s.book))
        .collect();

Instant::now() hoisted out of the closure — not for the nanoseconds, but so every
venue is judged against the same instant rather than one being fresh and another
stale within the same tick.

Thresholds differ per venue and that's the whole point. Binance publishes every
100ms whether or not the book changed, so silence means failure and a tight
threshold is safe. Bitstamp publishes only on change, so silence can be a quiet
market.

**Measure Bitstamp's rather than guessing it.** Run the client for five minutes,
watch the longest gap between updates, and take three or four times that. Then
say so in the README — a threshold chosen from observation reads differently from
one that looked about right. Tell me what you observed.

## Piece 6 — never-had-data is not the same as went-quiet

Because Bitstamp accepts any channel name, `--pair xyzabc` gives you two
connected feeds, two silences, two stale venues, merge returning None, and a
process that runs forever doing nothing. Exactly the silent failure this codebase
has been designed against everywhere else.

The distinction that is detectable: a venue in the map has produced data at least
once. An empty map means nothing ever has.

    if started_at.elapsed() > GRACE && self.venues.is_empty() {
        error!(pair = %pair, "no data from any venue after {GRACE:?} — check the pair name");
        return;   // aggregator ends, which is fatal
    }

Sixty seconds is my starting guess for the grace period. Bitstamp can be quiet
for a while, but sixty seconds of nothing from either venue isn't a quiet market.
Say what you'd pick and why.

This also updates the answer I gave the client. I told them an unsupported pair
should run on whichever venue has it and exit only if neither does. I can't
detect "neither supports it" — but I can detect "neither has ever produced
anything," which is the honest version of the same intent.

## Piece 7 — venue status in the client header

    ETHBTC    binance ●  bitstamp ○ stale 4.2s        14:32:07

This is how I verify pieces 5 and 6: kill the proxy, watch Binance go stale and
the book narrow to Bitstamp, restore it, watch Binance come back. Reading logs
for that is much worse.

Summary carries no venue health and the proto is fixed, so the client infers it
from which venues appear in the levels. Crude but it doesn't touch the schema.
Note in the README's production section that a real schema would carry venue
health rather than leaving the client to guess.

The header already takes the venue list rather than a fixed string — that was
shaped for this in step 6, so this should be filling in a field rather than
restructuring.

## What's dropped, and why

No give-up-after-N-failures counter. Nothing distinguishes a permanent from a
transient failure, so a feed that stops trying is strictly worse than one that's
patient — a venue can be down for an hour and come back.

No live_feeds counter. Feeds no longer end, so the supervisor only has to care
about the aggregator, the server, and panics.

No bts:error special path. Keep the match arm — it may fire on a malformed
request — but update the comment to say it was never observed for an invalid
channel, so nobody later infers a behaviour from it that doesn't exist.

Panics stay fatal. A panic is a bug, and restarting into a repeating panic is a
loop. Dying loudly is the right response.

## Tests

Every test names the bug it catches.

    backoff grows and caps                  unbounded growth, or no cap
    a short-lived connection does NOT reset the thrash loop in piece 3
    a stable connection does reset          30s waits two days into a run
    jitter stays within range               randomness applied wrongly
    bucket empties and refills              the limit never applies
    bucket doesn't exceed capacity          a three-hundred-attempt burst
    stale venue excluded from merge         staleness not wired at all
    fresh venue included                    threshold too tight, everything drops
    thresholds differ per venue             collapsed to one shared value
    empty map past grace exits              the silent-nothing case

All pure. Pass Instant in as a parameter rather than calling Instant::now()
inside, so tests can supply a fake clock without tokio::time::pause.

The second one is the most valuable — it's the only thing standing between the
code and a rate limit.

## Commit order

    1. JoinSet migration, behaviour unchanged
    2. backoff, jitter, stable reset
    3. token bucket
    4. staleness filter and per-venue thresholds
    5. grace period
    6. client header

Commit 1 is a refactor and its whole claim is that it changes nothing — existing
assertions must hold unchanged. Call sites may move where a signature forces it.
That's been the rule since step 4 and it's what makes the later commits
debuggable.

## README

Several things go stale in this step, and one of them is currently stated as a
fact in two places.

compose.yml and the README both say the container exits shortly after starting if
it can't reach Binance, describing select!'s exit-on-failure. After this step that
isn't true — it reconnects indefinitely and degrades to the remaining venue.
Update both, and check the Docker section and the top-of-file summary for the same
claim.

Add: reconnection with jittered backoff and why the reset waits for stability;
staleness with the per-venue thresholds and where Bitstamp's number came from;
the grace period; the connection rate limits.

The production section gains venue health on the wire.

Keep it tight. It was 1,157 words and this step has a lot to say, so cut
elsewhere if it grows past about 1,400.

## Acceptance

- kill the proxy mid-run: the client shows Binance going stale, the book narrows
  to Bitstamp, and the process keeps serving
- restore it: Binance comes back without a restart
- --pair xyzabc exits within the grace period with a message naming the pair
- merge.rs unchanged in git diff main --stat
- cargo build, test, clippy --all-targets -- -D warnings, fmt --check
- docker compose up survives a proxy interruption

## At the end

The short list for my handbook: the Bitstamp staleness threshold you measured and
what the gaps actually looked like, anything that surprised you, and anything
where implementation contradicted the design above.

Tell me plainly what you couldn't verify rather than reporting it as passing.

## How to work with me

Explain in Turkish, code and docs in English. Explain the idioms as they come up,
particularly JoinSet's three-layer result and what abort-on-drop actually does.

This is the largest step so far. Take the commits one at a time and verify between
them.
