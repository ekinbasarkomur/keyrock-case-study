# Tasks: 008-client

## Task Writing Rules

- Each task should describe a real unit of progress.
- Each task should name the expected files or areas touched.
- Each task should include explicit verification.
- Prefer behavior-level verification over mock-only checks.

No `cargo test` task is included for `src/bin/client.rs` — spec.md is
explicit that this step ships no new automated tests ("None,
deliberately"). The manual runs below (`cargo run --bin client`,
stdout-to-file, `docker compose up`) are this step's real acceptance
evidence; do not add a test file or a `#[test]` for the client mid-task to
compensate.

## Tasks

### 1. `src/bin/client.rs` — the terminal client

- Files or areas: `src/bin/client.rs` (new)
- Change:
  - `#[derive(clap::Parser)]` struct with one flag, `--addr` (e.g.
    `http://127.0.0.1:50051`), matching `src/main.rs`'s existing `clap`
    convention.
  - Connect via
    `rust_crypto_orderbook::orderbook::orderbook_aggregator_client::OrderbookAggregatorClient`,
    reachable through `lib.rs`'s existing `pub mod orderbook` re-export — no
    library change needed.
  - Reconnection loop, exactly the flat form from spec.md — no backoff, no
    jitter:
    ```rust
    loop {
        match connect_and_stream(&addr).await {
            Ok(())  => info!("stream ended, reconnecting"),
            Err(e)  => warn!(%e, "connect failed, retrying"),
        }
        sleep(Duration::from_secs(1)).await;
    }
    ```
  - Redraw-in-place rendering: `\x1b[H` (cursor home) once per frame, `\x1b[K`
    (clear-to-end-of-line) at the end of each printed line. Never
    `\x1b[2J`.
  - Layout: bids left, asks right, 10 rows per side, fixed-width columns
    (pad, never reflow); fewer than 10 levels on a side leaves the
    remaining rows blank rather than collapsing the layout.
  - Colour, auto-detected via `std::io::stdout().is_terminal()` (stable
    since 1.70, no new dependency), no CLI flag: bids green (`\x1b[32m`),
    asks red (`\x1b[31m`), venue labels dim (`\x1b[2m`), negative spread
    bold red (`\x1b[1;31m`), reset `\x1b[0m`.
  - Spread shown in bps alongside the raw value: `spread / mid * 10000`.
  - Update-rate counter: running total of messages received plus a rolling
    messages/second rate.
  - **Design requirement (not optional, not deferred to a test):** guard
    the rolling update-rate calculation against a near-zero elapsed
    interval. `messages / elapsed_secs` on the first frame (or any frame
    where the interval since the last rate computation is ~0) must not
    produce `inf`/`NaN` on screen — skip the update or hold the last
    computed rate until `elapsed_secs` is meaningfully positive. One line,
    reviewable by inspection; no test is written for it per spec.md, but it
    must be visually confirmed clean in this task's manual verification
    below.
  - Render function signature takes a venue list (e.g. `&[Venue]` or
    equivalent), not a hardcoded header string — so step 7 can add
    per-venue status later by filling in a field, not by restructuring the
    function. Do not build the status indicators themselves this step.
- Verification:
  - `cargo build` — clean.
  - `cargo clippy --all-targets -- -D warnings` — clean.
  - `cargo fmt --check` — clean.
  - Manual, real-path run (this is the actual proof this task works, not
    the three commands above): start a server with
    `cargo run -- --pair ethbtc --port 50051` in one terminal, then in
    another run `cargo run --bin client -- --addr http://127.0.0.1:50051`
    and observe a live, redrawing, colourised book for at least 10-15
    seconds. Specifically watch the first several frames for the
    update-rate guard: confirm the rate field never shows `inf` or `NaN`.
- Done when:
  - The client binary builds and, run against a real locally running
    server, renders a live redrawing book with correct colour, correct
    column alignment, and a sane (non-`inf`/`NaN`) update rate from the
    first frame onward — observed directly, not assumed from a green
    build.

