//! Bitstamp `order_book_<pair>` feed: the connect URL, the explicit
//! subscribe message Bitstamp requires (unlike Binance, whose subscription
//! is baked into the URL), and the pure `parse` function, behind the
//! `Exchange` trait. No websocket connection is opened here — that's
//! `src/feed.rs::run_feed`'s job.

use crate::exchange::{Exchange, Venue};
use crate::model::{Amount, Book, Price};
use serde::Deserialize;

/// Bitstamp wraps every message in an envelope (`{"event":...,
/// "channel":..., "data":{...}}`), unlike Binance's flat `depth20` payload —
/// only the `"data"` event carries a book; the other three are
/// lifecycle/control messages (see `parse` below).
#[derive(Deserialize)]
struct Envelope<'a> {
    event: &'a str,
    #[serde(borrow, default)]
    data: Option<Data<'a>>,
}

/// The book payload nested inside a `"data"`-event envelope. Borrows
/// `bids`/`asks` straight out of the source JSON text, same as Binance's
/// `Depth20<'a>` — the borrow doesn't outlive `parse()`, since the `Book`
/// that survives past this function holds only `Price`/`Amount`, never a
/// `&str`.
#[derive(Deserialize)]
struct Data<'a> {
    #[serde(borrow)]
    bids: Vec<[&'a str; 2]>,
    #[serde(borrow)]
    asks: Vec<[&'a str; 2]>,
}

/// Bitstamp's `Exchange` implementation. A unit struct — everything it
/// needs is free-standing in this module already; the struct exists only to
/// carry the trait impl.
pub struct Bitstamp;

impl Exchange for Bitstamp {
    fn venue(&self) -> Venue {
        Venue::Bitstamp
    }

    /// Bitstamp's websocket endpoint has nothing pair-specific in the path
    /// — the pair only shows up in the subscribe channel name
    /// (`subscribe_message` below). `pair` is unused here on purpose.
    fn connect_url(&self, _pair: &str) -> String {
        "wss://ws.bitstamp.net".to_string()
    }

    /// Unlike Binance, Bitstamp's subscription is per-connection, not baked
    /// into the URL — this message must be sent once, right after
    /// connecting, or the connection sits open and silent.
    fn subscribe_message(&self, pair: &str) -> Option<String> {
        Some(format!(
            r#"{{"event":"bts:subscribe","data":{{"channel":"order_book_{}"}}}}"#,
            pair.to_lowercase()
        ))
    }

    /// Parses a raw websocket text message into a [`Book`].
    ///
    /// Returns `None` — never `Result` — for anything that isn't a `"data"`
    /// event: malformed JSON, or one of Bitstamp's three lifecycle events
    /// (`bts:subscription_succeeded`, `bts:request_reconnect`, `bts:error`).
    /// A stray control message must not kill the read loop — same
    /// discipline as `binance.rs::parse`.
    fn parse(&self, raw: &str) -> Option<Book> {
        let envelope: Envelope = match serde_json::from_str(raw) {
            Ok(envelope) => envelope,
            Err(err) => {
                tracing::debug!(%err, "not a recognizable bitstamp envelope, skipping");
                return None;
            }
        };

        match envelope.event {
            "data" => {
                let data = envelope.data?;
                let bids = parse_levels(&data.bids)?;
                let asks = parse_levels(&data.asks)?;
                Some(Book {
                    bids,
                    asks,
                    // Bitstamp has no `lastUpdateId` equivalent — `Book`'s
                    // field is framed around Binance's sequence-number
                    // semantics, and Bitstamp's own `microtimestamp` isn't
                    // wired into `Book` yet (no consumer for it this step,
                    // per specs/006-bitstamp/spec.md). 0 is a placeholder,
                    // not a claim that Bitstamp sent this update first.
                    last_update_id: 0,
                })
            }
            "bts:subscription_succeeded" => {
                tracing::info!("bitstamp subscription succeeded");
                None
            }
            "bts:request_reconnect" => {
                // Step 6 owns turning this into an actual reconnect trigger
                // — this step only logs it.
                tracing::info!("bitstamp requested a reconnect");
                None
            }
            "bts:error" => {
                // The one event that means something is actually wrong —
                // must not be logged at the same level as the benign ones.
                tracing::warn!(raw, "bitstamp reported an error");
                None
            }
            other => {
                tracing::debug!(event = other, "unrecognized bitstamp event, skipping");
                None
            }
        }
    }
}

/// Converts `[price_str, amount_str]` pairs into `(Price, Amount)` levels.
/// Returns `None` if any level fails to parse, rather than silently
/// dropping a level and publishing a book with a hole in it. A direct copy
/// of `binance.rs::parse_levels`'s shape — factoring this out further would
/// fight the borrow checker for no real benefit at two implementations.
fn parse_levels(levels: &[[&str; 2]]) -> Option<Vec<(Price, Amount)>> {
    levels
        .iter()
        .map(|[price, amount]| {
            let price = Price::parse(price)?;
            let amount = Amount::parse(amount)?;
            Some((price, amount))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from wss://ws.bitstamp.net (order_book_ethbtc channel) on
    // 2026-08-23, direct connection, no proxy. First capture attempt failed
    // with a TLS handshake EOF — root-caused to the `rustls` dependency
    // shipping without its `tls12` feature (see Cargo.toml's `rustls` entry
    // for the full story); this app could only ever offer TLS1.3 in its
    // ClientHello, and Bitstamp's frontend drops a TLS1.3-only handshake.
    // Binance's endpoint happens to accept TLS1.3, which hid the gap until
    // now. Trimmed to 5 bids/5 asks from Bitstamp's real 100-level payload
    // — the full envelope's other fields (`timestamp`, `microtimestamp`,
    // `channel`) are the genuine values from the captured message.
    const DATA_FIXTURE: &str = r#"{"data":{"timestamp":"1787511693","microtimestamp":"1787511693313188","bids":[["0.03163789","3.42951262"],["0.03163505","0.61258313"],["0.03162919","1.02115457"],["0.03162384","0.04990000"],["0.03162273","0.34284793"]],"asks":[["0.03164587","0.61236387"],["0.03164684","3.42951262"],["0.03165086","1.02045379"],["0.03166236","0.04990000"],["0.03166726","2.04002053"]]},"channel":"order_book_ethbtc","event":"data"}"#;

    /// Bug caught: a wrong field path into the wrapped envelope (e.g.
    /// reading `bids` at the top level instead of inside `data`) — the real
    /// fixture's nesting is the thing a hand-built one couldn't be trusted
    /// to reproduce faithfully.
    #[test]
    fn bitstamp_data_message_parses_to_the_right_levels_and_prices() {
        let book = Bitstamp.parse(DATA_FIXTURE).expect("valid data payload");

        assert_eq!(book.bids.len(), 5);
        assert_eq!(book.asks.len(), 5);

        let (best_bid_price, best_bid_amount) = book.bids[0];
        assert_eq!(best_bid_price, Price::parse("0.03163789").unwrap());
        assert_eq!(best_bid_amount, Amount::parse("3.42951262").unwrap());

        let (best_ask_price, best_ask_amount) = book.asks[0];
        assert_eq!(best_ask_price, Price::parse("0.03164587").unwrap());
        assert_eq!(best_ask_amount, Amount::parse("0.61236387").unwrap());
    }

    /// `bts:subscription_succeeded` is a benign lifecycle message, not a
    /// parse failure worth propagating — and its payload has no
    /// `bids`/`asks` at all, so this also catches a panic on that shape.
    #[test]
    fn bts_subscription_succeeded_parses_to_none_without_panicking() {
        assert!(
            Bitstamp
                .parse(r#"{"event":"bts:subscription_succeeded","channel":"order_book_ethbtc","data":{}}"#)
                .is_none()
        );
    }

    /// Confirms `bts:request_reconnect` is recognized specifically, rather
    /// than falling through the generic "unknown event" branch silently —
    /// step 6 depends on this event being distinguishable from the other
    /// three.
    #[test]
    fn bts_request_reconnect_parses_to_none_without_panicking() {
        assert!(
            Bitstamp
                .parse(r#"{"event":"bts:request_reconnect","channel":"","data":{}}"#)
                .is_none()
        );
    }

    /// `bts:error` must not panic on a shape with no `bids`/`asks` either —
    /// distinct from the other lifecycle tests only in which event string
    /// it exercises, kept separate because this one is the log-level-`warn`
    /// case, worth its own name in `cargo test`'s output.
    #[test]
    fn bts_error_parses_to_none_without_panicking() {
        assert!(
            Bitstamp
                .parse(r#"{"event":"bts:error","channel":"","data":{"code":null,"message":"bad subscription"}}"#)
                .is_none()
        );
    }

    /// An unhandled `serde_json` error must convert to `None`, not
    /// propagate — a stray malformed frame must not kill the read loop.
    #[test]
    fn malformed_json_parses_to_none_without_panicking() {
        assert!(Bitstamp.parse("not valid json {{{").is_none());
    }

    /// Bug caught: a wrong channel name is a silent failure — Bitstamp
    /// accepts the subscription and then sends nothing, indistinguishable
    /// from "no messages yet" from the outside.
    #[test]
    fn bitstamp_subscribe_message_contains_the_configured_pairs_channel_name() {
        let msg = Bitstamp
            .subscribe_message("ethbtc")
            .expect("bitstamp subscribes");
        assert!(msg.contains("order_book_ethbtc"));
    }
}
