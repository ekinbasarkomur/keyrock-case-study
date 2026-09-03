# Tasks: 006-bitstamp

Five phases, matching `plan.md`'s phase and commit boundaries exactly. Phase 1
is Job A alone — the `BTreeMap` refactor, no behaviour change, its own commit,
fully independent of Bitstamp. Phases 2-4 are Job B, split by internal risk
per plan.md's Summary (generic loop proven against known-good Binance first,
then Bitstamp's parser proven pure and unwired, then the actual two-venue
wiring last). Phase 5 is the README pass plus the full-branch verification
gate. Work through them in order — each phase's verification gate must be
green before starting the next.

---

## Phase 1 (Job A): the venue map — `src/aggregator.rs`, `src/merge.rs`, `src/exchange/mod.rs`

**No Bitstamp code in this phase, no exceptions.** If any task below finds
itself referencing Bitstamp, stop — that content belongs in Phase 3 or 4.

### Task 1.1 — `Venue` gains `PartialOrd`/`Ord`

**Files:** `src/exchange/mod.rs`

**Change:**
- Change `#[derive(Clone, Copy, Debug, PartialEq, Eq)]` on `enum Venue` to
  `#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]`. No new
  variant — `Venue` still has exactly one variant, `Binance`. `Bitstamp` is
  Phase 3's addition, not this task's.