### 2. `Dockerfile` — ship the second binary

- Files or areas: `Dockerfile`
- Change: add one `COPY --from=builder /build/target/release/client
  /usr/local/bin/client` line alongside the existing
  `rust-crypto-orderbook` binary copy in the runtime stage. Do not touch
  `ENTRYPOINT` (`["rust-crypto-orderbook"]` stays as-is — `docker compose run
  --rm app --pair btcusd` depends on args passing through to the server
  binary unchanged).
- Verification:
  - Covered by task 4's live `docker compose up --build` (a missing or
    misnamed `COPY` line fails at container start, not at `cargo build`
    time — there is no build-time check for this on its own).
- Done when:
  - The built image contains both `/usr/local/bin/rust-crypto-orderbook` and
    `/usr/local/bin/client`, confirmed indirectly by task 4's compose run
    succeeding for both services.

### 3. `compose.yml` — the `client` service

- Files or areas: `compose.yml`
- Change: add a second service:
  ```yaml
  client:
    image: rust-crypto-orderbook:local
    entrypoint: ["client"]
    command: ["--addr", "http://app:50051"]
    depends_on: [app]
    tty: true
  ```
  `image:`, not `build:` — shares the image the `app` service builds rather
  than rebuilding. Add a comment noting that if `app` exits (no route to
  Binance, no proxy configured), `client` stays up retrying every second —
  correct behaviour, called out so it doesn't read as a bug in the logs.
- Verification:
  - Covered by task 4's live `docker compose up --build`.
- Done when:
  - `compose.yml` declares both `app` and `client` services, with `client`
    depending on `app` and sharing its image.

### 4. Live Docker/tty verification — the open question, resolved by running it

- Files or areas: `compose.yml` (only if `stdin_open: true` needs adding as
  a fallback)
- Change: none by default — this task is the verification step that
  answers spec.md's open question rather than a code change. Only touch
  `compose.yml` if the check below shows `tty: true` alone is
  insufficient.
