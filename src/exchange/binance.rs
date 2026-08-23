//! Binance `depth20@100ms` feed: the connect URL and the pure `parse`
//! function. No websocket connection is opened here — that's Phase 3's
//! `src/main.rs` read loop; this file proves `parse` is correct against
//! fixtures first (see `specs/002-binance-feed/plan.md`).

use crate::model::{Book, Price};
use serde::Deserialize;

/// Binance's `depth20@100ms` payload shape. Only the fields this step needs
/// are declared — `serde` ignores anything else Binance sends.
#[derive(Deserialize)]
struct Depth20 {
    #[serde(rename = "lastUpdateId")]
    last_update_id: u64,
    bids: Vec<[String; 2]>,
    asks: Vec<[String; 2]>,
}

/// Builds the connect URL for a given trading pair, lowercased as the
/// endpoint requires (e.g. `"ETHBTC"` -> `.../ethbtc@depth20@100ms`).
pub fn connect_url(pair: &str) -> String {
    format!(
        "wss://stream.binance.com:9443/ws/{}@depth20@100ms",
        pair.to_lowercase()
    )
}

/// Parses a raw websocket text message into a [`Book`].
///
/// Returns `None` — never `Result` — for anything that isn't a recognizable
/// book payload: malformed JSON, a control/lifecycle message like
/// `{"e":"serverShutdown",...}`, or a price/amount string that doesn't
/// parse. A `?`-based `Result` here would let a stray non-book message kill
/// a future read loop; `Option` makes "that wasn't a book" a normal,
/// expected outcome (see `specs/002-binance-feed/spec.md`, "Proposed
/// Design").
pub fn parse(text: &str) -> Option<Book> {
    let raw: Depth20 = match serde_json::from_str(text) {
        Ok(raw) => raw,
        Err(err) => {
            tracing::debug!(%err, "not a depth20 payload, skipping");
            return None;
        }
    };

    let bids = parse_levels(&raw.bids)?;
    let asks = parse_levels(&raw.asks)?;

    Some(Book {
        bids,
        asks,
        last_update_id: raw.last_update_id,
    })
}

/// Converts `[price_str, amount_str]` pairs into `(Price, Amount)` levels.
/// Returns `None` if any level fails to parse, rather than silently
/// dropping a level and publishing a book with a hole in it.
fn parse_levels(levels: &[[String; 2]]) -> Option<Vec<(Price, crate::model::Amount)>> {
    levels
        .iter()
        .map(|[price, amount]| {
            let price = Price::from_str_price(price)?;
            let amount = crate::model::Amount::from_str_price(amount)?;
            Some((price, amount))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real-shaped Binance `depth20` payload (constructed to match the
    /// documented wire shape at
    /// <https://www.binance.com/en/binance-api> depth-stream sample, since a
    /// live capture wasn't practical for this fixture) with 20 bid levels
    /// and 20 ask levels.
    const DEPTH20_FIXTURE: &str = r#"{"lastUpdateId": 7723441, "bids": [["0.03150000", "1.00000000"], ["0.03149000", "1.25000000"], ["0.03148000", "1.50000000"], ["0.03147000", "1.75000000"], ["0.03146000", "2.00000000"], ["0.03145000", "2.25000000"], ["0.03144000", "2.50000000"], ["0.03143000", "2.75000000"], ["0.03142000", "3.00000000"], ["0.03141000", "3.25000000"], ["0.03140000", "3.50000000"], ["0.03139000", "3.75000000"], ["0.03138000", "4.00000000"], ["0.03137000", "4.25000000"], ["0.03136000", "4.50000000"], ["0.03135000", "4.75000000"], ["0.03134000", "5.00000000"], ["0.03133000", "5.25000000"], ["0.03132000", "5.50000000"], ["0.03131000", "5.75000000"]], "asks": [["0.03151000", "2.00000000"], ["0.03152000", "2.50000000"], ["0.03153000", "3.00000000"], ["0.03154000", "3.50000000"], ["0.03155000", "4.00000000"], ["0.03156000", "4.50000000"], ["0.03157000", "5.00000000"], ["0.03158000", "5.50000000"], ["0.03159000", "6.00000000"], ["0.03160000", "6.50000000"], ["0.03161000", "7.00000000"], ["0.03162000", "7.50000000"], ["0.03163000", "8.00000000"], ["0.03164000", "8.50000000"], ["0.03165000", "9.00000000"], ["0.03166000", "9.50000000"], ["0.03167000", "10.00000000"], ["0.03168000", "10.50000000"], ["0.03169000", "11.00000000"], ["0.03170000", "11.50000000"]]}"#;

    #[test]
    fn parses_depth20_into_twenty_bids_and_twenty_asks_with_correct_values() {
        let book = parse(DEPTH20_FIXTURE).expect("valid depth20 payload");

        assert_eq!(book.bids.len(), 20);
        assert_eq!(book.asks.len(), 20);
        assert_eq!(book.last_update_id, 7_723_441);

        let (best_bid_price, best_bid_amount) = book.bids[0];
        assert_eq!(best_bid_price, Price::from_str_price("0.03150000").unwrap());
        assert_eq!(
            best_bid_amount,
            crate::model::Amount::from_str_price("1.00000000").unwrap()
        );

        let (best_ask_price, best_ask_amount) = book.asks[0];
        assert_eq!(best_ask_price, Price::from_str_price("0.03151000").unwrap());
        assert_eq!(
            best_ask_amount,
            crate::model::Amount::from_str_price("2.00000000").unwrap()
        );

        let (last_bid_price, last_bid_amount) = book.bids[19];
        assert_eq!(last_bid_price, Price::from_str_price("0.03131000").unwrap());
        assert_eq!(
            last_bid_amount,
            crate::model::Amount::from_str_price("5.75000000").unwrap()
        );

        let (last_ask_price, last_ask_amount) = book.asks[19];
        assert_eq!(last_ask_price, Price::from_str_price("0.03170000").unwrap());
        assert_eq!(
            last_ask_amount,
            crate::model::Amount::from_str_price("11.50000000").unwrap()
        );
    }

    #[test]
    fn server_shutdown_message_parses_to_none_without_panicking() {
        assert!(parse(r#"{"e":"serverShutdown","E":1234567890}"#).is_none());
    }

    #[test]
    fn malformed_json_parses_to_none_without_panicking() {
        assert!(parse("not valid json {{{").is_none());
    }
}
