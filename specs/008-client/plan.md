# Plan: 008-client

## Summary

Two phases, matching this step's `complexity: small` scope — one new code
file (`src/bin/client.rs`), two small existing-file edits (`Dockerfile`,
`compose.yml`), and a doc pass. Smaller than `007-merge`'s three (that step
had a hard sequencing requirement — README before algorithm — that forced a
phase boundary; this step has no such requirement, since the client is a new
binary with nothing existing to describe until it's written) and much
smaller than `006-bitstamp`'s five. Padding this to three phases just to
match prior steps would be exactly the kind of scope-inflation this
project's own conventions warn against.

- Phase 1 is the client itself: `src/bin/client.rs`, plus the `Dockerfile`
  `COPY` line and the `compose.yml` service, landed together because the
  compose service is what makes the Docker/`tty` open question testable at
  all — splitting the Docker edits into their own phase would mean shipping
  an unverifiable compose change first, then verifying it in a phase that
  touches no code, which is busywork, not a real boundary. This phase is
  where the update-rate divide-by-zero guard is built (a design requirement
  from spec.md's Tests section, not an afterthought), and where the `tty`
  vs `tty` + `stdin_open` question gets answered by actually running
  `docker compose up` and reading the output — not deferred to a later
  phase or left as a guess.
- Phase 2 is the README pass, landing last per this project's standing rule
  (`specs/003-step-1-fixes/revisions.md` entry 1: a README describes what
  was shipped, not what was planned) — the build-order reorder, the Docker
  section, and the production notes all need the client to exist and its
  compose behaviour to be actually observed before they can be written
  honestly, plus the full verification gate run once at the tip.

No tests are written for `client.rs` itself — spec.md is explicit and this
plan does not second-guess it. Verification throughout is real runs: `cargo
run --bin client`, `docker compose up`, and piping stdout to a file. These
manual runs are the actual acceptance evidence for this step, not `cargo
test` output, and every verification section below reflects that rather than
listing `cargo test` as if it covered the new behaviour.

## Phase Breakdown

### Phase 1: the client, Docker, compose — `src/bin/client.rs`, `Dockerfile`, `compose.yml`

- Objective: Land the demonstration client and make it runnable both
  directly and under `docker compose up`, resolving the one open technical
  question (`tty` vs `tty` + `stdin_open`) by observation rather than
  assumption.
- Main changes:
  - `src/bin/client.rs` (new, picked up automatically by Cargo — no
    `[[bin]]` section): `--addr` CLI flag via `clap::Parser`, matching
    `src/main.rs`'s existing convention. Connects using
    `rust_crypto_orderbook::orderbook::orderbook_aggregator_client::OrderbookAggregatorClient`
    (reachable because `src/lib.rs` already re-exports `pub mod orderbook`
    — confirmed by reading `src/lib.rs` before writing this plan). Redraw-
    in-place rendering (`\x1b[H` cursor home, `\x1b[K` clear-to-end-of-line
    per row, never `\x1b[2J`), fixed-width columns, ten rows per side,
    blank padding for fewer than ten levels — never a reflowed layout.
    Colour via `std::io::stdout().is_terminal()` (stable since 1.70, no new
    dependency), auto-detected, no flag. Spread shown in bps
    (`spread / mid * 10000`). Reconnection is the flat one-second-delay
    loop spec.md specifies verbatim — no backoff, no jitter (that belongs
    to step 7's feeds, not duplicated here).
  - **Design requirement, built here, not deferred:** the rolling
    update-rate calculation (`messages / elapsed_secs`) must guard against
    a near-zero elapsed interval — spec.md's Tests section identifies this
    as the one real risk in an otherwise test-free step. Guard by skipping
    or holding the last rate value until elapsed time is meaningfully
    positive (e.g. `elapsed_secs > 0.0`), not by a `cargo test` — per
    spec.md, this is a one-line defensive check reviewable by inspection,
    and the failure mode (`inf`/`NaN` briefly on screen) would be visible
    immediately in the manual run below if it regressed. Confirm during the
    manual run (see Verification) that the rate line never shows `inf` or
    `NaN` in the first frame.
  - The render function's signature takes a venue list, not a hardcoded
    header string — per spec.md's "design for step 7" note, so step 7 can
    fill in per-venue status later without restructuring this function. Not
    building the status indicators themselves this step.
  - `Dockerfile`: one `COPY --from=builder /build/target/release/client
    /usr/local/bin/client` line alongside the existing `rust-crypto-orderbook`
    binary copy. `ENTRYPOINT` for the `app` service stays
    `["rust-crypto-orderbook"]` — unchanged, since `docker compose run --rm
    app --pair btcusd` relies on args passing through to the server binary.
  - `compose.yml`: new `client` service — `image: rust-crypto-orderbook:local`
    (not `build:` — shares the image the `app` service builds),
    `entrypoint: ["client"]`, `command: ["--addr", "http://app:50051"]`,
    `depends_on: [app]`, `tty: true`. A comment noting that if `app` exits
    (no route to Binance, no proxy configured), `client` stays up retrying
    every second — correct behaviour, but the logs will read as if the
    client is the noisy one, called out so it doesn't look like a bug on
    review.
- Verification:
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
    --check` clean.
  - `cargo run --bin client -- --addr http://127.0.0.1:50051` against a
    separately running `cargo run -- --pair ethbtc --port 50051` — confirm
    a live, redrawing, colourised book renders in the terminal. Watch the
    first several frames specifically for the update-rate guard: no
    `inf`/`NaN` in the rate field.
  - Pipe the client's stdout to a file (`cargo run --bin client -- --addr
    http://127.0.0.1:50051 > /tmp/client-out.txt`, let it run briefly, kill
    it) and inspect the file for raw escape codes — confirm colour
    auto-detection produces clean text when not attached to a terminal.
  - **Live-tested checkpoint, not a guess:** `docker compose up --build`
    with both services running — actually observe whether `tty: true`
    alone is sufficient for the client's ANSI escape codes to render
    correctly in the compose output, or whether `stdin_open: true` is also
    needed. Report the real result plainly, including if the environment
    can't fully confirm it (no Docker daemon, no route to either exchange).
    If `tty: true` alone is insufficient, add `stdin_open: true` as a
    one-line compose fix and re-run to confirm.
  - Confirm the `client` service comes up and its entrypoint doesn't fail
    at container start (the concrete risk if the `Dockerfile` `COPY` line
    is missed or misnamed) — this is a real risk `cargo build` alone cannot
    catch, since it's a container-start-time failure, not a compile-time
    one.
  - **Scope check:** `git diff main --stat` — confirm only
    `src/bin/client.rs`, `Dockerfile`, `compose.yml`, and `specs/008-client/`
    appear (README not yet touched this phase). Any other path is a
    stop-and-flag condition.
- Done looks like: a working second binary, verified by direct run and by
  `docker compose up`, with the `tty`/`stdin_open` question answered by
  observation and reported honestly either way, and the update-rate guard
  confirmed not to show `inf`/`NaN` in practice.
- Commit boundary: `src/bin/client.rs`, `Dockerfile`, `compose.yml`. One
  commit. Reverting it removes the client entirely — `app`'s behaviour is
  unaffected, since `ENTRYPOINT`/`CMD` for that service is untouched.

### Phase 2: `README.md` — build-order reorder, client section, full verification gate

- Objective: Describe what Phase 1 actually shipped and actually observed,
  per this project's standing "README describes shipped behaviour" rule —
  including the real `tty`/`stdin_open` result from Phase 1, not a
  restatement of spec.md's open question.
- Main changes: `README.md`.
  - Reorder the build-order table: step 6 becomes "the example client"
    (marked done), step 7 becomes reconnection/staleness, step 8 tests,
    step 9 latency, step 10 README/delivery — matching this packet's
    numbering, per spec.md.
  - Add a short client section (three to four lines): what it is, the one
    command to see it (`docker compose up`, or `cargo run --bin client --
    --addr http://127.0.0.1:50051`), and an explicit line that it's a
    demonstration tool, not part of the service.
  - Add the `docker compose up` one-liner near the top of the README,
    wherever a reader first meets "how do I run this."
  - Check the Docker section and the production notes: neither should
    imply reconnection/staleness has landed (it hasn't — that's step 7) —
    fix any language that currently reads that way, per spec.md's explicit
    instruction to check both, not just the build-order table.
  - Word budget: README is ~1,157 words per spec.md; this can grow
    slightly, not much — trim elsewhere (per spec.md) if the client section
    runs long.
- Verification:
  - Read-through: build-order table matches actual step order; no stale
    "not yet implemented" language for the client; Docker section and
    production notes don't overstate reconnection/staleness.
  - `wc -w README.md` — confirm growth is slight, not a return toward the
    pre-`007-merge`-cut length.
  - **Scope check:** `git diff main --stat` — confirm the full branch shows
    only `src/bin/client.rs`, `Dockerfile`, `compose.yml`, `README.md`, and
    `specs/008-client/`. No other path, checked here at the tip as the
    final confirmation (Phase 1 already checked its own narrower slice).
  - **Full verification gate, run once here at the tip:**
    - `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D
      warnings`, `cargo fmt --check` — all clean. `cargo test`'s count is
      expected to be unchanged from the current baseline (no new tests this
      step, per spec.md) — report the actual observed count rather than
      assuming it matches.
    - Re-run the three manual checks from Phase 1 once more at the tip
      (`cargo run --bin client`, stdout-to-file, `docker compose up`) and
      quote actual observed output for each — this is the step's real
      acceptance evidence, not the `cargo test` line above.
- Done looks like: every README claim matches what Phase 1 actually shipped
  and what was actually observed running it (including the real
  `tty`/`stdin_open` answer), the full verification gate is clean, and the
  whole-branch scope check shows exactly the five expected paths.
- Commit boundary: `README.md` alone. Reverting it has no effect on build or
  runtime behaviour — the client keeps working with a README that's
  accurate through Phase 1 but silent on it.

## Cross-Cutting Considerations

- **Commit message length.** Per direct instruction this session: short,
  one-line commit messages, no multi-paragraph bodies. Two commits — e.g.
  "add client binary, Docker/compose wiring" and "README: document the
  client, reorder build order".
- **No tests for `client.rs`, by design — don't add any mid-implementation
  either.** If implementation finds itself reaching for a
  `cargo test`-style assertion on the render output, that's a scope
  deviation from spec.md's explicit call, not a nice-to-have — the update-
  rate guard is the one risk spec.md flags, and it's handled by review plus
  the manual run, not a test.
- **`tty` vs `tty` + `stdin_open` is a live question, not a planning
  decision.** Neither this plan nor spec.md picks an answer in advance —
  Phase 1's Docker verification step is where it gets resolved, by running
  the real command and reading the real output. If the environment can't
  run Docker at all, that is reported as "not verified here," per this
  project's standing honesty convention (same as every prior step's plan),
  not silently assumed to work.
