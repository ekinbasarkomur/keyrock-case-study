# Step 6 (was 9) — the example client

## Reordering

The client moves ahead of reconnection and staleness. Three reasons, the third
being the real one.

The merge just landed and its eight tests run against hand-built fixtures. Live
data is a different check — bad ordering, a missing venue, a nonsense spread are
things you see on a screen and don't see in a unit test.

Delivery risk: right now someone cloning this runs `docker compose up` and then
has to work out grpcurl. With a client they see the book.

And the one that decides it: the client is the test instrument for reconnection.
Watching a venue drop out of the header and come back is a far better check of
staleness handling than reading logs, and I'd rather build the instrument before
the thing it measures.

Steps become: 6 client, 7 reconnection and staleness, 8 tests, 9 latency, 10
README and delivery. Spec folders are numbered independently, so this is 008.

## What I want from you first

Write the spec and stop. Branch: 008-client. Spec packet is the first commit.
Merge with --no-ff. Two minutes of reading.

## Scope

IN: src/bin/client.rs, one COPY line in the Dockerfile, a second service in
compose.yml, README changes.

OUT: everything else. No change to src/main.rs, src/merge.rs, src/aggregator.rs,
or any of the exchange modules. `git diff main --stat` should show only those
paths plus the spec packet.

Explicitly not Python. This client is part of the deliverable for a Rust role,
the proto types are already generated and reachable from the library crate, and
a second language would mean either a second image or Python inside the Rust one.
Both are worse than eighty lines of Rust.

## Where it lives

src/bin/client.rs. Cargo picks that up automatically and produces a second binary
named `client` — no [[bin]] section needed.

It has to be in this crate because it needs the generated client:

    use rust_crypto_orderbook::orderbook::orderbook_aggregator_client::OrderbookAggregatorClient;

which is reachable because lib.rs re-exports the generated module. That's the
lib/bin split from step 0 paying off again.

## The display

This matters to me — I want to actually watch the book, not read a log.

Redraw in place, like top. Not a scrolling dump: at ten to thirty updates a second
a scrolling dump is unreadable.

    print!("\x1b[H");   // cursor home, then overwrite
    print!("\x1b[K");   // at the end of each line, clear the rest of it

