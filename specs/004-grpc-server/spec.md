---
spec_name: "Step 2 — gRPC server with fake data"
spec_id: "004"
spec_folder: "004-grpc-server"
status: "approved"
created_at: "2026-08-23"
updated_at: "2026-08-23"
created_by: "spec-synthesizer"
creation_mode: "human-brief"
source_inputs:
  - "inputs/human.md"
source_agents: []
goal: "Stand up the tonic OrderbookAggregator gRPC server, streaming a once-a-second fake Summary over a watch channel, with reflection enabled, three supervised tasks in main, and a container that stays up and publishes the port."
purpose: "Step 2 of the 11-step build order proves the fiddly watch::Receiver-to-tonic-stream plumbing (Pin<Box<dyn Stream>> erasure, WatchStream adaptation, Option<Summary>-before-first-value handling) against a fake producer, so step 3's real integration is just deleting the fake writer and pointing the watch at the real aggregator, not discovering the streaming plumbing and the real feed at the same time."
parent_request: "step-2 grpc-server brief, 2026-08-23"
related_paths:
  - "src/server.rs"
  - "src/main.rs"
  - "build.rs"
  - "Cargo.toml"
  - "compose.yml"
  - "tests/grpc.rs"
  - "README.md"
verification_level: "mixed"
complexity: "small"
---

# Spec: 004-grpc-server

## Problem

