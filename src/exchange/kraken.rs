//! Kraken WebSocket v2 `book` channel: connect URL, subscribe message, and
//! `parse`, behind the `Exchange` trait.
//!
//! Kraken's `book` channel is snapshot-then-incremental: one `snapshot` on
//! subscribe, then `update` messages with only changed levels (`qty: 0`
//! means remove). Unlike Binance/Bitstamp, this needs local mutable state
//! across messages, so `parse` here is order-dependent — calling it twice
//! with the same `update` double-applies the delta.

use std::sync::Mutex;

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::exchange::{Exchange, Venue};
use crate::model::{Amount, Book, Price};

/// Kraken's `Exchange` implementation. Unlike Binance/Bitstamp's stateless
/// unit structs, this holds the accumulated book across messages.
/// `std::sync::Mutex`, not `RefCell` — `RefCell` is `!Sync` and breaks the
/// `Send` bound `JoinSet::spawn` needs; nothing here is actually concurrent.
///
/// One `Kraken` value is reused across reconnects. A fresh `snapshot`
/// always arrives before any `update` on a new connection, replacing state
/// wholesale — that's a protocol guarantee, not one this struct enforces.
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

/// One book level: parsed `f64` for sorting, plus the raw wire-text digits
/// for the checksum (must match Kraken's own text representation).
#[derive(Clone, Debug)]
struct KrakenLevel {
    price: f64,
    price_raw: String,
    qty: f64,
    qty_raw: String,
}

/// The locally-accumulated book: bids descending, asks ascending.
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

    /// Converts to the exchange-agnostic `model::Book`. Re-parses each
    /// already-validated `f64` so `Price::parse`/`Amount::parse` stays the
    /// one place that constructs those newtypes.
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
            // No lastUpdateId equivalent, same placeholder as Bitstamp.
            last_update_id: 0,
            parse_started_at,
            parsed_at: std::time::Instant::now(),
        }
    }
}

/// Removes `level` if `qty` is 0, otherwise upserts it, then re-sorts.
/// `ascending` is true for asks, false for bids.
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