- Verification:
  - Run `docker compose up --build` with both `app` and `client` in the
    stack. Observe whether the client's ANSI escape codes (cursor-home,
    colour) render correctly in the compose log output, or whether it
    prints raw escape sequences / doesn't redraw as expected.
  - If `tty: true` alone is sufficient: report that plainly, no further
    change.
  - If it is not sufficient: add `stdin_open: true` to the `client` service
    in `compose.yml` and re-run `docker compose up --build` to confirm the
    fallback fixes it. Report which of the two outcomes actually occurred
    — do not assume `tty: true` works without having watched the output.
  - If Docker cannot be run at all in this environment (no daemon, no
    route to either exchange even through the configured proxy), report
    that explicitly as "not verified here" rather than silently skipping
    it or claiming success.
  - Separately confirm the `client` service's container actually starts
    (entrypoint doesn't fail) — this is the real-world check that task 2's
    `COPY` line was not missed or misnamed.
- Done when:
  - The `tty` vs `tty` + `stdin_open` question has a reported, observed
    answer (or an honest "could not verify" if Docker isn't available
    here) — not a guess — and both services are confirmed to start.

### 5. Stdout-to-file check — colour auto-detection under redirection

- Files or areas: none (verification-only task, no file changes)
- Change: none.
- Verification:
  - With a server running locally
    (`cargo run -- --pair ethbtc --port 50051`), run
    `cargo run --bin client -- --addr http://127.0.0.1:50051 >
    /private/tmp/claude-501/-Users-ekinbasarkomur-projects-rust-crypto-orderbook/171ee52b-46ec-42ce-8b19-7a11cee41686/scratchpad/client-out.txt`,
    let it run for a few seconds, then stop it (Ctrl-C or kill).
  - Inspect the file (`cat` or `Read`) and confirm it contains clean,
    readable text — book levels, spread, update rate — with no `\x1b[`
    escape sequences.
- Done when:
  - The captured file shows plain text with no ANSI escape codes, proving
    `std::io::IsTerminal` correctly disables colour/redraw codes when
    stdout isn't a terminal.

### 6. Phase 1 scope check

- Files or areas: none (verification-only task)
- Change: none.
- Verification:
  - `git diff main --stat` — confirm the only paths listed are
    `src/bin/client.rs`, `Dockerfile`, `compose.yml`, and
    `specs/008-client/` (README not yet touched at this point). Any other
    path is a stop-and-flag condition — do not proceed to Phase 2 until
    reconciled.
- Done when:
  - `git diff main --stat` output matches the expected path list exactly,
    quoted in the task's completion report.

### 7. `README.md` — build-order reorder and client section

- Files or areas: `README.md`
- Change:
  - Reorder the build-order table so step 6 is "the example client" (marked
    done), step 7 is reconnection/staleness, step 8 is tests, step 9 is
    latency, step 10 is README/delivery — matching this packet's numbering.
  - Add a short client section (three to four lines): what it is, the one
    command to see it
    (`docker compose up`, or
    `cargo run --bin client -- --addr http://127.0.0.1:50051`), and an
    explicit line that it is a demonstration tool, not part of the service.
  - Add the `docker compose up` one-liner near the top of the README,
    wherever a reader first meets "how do I run this."
  - Read through the Docker section and the production notes and fix any
    language that currently implies reconnection/staleness has already
    landed — it hasn't; that's step 7.
  - Keep the addition small — current README is ~1,157 words; it can grow
    slightly for this, not much. Trim elsewhere if the client section runs
    long.
- Verification:
  - Read-through: build-order table matches the actual step order; no
    stale "not yet implemented" language for the client; Docker
    section/production notes don't overstate reconnection/staleness.
  - `wc -w README.md` — confirm the word count grew only slightly from the
    pre-edit baseline, not back toward a much longer prior version.
- Done when:
  - The README accurately describes what task 1-4 actually shipped and
    what was actually observed running it (including the real
    `tty`/`stdin_open` result from task 4) — not a restatement of
    spec.md's open question as if still unresolved.

### 8. Full verification gate and final scope check

- Files or areas: none (verification-only task)
- Change: none.
- Verification:
  - `cargo build` — clean.
  - `cargo test` — clean; report the actual observed test count (expected
    unchanged from the pre-branch baseline of 32, since this step adds no
    new tests) rather than assuming it matches.
  - `cargo clippy --all-targets -- -D warnings` — clean.
  - `cargo fmt --check` — clean.
  - Re-run, at the tip of the branch, the three manual checks from tasks 1,
    4, and 5 once more and quote the actual observed output for each:
    - `cargo run --bin client -- --addr http://127.0.0.1:50051` against a
      locally running server — live redraw, correct colour, no
      `inf`/`NaN` in the rate field.
    - Stdout piped to a file — clean text, no escape codes.
    - `docker compose up --build` — both services up, client output
      visible in compose logs, `tty`/`stdin_open` result reconfirmed.
  - `git diff main --stat` — confirm the whole branch shows only
    `src/bin/client.rs`, `Dockerfile`, `compose.yml`, `README.md`, and
    `specs/008-client/`. No other path.
  - Confirm by inspection that no test file or `#[test]` was added for
    `client.rs` anywhere in the diff.
- Done when:
  - All four `cargo` commands pass, all three manual checks are
    re-confirmed at the tip with observed output quoted (not assumed
    unchanged from task-level runs), and the whole-branch `git diff
    main --stat` shows exactly the five expected paths.

## Final Verification

Before closing the packet, run:

- `cargo build && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
- `cargo run --bin client -- --addr http://127.0.0.1:50051` against a
  separately running `cargo run -- --pair ethbtc --port 50051` — the most
  representative real functionality check for this step: watch a live,
  redrawing, colourised combined book render in a real terminal, from a
  real server, over a real gRPC stream.
- `docker compose up --build` with both `app` and `client` — confirms the
  same behaviour end-to-end through the actual deployment path, and is
  where the `tty`/`stdin_open` question gets its final, reported answer.