Step 1 (`002-binance-feed`, plus its `003-step-1-fixes` follow-ups) landed a
working Binance feed that connects, parses, and logs to stderr — nothing
else. No `src/server.rs` exists, nothing implements the generated
`OrderbookAggregator` trait, and nothing listens on a port. The proto types
are compiled and included (`src/lib.rs`'s `pub mod orderbook`) but unused.
The build order's step 2 is to stand up the gRPC server itself, proven end
to end with fake data, before step 3 wires it to the real feed. No code
should land until this spec is approved.

## Goal

After this step:

- `src/server.rs` implements the generated `OrderbookAggregator` trait,
  streaming `Summary` values from a `tokio::sync::watch::Receiver<Option<Summary>>`.
- A fake-data task writes a plausible `Summary` (10 bids, 10 asks, small
  positive spread, `Level.exchange == "fake"`) into the watch channel once a
  second.
- `src/main.rs` spawns three tasks — the existing Binance feed, the new fake
  writer, and the new gRPC server — and exits the moment any one of them
  ends, via `tokio::select!` over their `JoinHandle`s.
- `grpcurl -plaintext localhost:50051 list` (or the container's published
  port) shows `orderbook.OrderbookAggregator`, with no local `.proto` file,
  via `tonic-reflection` registered alongside the aggregator service.
- `docker compose up` brings up a container that stays running, binds
  `0.0.0.0` inside the container, and is reachable at `127.0.0.1:50051` from
  the host — on a network where the container can reach Binance (directly or
  via the existing `PROXY_HOST`/`PROXY_PORT` pass-through). On a network
  where it can't, the container exits when the feed task ends, per the
  `select!` design — expected, not a defect (see Proposed Design's Docker
  section).
- `tests/grpc.rs` starts the real server on an OS-assigned port, connects a
  real `tonic` client, and asserts on two consecutive streamed messages.
- `README.md` reflects this step's state: the server exists and streams
  placeholder data, the `grpcurl` reflection one-liner, the corrected Docker
  section, `src/server.rs` in the Layout tree.
- `src/exchange/binance.rs`, `src/model.rs`, and `src/proxy.rs` are
  untouched — confirmed by `git diff main --stat` at the end.

## Purpose

The watch channel and the `watch::Receiver` → `tonic` response-stream
plumbing are, strictly, an aggregator-to-server concern that doesn't belong
until step 3, once a real aggregator exists to own the watch's sending side.
It lands here anyway, one step early, for the same reason Docker landed in
step 0 on an empty binary: the awkward part of this step is turning a
`watch::Receiver<Option<Summary>>` into something `tonic` will accept as a
`BookSummaryStream` — trait-object erasure, `WatchStream` adaptation, and
deciding what a subscriber sees before the first real value exists. Hitting
that with a fake producer, in isolation, means a mistake there is found
without simultaneously debugging a real feed. It also shrinks step 3 to a
near-trivial diff: delete the fake writer task, point the watch's sending
side at the real aggregator's output, done. Each step stays independently
runnable and reviewable, which matters more here than keeping "gRPC" and
"aggregator" as conceptually separate steps.

## Out of Scope

- `src/exchange/binance.rs`, `src/model.rs`, `src/proxy.rs` — no changes.
  Verified at the end via `git diff main --stat`.
- Any merge logic (`merge()`, a `Book`-to-`Summary` conversion driven by real
  data) — that's step 5.
- Bitstamp — step 4.
- Reconnection or staleness handling — step 6.
- A real `aggregator.rs` task that owns both venues' books — step 3 (wiring)
  and step 5 (merge) build this; this step's watch sender is the fake writer
  only.
- Gating `tonic-reflection` behind a feature/config flag for production —
  named as a limitation to document in the README, not implemented here.

## Current State

Verified by reading the files directly:

- `src/lib.rs` declares `pub mod config; pub mod exchange; pub mod model;
  pub mod proxy; pub mod telemetry;` and `pub mod orderbook { tonic::include_proto!("orderbook"); }`
  — the generated `Summary`/`Level`/`Empty`/`OrderbookAggregator` types
  already compile and are already reachable from library code, but nothing
  implements the service trait yet.
- `src/main.rs` is `#[tokio::main] async fn main()`, driving the Binance
  read loop directly in the `main` task (no `spawn`, no `select!` yet — see
  `specs/002-binance-feed/spec.md`). This step changes that shape: the feed
  loop becomes one of three spawned tasks.
- `Cargo.toml` already carries `tonic` `0.14.6`, `tonic-prost` `0.14.6`,
  `tonic-prost-build` `0.14.6` (build-dependency), and `tokio-stream`
  `0.1.19` — confirmed by reading `Cargo.toml`/`Cargo.lock` directly.
  `tonic-reflection` is **not** yet a dependency; it must be added, and
  `cargo add tonic-reflection` should resolve to `0.14.6` given the already
  pinned `tonic`/`tonic-prost` versions (confirmed available on crates.io at
  that version, matching this project's tonic line — see Proposed Design).
- `build.rs` currently runs only `tonic_prost_build::compile_protos("proto/orderbook.proto")`
  — it does not emit a file descriptor set, which reflection needs.
- `Config` (`src/config.rs`) already has `host` (default `127.0.0.1`, doc
  comment already states it must be `0.0.0.0` in a container) and `port`
  (default `50051`), both read from `ORDERBOOK_HOST`/`ORDERBOOK_PORT` — no
  config changes needed for this step, only actually acting on `host`/`port`
  by binding a listener to them.
- `compose.yml` has no `ports:` section and a comment stating "When a server
  lands: publish on loopback only and set ORDERBOOK_HOST=0.0.0.0" — this step
  is that moment.
- `Dockerfile`'s `CMD []` runs the binary with no arguments; there is a
  commented-out `HEALTHCHECK` block explicitly deferred until a server
  exists — still not added in this step (no acceptance criterion asks for
  it; adding one is a judgment call to flag, not silently take).
- `README.md`'s build-order table lists step 2 as "Not started" and its
  Configuration table already documents `host`/`port` as "once the gRPC
  server exists" — both need updating.
- No `src/server.rs`, no `tests/grpc.rs` exist yet.

## Proposed Design

### `src/server.rs` — the `OrderbookAggregator` implementation

```rust
type BookSummaryStream = Pin<Box<dyn Stream<Item = Result<Summary, Status>> + Send>>;
```

The generated trait's associated type requires `Stream<Item = Result<Summary,
Status>> + Send + 'static`. `WatchStream::new(rx).map(...)` (mapping
`Option<Summary>` down to `Summary`, filtering `None` — see below) produces
a concrete type that embeds the closure passed to `.map()`, and a closure
has no nameable type — so the return type of the trait method cannot spell
it out directly. `Pin<Box<dyn Stream<...> + Send>>` erases that concrete
type behind a trait object, exactly the way `Box<dyn Trait>` erases any
other unnameable or per-call-site-varying type; `Pin` is required in
addition because `Stream::poll_next` needs a stable address to hold
internal state safely across polls (the same reason `Future`s returned by
`async fn` are pinned).

**Is this the same shape as C++'s `unique_ptr<IStream>`?** Close. The
allocation and vtable-cost analogy holds exactly: one heap allocation per
client connection, not per message, and each `poll_next` goes through a
vtable indirection like a virtual call through `unique_ptr<IStream>`. `Pin`
is the same hazard C++ has, not a hazard C++ lacks — move a self-referential
object in either language and an interior pointer dangles. What differs is
where the fix lives: C++ leaves it to the move constructor and trusts the
author to get it right; Rust encodes the guarantee in the type system so the
compiler rejects the move at compile time. `unique_ptr` doesn't solve this
problem so much as sidestep it — the pointee never moves once allocated, so
the question never arises. `Pin` has no extra runtime cost (no allocation or
indirection beyond the `Box` itself); the nearest C++ analogy is a type that
deletes its move constructor.

### The `None` case: filter it out

The watch channel carries `Option<Summary>` because nothing exists to
publish before the first fake tick lands (or, from step 3 on, before the
real aggregator produces its first merged book). Two options exist: publish
a default/empty `Summary` for `None`, or skip it and let the subscriber wait
for the first `Some`. **Decision: filter `None` out of the stream** — a
`Summary` with `spread == 0.0` and empty bid/ask vectors reads to a client as
"the market has zero spread," which is a specific, false claim about market
state, not an honest "no data yet." A client that connects before any value
has ever been published sees nothing until the first tick, which is what
"no data yet" should look like on a stream — not a fabricated zero. This is
implemented as `WatchStream::new(rx).filter_map(|opt| opt.map(Ok))` (or
equivalent), not a hand-rolled poll loop.

**This sets the precedent for step 5's all-venues-stale case.** Step 6's
staleness handling excludes a venue from the merge once its feed goes quiet;
if every venue is stale at once, there is nothing honest to merge — that
case is also a `None`, and by this same decision the client receives
nothing and waits, rather than being handed a fabricated empty book. Writing
this down now means step 5/6 doesn't re-argue it from scratch, or worse,
land a different answer by accident: a fabricated book is worse than no
book, for the same reason a fabricated zero spread is worse than a wait
here.

### `WatchStream`'s subscribe behavior — researched, not assumed

Checked against `docs.rs` for `tokio-stream` `0.1.19` (the version already
resolved in `Cargo.lock`): `WatchStream::new(rx)` **yields the current value
of the receiver on first poll**, regardless of whether that value is the
channel's initial value or one sent afterward — it does not wait for the
next change. (`WatchStream::from_changes(rx)` is the opposite — it
specifically waits for a change and is not what this step uses.) Combined
with the `None`-filtering decision above: a client that connects while the
watch already holds `Some(summary)` (i.e., after the first fake tick has
ever landed) gets that current summary on its very first poll, not up to a
1-second wait for the next tick. A client that connects before the first
tick has ever landed sees nothing until that first `Some` arrives — at most
~1 second, since the fake writer ticks every second. This resolves the
question the input brief raised explicitly, with a checked answer rather
than a guess.

### Fake data generator

A `tokio::spawn`ed task, `tokio::time::interval` at 1 second, builds a
`Summary` each tick: 10 `Level`s on each side, prices clustered around
`0.0315` (ETHBTC scale, matching the pair's default), amounts in a plausible
range, `spread` a small positive value consistent with best-bid < best-ask.
Every `Level.exchange` field is the literal string `"fake"` — never
`"binance"` — both so a human eyeballing `grpcurl` output can tell
placeholder from real data at a glance, and so a future test (step 3 or
later) can assert the literal `"fake"` never appears once the real feed is
wired in, catching a forgotten deletion of this task. The task sends into
the `watch::Sender<Option<Summary>>`'s sending half; it does not read it
back — mirroring the aggregator task's eventual role in step 3/5.

### `src/main.rs` — three supervised tasks

`main` spawns three tasks — the existing Binance feed loop (moved into a
`tokio::spawn`ed function, unchanged in behavior), the new fake-writer task,
and the new gRPC server (`tonic::transport::Server::serve` future) — each
producing a `JoinHandle`. It then `tokio::select!`s over all three handles
and exits (propagating whichever `Result`/panic ended the race) the moment
any one of them completes, rather than:

- **Awaiting them in sequence**, which would mean the second and third
  handles are never reached while the first is still running — a dead gRPC
  server behind a live feed would go unnoticed indefinitely.
- **Not awaiting them at all** (fire-and-forget `tokio::spawn` with the
  handle dropped), which detaches the tasks; if one panics, the runtime
  gives no message and `main` can return with exit 0 while nothing is
  actually running anymore.

The behavior wanted is: the process ends the instant any one task ends,
because a server that keeps answering `BookSummary` requests behind a feed
that silently died is worse than a server that isn't running — it
publishes stale prices under a "still live" appearance. This reasoning
belongs in a code comment at the `select!` site during implementation, not
only in this spec, so it isn't rediscovered later.

### `build.rs` and reflection

`build.rs` gains a file descriptor set output, via
`tonic_prost_build::configure().file_descriptor_set_path(...)` before
compiling the proto — confirmed against `docs.rs` for the resolved
`tonic-prost-build` `0.14.6`: `Builder::file_descriptor_set_path(self, path:
impl AsRef<Path>)` exists on the `Builder` returned by
`tonic_prost_build::configure()`. The path should live under `OUT_DIR` (via
`std::env::var("OUT_DIR")`), consistent with how the rest of the generated
code is emitted, and `src/server.rs` includes the resulting bytes via
`include_bytes!(concat!(env!("OUT_DIR"), "/<name>.bin"))`.

`tonic-reflection` needs adding to `Cargo.toml` as a new dependency —
version `0.14.6` is published on crates.io and depends on `tonic ^0.14.6`
and `tonic-prost ^0.14.6`, matching what's already pinned here (confirmed by
querying crates.io/docs.rs directly, not assumed from training data). Its
`tonic_reflection::server::Builder` (via `tonic_reflection::server::Builder::configure()`)
exposes `register_encoded_file_descriptor_set(&[u8])` and `build_v1()` —
confirmed against `docs.rs` for `0.14.6`. The server registers the resulting
reflection service alongside `OrderbookAggregatorServer` when building the
`tonic::transport::Server`.

**Documented limitation, not fixed here:** reflection exposes the entire
schema to anything that can reach the port. The README must note that a
public-facing production deployment should gate this behind a feature flag
or config toggle — this step ships it unconditionally, appropriate for a
take-home reviewer's convenience, not for a production posture.

### Docker — three changes, landing together

- `compose.yml` sets `ORDERBOOK_HOST=0.0.0.0` inside the container. Without
  this, the server binds the container's own loopback interface, and the
  published port refuses every connection while the container's own logs
  look completely healthy — `Config`'s existing doc comment on `host`
  already names this exact trap.
- `compose.yml` publishes `127.0.0.1:50051:50051` — host-loopback-only, not
  `0.0.0.0:50051:50051`, consistent with this project's stated posture of
  not exposing anything beyond the local machine.
- The container now stays running (the gRPC server future never resolves on
  its own), replacing today's "logs one line and exits 0" behavior. Every
  place that behavior is currently described in comments (`compose.yml`'s
  own header comment) and in `README.md`'s Quick Start section must be
  corrected as part of this step — leaving them as-is would make both
  actively wrong, not just stale, the moment this lands.

**The proxy pass-through must keep working — verify it's still wired, don't
re-add it.** `compose.yml` already forwards `PROXY_HOST`/`PROXY_PORT` from
`.env` into `HTTP_PROXY`/`HTTPS_PROXY` inside the container (landed in step
1's proxy revision). This step's `compose.yml` edits (host/port/staying-up)
must not remove or shadow that block — confirm it's still present and
correct after this step's changes, since a reviewer on a network that can't
reach Binance directly depends on it.

**A container that exits because the feed can't connect is expected
behavior, not a defect, and is worth calling out explicitly.** The
three-task `select!` design means the moment the feed task ends (e.g. it
can't reach Binance and no proxy is configured, or the proxy itself can't
reach Binance), the whole process exits — including inside the container.
That's the supervision design working exactly as specified: it is the first
practical demonstration that `select!` actually does what its own rationale
claims, not a Docker-specific bug. On a network where the container can't
reach Binance (directly or via the proxy env vars above), `docker compose
up` will start, then exit shortly after — this is the correct, intended
outcome of this step's own design, not something to "fix" by adding
resilience out of scope for step 2.

### `tests/grpc.rs`

Integration test, real server, real client, no mocking:

- Bind a `TcpListener` (or let `tonic::transport::Server` bind) on port `0`
  and read back the OS-assigned port — never a fixed port, since `cargo
  test` runs tests in a binary concurrently and a fixed port is a flake
  waiting to happen against another test in the same run (this is also
  already the standing project convention, recorded in
  `specs/002-binance-feed/revisions.md` entry 3's table: "gRPC stream |
  integration, `tests/grpc.rs` | server up, connect, receive").
- Start the server (with the fake writer feeding its watch channel) on a
  spawned task; connect a real `tonic` generated client to
  `http://127.0.0.1:{port}`.
- Call `BookSummary(Empty {})`, then take **exactly two** messages off the
  returned stream before asserting anything — this exact requirement is
  already recorded as a standing convention (same `revisions.md` entry 3,
  "The gRPC test, specifically (step 2)"): one message only proves the RPC
  call returned; two proves the response is actually being streamed, which
  is what the schema promises (`returns (stream Summary)`) and the one
  thing that could silently regress to a single-shot response.
- Assert on meaningful content from those two messages — bids has 10
  entries, asks has 10 entries, spread is a positive value — not merely
  "a message arrived."

For every test added here, the spec (and the eventual test's doc comment)
must name the bug it catches; if none is nameable, it is dropped rather than
added for coverage's own sake, per the project's standing testing
convention (`specs/002-binance-feed/revisions.md` entry 3).

### `README.md`

Updated as part of this step's own commit boundary, not a deferred pass —
this is itself a standing rule from `specs/003-step-1-fixes/revisions.md`
entry 1 ("a step is not done until its README sections match the code
actually merged"). For this step specifically:

- Build-order table: step 2 moves from "Not started" to "Done."
- A new section (or extension of an existing one) describing the server:
  it streams placeholder/fake data (`Level.exchange == "fake"`), not yet
  wired to the real feed.
- The `grpcurl -plaintext localhost:50051 list` one-liner, and a note that
  reflection means no local `.proto` file or import path is needed.
- The Docker section corrected: `docker compose up` now stays running and
  exposes `127.0.0.1:50051`; the "logs and exits 0" phrasing is removed.
- `src/server.rs` added to the Layout tree.
- A short note that reflection is unconditionally on in this build and
  should be gated behind a flag for a production-facing deployment (the
  documented limitation from Proposed Design).

## Acceptance Criteria

- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` all clean.
- `grpcurl -plaintext localhost:50051 list` shows
  `orderbook.OrderbookAggregator`, with no `.proto` file present locally.
- `grpcurl -plaintext localhost:50051 orderbook.OrderbookAggregator/BookSummary`
  streams fake `Summary` values (bids/asks length 10, `exchange: "fake"`).
- The Binance feed's book lines are still visible, scrolling in the same
  process's logs, alongside the server running.
- `docker compose up` brings the container up and keeps it running (does not
  exit); `grpcurl` from the host reaches `127.0.0.1:50051`.
- `git diff main --stat` shows `src/exchange/binance.rs` and `src/model.rs`
  untouched.
- `git log --graph` shows this spec packet's commit as the first commit on
  the branch, and a `--no-ff` merge commit at the end (not a fast-forward).

## Invariants and Critical Don'ts

- The spec packet is the first commit on `004-grpc-server`, before any
  implementation — `002-binance-feed` landed the spec after the fact; that
  was a stated mistake to correct, not a pattern to repeat.
- `src/exchange/binance.rs`, `src/model.rs`, `src/proxy.rs` — no edits.
- No merge logic, no Bitstamp, no reconnection/staleness handling — all
  later steps.
- The `Option<Summary>` watch's `None` case is filtered, never rendered as a
  default/zeroed `Summary`.
- `Level.exchange` in the fake writer is always the literal `"fake"`, never
  `"binance"` or any other exchange name.
- `main` uses `tokio::select!` over three `JoinHandle`s, not sequential
  `.await`s and not detached `tokio::spawn`s with dropped handles.
- `proto/orderbook.proto` is not edited — reflection is additive
  (`build.rs` + a new dependency + server registration), it does not change
  the wire schema.
- Reflection ships unconditionally this step; gating it is documented as a
  production limitation, not silently implemented as a half-measure (e.g. no
  partial feature-flag that isn't wired to anything).
- `compose.yml`'s published port stays `127.0.0.1:50051:50051` —
  host-loopback only, not `0.0.0.0`.
- Merge back to `main` with `git merge --no-ff`.

## Risks and Tradeoffs

- **Landing the watch channel and streaming plumbing one step early is a
  deliberate scope trade** — it makes this step slightly larger than "just
  the server" in exchange for a materially smaller, lower-risk step 3. If
  this bet is wrong (the plumbing turns out trivial and step 3 would have
  been fine doing it fresh), the cost is a small amount of code written one
  step earlier than strictly necessary, not a design mistake to unwind.
- **`tonic-reflection` version compatibility was verified against published
  crates.io/docs.rs metadata for `0.14.6`, not by actually resolving it in
  this project's `Cargo.lock` yet** — `cargo add tonic-reflection` during
  implementation could still surface a version-resolution conflict with the
  already-pinned `tonic`/`tonic-prost` `0.14.6` that wasn't visible from
  metadata alone. Low risk (its own declared dependency line already pins
  `tonic ^0.14.6`/`tonic-prost ^0.14.6`), but not proven until `cargo build`
  actually happens.
- **Unconditional reflection is a real, documented gap**, not a false
  negative — this step ships it without a gate, consciously, because this is
  a take-home reviewer-facing service, not a production deployment. The
  README's limitation note is the mitigation, not a code change.
- **No `HEALTHCHECK` is added in this step**, despite a server now
  existing — the input brief's acceptance list doesn't ask for one, and
  adding one is a scope expansion beyond what's specified; flagged here as a
  judgment call rather than silently added or silently skipped without
  mention.

## Testing Strategy

Required real verification:

- `tests/grpc.rs`: start the real `tonic` server (with its real service
  implementation, real fake-writer task, real watch channel) bound to an
  OS-assigned port; connect a real generated `tonic` client over a real
  TCP/HTTP2 connection; call `BookSummary`; take two messages off the
  returned stream. This is the truth anchor for "the schema's `stream`
  contract is actually honored," which a single-message assertion cannot
  prove (a server that silently downgraded to one-shot-then-close would
  still pass a one-message test).
- Content assertions on those two messages: `bids.len() == 10`,
  `asks.len() == 10`, `spread > 0.0` — catches a fake-generator regression
  that produces the wrong shape or a crossed/zero spread by accident, not
  just "did anything arrive."
- Manual `docker compose up` + `grpcurl` from the host — proves the
  `ORDERBOOK_HOST=0.0.0.0` / published-port / stays-up trio actually works
  together end to end; this is exactly the kind of three-piece
  interaction that unit or integration tests inside the crate cannot
  observe (it's a container networking property, not a Rust-level one).
- `grpcurl -plaintext localhost:50051 list` run manually — proves
  reflection is actually wired to the running server, not just that the
  crate compiles.

Optional supporting checks:

- None identified beyond the above — the input brief's own instruction
  ("for every test you add, tell me what bug it catches; if you can't name
  one, we drop it") is deliberately kept as the bar here rather than adding
  unit tests around the fake generator's number formatting, which has no
  behavior worth a dedicated test beyond what the integration test's content
  assertions already cover.

## Rollback Plan

This step only adds `src/server.rs`, `tests/grpc.rs`, and a
`file_descriptor_set_path` call plus a new `tonic-reflection`
`[dependencies]` line, and changes `src/main.rs` (task spawning/`select!`),
`Cargo.toml`, `build.rs`, `compose.yml`, and `README.md`. If acceptance
criteria fail after landing, `git revert` the commit(s) this step produces
restores step 1's single-task, no-server `main.rs` and the pre-reflection
`build.rs`/`Cargo.toml` exactly — no other module is touched.

## Open Questions

None blocking — the input brief is prescriptive on every design point in
this step (stream type shape, `None`-filtering decision required as a
stated design call, `WatchStream` subscribe behavior required as researched
fact, fake-data shape, three-task supervision, reflection wiring, Docker
changes, test shape). The two items the brief explicitly asked to have
*researched rather than assumed* are answered above with their sources
(`docs.rs` for the pinned `tokio-stream` `0.1.19` and `tonic-prost-build`/
`tonic-reflection` `0.14.6`) — not left open.

**Deliverable owed at the end of implementation, not by this spec:** the
input brief asks for a short list, produced after implementation finishes,
of anything worth recording as a design decision or discovery for the
requester's own external decision handbook — at minimum, `WatchStream`'s
actual subscribe behavior and the reflection API's exact shape (both
already researched above and reusable verbatim), plus anything else
discovered while implementing. This is a plan/tasks/implementation-phase
deliverable, not something `spec.md` itself produces, and should not be
dropped by whatever consumes this spec next.
