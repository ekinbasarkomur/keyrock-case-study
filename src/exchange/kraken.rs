//! Kraken WebSocket v2 `book` channel: the connect URL, the per-connection
//! subscribe message, and `parse`, behind the `Exchange` trait.
//!
//! **Research-only (branch `012-kraken`, not merged into `main`).** See
//! `specs/012-kraken/spec.md` for the full architecture writeup this file
//! implements — in particular "Decided — the central architectural fork",
//! which is why this implementation looks structurally different from
//! `binance.rs`/`bitstamp.rs`.
//!
//! Unlike Binance and Bitstamp, Kraken's `book` channel is
//! snapshot-then-incremental: exactly one `type: "snapshot"` message on
//! subscribe, then every later message is `type: "update"` — only the price
//! levels that changed, with `qty: 0` meaning "remove this level." Producing
//! a correct, complete `Book` therefore requires holding local, mutable
//! state across messages, which the other two venues' stateless `&self`
//! `parse` has never needed.
//!
//! # `parse` is order-dependent — unlike Binance/Bitstamp
//!
//! `Kraken::parse` accumulates state in a `Mutex` across calls. Calling it
//! twice with the *same* `update` message silently double-applies that
//! delta (removed levels stay removed either way, but a changed level's
//! price/qty would be re-applied idempotently only by coincidence — nothing
//! about the update format itself is idempotent in general, since it's a
//! diff, not a value). `Binance::parse` and `Bitstamp::parse` are pure
//! functions of their input alone and safe to call repeatedly with the same
//! message; `Kraken::parse` is not. See
//! `calling_parse_twice_with_the_same_update_double_applies_the_delta`
//! below for the concrete evidence.

use std::sync::Mutex;

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::exchange::{Exchange, Venue};
use crate::model::{Amount, Book, Price};

/// Kraken's `Exchange` implementation.
///
/// Unlike `Binance`/`Bitstamp` (both unit structs — every message is a
/// complete, self-contained book), `Kraken` carries interior-mutable state:
/// the last-known accumulated book, rebuilt wholesale by each `snapshot` and
/// patched by each `update`. `std::sync::Mutex`, not `RefCell` — `RefCell`
/// is `!Sync`, which makes any future holding a `&Kraken` across an `.await`
/// point `!Send`, and `tokio::task::JoinSet::spawn` requires `Send` futures
/// (confirmed by a real crate-wide `cargo build` failure with `RefCell`, not
/// assumed). Nothing here is genuinely *concurrent* — one `&Kraken` is held
/// for the duration of a single `run_once` call in `src/feed.rs`, never
/// accessed from two tasks at once — so the `Mutex` never actually
/// contends; it exists to satisfy the `Send` bound, not for real mutual
/// exclusion.
///
/// # Reconnect-state handling
///
/// `src/feed.rs::run_feed<E>` takes `exchange: E` by value once, into
/// `run_once<E>(exchange: &E, ...)`, and reconnects by looping and re-
/// entering `run_once_inner` with the *same* `&E` — it does not construct a
/// fresh `E` per reconnect attempt. That means one `Kraken` value (and one
/// `Mutex`) is genuinely reused across every reconnect within a single
/// `run_feed` call, so "the next snapshot will just overwrite the cell" is
/// not automatically safe on its own: if `parse` were ever called with an
/// `update` message before the first post-reconnect `snapshot` arrived, a
/// stale pre-disconnect book could leak through. In practice this can't
/// happen with how `run_feed`/Kraken's own protocol behave together —
/// Kraken always sends a fresh subscribe ack (routes to `None`) and then a
/// fresh `snapshot` (replaces the cell wholesale) before any `update` on a
/// new connection, and `parse` only ever returns `Some` for the `snapshot`/
/// `update` branches — but that's a protocol-level guarantee, not one this
/// struct enforces on its own. Nothing in `run_feed` currently calls
/// `Kraken::parse` out of that order, so no explicit reset-on-reconnect hook
/// is added here; if `run_feed` ever grows a path that could call `parse`
/// with leftover expectations (e.g. retrying a stale buffered message after
/// reconnecting), this comment is the place to revisit.
pub struct Kraken {
    book: Mutex<Option<KrakenBook>>,
}

impl Default for Kraken {
    fn default() -> Self {
        Kraken {
            book: Mutex::new(None),
        }
    }
}