/// Strips a wire-text digit string for Kraken's checksum: no `.`, no
/// leading zeros. Must use the original wire text — reformatting through
/// `f64` would drop trailing zeros and break the checksum.
fn strip_for_checksum(raw: &str) -> String {
    let no_dot: String = raw.chars().filter(|c| *c != '.').collect();
    let trimmed = no_dot.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Kraken's checksum: top 10 asks then bids, price+qty digits concatenated,
/// CRC32'd.
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

/// A price/qty level as it arrives on the wire. Captured via `RawValue`
/// (exact source text) so the checksum uses Kraken's own digit string.
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

/// The payload inside a `channel: "book"` message's `data` array (always
/// one element per message).
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

/// A cheap first pass: every message has either "channel" or "method",
/// never both. Avoids a full `BookMessage` deserialize on non-book messages.
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

    /// Nothing pair-specific in the path — the pair is in the subscribe
    /// message, same as Bitstamp.
    fn connect_url(&self, _pair: &str) -> String {
        "wss://ws.kraken.com/v2".to_string()
    }

    /// Per-connection subscription, same as Bitstamp — must be resent on
    /// every reconnect.
    fn subscribe_message(&self, pair: &str) -> Option<String> {
        let symbol = to_kraken_symbol(pair)?;
        Some(format!(
            r#"{{"method":"subscribe","params":{{"channel":"book","symbol":["{symbol}"],"depth":10}}}}"#
        ))
    }

    /// Dispatches on "channel"/"method" first, then "type" for book
    /// messages. Returns `None` for anything that isn't a book payload.
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

/// Known quote-currency suffixes, longest first so e.g. "usdt" isn't
/// shadowed by a shorter "usd" match.
const QUOTE_SUFFIXES: &[&str] = &["usdt", "btc", "usd", "eur"];

/// Converts "ethbtc" into Kraken's "ETH/BTC" form. `None` if no known
/// quote-currency suffix matches.
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

    // Captured live from wss://ws.kraken.com/v2, 2026-08-26, ETH/BTC book
    // channel at depth 10.
    const SNAPSHOT_FIXTURE: &str = r#"{"channel":"book","type":"snapshot","data":[{"symbol":"ETH/BTC","bids":[{"price":0.031348,"qty":0.03740000},{"price":0.031347,"qty":2.87288049},{"price":0.031345,"qty":8.57986833},{"price":0.031344,"qty":14.40276069},{"price":0.031340,"qty":12.96065840},{"price":0.031339,"qty":13.19606540},{"price":0.031338,"qty":20.41389313},{"price":0.031336,"qty":0.03740000},{"price":0.031334,"qty":14.37158163},{"price":0.031332,"qty":14.40695990}],"asks":[{"price":0.031357,"qty":0.03740000},{"price":0.031358,"qty":2.87194815},{"price":0.031361,"qty":0.00767324},{"price":0.031362,"qty":4.27140781},{"price":0.031363,"qty":4.34586052},{"price":0.031364,"qty":27.28097668},{"price":0.031365,"qty":12.92325840},{"price":0.031366,"qty":4.14290322},{"price":0.031368,"qty":0.00738649},{"price":0.031370,"qty":0.03740000}],"checksum":3619791617,"timestamp":"2026-08-26T15:16:16.637831Z"}]}"#;

    const UPDATE_FIXTURE: &str = r#"{"channel":"book","type":"update","data":[{"symbol":"ETH/BTC","bids":[{"price":0.031347,"qty":0.00000000},{"price":0.031329,"qty":19.07893401}],"asks":[],"checksum":2505869009,"timestamp":"2026-08-26T15:16:16.730423Z"}]}"#;

    const STATUS_FIXTURE: &str = r#"{"channel":"status","type":"update","data":[{"version":"2.0.10","system":"online","api_version":"v2","connection_id":12072546403441331453}]}"#;

    const HEARTBEAT_FIXTURE: &str = r#"{"channel":"heartbeat"}"#;

    const SUBSCRIBE_ACK_SUCCESS_FIXTURE: &str = r#"{"method":"subscribe","result":{"channel":"book","depth":10,"snapshot":true,"symbol":"ETH/BTC"},"success":true,"time_in":"2026-08-26T15:16:16.576099Z","time_out":"2026-08-26T15:16:16.576140Z"}"#;

    // Hand-built, not a live capture — matches Kraken's documented
    // subscribe-ack shape with success: false and an error field.
    const SUBSCRIBE_ACK_FAILURE_FIXTURE: &str = r#"{"method":"subscribe","result":{"channel":"book","symbol":"NOT/REAL"},"success":false,"error":"Unknown symbol"}"#;

    /// Asserts actual top price/qty, not just a length.
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

    /// Feeds a real snapshot then update through the same `Kraken` and
    /// checks the changed levels: 0.031347 gone, 0.031329 present.
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

    /// States the qty:0 removal rule as its own guarantee.
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

    /// A `success: false` ack must still parse to `None`, not panic.
    #[test]
    fn a_false_subscribe_ack_parses_to_none_without_panicking() {
        assert!(Kraken::new().parse(SUBSCRIBE_ACK_FAILURE_FIXTURE).is_none());
    }

    /// Feeding the same update twice must not panic — parse() has no
    /// guard against re-applying the same delta.
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

        // Both calls apply the same delta silently — parse() has no
        // notion of "already applied this message."
        assert_eq!(first.bids, second.bids);
        assert_eq!(first.asks, second.asks);
    }

    /// Proves the raw-digit-string checksum approach reproduces Kraken's
    /// own checksum against real captured data.
    #[test]
    fn checksum_of_the_real_captured_snapshot_matches_krakens_own_value() {
        let kraken = Kraken::new();
        // Assert the raw function directly too, not just via parse()'s
        // control flow.
        let msg: BookMessage = serde_json::from_str(SNAPSHOT_FIXTURE).unwrap();
        let data = msg.data.into_iter().next().unwrap();
        let book = KrakenBook::from_snapshot(&data).expect("valid snapshot data");
        assert_eq!(compute_checksum(&book), 3619791617);

        assert!(
            kraken.parse(SNAPSHOT_FIXTURE).is_some(),
            "checksum must have matched for parse to succeed"
        );
    }

    /// A corrupted checksum returns `None` and clears held state, so a
    /// subsequent update also returns `None` instead of applying to stale
    /// data.
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

    /// A second, fresh snapshot (simulating a reconnect) must fully
    /// replace prior state, not merge with it.
    #[test]
    fn a_fresh_snapshot_after_a_reconnect_replaces_prior_state_wholesale() {
        let kraken = Kraken::new();
        kraken.parse(SNAPSHOT_FIXTURE).expect("valid snapshot");
        kraken.parse(UPDATE_FIXTURE).expect("valid update");

        let book = kraken
            .parse(SNAPSHOT_FIXTURE)
            .expect("valid second snapshot");

        // The update's changes must not still be reflected.
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
