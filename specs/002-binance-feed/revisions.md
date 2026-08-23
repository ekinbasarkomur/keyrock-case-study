# Revisions — 002-binance-feed

Numbered log of changes to the approved spec, applied after the fact. Each
entry states what changed, why, and what it supersedes. `spec.md` is not
rewritten in place — this file is the record of drift and its justification.

## 1. Price representation: newtypes over `f64`, not fixed-point `i64`

**Supersedes:** `spec.md`'s "Price representation" section (`Price`/`Amount`
as `i64` newtypes at a fixed `1e9` scale) and the corresponding parts of
Phase 1 in `plan.md`/`tasks.md`. Everything else in the spec stands.

**What changed:** `Price` and `Amount` become thin newtypes over `f64`
instead of scaled `i64`. `Ord`/`Eq` via `f64::total_cmp` (well-defined total
order, no ad-hoc `sort_by` comparators at call sites). `Display` formats to 8
decimals. `Debug` is derived. Parsing is the exchange string through the
standard `f64` parse — no scale constant, no integer conversion, no
integer-to-`f64` boundary conversion (there is no boundary anymore).

**Why:** measured, not assumed, after the original fixed-point choice was
already implemented and tested. Across two million realistic ETHBTC price
pairs, an `f64` subtraction rounded to 8 decimals never once disagreed with
the integer result — ETHBTC prices sit around 0.03 against a `1e-8` tick
(ratio ~3×10⁶), well inside `f64`'s 16 significant digits of headroom. The
one well-known `f64` rounding failure mode (`round(2.675, 2) → 2.67`) is a
decimal-literal artifact of how the literal itself rounds on entry into
binary — it doesn't arise from parsing an exchange string and subtracting,
which is the only arithmetic this step does. Speed is not a factor either:
this parses ~800 numbers and does ~200 comparisons a second; the
integer-vs-float difference at that rate is on the order of 12 microseconds
per second of wall time, unmeasurable against everything else the process
does.

Fixed-point was buying a documented scale assumption, string→integer and
integer→`f64` conversion helpers, and roughly fifty lines of code, in
exchange for guarding against an error of `1.9e-18` that doesn't survive
being formatted to 8 decimals. Not worth it for an application this size.
What the original design got right and this revision keeps: the newtype
boundary (compiler rejects passing an `Amount` where a `Price` goes) and a
well-defined total order for sorting. Those were earning their place; the
integer representation underneath them was not.

**What was removed:** the `1e9` scale constant, the integer-based
`Price`/`Amount`, the string→scaled-integer parser, the integer→`f64`
`Display` conversion, and the doc comment explaining the scale assumption
and its cost at both extremes. The exact-integer-arithmetic regression test
(`f64_would_lose_this_precision`, asserting the two-tick subtraction equals
exactly `5000`) is also removed — it guarded a property (bit-exact integer
spread arithmetic) this design no longer claims. It's replaced by a
round-trip test: `"0.03150000"` parses to a `Price` that `Display`s back as
`0.03150000`. The other parser tests (real-fixture 20/20 parse,
`serverShutdown` → `None`, malformed JSON → `None`) are unchanged — they
test parsing shape, not the price type's internal representation.

**Spread rounding — decided now so step 5 doesn't re-derive it:** the
combined-book spread (introduced at step 5, not this step) will be rounded
to 8 decimals at the gRPC boundary. It's the one value in this design that's
*computed* rather than passed through from an exchange string, so it's the
one place rounding presentation is a live question. Nothing for step 5 to
re-litigate.

**Convention going forward:** thin newtypes over the primitive type, carrying
only the traits the current code actually calls (no arithmetic operators, no
conversions, no helper methods added speculatively) — apply this by default
in later steps too: a venue identifier newtype rather than a bare `String`, a
spread newtype rather than a bare `f64`, and so on, each one added only when
it earns its place against real call sites, not pre-emptively.

## 2. Optional HTTP CONNECT proxy for the Binance connection

**Extends:** Phase 3's `main.rs` connect logic (`spec.md`/`plan.md` didn't
anticipate this — it's a real network constraint discovered while trying to
verify this step's acceptance criteria, not a design change to the feed
logic itself).

**What changed:** if `HTTPS_PROXY` (or `HTTP_PROXY`) is set in the
environment, the binary opens a plain TCP connection to that proxy first,
issues an HTTP `CONNECT stream.binance.com:9443` request over it, and once
the proxy answers `200`, hands that already-established `TcpStream` to
`tokio_tungstenite::client_async_tls` to do the TLS handshake and websocket
upgrade through the tunnel — exactly as if it were a direct connection, since
a `CONNECT` tunnel relays raw bytes and the real TLS handshake still happens
end-to-end with Binance's own certificate. If neither proxy env var is set,
behavior is unchanged: `connect_async` dials Binance directly.