- **Untouched-files discipline.** `src/main.rs`, `src/merge.rs`,
  `src/aggregator.rs`, every `src/exchange/*` file, and `src/feed.rs`
  should show zero diff at the tip of this branch — spec.md's Scope section
  is explicit that nothing in the server/feed/merge path changes for this
  step. A phase whose diff unexpectedly touches any of these is a
  stop-and-flag condition, checked at both phase boundaries above, not just
  the end.
- **`ENTRYPOINT` for `app` stays `["rust-crypto-orderbook"]`.** The client is
  wired entirely at the compose service level (`entrypoint: ["client"]`
  overriding only the `client` service), never by changing the shared
  image's default entrypoint — changing that would break `docker compose
  run --rm app --pair btcusd`.

## Verification Gates

Before this branch is considered ready to hand off:

- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` all clean at the tip of the branch.
- `cargo run --bin client -- --addr http://127.0.0.1:50051` renders a live,
  redrawing, colourised book against a locally running server — actually
  run and observed, quoted or described plainly, not assumed.
- Piping the client's stdout to a file produces clean text with no escape
  codes — actually run and confirmed.
- `docker compose up` brings both `app` and `client` up, and the client's
  rendered output appears in the compose output — actually run; the
  `tty`/`stdin_open` question answered by observation and reported
  honestly, including if the environment couldn't fully verify it.