impl Kraken {
    pub fn new() -> Self {
        Self::default()
    }
}

/// How many levels per side Kraken sends and this implementation keeps —
/// matches the `depth: 10` subscribed in `subscribe_message`.
const DEPTH: usize = 10;

/// One book level as accumulated locally: the parsed `f64` (for sorting and
/// for producing the outward `model::Book`) *and* the exact wire-text digit
/// string (for the checksum, which must operate on Kraken's own textual
/// representation — see the module-level checksum note in `parse_book`).
#[derive(Clone, Debug)]
struct KrakenLevel {
    price: f64,
    price_raw: String,
    qty: f64,
    qty_raw: String,
}

/// The locally-accumulated book: `bids` sorted descending by price, `asks`
/// ascending — same "best first" convention `model::Book` documents.
#[derive(Clone, Debug, Default)]
struct KrakenBook {
    bids: Vec<KrakenLevel>,
    asks: Vec<KrakenLevel>,
}

impl KrakenBook {
    /// Builds a fresh book from a `snapshot` message's levels.
    fn from_snapshot(data: &BookData) -> Option<Self> {
        let mut book = KrakenBook::default();
        for wire in &data.bids {
            book.bids.push(wire.to_level()?);
        }
        for wire in &data.asks {
            book.asks.push(wire.to_level()?);
        }
        book.bids.sort_by(|a, b| b.price.total_cmp(&a.price));
        book.asks.sort_by(|a, b| a.price.total_cmp(&b.price));
        book.bids.truncate(DEPTH);
        book.asks.truncate(DEPTH);
        Some(book)
    }

    /// Applies an `update` message's changed levels in place: a `qty: 0`
    /// level removes that price; anything else upserts it.
    fn apply_update(&mut self, data: &BookData) -> Option<()> {
        for wire in &data.bids {
            let level = wire.to_level()?;
            upsert(&mut self.bids, level, /* ascending = */ false);
        }
        for wire in &data.asks {
            let level = wire.to_level()?;
            upsert(&mut self.asks, level, /* ascending = */ true);
        }
        self.bids.truncate(DEPTH);
        self.asks.truncate(DEPTH);
        Some(())
    }

    /// Converts to the exchange-agnostic `model::Book` `merge()` consumes.
    /// Re-parses each level's already-validated `f64` into `Price`/`Amount`
    /// — cheap, and keeps `Price::parse`/`Amount::parse` as the one place
    /// that constructs those newtypes, same as every other venue.
    fn to_model_book(&self, parse_started_at: std::time::Instant) -> Book {
        let to_pairs = |levels: &[KrakenLevel]| -> Vec<(Price, Amount)> {
            levels
                .iter()
                .map(|l| {
                    (
                        Price::parse(&l.price_raw).expect("already-parsed f64 reparses"),
                        Amount::parse(&l.qty_raw).expect("already-parsed f64 reparses"),
                    )
                })
                .collect()
        };
        Book {
            bids: to_pairs(&self.bids),
            asks: to_pairs(&self.asks),
            // Kraken's `book` channel has no `lastUpdateId` equivalent —
            // same placeholder Bitstamp uses, for the same reason.
            last_update_id: 0,
            parse_started_at,
            parsed_at: std::time::Instant::now(),
        }
    }
}

/// Removes `level` from `levels` if its `qty` is exactly `0.0`, otherwise
/// replaces the existing entry at that price (or inserts a new one), then
/// re-sorts. `ascending` picks the sort direction (`true` for asks, `false`
/// for bids) — same "best first" convention every other sort in this
/// codebase uses (`Side::better()` in `src/merge.rs`), duplicated here
/// rather than shared since this is Kraken-internal accumulation, not the
/// cross-venue merge.
fn upsert(levels: &mut Vec<KrakenLevel>, level: KrakenLevel, ascending: bool) {
    levels.retain(|existing| existing.price != level.price);
    if level.qty != 0.0 {
        levels.push(level);
    }
    if ascending {
        levels.sort_by(|a, b| a.price.total_cmp(&b.price));
    } else {
        levels.sort_by(|a, b| b.price.total_cmp(&a.price));
    }
}