- No other change to this file in this task (the `Exchange` trait doesn't
  exist yet — that's Phase 2).

**Verification:**
- `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
  --check` — clean.
- Inspection: `grep -n "derive" src/exchange/mod.rs` shows `PartialOrd, Ord`
  present alongside the existing five derives, and `enum Venue` still has
  only one variant listed.

**Done when:** `Venue` is orderable and the crate still builds with no
behaviour anywhere depending on that yet.

---

### Task 1.2 — `Aggregator` becomes `BTreeMap<Venue, VenueState>`; `run`'s match becomes an insert

**Files:** `src/aggregator.rs`

**Change:**
- Add `use std::collections::BTreeMap;` to imports.
- Replace:
  ```rust
  struct Aggregator {
      binance: Option<VenueState>,
  }
  ```
  with:
  ```rust
  struct Aggregator {
      venues: BTreeMap<Venue, VenueState>,
  }
  ```
- In `run`, replace `let mut aggregator = Aggregator { binance: None };` with
  `let mut aggregator = Aggregator { venues: BTreeMap::new() };`.
- Replace the current body:
  ```rust
  match venue {
      Venue::Binance => {
          aggregator.binance = Some(VenueState {
              book,
              last_update: Instant::now(),
          });
      }
  }
  ```
  with a single generic insert — no `match` on `venue` anywhere in this
  function:
  ```rust
  aggregator.venues.insert(
      venue,
      VenueState {
          book,
          last_update: Instant::now(),
      },
  );
  ```
- Replace the `summarise` call site:
  ```rust
  let summary = merge::summarise(venue, aggregator.binance.as_ref().map(|s| &s.book));
  ```
  with the exact two-line shape spec.md and plan.md specify — build the
  borrowed map, then call `summarise` with it:
  ```rust
  let venues: BTreeMap<Venue, &Book> =
      aggregator.venues.iter().map(|(v, s)| (*v, &s.book)).collect();
  let summary = merge::summarise(&venues);
  ```
- Update the doc comment on `struct Aggregator` (currently "`Option` because
  there's nothing to publish before the first message from a venue arrives")
  to describe the map instead — an empty map is the "nothing yet" state now,
  not a `None` field.
- Remove the stale doc comment on the deleted `match venue { ... }` block
  that says "A real match, not a single-arm shortcut: adding Bitstamp in
  step 4 makes this fail to compile until a new arm updates its own slot" —
  that claim is no longer true after this task; the whole point of the map
  is that adding Bitstamp needs no new arm here.

**Verification:**
- `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
  --check` — clean (this task alone will not compile until Task 1.3 lands
  `summarise`'s new signature — implement 1.2 and 1.3 together before
  running `cargo build`, matching `005-aggregator` Phase 3's "these files
  don't compile independently" precedent).
- Inspection (after 1.3 lands too): `grep -n "match venue" src/aggregator.rs`
  returns nothing — a leftover match would mean the map refactor didn't
  remove per-venue branching, defeating Job A's stated purpose per plan.md's
  Cross-Cutting section.

**Done when:** `Aggregator` holds a `BTreeMap<Venue, VenueState>`, the update
path is a single generic `.insert(...)` with no `match` on `venue`, and the
borrowed-map construction matches spec.md's exact line.

---

### Task 1.3 — `summarise`'s signature becomes `&BTreeMap<Venue, &Book>`

**Files:** `src/merge.rs`

**Change:**
- Add `use std::collections::BTreeMap;` to imports.
- Change the signature:
  ```rust
  pub fn summarise(venue: Venue, book: Option<&Book>) -> Option<Summary>
  ```
  to:
  ```rust
  pub fn summarise(venues: &BTreeMap<Venue, &Book>) -> Option<Summary>
  ```
- Replace the current `let book = book?;` with the exact line spec.md and
  plan.md specify:
  ```rust
  let (&venue, &book) = venues.iter().next()?;
  ```
- Everything downstream (`best_bid`/`best_ask`/`spread`/`to_level`'s
  `exchange: venue.to_string()`) is unchanged in logic — it now reads
  `venue` and `book` from this destructured binding instead of from
  function parameters. **No branch, no comment framing this as "ignoring
  extra entries" or "defensively guarding against a caller mistake"** — per
  spec.md's explicit resolution, this is plain first-entry iteration and
  nothing else, because step 5 replaces this selection outright. Update the
  function's doc comment only to the extent it currently claims something
  no longer true (e.g. "single venue's book" framing) — do not add new
  prose speculating about multi-venue behaviour beyond what spec.md already
  settled.
- Update the five existing tests' setup lines only — **no `assert_eq!`/
  `assert!` value or condition may change, anywhere in this file.** This is
  the phase's actual acceptance gate, not "tests still pass":
  - `summarise_on_a_twenty_level_book_returns_ten_bids_ten_asks_and_correct_spread`:
    change the call from `summarise(Venue::Binance, Some(&book))` to
    `summarise(&BTreeMap::from([(Venue::Binance, &book)]))`.
  - `summarise_returns_bids_descending_by_price_and_asks_ascending`: same
    call-site change.
  - `summarise_on_a_six_level_book_returns_six_levels_per_side_not_padded_to_ten`:
    same call-site change.
  - `summarise_with_no_book_returns_none`: change
    `summarise(Venue::Binance, None)` to `summarise(&BTreeMap::new())`; the
    assertion (`assert_eq!(..., None)`) stays byte-for-byte identical.
  - `summarise_on_a_one_sided_book_returns_none`: same call-site change as
    the twenty-level test.

**Verification:**
- `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
  --check` — clean (paired with Task 1.2, per that task's note).
- `cargo test` — all five tests above pass. Before running, diff this file's
  test bodies against what was read at the start of this packet (quoted in
  the prompt that produced this tasks.md) to confirm every `assert_eq!`/
  `assert!` line is byte-for-byte unchanged — only the `summarise(...)` call
  expressions inside each test changed.
- `git diff main --stat -- src/merge.rs` — record the output now as this
  phase's baseline (quote it in the implementation report). Per plan.md's
  Phase 1 verification, it should show a small diff confined to the function
  signature, the one new `let (&venue, &book) = ...` line, and the five
  tests' setup/call-site lines — nothing else. This is what Phase 5's final
  gate re-confirms is unchanged.

**Done when:** `summarise` takes `&BTreeMap<Venue, &Book>`, all five tests
pass with assertions identical to before this task, and the recorded
`git diff main --stat -- src/merge.rs` baseline is captured for Phase 5.

**Commit boundary for Phase 1:** `src/aggregator.rs`, `src/merge.rs`,
`src/exchange/mod.rs`. This is Job A's commit, standalone. Reverting it
restores the single `Option<VenueState>` field and the two-argument
`summarise`, with nothing yet depending on the map shape.

---

## Phase 2 (Job B, part 1): the `Exchange` trait, Binance wrapped, `src/feed.rs` — Binance-only regression

### Task 2.1 — Define the `Exchange` trait in `src/exchange/mod.rs`

**Files:** `src/exchange/mod.rs`

**Change:**
- Add the trait exactly as spec.md and plan.md specify, synchronous (the
  trait describes protocol data, not control flow — an async `connect`
  would give step 6's reconnection two places to land instead of one):
  ```rust
  pub trait Exchange {
      fn venue(&self) -> Venue;
      fn connect_url(&self, pair: &str) -> String;
      fn subscribe_message(&self, pair: &str) -> Option<String>;
      fn parse(&self, raw: &str) -> Option<Book>;
  }
  ```
- Add `use crate::model::Book;` to this file's imports.
- No `dyn` anywhere — the venue set is compile-time-known; every call site
  this step and Phase 4 use a concrete generic `E: Exchange`, never a trait
  object.

**Verification:**
- `cargo build` will fail at this point (nothing implements `Exchange` yet)
  — that's expected; this task alone is not independently buildable. Verify
  syntactically instead: `cargo check --lib 2>&1 | grep -c "error\[E0"` to
  confirm the only errors are "no implementations found" style, not a
  syntax error in the trait definition itself. Full verification happens at
  the end of Task 2.3, once `Binance` implements it.

**Done when:** the trait's four methods match spec.md's signatures exactly,
and the only remaining build errors are "nothing implements this yet."

---

### Task 2.2 — Wrap Binance into `pub struct Binance;` implementing `Exchange`

**Files:** `src/exchange/binance.rs`

**Change:**
- Add `use crate::exchange::{Exchange, Venue};` (adjust path/import style to
  match this crate's existing convention).
- Add `pub struct Binance;` and:
  ```rust
  impl Exchange for Binance {
      fn venue(&self) -> Venue {
          Venue::Binance
      }

      fn connect_url(&self, pair: &str) -> String {
          // existing connect_url(pair) body, unchanged
      }

      fn subscribe_message(&self, _pair: &str) -> Option<String> {
          None
      }

      fn parse(&self, raw: &str) -> Option<Book> {
          // existing parse(text) body, unchanged
      }
  }
  ```
- The free functions `connect_url(pair: &str) -> String` and
  `parse(text: &str) -> Option<Book>` — their *logic* moves into the trait
  methods unchanged; this is a reshape of Binance's public shape, not a
  behaviour change (mirrors Job A's own "call sites may change, assertions
  may not" discipline, per plan.md's explicit parallel). Decide during
  implementation whether the free functions are deleted outright (preferred
  — nothing should call them once `Binance::parse`/`Binance::connect_url`
  exist) or kept as thin wrappers calling the trait method; prefer deletion
  unless something not covered by this plan still needs the free-function
  form.
- `HOST`/`PORT` constants stay exactly as they are — still used inside
  `connect_url` to build the URL, and still referenced by `src/feed.rs`'s
  proxy-target parsing (see Task 2.3's note on why that parsing exists at
  all rather than reusing these constants directly).
- Update the three existing tests' call sites from the free-function form
  (`parse(DEPTH20_FIXTURE)`, `parse(r#"..."#)`) to the trait-method form
  (`Binance.parse(DEPTH20_FIXTURE)` or `Binance {}.parse(...)`, whichever
  compiles — `Binance` is a unit struct so `Binance.parse(...)` should work
  directly). **No assertion changes** — same discipline as Phase 1.
- Add the one new test spec.md's trait-behaviour list calls for, filed here
  per this project's unit-test-by-access convention (nothing beyond what's
  already `pub` in this file is needed to write it):
  ```rust
  #[test]
  fn binance_subscribe_message_is_none() {
      assert_eq!(Binance.subscribe_message("ethbtc"), None);
  }
  ```
  Bug this catches: a future edit accidentally returning `Some(...)` for
  Binance, which would double-subscribe against a stream whose subscription
  is already baked into the URL.

**Verification:**
- `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
  --check` — clean (paired with Task 2.1; the trait and its one
  implementation compile together).
- `cargo test` — the three existing Binance parser tests pass with
  assertions unchanged (only call-site syntax differs):
  `parses_depth20_into_twenty_bids_and_twenty_asks_with_correct_values`,
  `server_shutdown_message_parses_to_none_without_panicking`,
  `malformed_json_parses_to_none_without_panicking` — plus the new
  `binance_subscribe_message_is_none`.

**Done when:** `Binance` is a full `Exchange` implementation, all four tests
in this file pass, and no free-standing `connect_url`/`parse` functions
remain referenced from outside this file (check via `cargo build` — an
unused free function would be a clippy `dead_code` warning, which fails the
`-D warnings` gate).

---

### Task 2.3 — `src/feed.rs` (new): the generic `run_feed<E>` driver loop

**Files:** `src/feed.rs` (new), `src/lib.rs`

**Change:**
- `src/lib.rs`: add `pub mod feed;` alongside the existing `pub mod`
  declarations, in this file's established alphabetical order (after
  `pub mod exchange;`, before `pub mod merge;`).
- `src/feed.rs`: absorb, unchanged in behaviour, everything
  `src/main.rs::run_feed` currently does by hand for Binance alone:
  ```rust
  pub async fn run_feed<E: Exchange>(
      exchange: E,
      pair: String,
      tx: mpsc::Sender<(Venue, Book)>,
  ) -> Result<()> {
      let url = exchange.connect_url(&pair);
      let (mut ws, _response) = match proxy_addr() {
          Some((proxy_host, proxy_port)) => {
              let (target_host, target_port) = parse_connect_target(&url);
              let tunnel = proxy::connect_through_proxy(
                  &proxy_host,
                  proxy_port,
                  &target_host,
                  target_port,
              )
              .await
              .with_context(|| format!("failed to establish CONNECT tunnel to {target_host}:{target_port} through proxy"))?;
              client_async_tls(&url, tunnel)
                  .await
                  .with_context(|| format!("failed to connect to {} at {url} via proxy", exchange.venue()))?
          }
          None => connect_async(&url)
              .await
              .with_context(|| format!("failed to connect to {} at {url}", exchange.venue()))?,
      };

      if let Some(msg) = exchange.subscribe_message(&pair) {
          ws.send(Message::Text(msg.into())).await?;
      }

      while let Some(message) = ws.next().await {
          let message = message.context("websocket read failed")?;
          match message {
              Message::Text(text) => {
                  if let Some(book) = exchange.parse(&text) {
                      // log line, interpolating exchange.venue() instead of
                      // the hardcoded "binance" the current main.rs uses
                      let _ = tx.send((exchange.venue(), book)).await;
                  }
              }
              Message::Ping(_) | Message::Pong(_) => {}
              Message::Close(_) => break,
              Message::Binary(_) => {}
              Message::Frame(_) => {}
          }
      }

      Ok(())
  }
  ```
  Reuse the existing doc-comment content from `src/main.rs::run_feed` and
  its `Message` match arms' existing comments (the "no `.split()`", "single
  `next()` loop", "bounded `.send(...).await` naturally backpressures"
  reasoning) — move the comments along with the code, don't rewrite the
  reasoning from scratch.
- **The proxy `CONNECT` target's host/port**, per plan.md's "Plan Review
  Notes" (this is a resolved gap, not open — implement exactly this):
  add a private helper in this file,
  ```rust
  /// Parses `(target_host, target_port)` out of a `connect_url()`-shaped
  /// string (`wss://host[:port]/path`) for the proxy CONNECT tunnel. The
  /// `Exchange` trait only exposes the full URL, not host/port separately —
  /// see specs/006-bitstamp/plan.md, "Plan Review Notes" for why this lives
  /// here instead of on the trait.
  fn parse_connect_target(url: &str) -> (String, u16) {
      let without_scheme = url.strip_prefix("wss://").unwrap_or(url);
      let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
      match authority.rsplit_once(':') {
          Some((host, port)) => (
              host.to_string(),
              port.parse().unwrap_or(443),
          ),
          None => (authority.to_string(), 443),
      }
  }
  ```
  Size this as its own small function, not folded into the middle of the
  connect logic, per plan.md's explicit instruction. Default port is `443`
  (the `wss` scheme's standard port) when there's no explicit `:port` in the
  authority — this is what Bitstamp's `wss://ws.bitstamp.net` (no port in
  the URL) relies on once Phase 4 wires it in.
- `proxy_addr()` (the `HTTPS_PROXY`/`HTTP_PROXY` env-var reader) moves from
  `src/main.rs` into this file as a private function, unchanged in body —
  `run_feed` is now its only caller.
- Add whatever imports this file needs (`anyhow::{Context, Result}`,
  `futures_util::{SinkExt, StreamExt}` — note `SinkExt` is newly needed here
  for `.send(...)` on the subscribe message, which the current
  `src/main.rs` doesn't need since Binance never sends anything post-connect,
  `rust_crypto_orderbook::exchange::{Exchange, Venue}`, `rust_crypto_orderbook::model::Book`,
  `rust_crypto_orderbook::proxy::{self, parse_proxy_addr}`, `tokio::sync::mpsc`,
  `tokio_tungstenite::tungstenite::Message`, `tokio_tungstenite::{client_async_tls, connect_async}`,
  `tracing::{debug, info, warn}`).

**Verification:** folded into Task 2.4, since `src/feed.rs` alone does not
compile without `src/main.rs`'s updated call site.

---

### Task 2.4 — `src/main.rs`: delete the hand-written `run_feed`, spawn `feed::run_feed::<Binance>` alone

**Files:** `src/main.rs`

**Change:**
- Delete the entire `run_feed` function and the `proxy_addr` function from
  this file — both now live in `src/feed.rs`.
- Add `use rust_crypto_orderbook::exchange::binance::Binance;` (or the
  equivalent path once Task 2.2 lands `Binance` as a struct) and
  `use rust_crypto_orderbook::feed;`.
- Remove now-unused imports this file no longer needs directly (`futures_util::StreamExt`,
  `tokio_tungstenite::tungstenite::Message`, `tokio_tungstenite::{client_async_tls, connect_async}`,
  `rust_crypto_orderbook::proxy::{self, parse_proxy_addr}`, `tracing::{debug, warn}`
  if `warn`/`debug` are no longer used elsewhere in this file) — confirm via
  `cargo clippy --all-targets -- -D warnings`, which will flag any leftover
  unused import as an error under this project's warnings-are-errors gate.
- Change the feed spawn from:
  ```rust
  let feed_handle = tokio::spawn(async move { run_feed(pair, feed_tx).await });
  ```
  to:
  ```rust
  let feed_handle = tokio::spawn(feed::run_feed(Binance, pair, feed_tx));
  ```
  **Still single-venue this phase** — Bitstamp isn't spawned until Phase 4;
  this is a structural/behavioural no-op from the outside.
- Update this file's module-level doc comment (currently describes "the
  Binance feed loop... moved into `run_feed` so it can be spawned") to
  describe the generic `feed::run_feed::<E>` this file now calls into,
  rather than owning the loop itself.

**Verification:**
- `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
  --check` — clean (Tasks 2.3 and 2.4 verified together — the crate only
  compiles once both land).
- `cargo test` — the four tests in `src/exchange/binance.rs` (three existing
  + `binance_subscribe_message_is_none`) pass.
- `cargo run -- --pair ethbtc --port 50051`, then in a second terminal:
  `grpcurl -plaintext 127.0.0.1:50051 orderbook.OrderbookAggregator/BookSummary`
  (network permitting — report honestly if this environment can't reach
  Binance, per the project's standing honesty requirement, same as prior
  steps' plans). Confirm the stream still carries real Binance data with
  `Level.exchange == "binance"` — this proves the generic loop reproduces
  pre-refactor behaviour exactly, the whole point of isolating this phase
  before Bitstamp exists.
- Inspection: `git diff main --stat -- src/aggregator.rs` shows no diff
  beyond Phase 1's — this phase touches the feed side only.

**Done when:** the crate builds, Binance's four tests pass unchanged, and a
live `grpcurl` session against `run_feed::<Binance>` looks identical to what
`005-aggregator` shipped.

**Commit boundary for Phase 2:** `src/exchange/mod.rs`, `src/exchange/binance.rs`,
`src/feed.rs`, `src/main.rs`, `src/lib.rs`. Reverting this phase (with Phase 1
still in place) restores the hand-written single-venue `run_feed` in
`main.rs`, on top of an already-landed, independently-verified `BTreeMap`
aggregator.

---

## Phase 3 (Job B, part 2): `src/exchange/bitstamp.rs` — pure, unwired

**Nothing in `src/main.rs` changes in this phase.** If any task below finds
itself editing `main.rs`, stop — that's Phase 4's work.

### Task 3.1 — `Venue` gains a `Bitstamp` variant

**Files:** `src/exchange/mod.rs`

**Change:**
- Add `Bitstamp` to `enum Venue`, appended after `Binance` — declaration
  order matters: `Binance` must stay first, since Phase 1's `BTreeMap`
  ordering guarantee (and this step's "first entry" `summarise` selection)
  depends on it:
  ```rust
  pub enum Venue {
      Binance,
      Bitstamp,
  }
  ```
- Add the matching `Display` arm:
  ```rust
  Venue::Bitstamp => write!(f, "bitstamp"),
  ```
- Add `pub mod bitstamp;` alongside the existing `pub mod binance;`.

**Verification:**
- `cargo build` will fail until Task 3.2 lands `mod bitstamp` with at least
  a stub — implement 3.1 and 3.2 together, same pairing discipline as
  Phases 1 and 2's paired tasks.
- Once paired: `cargo build`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` — clean.

**Done when:** `Venue::Bitstamp.to_string() == "bitstamp"`, declared after
`Binance` in the enum.

---

### Task 3.2 — Capture a real Bitstamp `"data"` fixture (hard constraint, do this before writing the parser's test)

**Files:** none committed by this task alone — a throwaway debug artifact
only, fully reverted before Task 3.3's test file is written.

**Change — literal instruction, not a suggestion:**
- Attempt a real captured `"data"` message from `wss://ws.bitstamp.net`
  before writing any test that asserts on Bitstamp payload shape. Use a
  short-lived, throwaway mechanism — e.g. a temporary `#[tokio::test]` that
  connects, sends `{"event":"bts:subscribe","data":{"channel":"order_book_ethbtc"}}`,
  prints the first `"data"`-typed message to stdout, and is then deleted —
  or a scratch binary run via `cargo run --example` outside the crate's
  normal source tree. This mirrors the precedent `002-binance-feed` used to
  capture its own Binance fixture.
- If reachable: trim the captured payload if unwieldy (keep it valid JSON —
  a handful of bid/ask levels is enough, does not need to be the full 100),
  and prepare a `// Captured from wss://ws.bitstamp.net on <actual date>`
  comment for Task 3.3's fixture constant, mirroring `binance.rs`'s existing
  fixture comment style exactly.
- **Fully revert any temporary debug code used to capture it** — the
  throwaway test/binary/print statements must not survive into the commit.
  Only the trimmed JSON payload and its provenance comment carry forward
  into `src/exchange/bitstamp.rs`.
- **If Bitstamp is genuinely unreachable from the implementation environment**
  (both a live debug connection attempt and any reasonable retry fail) —
  do not fabricate a "plausible-looking" payload under any circumstance.
  Instead, leave a `TODO` comment naming the user (e.g.
  `// TODO(user): capture a real Bitstamp "data" fixture from
  wss://ws.bitstamp.net — unreachable from this implementation environment
  as of <date>; see specs/006-bitstamp/plan.md "Expected Drift Triggers"`)
  in place of the fixture constant, and correspondingly skip (do not write,
  do not fake) the one test in Task 3.3's list that depends on it (the
  real-fixture parse test). The other five tests in Task 3.3's list do not
  depend on a real fixture and must still be written regardless of this
  outcome.
- Report the outcome of this task explicitly and honestly in the
  implementation report — "captured on \<date\>, provenance comment reads
  \<X\>" or "unreachable, TODO left naming the user, one test skipped" — not
  silently folded into "Phase 3 done."

**Verification:**
- Inspection: either a real fixture with a genuine date/URL comment exists
  in the next task's file, or a `TODO` naming the user exists in its place —
  never neither, never a fabricated payload passed off as real.

**Done when:** the outcome (captured or genuinely blocked) is known and
reported before Task 3.3 is implemented.

---

### Task 3.3 — `src/exchange/bitstamp.rs` (new): `Bitstamp` struct, envelope parsing, six tests

**Files:** `src/exchange/bitstamp.rs` (new)

**Change:**
- `pub struct Bitstamp;` implementing `Exchange`:
  ```rust
  impl Exchange for Bitstamp {
      fn venue(&self) -> Venue {
          Venue::Bitstamp
      }

      fn connect_url(&self, _pair: &str) -> String {
          "wss://ws.bitstamp.net".to_string()
      }

      fn subscribe_message(&self, pair: &str) -> Option<String> {
          Some(format!(
              r#"{{"event":"bts:subscribe","data":{{"channel":"order_book_{}"}}}}"#,
              pair.to_lowercase()
          ))
      }

      fn parse(&self, raw: &str) -> Option<Book> {
          // see below
      }
  }
  ```
  `connect_url` ignores `pair` — nothing in the path per spec.md (the pair
  only shows up in the subscribe channel name). Symbol casing/formatting
  stays local to this impl, not centralised into a shared helper, per
  spec.md's explicit reasoning.
- `parse`: deserialise the wrapped envelope using the same `#[serde(borrow)]`
  pattern `Depth20<'a>` uses in `binance.rs` — an envelope struct with an
  `event: &'a str` field and (for the `"data"` case) a nested `Data<'a>`
  struct borrowing `bids`/`asks` as `Vec<[&'a str; 2]>`:
  ```rust
  #[derive(Deserialize)]
  struct Envelope<'a> {
      event: &'a str,
      #[serde(borrow, default)]
      data: Option<Data<'a>>,
  }

  #[derive(Deserialize)]
  struct Data<'a> {
      #[serde(borrow)]
      bids: Vec<[&'a str; 2]>,
      #[serde(borrow)]
      asks: Vec<[&'a str; 2]>,
  }
  ```
  (adjust the exact `data` field typing/attributes as needed once the real
  captured fixture's actual shape is in hand from Task 3.2 — if the real
  shape disagrees with this sketch in a way that isn't just field-typing
  mechanics, that's plan.md's "Expected Drift Trigger" about the envelope
  shape not matching what spec.md described — stop and flag it, don't
  silently reshape past what spec.md documented). Reuse `Price::parse`/
  `Amount::parse` exactly as `binance.rs`'s `parse_levels` does — factor out
  a shared level-parsing helper only if it doesn't fight the borrow checker
  or add indirection this file doesn't already have a reason for; a direct
  copy of `binance.rs`'s `parse_levels` shape (same signature, same body) is
  an acceptable, simple choice here.
  Branch on `event`:
  - `"data"` → parse `data.bids`/`data.asks` into a `Book` (no
    `last_update_id` field on Bitstamp's payload — use `0` or whatever
    placeholder the real captured fixture's timestamp field suggests is
    more honest; document the choice in a one-line comment since `Book`'s
    field is currently framed around Binance's `lastUpdateId` semantics).
  - `"bts:subscription_succeeded"` → `None`, `tracing::info!(...)`.
  - `"bts:request_reconnect"` → `None`, `tracing::info!(...)`, plus a
    comment noting step 6 owns turning this into an actual reconnect
    trigger — this step only logs it, never triggers a reconnect.
  - `"bts:error"` → `None`, `tracing::warn!(...)` — **not** `info!`, this is
    the one event that means something is actually wrong.
  - Any other event, or malformed JSON that fails to deserialise at all →
    `None`, never a panic or `Err` — same "never kill the read loop on a
    stray control message" discipline `binance.rs::parse` already
    documents.
- Tests, all pure, co-located `#[cfg(test)] mod tests` in this file:
  - `bitstamp_data_message_parses_to_the_right_levels_and_prices` (or
    equivalent behaviour-sentence name) — uses the real fixture captured in
    Task 3.2, with its provenance comment. **Skip this test, replaced by
    the `TODO` from Task 3.2, if capture was genuinely unreachable** — do
    not write it against fabricated data under any circumstance.
  - `bts_subscription_succeeded_parses_to_none_without_panicking`
  - `bts_request_reconnect_parses_to_none_without_panicking`
  - `bts_error_parses_to_none_without_panicking`
  - `malformed_json_parses_to_none_without_panicking`
  - `bitstamp_subscribe_message_contains_the_configured_pairs_channel_name`
    — asserts the returned string contains `"order_book_ethbtc"` for pair
    `"ethbtc"`. Bug caught: a wrong channel name is a silent failure —
    Bitstamp accepts the subscription and then sends nothing, indistinguishable
    from "no messages yet" from the outside.
  No test beyond this list of six (five if the fixture test is replaced by
  a `TODO`) — per this project's testing convention, don't pad coverage.

**Verification:**
- `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
  --check` — clean.
- `cargo test` — all tests in this file pass (six, or five plus a reported
  `TODO` if fixture capture failed), each reported individually by name, not
  folded into an aggregate count.
- Inspection: the fixture's capture-provenance comment is present and
  honest, or the `TODO` naming the user is present in its place — reported
  explicitly as a phase outcome per Task 3.2.
- `git diff main --stat -- src/aggregator.rs src/merge.rs` — zero diff. This
  phase adds a new file and touches only `src/exchange/mod.rs` (the new
  `Venue` variant); nothing about parsing a second venue should ripple into
  the aggregator or merge logic.

**Done when:** `Bitstamp` is a fully-tested `Exchange` implementation that
nothing in `main.rs` references yet.

**Commit boundary for Phase 3:** `src/exchange/bitstamp.rs`,
`src/exchange/mod.rs`. Reverting this phase (with Phases 1-2 still in place)
leaves Binance running through the generic loop alone, exactly Phase 2's end
state.

---

## Phase 4 (Job B, part 3): wire Bitstamp into `src/main.rs`

### Task 4.1 — Spawn `feed::run_feed::<Bitstamp>` alongside Binance, fourth `select!` arm

**Files:** `src/main.rs`

**Change:**
- Add `use rust_crypto_orderbook::exchange::bitstamp::Bitstamp;`.
- Clone `feed_tx` once more (one `mpsc::Sender`, cloned for two producers):
  ```rust
  let binance_tx = feed_tx.clone();
  let bitstamp_tx = feed_tx;
  ```
  (adjust naming to whatever reads clearest against the existing
  `feed_handle` naming convention — the point is two distinct `Sender`
  clones feeding the one aggregator `Receiver`, not two channels).
- Add a second spawn alongside the existing Binance one:
  ```rust
  let binance_handle = tokio::spawn(feed::run_feed(Binance, pair.clone(), binance_tx));
  let bitstamp_handle = tokio::spawn(feed::run_feed(Bitstamp, pair, bitstamp_tx));
  ```
  (rename the existing `feed_handle` to `binance_handle` for symmetry, and
  note `pair` needs `.clone()` on at least one of the two spawns since both
  tasks need an owned `String`).
- Extend the `select!` block with a fourth arm for `bitstamp_handle`,
  following the exact same match-on-`Ok`/`Err`/panic shape the
  `binance_handle`/`feed_handle` arm already uses — **not** a restructuring
  of the existing three-arm supervision pattern, just one more task under
  the same discipline (any one task ending ends the whole process):
  ```rust
  res = bitstamp_handle => match res {
      Ok(Ok(())) => {
          info!("bitstamp feed task ended");
          Ok(())
      }
      Ok(Err(e)) => Err(e).context("bitstamp feed task failed"),
      Err(e) => Err(e).context("bitstamp feed task panicked"),
  },
  ```
  and update the existing Binance arm's log line/context strings similarly
  (`"binance feed task ended"` / `"binance feed task failed"` / `"binance
  feed task panicked"`) so the two arms are distinguishable in logs.

**Verification:**
- `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
  --check` — clean.
- `cargo run -- --pair ethbtc --port 50051` — confirm both venues' book
  lines are visible scrolling in the logs. Actually observe and report this
  (this environment's Binance reachability depends on the proxy per prior
  steps; Bitstamp is expected to be directly reachable per spec.md's
  confirmed manual `CONNECT` test, but this must be actually observed here,
  not assumed from spec.md's claim — if either venue can't be reached in
  this environment, say so plainly).
- `grpcurl -plaintext 127.0.0.1:50051 orderbook.OrderbookAggregator/BookSummary`
  — still streams, single-venue output (`exchange == "binance"`, since
  `summarise` still reads the map's lowest-ordered entry — real merging is
  step 5). This is a regression check: two live feeds now write into the
  same aggregator, and the gRPC surface must look unchanged to a client from
  before this step.
- `docker compose up --build` — confirm both feed tasks connect; report
  actual observed output, including any environment limitation.
- Inspection: `git diff main --stat -- src/merge.rs` — unchanged from
  Phase 1's recorded baseline (Task 1.3's verification step); `git diff
  main --stat -- src/aggregator.rs` — unchanged from Phase 1's diff.

**Done when:** killing either feed still ends the whole process via the same
four-arm `select!`; both venues' parsed books are visibly reaching the
aggregator (or the environment limitation is reported plainly); the gRPC
contract is unchanged from a client's perspective.

**Commit boundary for Phase 4:** `src/main.rs` alone (confirm no further
changes are needed to `src/exchange/mod.rs` — Phase 3 already added
`pub mod bitstamp;`). Reverting this phase (with Phases 1-3 in place) leaves
Bitstamp fully built and tested but not spawned — a safe, buildable
intermediate state.

---

## Phase 5: `README.md` + full-branch verification gate

### Task 5.1 — Update README to describe what Phases 1-4 shipped

**Files:** `README.md`

**Change:**
- Build-order table: move step 4's row to "Done" (matching the existing
  status-column convention).
- Add a short note that the aggregator now holds a
  `BTreeMap<Venue, VenueState>` rather than a single named field, and why
  (fixes `merge`'s/`summarise`'s signature ahead of further venues, per
  spec.md's own framing) — conclusion plus one sentence why, not a full
  derivation, per this repo's spec-length convention.
- Add the `Exchange` trait, `src/feed.rs`'s generic `run_feed<E>`, and
  `src/exchange/bitstamp.rs` to the Layout/file-tree section, each with a
  one-line description of its actual role.
- Add an explicit statement that gRPC output is still single-venue this step
  (real merging is step 5) so a reader doesn't mistake "two feeds running"
  for "two venues in the published book."
- Reference the Bitstamp fixture's capture provenance (or the `TODO`, if
  capture failed in every environment tried) so a reader can see the same
  honesty this plan requires of the implementation itself.
- Read through the whole file for any stale "step 4: not yet implemented"
  language or leftover reference to a single `binance` field on
  `Aggregator`, and correct it.

**Verification:**
- Manually run every command the README's Quick Start / gRPC sections show
  and confirm actual output matches what's documented, to the extent this
  environment allows.
- Read-through: `grep -n "not yet implemented" README.md` shows no
  reference to step 4 remaining; `grep -n "binance:" README.md` (or
  equivalent) shows no stale single-field `Aggregator` description.

**Done when:** every claim in the README matches what was actually shipped
and actually verified in this environment.

---

### Task 5.2 — Full-branch verification gate

**Files:** none (verification only, run at the tip of the branch after all
five phases)

**Verification, run and quote actual output for each:**
- `cargo build` — clean.
- `cargo test` — clean; explicitly confirm at least these behaviour-named
  tests are present and passing, each reported individually, not folded
  into an aggregate count:
  - `summarise_on_a_twenty_level_book_returns_ten_bids_ten_asks_and_correct_spread`
  - `summarise_returns_bids_descending_by_price_and_asks_ascending`
  - `summarise_on_a_six_level_book_returns_six_levels_per_side_not_padded_to_ten`
  - `summarise_with_no_book_returns_none`
  - `summarise_on_a_one_sided_book_returns_none`
    (all five with assertions byte-for-byte unchanged from before Phase 1,
    per Task 1.3's recorded diff)
  - `parses_depth20_into_twenty_bids_and_twenty_asks_with_correct_values`
  - `server_shutdown_message_parses_to_none_without_panicking`
  - `malformed_json_parses_to_none_without_panicking`
  - `binance_subscribe_message_is_none`
  - the Bitstamp `"data"` fixture parse test (or its `TODO` in place,
    reported honestly)
  - `bts_subscription_succeeded_parses_to_none_without_panicking`
  - `bts_request_reconnect_parses_to_none_without_panicking`
  - `bts_error_parses_to_none_without_panicking`
  - `malformed_json_parses_to_none_without_panicking` (Bitstamp's own,
    distinctly named from Binance's if both exist as the same literal
    name in different files — `cargo test`'s output disambiguates by
    module path)
  - `bitstamp_subscribe_message_contains_the_configured_pairs_channel_name`
  - existing `tests/grpc.rs` and `tests/cli.rs` tests, unaffected by this
    step's scope
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo fmt --check` — clean.
- Both venues' book lines observed scrolling in the logs from a live
  `cargo run -- --pair ethbtc --port 50051`, or an honest statement of which
  venue(s) this environment could not reach.
- `grpcurl -plaintext 127.0.0.1:50051 orderbook.OrderbookAggregator/BookSummary`
  against a live `cargo run` — still streams, single-venue,
  `exchange == "binance"` — actual output quoted.
- `docker compose up --build` — brings up both feed tasks; actual observed
  output reported, including any environment limitation.
- `git diff main --stat -- src/merge.rs` — compared against Phase 1's
  (Task 1.3) recorded baseline. **Must be identical.** Any additional diff
  means Job B touched `merge.rs`, out of scope for this step, violating
  spec.md's explicit acceptance criterion.
- `git diff main --stat -- proto/orderbook.proto src/config.rs
  src/telemetry.rs src/proxy.rs` — zero diff on all four.
- The Bitstamp `"data"` fixture is real, with capture provenance in a
  comment — or a `TODO` naming the user, honestly reported, if capture was
  genuinely unreachable in every environment tried.
- README's claims match what was actually shipped and actually verified in
  this environment.

**Done when:** every check above is green (or, where environment-limited,
honestly reported as such) at the tip of the branch, and both `git diff
--stat` checks against `main` are clean.

**Commit boundary for Phase 5:** `README.md` alone. Reverting it has no
effect on build or test state.

---

## Final Verification

Before closing this packet:

- `cargo build` — clean.
- `cargo test` — clean, all tests listed in Task 5.2 present and passing
  (or the Bitstamp fixture test's `TODO` honestly in its place).
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo fmt --check` — clean.
- Most representative real functionality check: `cargo run -- --pair ethbtc
  --port 50051` with `grpcurl -plaintext 127.0.0.1:50051
  orderbook.OrderbookAggregator/BookSummary` in a second terminal, both
  venues' book lines visible in the logs, gRPC stream still single-venue
  (`exchange == "binance"`) — actual output quoted, network permitting.
