# Step 1 — Binance feed

## Where we are

Step 0 is done and merged: cargo project, dependencies, proto compiling through
build.rs and included in lib.rs, CLI with --pair and --port, config from
environment, Dockerfile and compose, tests, clean git history.

This is step 1 of 11. Connect to Binance's websocket, parse the order book,
print it. Nothing else.

## What I want from you first

Write the spec and stop. Don't write code until I've approved it. List every
file you'll create or change, what goes in it, and why. Short enough to read in
two minutes. Same spec packet format as 001.

Branch: 002-binance-feed. Keep inputs/ out of git the way 001 does.

## Scope

IN:
- src/model.rs — internal, exchange-agnostic types: a price level, a book
- src/exchange/mod.rs, src/exchange/binance.rs — connect, read loop, parse
- main.rs becomes async and drives the feed
- Tests for the parser and for the price conversion
- README updated

OUT — don't write these, and don't leave stubs or todo!() for them:
- Any Exchange trait or abstraction over exchanges
- tokio::spawn
- stream .split()
- Reconnection, backoff, staleness
- Anything gRPC
- merge.rs, server.rs

The trait especially. I have exactly one exchange right now. Generalising from
one example produces the wrong abstraction, and I'd rather introduce the trait
at step 4 when I can see what actually differs between two implementations.
That's a position I want to defend in the follow-up, so please don't pre-empt it.

Same for spawn: one concurrent job means the read loop runs directly in main.
spawn arrives at step 3 or 4 when there are two feeds.

## The endpoint

    wss://stream.binance.com:9443/ws/{pair}@depth20@100ms

Pair goes in lowercase. This is the partial book depth stream: a full top-20
snapshot every 100ms, not a delta stream. Each message replaces the previous
book wholesale — no sequence reconciliation.

Payload, verbatim from Binance's docs:

    {
      "lastUpdateId": 160,
      "bids": [["0.0024", "10"]],
      "asks": [["0.0026", "100"]]
    }

Note what isn't there: no event time, no symbol, only lastUpdateId. Prices and
quantities are JSON strings, not numbers.

## Price representation — read this whole section before designing model.rs

Parse the strings into scaled integers. i64, fixed scale of 1e9. Convert to f64
in exactly one place, at the gRPC boundary, which doesn't exist yet — so for
now the conversion lives only in Display.

### Why not f64, stated accurately

I want the reasoning in the code and the README to be precise, because I'll be
asked about it and an overstated claim is worse than no claim. I measured this.

Computing a spread in f64:

    0.031505 - 0.031500  =  4.9999999999980616e-06
    true value           =  5e-06
    absolute error       =  1.9e-18
    relative error       =  3.9e-13
    error as a fraction of one tick (1e-8) = 0.0000000002 ticks

And formatted to eight decimals, both print 0.00000500. So a gRPC client would
not see the difference.

So do NOT write "f64 can't represent decimals, so the spread would be wrong."
That invites "how wrong?" and the answer collapses the argument.

### The argument that actually holds

Prices are not continuous. There is nothing between 0.031505 and 0.031506 — an
exchange won't accept an order there. A price is an integer multiple of a tick.
Representing a discrete quantity with a continuous type is a modelling error
before it's a precision one, and that's true regardless of how small the
numeric error happens to be here.

The precision benefit is real but small, and I want it described as small.
It would stop being small in a system that accumulates: summing many levels,
computing averages, compounding positions. This one does a single subtraction,
so it doesn't. Say that.

Write the comment in model.rs in that shape: the modelling reason first, the
measured numbers second as an honest bound, the accumulation case third as
where it would matter.

### Scale

Fixed at 1e9 in i64. Tell me in the spec what that costs at both extremes —
the largest price representable and the smallest tick — so the assumption is a
decision rather than a default.

Deriving real tick sizes from Binance's exchangeInfo is the correct production
answer and is scope creep here. It goes in the README's "what would change for
production" section, not in the code.

## Display — I want to read these values, not decode them

31505000 in a log line is unreadable and I'll be staring at these for the next
week. Separate the internal representation from how it's shown:

- Display formats as a decimal at the scale's precision, so 0.03150500
- Debug can show the raw integer, since that's what Debug is for

Same treatment for amounts. The point is that the terminal shows me prices and
the machine holds integers, and neither compromises for the other.

Give the price and amount types real newtypes rather than bare i64s. I want the
compiler to stop me passing an amount where a price goes.

## Parsing must tolerate non-book messages

Binance sends {"e":"serverShutdown","E":...} before closing a connection. Ping
and pong frames arrive too. If the parser returns Result and we use ?, one of
those kills the read loop and the program keeps running with a dead feed,
silently, which is the worst version of this failure.

So the parse function returns Option<Book>. "That wasn't a book" is a normal
outcome, not an error. Log unrecognised messages at debug and continue.

Handle the Message variants explicitly: Text is the real work, Ping/Pong are
ignored because tungstenite answers them, Close breaks the loop, Binary gets
logged and skipped.

## Output

The terminal drowns if you print 40 levels ten times a second. One line per
update: best bid, best ask, lastUpdateId. Something like

    binance ethbtc | bid 0.03150000 x 5.00000000 | ask 0.03151000 x 12.50000000 | id 7723441

Use tracing, not println!, so it goes to stderr with the rest of the logging
and respects ORDERBOOK_LOG_LEVEL.

## Tests

All pure — no websocket, no mocking. Capture one real message from the terminal
and embed it as a string literal.

1. A real Binance payload parses to 20 bids and 20 asks, with string prices
   converted to the right integers. Assert on actual values, not just counts.
2. {"e":"serverShutdown","E":1234567890} returns None, doesn't panic.
3. Malformed JSON returns None, doesn't panic.
4. Price conversion round-trips: "0.03150000" parses to 31500000 and Displays
   back as 0.03150000.
5. The measured claim itself: 0.031505 - 0.031500 in integers is exactly 5000,
   which converts to 5e-06. Assert exact equality. This is the test that would
   fail if someone later "simplifies" the type to f64, so give it a name that
   says so.

## The thing I want verified empirically

Binance's server sends a ping every 20 seconds and drops the connection if no
pong comes back within a minute. tungstenite queues a pong automatically, but
it only goes out when the write half makes progress — and this loop never
writes anything.

I don't want to assume this works. Run the binary for 25+ minutes and tell me
whether the connection survives. Turn on trace-level logging for tungstenite so
ping and pong frames are visible.

If it survives, that's a measured claim for the README. If it doesn't, we found
a real bug on day one instead of at hour forty, and we fix it before moving on.

## README

Add a "Price representation" section under design decisions. It should carry:

- the modelling argument: prices are tick multiples, discrete not continuous
- the measured numbers above, presented as the honest bound on the precision
  benefit rather than as the justification
- where it would matter: accumulating arithmetic, which this doesn't do
- the fixed 1e9 scale as a documented assumption, with what it costs
- exchangeInfo-derived tick sizes as the production answer, in the production
  section

Write it in your own words. It should read like someone who measured before
deciding, not like someone quoting a rule.

## Acceptance

Report all of these:
- cargo run -- --pair ethbtc showing live book lines with readable decimal
  prices in a sane range for ETHBTC (~0.03)
- cargo test
- cargo clippy --all-targets -- -D warnings
- cargo fmt --check
- docker compose up --build working the same way
- the 25-minute connection test result with ping/pong evidence
- confirmation that no Exchange trait, no spawn, no split, no reconnection and
  no gRPC code was added

## How to work with me

Explain in Turkish, write code and docs in English. I come from C and C++ and
have not shipped Tokio in production, so explain the async idioms as they come
up rather than assuming — particularly what #[tokio::main] expands to, why
next() yields Option<Result<Message>> and what each layer means, and what
.await does and doesn't suspend. C++ comparisons help.

I have to talk through every decision afterwards, so prefer the explainable
option over the clever one, and flag any judgement call I should know about.

## Pace

Step 0 took most of a day. The deliverable that gets reviewed is the merge at
step 5, so from here I want less polish per file and more progress per hour.
Match the existing code's conventions, don't re-litigate them, and push back if
you think I'm cutting something that matters.