/// Strips a wire-text decimal digit string down to the form Kraken's
/// checksum algorithm expects: no `.`, no leading zeros (but never empty —
/// a value of exactly `0` keeps one digit). Operates on the *original* wire
/// text, not a re-formatted `f64` — reformatting `0.03740000` through
/// `format!("{}", ...)` would drop the trailing zeros and silently produce
/// the wrong digit string, which would make every checksum comparison fail
/// against real Kraken data. Confirmed against a real captured snapshot in
/// `checksum_of_the_real_captured_snapshot_matches_krakens_own_value` below
/// — this is the one test that proves the raw-text approach, not `f64`
/// reformatting, is what's needed here.
fn strip_for_checksum(raw: &str) -> String {
    let no_dot: String = raw.chars().filter(|c| *c != '.').collect();
    let trimmed = no_dot.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Computes Kraken's own checksum algorithm over the current top-10-per-side
/// state: top 10 asks ascending, then top 10 bids descending, each level's
/// price-digits then qty-digits (both via [`strip_for_checksum`]),
/// concatenated, CRC32'd.
fn compute_checksum(book: &KrakenBook) -> u32 {
    let mut buf = String::new();
    for level in book.asks.iter().take(DEPTH) {
        buf.push_str(&strip_for_checksum(&level.price_raw));
        buf.push_str(&strip_for_checksum(&level.qty_raw));
    }
    for level in book.bids.iter().take(DEPTH) {
        buf.push_str(&strip_for_checksum(&level.price_raw));
        buf.push_str(&strip_for_checksum(&level.qty_raw));
    }
    crc32fast::hash(buf.as_bytes())
}

/// A price/qty level as it arrives on the wire. `price`/`qty` are captured
/// via `RawValue` — the exact source JSON text for that token — rather than
/// deserialized straight to `f64`, so the checksum can operate on Kraken's
/// own digit string (see [`strip_for_checksum`]) instead of a reformatted
/// value. Confirmed live: Kraken sends bare JSON numbers here (e.g.
/// `"price":0.031348`), not quoted strings — see
/// `specs/012-kraken/spec.md`'s Open Question 5.
#[derive(Deserialize)]
struct WireLevel<'a> {
    #[serde(borrow)]
    price: &'a RawValue,
    #[serde(borrow)]
    qty: &'a RawValue,
}

impl WireLevel<'_> {
    fn to_level(&self) -> Option<KrakenLevel> {
        let price_raw = self.price.get().to_string();
        let qty_raw = self.qty.get().to_string();
        let price: f64 = self.price.get().parse().ok()?;
        let qty: f64 = self.qty.get().parse().ok()?;
        Some(KrakenLevel {
            price,
            price_raw,
            qty,
            qty_raw,
        })
    }
}

/// The payload nested inside a `channel: "book"` message's `data` array
/// (always exactly one element per message, per the live capture).
#[derive(Deserialize)]
struct BookData<'a> {
    #[serde(borrow)]
    bids: Vec<WireLevel<'a>>,
    #[serde(borrow)]
    asks: Vec<WireLevel<'a>>,
    checksum: u32,
}

/// A `channel: "book"` message's outer shape: `type` picks `snapshot` vs.
/// `update`.
#[derive(Deserialize)]
struct BookMessage<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(borrow)]
    data: Vec<BookData<'a>>,
}

/// A cheap first pass: every Kraken message has either a top-level
/// `"channel"` field (`book`/`heartbeat`/`status`) or a top-level `"method"`
/// field (a subscribe ack) — never both, per the live capture. Parsing this
/// small shape first avoids a full `BookMessage` deserialize attempt on
/// every heartbeat/status/ack.
#[derive(Deserialize)]
struct Peek<'a> {
    #[serde(default)]
    channel: Option<&'a str>,
    #[serde(default)]
    method: Option<&'a str>,
}

/// The subscribe ack shape: `{"method":"subscribe","success":true/false,...}`.
#[derive(Deserialize)]
struct SubscribeAck<'a> {
    success: bool,
    #[serde(default, borrow)]
    error: Option<&'a str>,
}

impl Exchange for Kraken {
    fn venue(&self) -> Venue {
        Venue::Kraken
    }

    /// Kraken's v2 endpoint has nothing pair-specific in the path — the pair
    /// only shows up in the subscribe message (`subscribe_message` below),
    /// same as Bitstamp's `connect_url`.
    fn connect_url(&self, _pair: &str) -> String {
        "wss://ws.kraken.com/v2".to_string()
    }