- README's build-order table matches the actual step order and marks step 6
  done; Docker section and production notes don't imply reconnection/
  staleness has landed.
- `git diff main --stat` at the tip shows only `src/bin/client.rs`,
  `Dockerfile`, `compose.yml`, `README.md`, and `specs/008-client/` —
  checked at both phase boundaries, not only at the end.
- No test file or `#[test]` added for `client.rs` — confirmed by inspection,
  matching spec.md's explicit "none, deliberately" call.

## Expected Drift Triggers

If any of the following becomes true while implementing, update spec.md
before continuing rather than improvising past it:

- `tty: true` alone turns out insufficient and `stdin_open: true` doesn't
  fix it either — spec.md anticipates only the first fallback; a second
  fallback (or a fundamentally different compose shape) would need a
  spec.md update recording what was actually needed, not a silent
  workaround.
- The `Dockerfile`'s `COPY` line for the client binary is missed and this
  surfaces only at container start (`entrypoint: ["client"]` failing) —
  worth confirming caught in Phase 1's own verification rather than
  discovered later in Phase 2's full gate.
- Implementation finds a second real correctness risk beyond the
  update-rate guard that spec.md's Tests section didn't anticipate (e.g.
  the bps calculation genuinely hitting a zero-mid input, contrary to
  spec.md's rejection of that risk) — worth flagging and reconsidering
  whether a narrow test is warranted, rather than silently guarding it
  without a note.
- `docker compose up` cannot be run at all in this environment (no Docker
  daemon, no route to either exchange even through the configured proxy) —
  report this as "not verified here," not silently omitted, same standing
  rule every prior step's plan in this repo has used.
- `git diff main --stat` shows a touched path outside the five expected
  ones at either phase boundary — stop and reconcile before continuing,
  rather than folding an unplanned change into the same commit.
