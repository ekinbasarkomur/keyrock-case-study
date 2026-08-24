# Revisions — 008-client

Numbered log of changes applied after `spec.md` was approved and
implementation landed. Each entry states what changed, why, and what it
supersedes. `spec.md` is not rewritten in place — this file is the record of
drift and its justification.

## 1. `src/feed.rs`'s per-message log downgraded from `info!` to `debug!`

**Supersedes:** nothing in `spec.md` directly — `src/feed.rs` isn't in the
spec's declared IN scope (`src/bin/client.rs`, `Dockerfile`, `compose.yml`,
`README.md` only). This is the one line that reaches outside it.

**What changed:** `feed.rs`'s per-parsed-book log line (previously `info!`)
is now `debug!`. Still visible via `RUST_LOG=debug`.

**Why:** live testing found the client's redraw flickering under
`docker compose up`. Root cause: Binance alone pushes this log ~10/s, and at
`info` (the default level) it floods stdout, interleaved with the client's
own frames on the same combined stream. It's per-tick diagnostic detail, not
a state-change event — never worth `info` on its own merits, but only
surfaced as a problem once there was a redraw-in-place consumer of the same
terminal to break. One-line fix, justified by what it fixed, not scope
creep — no new behavior, no new dependency.

## 2. Client display rebuilt as a bordered table, not the plain-line layout `spec.md` specified

**Supersedes:** `spec.md`'s "Display" section, which described plain
fixed-width lines (`  {:<50}{:>10}`-style, no borders) matching the brief's
ASCII mockup literally.

**What changed:** `render()` now draws the frame inside a box-drawing border
(`┌─┬─┐` / `├─┼─┤` / `└─┴─┘`), via new `border()`/`row()`/`visible_len()`
helpers. `visible_len()` strips ANSI colour codes before computing padding,
so a coloured cell doesn't throw off border alignment. `push_line()` and its
`\x1b[K` clear-to-end-of-line approach are gone — replaced by every row
being padded to a fixed `TABLE_WIDTH`, which makes clearing trailing
characters unnecessary (every frame overwrites the same width).

**Why:** asked for directly ("print it as a table so I can see better") once
the redraw was working end to end. Fixed-width padding turned out to be a
prerequisite for the flicker fix in entry 3 as well, not just cosmetic — the
first symptom that led to entry 3's diagnosis was border corruption from
line-wrapping, and having every row at one deterministic printed width made
that wrapping visible and diagnosable in the first place.

**Also included:** `TABLE_WIDTH` cut from an interior implying an 80-column
total row to a 76-column total, for margin against terminals narrower than
exactly 80 and inconsistent exact-width wrap behaviour at the boundary.

## 3. `client` service: removed from `compose.yml`, then restored behind `profiles: ["demo"]`

**Supersedes:** `spec.md`'s Docker section and Open Question, which assumed
`client` would run under `docker compose up` (the only open question was
`tty: true` alone vs. plus `stdin_open: true`).

**What changed, in order:**
1. First diagnosed the flicker as `feed.rs`'s log noise (entry 1) — real,
   but not the whole story.
2. Redesigned the table (entry 2) for readability — surfaced a second,
   distinct symptom: borders tearing across frames even with the log fix
   applied.
3. Misdiagnosed the second symptom as "the client doesn't belong in
   `compose.yml` at all" and removed the service, replacing it with
   `cargo run --bin client` as the documented way to view the book.
4. Correct diagnosis, on review: `docker compose up` multiplexes every
   service's output and prefixes each line with the service name
   (`"client  | "`, ~12 columns). Added to the table's own width, that
   pushes lines past 80 columns and wraps them — which breaks
   cursor-addressed redraw (`\x1b[H` assumes one logical row is one
   physical terminal row). No `tty:`/escape-code change fixes a
   multiplexed-log-wrapping problem. Restored `client` in `compose.yml`,
   now behind `profiles: ["demo"]` so `docker compose up` starts only
   `app`, with `docker compose run --rm client` as the documented way to
   view it — `run` attaches stdin/stdout to one service directly, no
   prefix, no interleaving, which is the correct idiom for an interactive
   tool under compose regardless of this specific bug.

**Why the two extra round-trips are recorded, not smoothed over:** the
wrong conclusion ("the client is broken") was the obvious one from the
symptom, and the fix was in the invocation, not the code — worth having
straight, including the detour, in case the same symptom resurfaces
elsewhere in this repo's Docker usage.

**Open Question resolution:** `tty: true` alone was never actually the
deciding factor — confirmed separately that neither `tty: true` alone nor
`tty: true` + `stdin_open: true` changes multiplexed-log wrapping. Both are
still set on the restored service (needed for ANSI rendering itself under
`run`, unrelated to the multiplexing question).

## 4. README's "What I'd change for production" section restored, as a table

**Supersedes:** `spec.md`'s Scope section, which listed only "build-order
table reorder + short client section + a compose one-liner" as the README
changes — restoring a whole section wasn't in that list.