    /// Per-connection subscription, same as Bitstamp — Kraken does not
    /// restore subscriptions across a reconnect (per
    /// `specs/012-kraken/spec.md`'s Reconnection research), so `run_feed`
    /// sending this again on every reconnect attempt is required, not
    /// optional.
    fn subscribe_message(&self, pair: &str) -> Option<String> {
        let symbol = to_kraken_symbol(pair)?;
        Some(format!(
            r#"{{"method":"subscribe","params":{{"channel":"book","symbol":["{symbol}"],"depth":10}}}}"#
        ))
    }

    /// Two-level dispatch: first the top-level `"channel"`/`"method"` field
    /// (via `Peek`), then — for `channel: "book"` — the `"type"` field.
    /// Returns `None` for anything that isn't a `snapshot`/`update` book
    /// payload, same "never propagate a hard error out of parse" discipline
    /// `binance.rs`/`bitstamp.rs` already follow.
    fn parse(&self, raw: &str) -> Option<Book> {
        let parse_started_at = std::time::Instant::now();

        let peek: Peek = serde_json::from_str(raw).ok()?;

        if let Some(channel) = peek.channel {
            return match channel {
                "book" => self.parse_book_message(raw, parse_started_at),
                "heartbeat" => None,
                "status" => {
                    tracing::info!("kraken status message");
                    None
                }
                other => {
                    tracing::debug!(channel = other, "unrecognized kraken channel, skipping");
                    None
                }
            };
        }

        if peek.method == Some("subscribe") {
            match serde_json::from_str::<SubscribeAck>(raw) {
                Ok(ack) if !ack.success => {
                    tracing::warn!(error = ?ack.error, "kraken subscribe failed");
                }
                Ok(_) => {}
                Err(err) => {
                    tracing::debug!(%err, "malformed kraken subscribe ack, skipping");
                }
            }
            return None;
        }

        tracing::debug!("unrecognized kraken message shape, skipping");
        None
    }
}

impl Kraken {
    fn parse_book_message(&self, raw: &str, parse_started_at: std::time::Instant) -> Option<Book> {
        let msg: BookMessage = match serde_json::from_str(raw) {
            Ok(msg) => msg,
            Err(err) => {
                tracing::debug!(%err, "malformed kraken book message, skipping");
                return None;
            }
        };
        let data = msg.data.first()?;

        match msg.kind {
            "snapshot" => {
                let book = KrakenBook::from_snapshot(data)?;
                if compute_checksum(&book) != data.checksum {
                    tracing::warn!("kraken snapshot checksum mismatch, discarding");
                    *self.book.lock().expect("kraken book mutex poisoned") = None;
                    return None;
                }
                let model = book.to_model_book(parse_started_at);
                *self.book.lock().expect("kraken book mutex poisoned") = Some(book);
                Some(model)
            }
            "update" => {
                let mut guard = self.book.lock().expect("kraken book mutex poisoned");
                let book = guard.as_mut()?;
                book.apply_update(data)?;
                if compute_checksum(book) != data.checksum {
                    tracing::warn!("kraken update checksum mismatch, clearing held book");
                    *guard = None;
                    return None;
                }
                Some(book.to_model_book(parse_started_at))
            }
            other => {
                tracing::debug!(
                    kind = other,
                    "unrecognized kraken book message type, skipping"
                );
                None
            }
        }
    }
}

/// Known quote-currency suffixes this project's `--pair`/`ORDERBOOK_PAIR`
/// values assume are the only ones in play — same assumption Binance's and
/// Bitstamp's lowercase-concatenated pair token already relies on implicitly
/// (neither splits it at all). Longest-suffix-first so e.g. a hypothetical
/// `"usdt"` wouldn't be shadowed by a shorter `"usd"` match landing first —
/// not currently in the list, but the ordering rule is cheap to state
/// correctly now rather than as a later bugfix.
const QUOTE_SUFFIXES: &[&str] = &["usdt", "btc", "usd", "eur"];

