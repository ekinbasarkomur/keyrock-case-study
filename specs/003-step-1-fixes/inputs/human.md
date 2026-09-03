# Step 1 follow-up fixes

Reviewed the merged step 1. The code is in good shape but there are six things
to fix before we move on. Do this on a branch named 003-step-1-fixes and merge
it back with --no-ff.

Order matters — 1 and 2 are the ones a reviewer would see first.

## 1. The README still describes step 0

This is the most visible problem in the repo. Someone opening the link reads
this first, and right now it says:

    **None of that logic exists yet.** This is step 0 of an 11-step build
    order ... but no websocket client, merge logic, or gRPC service yet.

Step 1 is merged. The websocket client exists and works. Also wrong:

- "Both parse arguments, build a Config, log one starting line to stderr, and
  exit 0" — main now connects to Binance and runs until the connection closes
- The `# defaults, logs, exits 0` comment on the docker compose line
- The Layout section, which never gained model.rs, exchange/, or proxy.rs

Bring all of it in line with what actually ships today: one Binance feed
connecting, parsing, and logging the top of book; no second venue, no merge, no
gRPC server yet.

Then make this permanent: from here on, updating the README is part of
finishing a step, not an optional last task. Note that in the revisions file so
later steps inherit it.

## 2. The test fixture is fabricated, and says so

In binance.rs the DEPTH20_FIXTURE comment reads:

    A real-shaped Binance depth20 payload (constructed to match the documented
    wire shape at <https://www.binance.com/en/binance-api> depth-stream sample,
    since a live capture wasn't practical for this fixture)

Three problems. The testing convention we agreed asks for real captured data,
because invented JSON tests my idea of the format rather than the format. The
URL cited isn't a depth-stream doc, it's a marketing page. And the data is
visibly synthetic — bids stepping down in exact 0.00001 increments with amounts
in exact 0.25 increments. Real books don't look like that, and anyone at a
market maker will recognise it on sight.

The practical risk is that the parser has never met real Binance output: varying
decimal counts, short values like "0", irregular gaps.

I'll capture a real message when I run the binary through my proxy. Prepare the
change so I can drop it in: keep the test structure, replace the fixture
constant, and rewrite the comment to record where and when it was captured,
e.g.

    // Captured from wss://stream.binance.com:9443/ws/ethbtc@depth20@100ms
    // on <date>.

Don't fabricate a replacement. If you can't reach the network, leave the
constant with a clear TODO naming me, and tell me it's outstanding. I'd rather
have a visible gap than a plausible-looking invention.

Going forward: never invent data that's presented as captured. If real data
isn't reachable, say the fixture is synthetic and say why, or ask me to capture
it.

## 3. The newtype's main justification has no test

model.rs has one test, a Display round-trip. But the newtypes exist for two
reasons, and the README and the module doc both cite them:

- type safety, which the compiler enforces and needs no test
- a total order via f64::total_cmp, which is tested nowhere

Ord is implemented and never exercised, and step 5's merge is going to rest
entirely on it. Add tests that actually pin the ordering behaviour: that a
collection of Prices sorts ascending by value, and that ordering is total and
consistent for equal values.

Two or three tests, not six. The point is that the claim in the README has
evidence behind it.

## 4. Test weight is inverted

Current distribution:

    proxy.rs        6 tests   (an env-var string splitter, my own network workaround)
    binance.rs      3 tests   (the actual feed parser)
    model.rs        1 test    (the foundation step 5 builds on)

The env-var helper has twice the coverage of the Binance parser. And four of
those six fail for the same reason — rejects_a_missing_port,
rejects_a_non_numeric_port and rejects_an_empty_string all assert "unparseable
input returns None". The convention says one test, not three, when they fail
together.

Cut proxy.rs down to two: one that parses a well-formed value with a scheme,
one that rejects unparseable input. Move the effort into model.rs and
binance.rs per items 3 and 2.

## 5. Amount::from_str_price is misnamed

The whole point of the newtypes is that a Price and an Amount can't be
confused, and the constructor on Amount is called from_str_price. Rename both
to Price::parse and Amount::parse, or from_decimal_str if you prefer something
more explicit. Update the call sites.

Small, but it's exactly the detail that undercuts the story the types are
telling.

## 6. Move the proxy plumbing out of main.rs

main.rs is 203 lines and roughly a hundred of them are the CONNECT tunnel,
which makes the read loop — the actual subject of step 1 — hard to find.

Move connect_through_proxy and read_http_response_headers into src/proxy.rs
next to parse_proxy_addr. main.rs should call one function and get back a
connected stream.

Two things to fix while they move:

read_http_response_headers reads a byte at a time, so a forty-byte response
costs forty syscalls. The comment is right that a full HTTP parser is overkill,
but BufReader::read_until is one line and isn't.

The `"://:"` special case is working around compose.yml producing a broken
value from empty PROXY_HOST/PROXY_PORT defaults. Fix the compose file so it
doesn't set the variable at all when those are unset, and delete the special
case. Configuration shouldn't emit a broken value that code then has to
recognise.

Keep the README framing as it is — an optional proxy for networks that can't
reach stream.binance.com directly is a legitimate feature and that's how it
should read.

## 7. Small

Message::Frame(_) can't arrive through next() on a normal stream; that variant
only appears with the raw frame API. Drop the arm if the match still compiles
exhaustively without it, and tell me if it doesn't.

## Process, for step 3 onward

Two things the commit history currently contradicts:

The spec packet for 002 was committed after all the implementation phases. I've
told the reviewer I write a spec first and implement against it, and the commit
order says the opposite. From step 3 on, the spec packet is the first commit on
the branch.

002-binance-feed was fast-forwarded into main, so there's no merge commit
marking the branch. Use --no-ff, including for this branch.

## Still outstanding, and I'll do these myself

Not for you, listed so they're tracked:

- The 25-minute live connection test against Binance's 20s ping / 60s pong
  timeout, with tungstenite=trace logging
- Verifying the CONNECT tunnel end to end against a real proxy
- Capturing the real fixture for item 2

## Acceptance

- cargo build, cargo test, cargo clippy --all-targets -- -D warnings,
  cargo fmt --check
- docker compose build
- test counts per module after the rebalance
- git log --graph showing the merge commit
- for every test you add, tell me what bug it catches; if you can't name one,
  don't add it
