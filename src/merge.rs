//! Pure summarisation of a single venue's book into a publishable `Summary`.
//!
//! No clock, no I/O, no channel reference — deliberately pure, per
//! `specs/005-aggregator/spec.md` decision 6, so it can be unit tested
//! against hand-built `Book` fixtures without faking a websocket. Named and
//! shaped for a future two-book signature (step 5's real `merge()`), not a
//! rename later.

use std::collections::BTreeMap;

use crate::exchange::Venue;
use crate::model::Book;
use crate::orderbook::{Level, Summary};

/// Takes the first 10 bids and first 10 asks from the map's first
/// (lowest-ordered) venue's book (Binance already hands over a sorted
/// `depth20` snapshot, so this is truncation, not sorting — the real
/// sort/merge work is step 5's), and computes the spread. Returns `None` if
/// the map is empty.
pub fn summarise(venues: &BTreeMap<Venue, &Book>) -> Option<Summary> {
    let (&venue, &book) = venues.iter().next()?;

    // A one-sided book has no publishable spread, so nothing gets
    // published — same reasoning as filtering `None` out of the watch
    // stream in step 2: a fabricated value (e.g. spread 0.0, which is
    // itself a specific claim that the best bid and best ask are at the
    // same price) is worse than an absent one, because a client can't tell
    // it apart from a real reading. Real Binance `depth20` data always
    // returns 20/20, so this branch is never expected to trigger; a
    // genuinely empty single-venue book is otherwise step 5's "one venue's
    // book empty" territory.
    let (best_bid, _) = book.bids.first()?;
    let (best_ask, _) = book.asks.first()?;
    let spread = f64::from(*best_ask) - f64::from(*best_bid);

    let to_level = |(price, amount): &(crate::model::Price, crate::model::Amount)| Level {
        exchange: venue.to_string(),
        price: f64::from(*price),
        amount: f64::from(*amount),
    };

    let bids: Vec<Level> = book.bids.iter().take(10).map(to_level).collect();
    let asks: Vec<Level> = book.asks.iter().take(10).map(to_level).collect();

    Some(Summary { spread, bids, asks })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Amount, Price};

    /// Builds a `Book` from parallel lists of `(price_str, amount_str)`
    /// pairs, in the order given — mirrors how a real parsed book already
    /// arrives sorted, so tests control order explicitly rather than relying
    /// on this helper to sort.
    fn book_from(bids: &[(&str, &str)], asks: &[(&str, &str)]) -> Book {
        let level = |(price, amount): &(&str, &str)| {
            (
                Price::parse(price).expect("valid decimal string"),
                Amount::parse(amount).expect("valid decimal string"),
            )
        };
        Book {
            bids: bids.iter().map(level).collect(),
            asks: asks.iter().map(level).collect(),
            last_update_id: 1,
        }
    }

    /// A 20-level book (10 bids, 10 asks would already be within budget, so
    /// use 20 on each side to actually exercise truncation) yields 10 bids,
    /// 10 asks, and the spread computed from the best bid/ask. Catches an
    /// off-by-one or missing truncation in the top-10 selection.
    #[test]
    fn summarise_on_a_twenty_level_book_returns_ten_bids_ten_asks_and_correct_spread() {
        let bids: Vec<(&str, &str)> = (0..20)
            .map(|i| match i {
                0 => ("0.0314", "1.0"),
                _ => ("0.0300", "1.0"),
            })
            .collect();
        let asks: Vec<(&str, &str)> = (0..20)
            .map(|i| match i {
                0 => ("0.0316", "1.0"),
                _ => ("0.0330", "1.0"),
            })
            .collect();
        let book = book_from(&bids, &asks);

        let summary = summarise(&BTreeMap::from([(Venue::Binance, &book)]))
            .expect("Some(book) yields Some(summary)");

        assert_eq!(summary.bids.len(), 10);
        assert_eq!(summary.asks.len(), 10);
        assert!((summary.spread - (0.0316 - 0.0314)).abs() < 1e-12);
    }

    /// Bids come back in the order given (descending by price, since that's
    /// how a real book already arrives), asks ascending — currently just
    /// truncation, but this locks the contract in before step 5's real
    /// `merge()` does actual ordering work.
    #[test]
    fn summarise_returns_bids_descending_by_price_and_asks_ascending() {
        let book = book_from(
            &[("0.0314", "1.0"), ("0.0313", "1.0"), ("0.0312", "1.0")],
            &[("0.0316", "1.0"), ("0.0317", "1.0"), ("0.0318", "1.0")],
        );

        let summary = summarise(&BTreeMap::from([(Venue::Binance, &book)]))
            .expect("Some(book) yields Some(summary)");

        let bid_prices: Vec<f64> = summary.bids.iter().map(|level| level.price).collect();
        let ask_prices: Vec<f64> = summary.asks.iter().map(|level| level.price).collect();
        assert!(bid_prices.windows(2).all(|pair| pair[0] > pair[1]));
        assert!(ask_prices.windows(2).all(|pair| pair[0] < pair[1]));
    }

    /// A 6-level book returns 6 levels per side, not padded to 10. Catches
    /// accidental zero-padding of a short book.
    #[test]
    fn summarise_on_a_six_level_book_returns_six_levels_per_side_not_padded_to_ten() {
        let bids: Vec<(&str, &str)> = (0..6).map(|_| ("0.0314", "1.0")).collect();
        let asks: Vec<(&str, &str)> = (0..6).map(|_| ("0.0316", "1.0")).collect();
        let book = book_from(&bids, &asks);

        let summary = summarise(&BTreeMap::from([(Venue::Binance, &book)]))
            .expect("Some(book) yields Some(summary)");

        assert_eq!(summary.bids.len(), 6);
        assert_eq!(summary.asks.len(), 6);
    }

    /// `summarise(&BTreeMap::new())` returns `None` — catches a panic or
    /// a synthesized-empty-summary bug on the "no data yet" path.
    #[test]
    fn summarise_with_no_book_returns_none() {
        assert_eq!(summarise(&BTreeMap::new()), None);
    }

    /// A one-sided book (one side has no levels) returns `None`, not a
    /// fabricated `0.0` spread — catches a regression back to publishing a
    /// specific-but-false market-state claim (spread `0.0` asserts the best
    /// bid and best ask are at the same price) instead of honestly
    /// reporting "nothing to publish."
    #[test]
    fn summarise_on_a_one_sided_book_returns_none() {
        let book = book_from(&[("0.0314", "1.0")], &[]);
        assert_eq!(summarise(&BTreeMap::from([(Venue::Binance, &book)])), None);
    }
}
