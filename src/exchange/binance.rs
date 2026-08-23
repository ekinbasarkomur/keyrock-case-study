//! Binance `depth20@100ms` feed: the connect URL and the pure `parse`
//! function. No websocket connection is opened here — that's Phase 3's
//! `src/main.rs` read loop; this file proves `parse` is correct against
//! fixtures first (see `specs/002-binance-feed/plan.md`).

use crate::model::{Book, Price};
use serde::Deserialize;

/// Binance's websocket host and port, named so `connect_url` and the proxy
/// `CONNECT` tunnel (see `src/main.rs`) share one source instead of each
/// hardcoding the pair separately.
pub const HOST: &str = "stream.binance.com";
pub const PORT: u16 = 9443;

/// Binance's `depth20@100ms` payload shape. Only the fields this step needs
/// are declared — `serde` ignores anything else Binance sends.
///
/// Borrows `bids`/`asks` straight out of the source JSON text instead of
/// allocating a `String` per price/amount (~107 allocations per message down
/// to ~27). The borrow must not outlive `parse()`: `Book` (what actually
/// survives past this function, down the `mpsc`, into the aggregator) holds
/// only `Price`/`Amount` — both `f64`-backed and `Copy` — never a `&str`, so
/// the source text can drop once `parse()` returns.
#[derive(Deserialize)]
struct Depth20<'a> {
    #[serde(rename = "lastUpdateId")]
    last_update_id: u64,
    #[serde(borrow)]
    bids: Vec<[&'a str; 2]>,
    #[serde(borrow)]
    asks: Vec<[&'a str; 2]>,
}

/// Builds the connect URL for a given trading pair, lowercased as the
/// endpoint requires (e.g. `"ETHBTC"` -> `.../ethbtc@depth20@100ms`).
pub fn connect_url(pair: &str) -> String {
    format!(
        "wss://{HOST}:{PORT}/ws/{}@depth20@100ms",
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
fn parse_levels(levels: &[[&str; 2]]) -> Option<Vec<(Price, crate::model::Amount)>> {
    levels
        .iter()
        .map(|[price, amount]| {
            let price = Price::parse(price)?;
            let amount = crate::model::Amount::parse(amount)?;
            Some((price, amount))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from wss://stream.binance.com:9443/ws/ethbtc@depth20@100ms
    // on 2026-08-23, via the project's own HTTP CONNECT proxy support.
    const DEPTH20_FIXTURE: &str = r#"{"lastUpdateId":9118724657,"bids":[["0.03144000","36.35500000"],["0.03143000","59.83450000"],["0.03142000","141.81680000"],["0.03141000","106.58830000"],["0.03140000","114.49470000"],["0.03139000","96.53310000"],["0.03138000","86.20680000"],["0.03137000","126.79540000"],["0.03136000","128.94000000"],["0.03135000","135.11290000"],["0.03134000","130.71860000"],["0.03133000","127.34300000"],["0.03132000","3.88700000"],["0.03131000","4.52020000"],["0.03130000","4.02660000"],["0.03129000","3.94970000"],["0.03128000","19.31970000"],["0.03127000","4.04340000"],["0.03126000","3.56970000"],["0.03125000","4.47450000"]],"asks":[["0.03145000","37.64560000"],["0.03146000","36.96670000"],["0.03147000","34.83630000"],["0.03148000","68.41130000"],["0.03149000","163.81090000"],["0.03150000","138.90970000"],["0.03151000","119.96530000"],["0.03152000","113.25420000"],["0.03153000","95.42750000"],["0.03154000","86.32880000"],["0.03155000","83.70930000"],["0.03156000","122.40940000"],["0.03157000","0.35420000"],["0.03158000","0.40210000"],["0.03159000","0.81870000"],["0.03160000","0.37230000"],["0.03161000","0.77060000"],["0.03162000","0.82900000"],["0.03163000","17.03620000"],["0.03164000","0.82870000"]]}"#;

    #[test]
    fn parses_depth20_into_twenty_bids_and_twenty_asks_with_correct_values() {
        let book = parse(DEPTH20_FIXTURE).expect("valid depth20 payload");

        assert_eq!(book.bids.len(), 20);
        assert_eq!(book.asks.len(), 20);
        assert_eq!(book.last_update_id, 9_118_724_657);

        let (best_bid_price, best_bid_amount) = book.bids[0];
        assert_eq!(best_bid_price, Price::parse("0.03144000").unwrap());
        assert_eq!(
            best_bid_amount,
            crate::model::Amount::parse("36.35500000").unwrap()
        );

        let (best_ask_price, best_ask_amount) = book.asks[0];
        assert_eq!(best_ask_price, Price::parse("0.03145000").unwrap());
        assert_eq!(
            best_ask_amount,
            crate::model::Amount::parse("37.64560000").unwrap()
        );

        let (last_bid_price, last_bid_amount) = book.bids[19];
        assert_eq!(last_bid_price, Price::parse("0.03125000").unwrap());
        assert_eq!(
            last_bid_amount,
            crate::model::Amount::parse("4.47450000").unwrap()
        );

        let (last_ask_price, last_ask_amount) = book.asks[19];
        assert_eq!(last_ask_price, Price::parse("0.03164000").unwrap());
        assert_eq!(
            last_ask_amount,
            crate::model::Amount::parse("0.82870000").unwrap()
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
