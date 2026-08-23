---
spec_name: "Step 6 — the example client"
spec_id: "008"
spec_folder: "008-client"
status: "approved"
created_at: "2026-08-24"
updated_at: "2026-08-24"
created_by: "spec-synthesizer"
creation_mode: "human-brief"
source_inputs:
  - "inputs/001-step-6-brief.md"
source_agents: []
goal: "Add src/bin/client.rs — a redrawing, colourised terminal viewer that streams BookSummary from the gRPC server and reconnects forever on a fixed delay — as the demonstration client and the instrument that will later verify step 7's reconnection/staleness work."
purpose: "Closes the delivery gap between `docker compose up` and actually seeing the merged book: today that requires grpcurl. It also front-runs step 7 (reconnection/staleness) by building the tool that will visually verify it, before building the thing itself — reordered ahead of it deliberately."
parent_request: "step-6 brief, 2026-08-24 (specs/008-client/inputs/001-step-6-brief.md)"
related_paths:
  - "src/bin/client.rs"
  - "Dockerfile"
  - "compose.yml"
  - "README.md"
verification_level: "manual"
complexity: "small"
---

# Spec: 008-client

## Problem

The repo has real end-to-end gRPC output (steps 0-5) but the only way to see
it is `grpcurl`. This step adds a second binary — a terminal client that
redraws the combined book in place, like `top` — as part of the deliverable
for a Rust role, and as the tool that will later make step 7's reconnection
and staleness handling visible on screen rather than only in logs.

**The design below is settled — implement it, don't re-derive the
reasoning.** Full rationale lives in the user's handbook, per this project's
"two minutes of reading" spec convention (matching 005/006/007).

## Scope

**IN:** `src/bin/client.rs` (new binary, picked up automatically by Cargo —
no `[[bin]]` section needed), one `COPY` line in the `Dockerfile`, a second
service in `compose.yml`, README changes (build-order table reorder +
short client section + a compose one-liner near the top).

**OUT, explicitly:** everything else. No change to `src/main.rs`,
`src/merge.rs`, `src/aggregator.rs`, or any exchange module. Not Python —
the client is part of the deliverable for a Rust role, the generated proto
client is already reachable from the library crate via `lib.rs`'s
`pub mod orderbook`, and a second language means a second image or Python
inside the Rust one. Both are worse than roughly eighty lines of Rust.

**Scope check:** `git diff main --stat` must show only `src/bin/client.rs`,
`Dockerfile`, `compose.yml`, `README.md`, and the `specs/008-client/`
packet. No other path.

## Design

### Where it lives and how it connects

`src/bin/client.rs`. It needs the generated client:

```rust
use keyrock_case_study::orderbook::orderbook_aggregator_client::OrderbookAggregatorClient;
```

reachable because `lib.rs` already re-exports the generated `orderbook`
module — the lib/bin split from step 0 paying off again. Takes one CLI flag,
`--addr` (e.g. `http://127.0.0.1:50051`), following the existing `clap`
`#[derive(Parser)]` convention used in `src/main.rs`.

### Display — redraw in place, not a scrolling dump

At 10-30 updates/second a scrolling dump is unreadable. Home the cursor and
overwrite, clearing to end of line as it goes:

```rust
print!("\x1b[H");   // cursor home
print!("\x1b[K");   // at the end of each line, clear the rest of it
```

**Never `\x1b[2J`** — clearing the whole screen every frame flickers.

Layout, sized to fit 80 columns:

```
  ETHBTC                          binance + bitstamp        14:32:07

         BIDS                                  ASKS
  0.03164010     5.00000000 bitstamp    0.03164890    12.50000000 binance
  0.03163990    22.00000000 binance     0.03164950     3.20000000 bitstamp
  ... ten rows ...

  spread  0.00000880  (2.8 bps)              1247 updates   12.3/s
```