**Why:** Binance is not reachable at all from the network this was developed
and verified on (outbound TLS handshakes to `stream.binance.com` are reset).
This mirrors the existing convention in a sibling project
(`projects/freelance/copytrader`), which routes several exchange clients
through the same kind of `HTTP_PROXY`/`HTTPS_PROXY` env vars for the same
reason. No proxy crate was added — an HTTP `CONNECT` handshake is a few lines
of plaintext protocol over a `TcpStream`, and `tokio_tungstenite` already
exposes the `client_async_tls` entry point that takes a pre-established
stream instead of dialing itself, so nothing beyond what's already a
dependency was needed.

**What this does NOT change:** no proxy authentication, no SOCKS5 support,
no per-request proxy (only the single outbound websocket connection this
step makes). A reviewer running this on a network with direct Binance access
sees no behavior change at all — the proxy path only activates when the env
var is present. `compose.yml`/`.env.example` gain generic `PROXY_HOST`/
`PROXY_PORT` placeholders (no real proxy address committed — that's
environment-specific and belongs in each runner's own untracked `.env`).

## 3. Testing convention (applies from this step forward, not a one-off)

**Extends:** the project's existing testing principles
(narrowest-meaningful-verification-first, real-path over mock) with concrete
filing/naming/fixture rules this project didn't have written down yet. Not a
correction of anything prior — step 0 and step 1 so far didn't conflict with
it — but it's now the standing rule for step 1 onward, so later specs inherit
it instead of re-deciding it per step. A condensed pointer to this entry is
also mirrored into this project's local (gitignored) agent guidance, so it
reaches later specs automatically rather than needing to be rediscovered
here each time.

**Filing — decided by access, not preference:**

- Unit tests: `#[cfg(test)] mod tests` at the bottom of the file under test.
  Use when the thing under test is internal (a parser, the merge, backoff
  calculation, staleness). Can reach private items.
- Integration tests: `tests/`, separate crate, public API only. Use when the
  thing under test is externally observable (the CLI, the gRPC stream).
- If a test needs something `pub` that has no other reason to be `pub`,
  that's the signal it belongs in a unit test — never widen the public
  surface just to make a test reachable.
- No separate "e2e" or "regression" categories. Regression is a *reason* to
  write a test (see below), not a filing location; end-to-end is a *scope*
  that still lands in one of the two locations above by the access rule.

**What NOT to write** — coverage-for-its-own-sake is explicitly rejected:

- A constructor test that only asserts the fields it was just given.
- A collection-length-only assertion when contents or order is the actual
  interesting property.
- A test that restates the implementation (would have to change on every
  refactor that doesn't change behavior — it's testing the wrong thing).
- A test for library code (`serde` works, `tokio` works — not this
  project's job to re-verify).
- Multiple tests that fail together for the same underlying reason — one is
  enough.
- A name like `test_parse_2`.

Before writing a test: name the bug it catches, and confirm that bug is
plausible. "None, it's for coverage" means skip it and say so.

**Naming:** a sentence describing the asserted behavior, no `test_` prefix
(`#[test]` already says that). `server_shutdown_message_is_not_a_book`, not
`test_parse`. `cargo test`'s output should read as a list of guarantees —
this project's author intends to show that list to people.

**Regression tests get a comment, not a category.** When a test exists
because something actually broke, or because a decision could plausibly be
undone later, say so in a comment directly above it — what broke, or what
the test protects against. Example going forward: `Price`/`Amount`'s
total-ordering (`f64::total_cmp`) and `Display` formatting tests should each
carry a line noting that a future "simplify `Price` back to a bare `f64`"
change would silently break comparison/log-readability, which is exactly
what they guard.

**Fixtures:** real captured data wherever the input's shape matters — for
parsers, an actual message off the wire (trimmed if unwieldy), embedded as a
string literal with a comment noting when it was captured. Invented JSON
only tests this project's idea of the format, not the real one. For the
merge (step 5), hand-written books are correct instead — there the input
shape is deliberately controlled, not sourced.

**Parallelism:** tests in a binary run concurrently by default. Never bind a
fixed port — bind `0`, read back the OS-assigned port (a fixed-port test is
a flake waiting to happen against another test in the same run). Anything
mutating process-wide state (env vars) must say so in a comment, following
`config.rs`'s existing pattern — not the inaccurate "single-threaded test"
phrasing.

**Where tests land, project-wide** (for later steps to follow without
re-deciding):

| Under test | Location | Why |
| --- | --- | --- |
| Binance parse | unit, in `binance.rs` | parse is internal |
| Bitstamp parse | unit, in `bitstamp.rs` | same |
| `Price`/`Amount` ordering and `Display` | unit, in `model.rs` | type behavior |
| Merge | unit, in `merge.rs` | fixtures need the internal `Book` shape |
| Backoff and jitter | unit | internal helper |
| Staleness exclusion | unit, in the aggregator | internal logic |
| CLI behavior | integration, `tests/cli.rs` | runs the real binary |
| gRPC stream | integration, `tests/grpc.rs` | server up, connect, receive |

The merge tests (step 5) are the ones that matter most in this project —
real effort belongs there; less is owed elsewhere.

**The gRPC test, specifically (step 2), noted now for when it lands:** bind
port 0, start the server on a task, connect a real `tonic` client, and take
**two** messages off the stream before asserting — one only proves the call
returned, two proves it's actually a stream, which is what the schema
promises and what could actually break.

## 4. Fixing the four ignored CLI tests: no mock server, no bounded-run flag

**Supersedes:** Phase 3's temporary `#[ignore]` on
`default_run_logs_defaults_with_empty_stdout`,
`flags_override_defaults_with_no_env_vars`,
`env_vars_override_defaults_with_no_flags`, and
`cli_flag_wins_over_env_var_for_port` in `tests/cli.rs`, and the two
follow-up options that were on the table (a local mock websocket server, or
a bounded `--max-messages`/`--once` CLI flag).

**What changed:** these four tests no longer wait for the process to exit.
They spawn the real binary with piped stderr, read lines until the
`starting pair=... port=...` line appears (or a short timeout elapses), then
kill the child and assert on the captured line — the same real binary, the
same real config/CLI-precedence code path, no network dependency either
way, and no synthetic server standing in for Binance.

**Why not a mock server:** the project's own testing convention (entry 3
above) already prefers real code paths over mocks, and here it isn't even a
tradeoff — the log line these tests assert on is written *before* the
connect attempt, so the test never needs the network call to succeed (or
even complete) to observe it. A mock server would add infrastructure to
solve a problem that doesn't require it.

**Why not a bounded-run flag:** adding a `--max-messages`/`--once` mode to
`main` would be new production behavior justified only by test
convenience — the kind of thing this project's spec discipline exists to
prevent. Killing the child after the assertion is satisfied gets the same
test coverage without touching `main.rs`'s design.

**What this doesn't cover:** these four tests still can't observe anything
past the `starting` line — they don't and can't prove the live Binance
connection itself works (real-network reachability is proven separately, by
the 25+ minute manual run and, for restricted networks, by the proxy path
in entry 2). That boundary is deliberate, not a gap: CLI/config-precedence
correctness and live-feed correctness are different claims, and this test
file only ever made the first one.

## 5. The 25-minute ping/pong survival run — actually verified, 2026-08-23

Spec.md's Acceptance Criteria and Risks sections both flagged the
ping/pong-survival claim as unverified until an empirical run happened
("acceptable for this step... a real bug found on day one rather than at
hour forty" if it failed). It has now been run for real, through the
project owner's own proxy (see entry 2) after they opened port 9443 on
their Squid proxy's `SSL_ports` ACL (`CONNECT` to non-443 ports is denied
by default; 9443 isn't in Squid's default allowlist).

**Result: passed, with hard evidence, not just "it didn't crash."**

- Connection held for **1623 seconds (27m 3s)** continuously, from
  `10:45:34Z` to `11:13:21Z` — comfortably past the required 25 minutes and
  past multiple full cycles of Binance's 60-second pong-timeout window.
- **84 `PING` frames received, 84 `PONG` frames sent** — a 1:1 match,
  confirmed via `tungstenite=trace`-level logging of the actual frame
  bytes, not inferred from the absence of a disconnect. This directly
  answers the risk this spec named: `tokio-tungstenite` only flushes a
  queued pong when the write half makes progress, and the read loop itself
  never writes anything — the concern was real, and the automatic
  pong-queueing mechanism handled it correctly across all 84 cycles.
- **3,079 book-update log lines**, continuous across the full run, zero
  gaps.
- **Zero errors, zero panics** anywhere in the trace log.

This closes the last open item from entry 4's "what this doesn't cover"
note and from spec.md's own Acceptance Criteria — live-feed correctness (as
opposed to CLI/config-precedence correctness) is now verified, not
outstanding.