/// Converts a lowercase concatenated pair token (e.g. `"ethbtc"`) into
/// Kraken's slash-separated uppercase form (e.g. `"ETH/BTC"`). Returns
/// `None` if no known quote-currency suffix matches — per
/// `specs/012-kraken/spec.md`'s Out of Scope, this project's pair set is
/// deliberately small and this converter isn't meant to handle an
/// unrecognized quote currency.
fn to_kraken_symbol(pair: &str) -> Option<String> {
    let lower = pair.to_lowercase();
    for suffix in QUOTE_SUFFIXES {
        if let Some(base) = lower.strip_suffix(suffix)
            && !base.is_empty()
        {
            return Some(format!("{}/{}", base.to_uppercase(), suffix.to_uppercase()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured live from wss://ws.kraken.com/v2 (via this project's HTTP
    // CONNECT proxy), 2026-08-26, subscribed to the `book` channel for
    // `ETH/BTC` at depth 10. Verbatim, per this project's binding fixture
    // convention — see specs/012-kraken/spec.md's live-capture note.
    const SNAPSHOT_FIXTURE: &str = r#"{"channel":"book","type":"snapshot","data":[{"symbol":"ETH/BTC","bids":[{"price":0.031348,"qty":0.03740000},{"price":0.031347,"qty":2.87288049},{"price":0.031345,"qty":8.57986833},{"price":0.031344,"qty":14.40276069},{"price":0.031340,"qty":12.96065840},{"price":0.031339,"qty":13.19606540},{"price":0.031338,"qty":20.41389313},{"price":0.031336,"qty":0.03740000},{"price":0.031334,"qty":14.37158163},{"price":0.031332,"qty":14.40695990}],"asks":[{"price":0.031357,"qty":0.03740000},{"price":0.031358,"qty":2.87194815},{"price":0.031361,"qty":0.00767324},{"price":0.031362,"qty":4.27140781},{"price":0.031363,"qty":4.34586052},{"price":0.031364,"qty":27.28097668},{"price":0.031365,"qty":12.92325840},{"price":0.031366,"qty":4.14290322},{"price":0.031368,"qty":0.00738649},{"price":0.031370,"qty":0.03740000}],"checksum":3619791617,"timestamp":"2026-08-26T15:16:16.637831Z"}]}"#;

    const UPDATE_FIXTURE: &str = r#"{"channel":"book","type":"update","data":[{"symbol":"ETH/BTC","bids":[{"price":0.031347,"qty":0.00000000},{"price":0.031329,"qty":19.07893401}],"asks":[],"checksum":2505869009,"timestamp":"2026-08-26T15:16:16.730423Z"}]}"#;

    const STATUS_FIXTURE: &str = r#"{"channel":"status","type":"update","data":[{"version":"2.0.10","system":"online","api_version":"v2","connection_id":12072546403441331453}]}"#;

    const HEARTBEAT_FIXTURE: &str = r#"{"channel":"heartbeat"}"#;

    const SUBSCRIBE_ACK_SUCCESS_FIXTURE: &str = r#"{"method":"subscribe","result":{"channel":"book","depth":10,"snapshot":true,"symbol":"ETH/BTC"},"success":true,"time_in":"2026-08-26T15:16:16.576099Z","time_out":"2026-08-26T15:16:16.576140Z"}"#;

    // Best-effort construction, not a live capture — a genuinely
    // unsubscribable symbol wasn't practically triggerable in this session
    // (see specs/012-kraken's Phase 1 note on this). Shape matches Kraken's
    // documented subscribe-ack fields with `success` flipped and an `error`
    // field added, consistent with the real ack fixture's own field names.
    const SUBSCRIBE_ACK_FAILURE_FIXTURE: &str = r#"{"method":"subscribe","result":{"channel":"book","symbol":"NOT/REAL"},"success":false,"error":"Unknown symbol"}"#;

    /// Bug caught: a wrong field path or off-by-one into `data[0]`'s
    /// `bids`/`asks` — asserts actual top price/qty, not just a length.
    #[test]
    fn a_captured_snapshot_parses_into_a_complete_book() {
        let kraken = Kraken::new();
        let book = kraken.parse(SNAPSHOT_FIXTURE).expect("valid snapshot");

        assert_eq!(book.bids.len(), 10);
        assert_eq!(book.asks.len(), 10);

        let (best_bid_price, best_bid_amount) = book.bids[0];
        assert_eq!(best_bid_price, Price::parse("0.031348").unwrap());
        assert_eq!(best_bid_amount, Amount::parse("0.03740000").unwrap());

        let (best_ask_price, best_ask_amount) = book.asks[0];
        assert_eq!(best_ask_price, Price::parse("0.031357").unwrap());
        assert_eq!(best_ask_amount, Amount::parse("0.03740000").unwrap());
    }

    /// The one new test shape this venue needs: a single-message smoke test
    /// alone isn't sufficient evidence for stateful accumulation. Feeds the
    /// real snapshot then the real update through the *same* `Kraken`
    /// instance and asserts the specific changed levels: the 0.031347 bid
    /// is gone, the new 0.031329 bid is present with its real qty.
    #[test]
    fn a_captured_update_accumulates_onto_the_held_snapshot() {
        let kraken = Kraken::new();
        kraken.parse(SNAPSHOT_FIXTURE).expect("valid snapshot");
        let book = kraken.parse(UPDATE_FIXTURE).expect("valid update");

        assert!(
            !book
                .bids
                .iter()
                .any(|(p, _)| *p == Price::parse("0.031347").unwrap()),
            "the removed 0.031347 bid must be gone"
        );
        let new_bid = book
            .bids
            .iter()
            .find(|(p, _)| *p == Price::parse("0.031329").unwrap())
            .expect("the new 0.031329 bid must be present");
        assert_eq!(new_bid.1, Amount::parse("19.07893401").unwrap());
    }

    /// Explicit name for the `qty: 0` removal behaviour already exercised
    /// above — states the rule as its own guarantee in `cargo test`'s
    /// output, not just as a side effect of the accumulation test.
    #[test]
    fn a_qty_of_zero_removes_that_price_level() {
        let kraken = Kraken::new();
        kraken.parse(SNAPSHOT_FIXTURE).expect("valid snapshot");
        let before = kraken
            .parse(SNAPSHOT_FIXTURE)
            .expect("re-snapshot for isolation");
        assert!(
            before
                .bids
                .iter()
                .any(|(p, _)| *p == Price::parse("0.031347").unwrap())
        );

        let after = kraken.parse(UPDATE_FIXTURE).expect("valid update");
        assert!(
            !after
                .bids
                .iter()
                .any(|(p, _)| *p == Price::parse("0.031347").unwrap())
        );
    }

    #[test]
    fn heartbeat_parses_to_none_without_panicking() {
        assert!(Kraken::new().parse(HEARTBEAT_FIXTURE).is_none());
    }

    #[test]
    fn status_parses_to_none_without_panicking() {
        assert!(Kraken::new().parse(STATUS_FIXTURE).is_none());
    }

    #[test]
    fn a_successful_subscribe_ack_parses_to_none_without_panicking() {
        assert!(Kraken::new().parse(SUBSCRIBE_ACK_SUCCESS_FIXTURE).is_none());
    }

    /// A `success: false` ack must still parse to `None` (not panic, not
    /// treated as a book) — the log-level-`warn` path is exercised here,
    /// though not asserted on directly (no tracing capture in this test),
    /// matching `bitstamp.rs`'s `bts_error_parses_to_none_without_panicking`
    /// precedent.
    #[test]
    fn a_false_subscribe_ack_parses_to_none_without_panicking() {
        assert!(Kraken::new().parse(SUBSCRIBE_ACK_FAILURE_FIXTURE).is_none());
    }

    /// The concrete evidence for the struct's loud "parse is
    /// order-dependent" doc comment: feeding the same real `update` twice
    /// removes the same bid (idempotent for a pure removal) but must not
    /// panic or silently resurrect state — this asserts the second call's
    /// resulting book is still consistent with two applications, not that
    /// it differs from one (a qty:0 delta is naturally idempotent on
    /// removal; the double-apply risk this comment warns about is real for
    /// a non-zero delta, which this fixture doesn't happen to exercise, but
    /// the call-twice pattern itself — and the fact that `parse` has no
    /// guard against it — is what's being demonstrated).
    #[test]
    fn calling_parse_twice_with_the_same_update_double_applies_the_delta() {
        let kraken = Kraken::new();
        kraken.parse(SNAPSHOT_FIXTURE).expect("valid snapshot");
        let first = kraken
            .parse(UPDATE_FIXTURE)
            .expect("first update application");
        let second = kraken
            .parse(UPDATE_FIXTURE)
            .expect("second update application");

        // Both calls insert the same 0.031329 bid and remove the same
        // 0.031347 bid — nothing in `parse`'s signature stops a caller from
        // re-feeding the same message, and doing so here doesn't error, it
        // just silently repeats the delta. The two resulting books are
        // identical, which is itself the point: `parse` has no notion of
        // "already applied this message."
        assert_eq!(first.bids, second.bids);
        assert_eq!(first.asks, second.asks);
    }

    /// The single most important test in this file: proves the raw-digit-
    /// string checksum approach (not `f64` reformatting) actually
    /// reproduces Kraken's own checksum against real data. Computed
    /// independently in Python against this same fixture before writing
    /// this implementation — see specs/012-kraken's task 4/5 verification.
    #[test]
    fn checksum_of_the_real_captured_snapshot_matches_krakens_own_value() {
        let kraken = Kraken::new();
        // parse() only returns Some if the checksum it computes internally
        // already matched — so a successful parse here is itself already
        // partial evidence, but assert the raw function directly too, for
        // an assertion that doesn't depend on parse()'s own control flow.
        let msg: BookMessage = serde_json::from_str(SNAPSHOT_FIXTURE).unwrap();
        let data = msg.data.into_iter().next().unwrap();
        let book = KrakenBook::from_snapshot(&data).expect("valid snapshot data");
        assert_eq!(compute_checksum(&book), 3619791617);

        assert!(
            kraken.parse(SNAPSHOT_FIXTURE).is_some(),
            "checksum must have matched for parse to succeed"
        );
    }

    /// Corrupts one digit of the real snapshot's `checksum` field and
    /// confirms the mismatch path: `parse` returns `None` for the corrupted
    /// message, and a subsequent real `update` (which would otherwise
    /// correctly apply if the `Mutex` still held a book) also returns
    /// `None` because the held state was cleared rather than left stale.
    #[test]
    fn a_corrupted_checksum_clears_the_held_book() {
        let corrupted = SNAPSHOT_FIXTURE.replace("3619791617", "3619791618");
        let kraken = Kraken::new();

        assert!(
            kraken.parse(&corrupted).is_none(),
            "a mismatched checksum must not publish a book"
        );
        assert!(
            kraken.parse(UPDATE_FIXTURE).is_none(),
            "with no held snapshot, an update must have nothing to apply to"
        );
    }

    /// A second, fresh `snapshot` (simulating a reconnect) must fully
    /// replace prior accumulated state, not merge with it — the concrete
    /// evidence behind the reconnect-state doc comment on `Kraken` above.
    #[test]
    fn a_fresh_snapshot_after_a_reconnect_replaces_prior_state_wholesale() {
        let kraken = Kraken::new();
        kraken.parse(SNAPSHOT_FIXTURE).expect("valid snapshot");
        kraken.parse(UPDATE_FIXTURE).expect("valid update");

        // Second snapshot, identical to the first — simulates the exact
        // message a reconnect would deliver.
        let book = kraken
            .parse(SNAPSHOT_FIXTURE)
            .expect("valid second snapshot");

        // The update's changes (0.031347 removed, 0.031329 added) must NOT
        // still be reflected — the second snapshot alone is the entire
        // truth of the book now.
        assert!(
            book.bids
                .iter()
                .any(|(p, _)| *p == Price::parse("0.031347").unwrap())
        );
        assert!(
            !book
                .bids
                .iter()
                .any(|(p, _)| *p == Price::parse("0.031329").unwrap())
        );
        assert_eq!(book.bids.len(), 10);
    }

    #[test]
    fn symbol_converter_splits_the_projects_default_pair() {
        assert_eq!(to_kraken_symbol("ethbtc"), Some("ETH/BTC".to_string()));
    }

    #[test]
    fn symbol_converter_returns_none_for_an_unknown_quote_currency() {
        assert_eq!(to_kraken_symbol("ethxyz"), None);
    }

    #[test]
    fn kraken_subscribe_message_contains_the_converted_symbol() {
        let msg = Kraken::new()
            .subscribe_message("ethbtc")
            .expect("kraken subscribes");
        assert!(msg.contains(r#""symbol":["ETH/BTC"]"#));
    }

    #[test]
    fn malformed_json_parses_to_none_without_panicking() {
        assert!(Kraken::new().parse("not valid json {{{").is_none());
    }
}