Do not use \x1b[2J. Clearing the whole screen every frame flickers. Home the
cursor and overwrite, clearing to end of line as you go.

Layout, sized to fit 80 columns:

      ETHBTC                          binance + bitstamp        14:32:07

             BIDS                                  ASKS
      0.03164010     5.00000000 bitstamp    0.03164890    12.50000000 binance
      0.03163990    22.00000000 binance     0.03164950     3.20000000 bitstamp
      ... ten rows ...

      spread  0.00000880  (2.8 bps)              1247 updates   12.3/s

Bids left, asks right — that's the convention in every trading terminal and it
costs nothing to match it. Ten rows each side. Fixed-width columns so the numbers
line up and don't jitter as values change; pad rather than reflow.

Fewer than ten levels on a side: leave the rows blank rather than collapsing the
layout, so nothing jumps.

## Colour

Also matters. Auto-detect, don't add a flag:

    use std::io::IsTerminal;
    let colour = std::io::stdout().is_terminal();

Stable since 1.70, no dependency. Redirecting to a file gets clean text with no
escape codes, and that's the right behaviour without me asking for it.

    bids            green      \x1b[32m
    asks            red        \x1b[31m
    venue labels    dim        \x1b[2m
    negative spread bold red   \x1b[1;31m
    reset                      \x1b[0m

The negative-spread highlight is the one that earns its place. Crossed books are
a deliberate decision — we publish the negative spread rather than clamping it —
and this makes it visible whether it actually happens, and how often. The unit
test proves the code handles it; the screen shows whether it occurs in practice.

No other colour. No TUI framework — ratatui or crossterm is a new dependency for
a demo tool.

## Spread in basis points

    spread  0.00000880  (2.8 bps)

spread / mid * 10000. Three lines of arithmetic, and it's the number that actually
means something — 0.0000088 on its own says nothing, 2.8 bps says the market is
tight.

## Update rate

Count messages, show total and a rolling rate. It proves the stream is live, it
previews the latency work in step 9, and it makes the difference between the two
venues' cadences visible — Binance every 100ms regardless, Bitstamp only on
change.

## Reconnection, deliberately simple

The client will start before the server is listening. depends_on waits for the
container to start, not for the port to accept, so a connect will fail.

    loop {
        match connect_and_stream(&addr).await {
            Ok(())  => info!("stream ended, reconnecting"),
            Err(e)  => warn!(%e, "connect failed, retrying"),
        }
        sleep(Duration::from_secs(1)).await;
    }

One loop covers both waiting for startup and recovering if the server dies —
which is useful in itself, since I can restart the server and watch the client
come back.

Fixed one-second delay. No backoff, no jitter. That machinery belongs to the
feeds in step 7 and building a lesser version of it here just makes two things
to keep straight.

## Docker

One COPY line for the second binary. Leave ENTRYPOINT alone — it's
["rust-crypto-orderbook"], and `docker compose run --rm app --pair btcusd` currently
relies on args passing through to the server. Emptying it breaks that.

The client service overrides at the service level:

    client:
      image: rust-crypto-orderbook:local
      entrypoint: ["client"]
      command: ["--addr", "http://app:50051"]
      depends_on: [app]
      tty: true

image rather than build, so the image is built once and shared.

tty: true so the escape codes render under docker compose up. Check whether that's
enough or whether it needs stdin_open as well — test it, don't guess.

Note in compose that if app exits (no route to Binance, no proxy set) the client
stays up retrying. That's correct behaviour — it makes the server's death visible
— but the logs will look like the client is the noisy one, so say so.

## Tests

None, deliberately, and say so in the spec.

The convention here is that every test names the bug it catches. A demo client's
rendering catches nothing a person looking at the screen wouldn't catch faster —
that's the entire purpose of the tool. A test asserting that a formatter produces
a particular string is testing my choice of layout, not correctness.

If you disagree and think something here has a bug worth catching, say what it is
rather than adding a test for coverage.

## Design for step 7 without building it

When reconnection and staleness land, the header gains venue status:

    ETHBTC     binance ●  bitstamp ○ stale 4.2s     14:32:07

That's how I'll verify staleness handling — watch a venue drop out and come back.
Don't build it now, but shape the render function so the header already takes the
venue list rather than a fixed string, so step 7 fills in a field rather than
restructuring.

## README

Two separate edits, and the first is easy to forget.

**The build order table has to be reordered.** It currently lists reconnection as
step 6 and the client as step 9. That's no longer true, and a README describing a
plan the repo isn't following is worse than one with no plan at all — it's the
same staleness problem that had the README claiming step 0 while step 1 was
merged. Renumber so it reads 6 client, 7 reconnection and staleness, 8 tests,
9 latency, 10 README and delivery, and mark 6 done.

While you're in that table, check nothing else in the README assumes the old
order — the Docker section and the production notes both reference behaviour that
changes in reconnection, so make sure neither implies it's already landed.

**Then a short section on the client itself.** What it is, the one command to see
it, and a line saying it's a demonstration tool rather than part of the service.
Three or four lines.

Also add the compose one-liner near the top, wherever a reader first meets "how
do I run this" — that command is now the best answer to that question and it
should be findable without reading down to a client section.

The README is at 1,157 words. It can grow slightly for this but not much; if the
client section runs long, trim elsewhere.

## Acceptance

- cargo run --bin client -- --addr http://127.0.0.1:50051 renders a live,
  redrawing, colourised book against a locally running server
- docker compose up brings both services up and the client renders in the
  compose output
- piping the client's output to a file produces clean text with no escape codes
- the README build order matches reality and step 6 is marked done
- cargo build, test, clippy --all-targets -- -D warnings, fmt --check
- git diff main --stat shows only client.rs, Dockerfile, compose.yml, README,
  and the spec packet

## At the end

The short list for my handbook: anything that surprised you, and specifically
whether tty/stdin_open behaved as expected under compose — that's the part I
expect to be fiddly.

Tell me plainly what you couldn't verify rather than reporting it as passing.

## How to work with me

Explain in Turkish, code and docs in English. This one is small; don't
over-engineer it. It's an instrument, not a product.
