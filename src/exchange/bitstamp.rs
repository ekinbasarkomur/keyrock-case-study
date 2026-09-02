//! Bitstamp `order_book_<pair>` feed: connect URL, explicit subscribe
//! message, and pure `parse` function behind the `Exchange` trait. No
//! websocket connection opened here — see `src/feed.rs::run_feed`.

use crate::exchange::{Exchange, Venue};
use crate::model::{Amount, Book, Price};
use serde::Deserialize;

/// Bitstamp wraps every message in an envelope — only the "data" event
/// carries a book, the rest are lifecycle/control messages.
#[derive(Deserialize)]
struct Envelope<'a> {
    event: &'a str,
    #[serde(borrow, default)]
    data: Option<Data<'a>>,
}

/// The book payload inside a "data" event. Borrows bids/asks from the JSON
/// text, same as Binance's Depth20.
#[derive(Deserialize)]
struct Data<'a> {
    #[serde(borrow)]
    bids: Vec<[&'a str; 2]>,
    #[serde(borrow)]
    asks: Vec<[&'a str; 2]>,
}

/// Bitstamp's `Exchange` implementation. A unit struct — just carries the
/// trait impl.
pub struct Bitstamp;

impl Exchange for Bitstamp {
    fn venue(&self) -> Venue {
        Venue::Bitstamp
    }

    /// Bitstamp's endpoint has nothing pair-specific in the path — the pair
    /// only shows up in the subscribe channel name.
    fn connect_url(&self, _pair: &str) -> String {
        "wss://ws.bitstamp.net".to_string()
    }

    /// Unlike Binance, Bitstamp's subscription is per-connection — must be
    /// sent right after connecting or the connection stays silent.
    fn subscribe_message(&self, pair: &str) -> Option<String> {
        Some(format!(
            r#"{{"event":"bts:subscribe","data":{{"channel":"order_book_{}"}}}}"#,
            pair.to_lowercase()
        ))
    }

    /// Returns `None`, never `Err`, for anything that isn't a "data" event
    /// — malformed JSON or a lifecycle message.
    fn parse(&self, raw: &str) -> Option<Book> {
        let parse_started_at = std::time::Instant::now();

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
                    // Bitstamp has no lastUpdateId equivalent — 0 is a
                    // placeholder, not a real sequence number.
                    last_update_id: 0,
                    parse_started_at,
                    parsed_at: std::time::Instant::now(),
                })
            }
            "bts:subscription_succeeded" => {
                tracing::info!("bitstamp subscription succeeded");
                None
            }
            "bts:request_reconnect" => {
                tracing::info!("bitstamp requested a reconnect");
                None
            }
            "bts:error" => {
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

/// Converts `[price_str, amount_str]` pairs into levels. `None` if any
/// level fails to parse.
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

    // Captured from wss://ws.bitstamp.net (order_book_ethbtc) on 2026-08-23.
    // Trimmed to 5 bids/5 asks from the real 100-level payload.
    const DATA_FIXTURE: &str = r#"{"data":{"timestamp":"1787511693","microtimestamp":"1787511693313188","bids":[["0.03163789","3.42951262"],["0.03163505","0.61258313"],["0.03162919","1.02115457"],["0.03162384","0.04990000"],["0.03162273","0.34284793"]],"asks":[["0.03164587","0.61236387"],["0.03164684","3.42951262"],["0.03165086","1.02045379"],["0.03166236","0.04990000"],["0.03166726","2.04002053"]]},"channel":"order_book_ethbtc","event":"data"}"#;

    /// Catches reading fields at the wrong nesting level in the envelope.
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

    /// A benign lifecycle message with no bids/asks must not panic.
    #[test]
    fn bts_subscription_succeeded_parses_to_none_without_panicking() {
        assert!(
            Bitstamp
                .parse(r#"{"event":"bts:subscription_succeeded","channel":"order_book_ethbtc","data":{}}"#)
                .is_none()
        );
    }

    /// bts:request_reconnect must be recognized, not fall into "unknown".
    #[test]
    fn bts_request_reconnect_parses_to_none_without_panicking() {
        assert!(
            Bitstamp
                .parse(r#"{"event":"bts:request_reconnect","channel":"","data":{}}"#)
                .is_none()
        );
    }

    /// bts:error must not panic either — the log-level-warn case.
    #[test]
    fn bts_error_parses_to_none_without_panicking() {
        assert!(
            Bitstamp
                .parse(r#"{"event":"bts:error","channel":"","data":{"code":null,"message":"bad subscription"}}"#)
                .is_none()
        );
    }

    /// A malformed frame must convert to None, not kill the read loop.
    #[test]
    fn malformed_json_parses_to_none_without_panicking() {
        assert!(Bitstamp.parse("not valid json {{{").is_none());
    }

    /// A wrong channel name is a silent failure — Bitstamp accepts the
    /// subscription and sends nothing back.
    #[test]
    fn bitstamp_subscribe_message_contains_the_configured_pairs_channel_name() {
        let msg = Bitstamp
            .subscribe_message("ethbtc")
            .expect("bitstamp subscribes");
        assert!(msg.contains("order_book_ethbtc"));
    }

    /// One bad level rejects the whole message, rather than a short book.
    #[test]
    fn one_malformed_bid_level_rejects_the_whole_message() {
        let raw = r#"{"data":{"timestamp":"1","microtimestamp":"1","bids":[["0.03163789","3.42951262"],["not_a_number","1.00000000"]],"asks":[["0.03164587","0.61236387"]]},"channel":"order_book_ethbtc","event":"data"}"#;
        assert!(
            Bitstamp.parse(raw).is_none(),
            "one unparseable level must reject the entire book, not produce a short one"
        );
    }

    /// An empty side is legal and must not panic.
    #[test]
    fn empty_bids_side_parses_without_panicking() {
        let raw = r#"{"data":{"timestamp":"1","microtimestamp":"1","bids":[],"asks":[["0.03164587","0.61236387"]]},"channel":"order_book_ethbtc","event":"data"}"#;
        let book = Bitstamp
            .parse(raw)
            .expect("an empty side is still a valid book");
        assert!(book.bids.is_empty());
        assert_eq!(book.asks.len(), 1);
    }
}