Bids left, asks right (the trading-terminal convention). Ten rows per side,
fixed-width columns so numbers line up and don't jitter — pad, never
reflow. Fewer than ten levels on a side: leave the remaining rows blank
rather than collapsing the layout, so nothing jumps.

Shape the render function to take the venue list, not a fixed header
string — see "Design for step 7" below.

### Colour — auto-detect, no flag

```rust
use std::io::IsTerminal;
let colour = std::io::stdout().is_terminal();
```

Stable since Rust 1.70, no dependency. Redirected output gets clean text
with no escape codes automatically.

| Element | Style | Code |
| --- | --- | --- |
| bids | green | `\x1b[32m` |
| asks | red | `\x1b[31m` |
| venue labels | dim | `\x1b[2m` |
| negative spread | bold red | `\x1b[1;31m` |
| reset | | `\x1b[0m` |

The negative-spread highlight is the one that earns its place: crossed
books are a deliberate design decision (published as-is, never clamped —
see `specs/007-merge/spec.md`), and this makes visible whether it actually
happens in practice, and how often. No other colour, no TUI framework
(`ratatui`/`crossterm` is a new dependency for a demo tool).

### Spread in basis points

`spread / mid * 10000`, displayed alongside the raw spread. Three lines of
arithmetic; the raw number alone (`0.0000088`) says nothing, `2.8 bps` says
whether the market is tight.

### Update rate

Count messages received; show a running total and a rolling rate
(messages/second). Proves the stream is live, previews the latency work in
step 9, and makes the two venues' different cadences visible — Binance
pushes every ~100ms regardless of change, Bitstamp only on change.

### Reconnection — deliberately simple, not step 7's version

```rust
loop {
    match connect_and_stream(&addr).await {
        Ok(())  => info!("stream ended, reconnecting"),
        Err(e)  => warn!(%e, "connect failed, retrying"),
    }
    sleep(Duration::from_secs(1)).await;
}
```

One loop covers both "server not listening yet" (compose's `depends_on`
waits for the container to start, not for the port to accept) and "server
died and came back." Fixed one-second delay, no backoff, no jitter — that
machinery belongs to the feeds in step 7; a lesser duplicate of it here is
one more thing to keep in sync with no benefit.

### Docker

One `COPY` line for the second binary in the existing builder/runtime
split. `ENTRYPOINT` stays `["keyrock-case-study"]` — `docker compose run
--rm app --pair btcusd` relies on args passing through to the server, so it
must not change.

The client is a second `compose.yml` service, overriding at the service
level:

```yaml
client:
  image: keyrock-case-study:local
  entrypoint: ["client"]
  command: ["--addr", "http://app:50051"]
  depends_on: [app]
  tty: true
```

`image:`, not `build:`, so the image is built once (by the `app` service)
and shared. `tty: true` so the escape codes render under `docker compose
up`.

Note in the compose file's comments: if `app` exits (no route to Binance,
no proxy configured), the `client` service stays up retrying every second.
That is correct behaviour — it makes the server's death visible — but the
logs will read as if the client is the noisy one, so it should be called
out rather than left to look like a bug.

### Design for step 7 without building it

When reconnection/staleness land, the header gains per-venue status:

```
ETHBTC     binance ●  bitstamp ○ stale 4.2s     14:32:07
```

Watching a venue drop out of the header and come back is how step 7 gets
verified — this client is the instrument built ahead of the thing it
measures. Don't build the status indicators now, but the render function's
signature should already take a venue list rather than a hardcoded header
string, so step 7 fills in a field instead of restructuring the function.

### README

Two edits:

1. **Reorder the build-order table.** It currently lists step 6 as
   reconnection and step 9 as the client; the actual order (per this
   packet's numbering) is 6 client, 7 reconnection and staleness, 8 tests,
   9 latency, 10 README and delivery. Mark step 6 done once this lands. A
   README describing a plan the repo isn't following is worse than one with
   no plan — check the Docker section and the production notes too, since
   both currently reference behaviour (reconnection/staleness) that hasn't
   landed yet; make sure neither implies it already has.
2. **A short client section** (three to four lines): what it is, the one
   command to see it, and a line stating it's a demonstration tool, not
   part of the service. Add the `docker compose up` one-liner near the top
   of the README as well, wherever a reader first meets "how do I run
   this" — that's now the best answer to that question.

Current README is ~1,157 words (per the brief); it can grow slightly for
this, not much — trim elsewhere if the client section runs long.

## Tests

**None, deliberately.** This project's testing convention (`.claude/rules/
testing.md`, and this project's own established practice per specs
005-007) is that every test names the bug it catches. A demo client's
rendering catches nothing a person watching the screen wouldn't catch
faster — that is the entire purpose of the tool. A test asserting a
formatter produces a specific string tests a layout choice, not
correctness, and this project rejects coverage-for-its-own-sake tests on
exactly that basis.

**Recommendation reviewed against this default — one real risk found,
flagged rather than silently resolved:**

- **Rolling update-rate divides by elapsed time.** On the very first frame
  (or any frame where the interval since the last rate computation is ~0),
  `messages / elapsed_secs` risks a division by zero, producing `inf` or
  `NaN` in the displayed rate rather than a panic — a real, if cosmetic,
  correctness bug (silent wrong output, this project's usual concern
  category), not a rendering-choice question. **Recommendation: guard the
  rate calculation (e.g. skip/hold-last-value until elapsed > 0, or clamp
  the denominator), and it does not need a dedicated test** — the guard is
  a one-line defensive check, cheap enough to review by inspection, and the
  screen itself would immediately show `inf/s` if it regressed. Flagging
  it here as a design note to implement, not proposing a test for it.
- Other candidate risks were considered and rejected as not warranting a
  test: the bps calculation (`spread / mid * 10000`) has no realistic
  zero-mid input given real market data; fixed-width column overflow on an
  unusually large number is a display nit, not a correctness bug, and this
  is explicitly a demo instrument, not a production formatter. No test is
  recommended for either.

## Open Questions (do not guess — verify during implementation)

- **`tty: true` alone vs. `tty: true` + `stdin_open: true` under `docker
  compose up`.** The brief poses this explicitly: check whether `tty: true`
  is sufficient for the client's ANSI escape codes to render correctly in
  compose's output, or whether `stdin_open: true` is also needed. This is
  not resolved here — it must be tested live during implementation (added
  to `plan.md`/the implementation phase as a verification step), and the
  result reported plainly, including if it could not be fully verified
  rather than reported as passing.

## Acceptance Criteria

- `cargo run --bin client -- --addr http://127.0.0.1:50051` renders a live,
  redrawing, colourised book against a locally running server.
- `docker compose up` brings both services (`app` and `client`) up, and the
  client's rendered output appears in the compose output.
- Piping the client's stdout to a file produces clean text with no escape
  codes (colour auto-detection working).
- The README build-order table matches the actual step order and marks
  step 6 done; the Docker section and production notes do not imply
  reconnection/staleness has landed.
- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` all pass.
- `git diff main --stat` shows only `src/bin/client.rs`, `Dockerfile`,
  `compose.yml`, `README.md`, and the `specs/008-client/` packet.

## Risks

- **`tty`/`stdin_open` behaviour under compose is unverified** — see Open
  Questions. If escape codes don't render as expected with `tty: true`
  alone, the fix (adding `stdin_open: true`) is a one-line compose change,
  but it must be confirmed by running it, not assumed.
- The rolling-rate division-by-zero risk noted under Tests should be
  guarded in the implementation; if skipped, the failure mode is cosmetic
  (`inf`/`NaN` briefly on screen) rather than a crash, so it is a review
  note, not a blocker.
- Client and server share one image; if the `COPY` line for the second
  binary is missed, `entrypoint: ["client"]` fails at container start
  rather than at build time — worth confirming with a real `docker compose
  up`, not just `cargo build`.