**What changed:** a 5-row, one-sentence-per-row table (pair-per-process,
hardcoded 8-decimal tick, `merge()` returning proto types directly,
reflection always on, `Level.exchange` allocating per level) added back to
`README.md`. It had been deleted entirely on `main` before this spec
branched (`f3cc71f`, during 007-merge cleanup — "What would change for
production is stupid lets delete it from readme").

**Why:** on review, the content wasn't the problem with the original
section — verbosity was (five paragraphs restating what the code already
said). Two of the five items (pair-per-process, the hardcoded tick) are
consequences of decisions the README already states elsewhere as
deliberate; a reader who spots either with nothing acknowledging it
concludes it wasn't noticed, when it was. A table with one sentence per row
keeps the acknowledgment without the padding that got it deleted the first
time.

## 5. Client handles `SIGINT` explicitly

**Supersedes:** nothing stated in `spec.md` — signal handling wasn't
discussed there at all; the reconnect loop (`spec.md`'s "Reconnection"
section) was silent on how the process ever exits.

**What changed:** `main()`'s reconnect loop now races against
`tokio::signal::ctrl_c()` in a `tokio::select!`, logging and returning
`Ok(())` on `SIGINT` instead of relying on the OS default disposition.

**Why:** `client` runs as PID 1 in its container (exec-form
`entrypoint: ["client"]`, no shell in between). Linux does not apply a
signal's default action (terminate, for `SIGINT`) to PID 1 unless the
process installs its own handler for that signal — an unhandled `SIGINT`
is silently ignored by PID 1 rather than killing it. Confirmed empirically
before fixing: `docker kill --signal SIGINT` on a running (un-fixed)
`client` container left it `Up`; the same command against the fixed binary
produced `Exited (0)` with a logged `received ctrl-c, exiting` line. This
also fixes Ctrl-C under plain `cargo run --bin client` (not PID 1 there,
already worked, but the explicit handler is now the same code path either
way — one behavior, not two).

## 6. Depth bar: shaded row background, scaled to the largest amount shown

**Supersedes:** `spec.md`'s "Display" and "Colour" sections, which specified
only foreground colour (bids green, asks red, venue dim, negative spread
bold red) and no per-row background treatment.

**What changed:** each bid/ask cell's background is now shaded left-to-right
proportional to that level's amount against `max_amount` — the largest
amount across *both* sides currently displayed (a shared scale, not
per-side), so a bar of the same length on either side represents the same
size. Implemented as `shade_cell()`: builds the cell's plain 35-char text,
then emits ANSI runs merging two independent boundaries — `CELL_FG_SPLIT`
(price/amount foreground vs. dim venue-label foreground) and `fill_len`
(bar background vs. terminal-default background) — each run re-stating both
its foreground and background explicitly, since SGR state otherwise
persists across writes and an unstated attribute would bleed into the next
run.

**Why:** asked for directly — the standard depth-bar convention from
exchange order-book UIs (Binance/Bitstamp's own), so market depth
concentration reads at a glance instead of requiring the numbers to be
read and compared by eye. Scale choice (shared max, not per-side) and
render style (shaded background, not a separate bar of block characters)
were both explicit choices confirmed before implementing, not defaults —
per-side max would make the two columns incomparable at a glance, and a
foreground block-character bar risked competing visually with the existing
bid/ask colour coding a plain background shade doesn't.

**Verification:** the existing testing convention (`spec.md`'s "Tests"
section: none, deliberately — nothing here a person watching the screen
wouldn't catch faster) still applies; this was checked by capturing real
PTY output (`cargo run` under a pseudo-terminal, not a redirected pipe,
since `is_terminal()` — and therefore all colour/bar output — is false
under redirection) and confirming programmatically that the row with the
largest amount got a full-width bar and every other row's bar length
matched its amount's fraction of that max.

## 7. Depth bar colour: side-matched (green/red) and darker, not neutral grey

**Supersedes:** entry 6's `BAR_BG` (a single neutral grey, `48;5;238`, for
both sides).

**What changed:** `BAR_BG` split into `BAR_BG_BID` (`48;5;22`, dark green)
and `BAR_BG_ASK` (`48;5;52`, dark red) — the bar now matches its side's
existing foreground colour instead of a neutral shade, and both are a
darker/more solid 256-colour tone than the grey they replaced. `level_cell`
and `shade_cell` take the bg colour as a parameter now, rather than a single
module constant.

**Why:** asked for directly, on seeing entry 6's neutral-grey version live —
grey read as a separate, unrelated highlight rather than an extension of
the bid/ask colour coding already in place, and the grey's tone read as too
faint ("less transparent" — a more saturated, solid fill, not a pale tint).

**Verification:** same PTY-capture method as entry 6, re-run against this
change — confirmed bid cells now emit `48;5;22` and ask cells `48;5;52` in
their bar runs, replacing every prior `48;5;238` occurrence.
