//! The real, two-book merge: combines every venue's book in the map into one
//! publishable `Summary`.
//!
//! No clock, no I/O, no channel reference — deliberately pure, per
//! `specs/005-aggregator/spec.md` decision 6, so it can be unit tested
//! against hand-built `Book` fixtures without faking a websocket. Three
//! layers, each separately testable: `merge()` handles the edge cases and
//! the spread; `merge_side()` walks all venues' cursors for one side;
//! `Side::better()`/`Side::levels()` hold the one rule that differs between
//! bids and asks.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::exchange::Venue;
use crate::model::{Amount, Book, Price};
use crate::orderbook::{Level, Summary};

const TOP_N: usize = 10;

/// Which side of the book is being merged. An enum, not a `bool` — a bool
/// parameter is silently invertible with no compile error and would produce
/// plausible-looking wrong numbers, the same silent-failure category this
/// project has designed against since step 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Bid,
    Ask,
}

impl Side {
    /// Returns `Less` when `a` should come before `b` in this side's
    /// ordering. Price direction depends on the side (asks ascending, bids
    /// descending); the amount tie-break (larger first) is identical on
    /// both sides — only the price rule inverts.
    fn better(self, a: &(Price, Amount), b: &(Price, Amount)) -> Ordering {
        let (a_price, a_amount) = a;
        let (b_price, b_amount) = b;
        let by_price = match self {
            Side::Ask => a_price.cmp(b_price),
            Side::Bid => b_price.cmp(a_price),
        };
        by_price.then(b_amount.cmp(a_amount))
    }

    /// Which of `book`'s two sorted lists this side reads.
    fn levels(self, book: &Book) -> &[(Price, Amount)] {
        match self {
            Side::Bid => &book.bids,
            Side::Ask => &book.asks,
        }
    }
}

/// Merges one side (bids or asks) across every venue in `venues` into the
/// top `TOP_N` levels, best first.
///
/// One `Peekable` cursor per venue, each already sorted (every venue hands
/// over an already-sorted book) — repeatedly takes whichever cursor's front
/// element is currently best, per `Side::better`, and advances only that
/// cursor. `filter_map` drops exhausted cursors with no bounds checks; the
/// `while out.len() < TOP_N` bound makes the cost independent of book depth.
/// A min-heap would beat this past four or five venues; at two, it's more
/// machinery than the problem has.
fn merge_side(venues: &BTreeMap<Venue, &Book>, side: Side) -> Vec<Level> {
    // `venues.iter()` walks the `BTreeMap` in `Venue`'s `Ord` order, so
    // `cursors` is built in that same order. `min_by` returns the *first* of
    // equal elements it sees, so a full price+amount tie between two venues
    // resolves deterministically to whichever venue sorts first under
    // `Venue`'s `Ord` (Binance) — not to run-to-run iteration order. This is
    // the entire reason step 4 chose `BTreeMap` over `HashMap`; a `HashMap`
    // here would make that tie flaky. Do not "simplify" this to `HashMap`.
    let mut cursors: Vec<_> = venues
        .iter()
        .map(|(venue, book)| (*venue, side.levels(book).iter().peekable()))
        .collect();

    let mut out = Vec::with_capacity(TOP_N);
    while out.len() < TOP_N {
        let best = cursors
            .iter_mut()
            .enumerate()
            .filter_map(|(i, (venue, cursor))| cursor.peek().map(|level| (i, *venue, **level)))
            .min_by(|(_, _, a), (_, _, b)| side.better(a, b));

        match best {
            None => break,
            Some((i, venue, (price, amount))) => {
                out.push(Level {
                    exchange: venue.to_string(),
                    price: price.into(),
                    amount: amount.into(),
                });
                cursors[i].1.next();
            }
        }
    }
    out
}

/// Merges every venue's book in `venues` into one `Summary`: the top 10 bids
/// (highest first), the top 10 asks (lowest first), and the spread between
/// them. Returns `None` if there's nothing publishable — no venues, or the
/// merged book is one-sided — rather than a fabricated `0.0` spread, which
/// would itself be a specific (and false) claim that the best bid and best
/// ask sit at the same price.
///
/// No venues, a one-sided merged book, and a single live venue are all
/// handled by the `?` on `.first()` below, with no explicit branch for any
/// of them.
pub fn merge(venues: &BTreeMap<Venue, &Book>) -> Option<Summary> {
    let bids = merge_side(venues, Side::Bid);
    let asks = merge_side(venues, Side::Ask);

    let (best_bid, best_ask) = (bids.first()?, asks.first()?);

    // A crossed book (best ask below best bid, so `spread < 0.0`) is
    // published as-is, not clamped or `abs()`-ed. Within one exchange this
    // can't happen — its own matching engine would have already crossed the
    // trade — but across two independently-matched venues it's routine, and
    // represents a real (if fleeting) arbitrage opportunity worth reporting
    // honestly rather than hiding.
    let spread = best_ask.price - best_bid.price;

    Some(Summary { spread, bids, asks })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn level(price: &str, amount: &str) -> (Price, Amount) {
        (
            Price::parse(price).expect("valid decimal string"),
            Amount::parse(amount).expect("valid decimal string"),
        )
    }

    /// `Side::Ask` prefers the lower price, `Side::Bid` prefers the higher —
    /// catches an inverted comparison in `Side::better`.
    #[test]
    fn ask_prefers_lower_price_bid_prefers_higher() {
        let cheap = level("0.0314", "1.0");
        let expensive = level("0.0316", "1.0");

        assert_eq!(Side::Ask.better(&cheap, &expensive), Ordering::Less);
        assert_eq!(Side::Bid.better(&expensive, &cheap), Ordering::Less);
    }

    /// Equal prices prefer the larger amount, on both sides — catches
    /// someone inverting the amount rule along with the price rule in the
    /// `Bid` arm.
    #[test]
    fn equal_price_prefers_larger_amount_on_both_sides() {
        let small = level("0.0314", "1.0");
        let large = level("0.0314", "2.0");

        assert_eq!(Side::Ask.better(&large, &small), Ordering::Less);
        assert_eq!(Side::Bid.better(&large, &small), Ordering::Less);
    }

    /// Two hand-built books across both venues produce the right top-10 on
    /// each side and the right spread.
    #[test]
    fn two_books_merge_into_correct_top_ten_and_spread() {
        let binance = book_from(
            &[("0.0314", "1.0"), ("0.0312", "1.0")],
            &[("0.0316", "1.0"), ("0.0318", "1.0")],
        );
        let bitstamp = book_from(
            &[("0.0315", "1.0"), ("0.0313", "1.0")],
            &[("0.0317", "1.0"), ("0.0319", "1.0")],
        );
        let venues = BTreeMap::from([(Venue::Binance, &binance), (Venue::Bitstamp, &bitstamp)]);

        let summary = merge(&venues).expect("two live venues yield Some(summary)");

        let bid_prices: Vec<f64> = summary.bids.iter().map(|l| l.price).collect();
        let ask_prices: Vec<f64> = summary.asks.iter().map(|l| l.price).collect();
        assert_eq!(bid_prices, vec![0.0315, 0.0314, 0.0313, 0.0312]);
        assert_eq!(ask_prices, vec![0.0316, 0.0317, 0.0318, 0.0319]);
        assert!((summary.spread - (0.0316 - 0.0315)).abs() < 1e-12);
    }

    /// Equal price and equal amount across venues resolves deterministically
    /// to Binance (the venue that sorts first under `Venue`'s `Ord`) —
    /// catches a regression back to `HashMap`, whose iteration order isn't
    /// fixed run to run.
    #[test]
    fn equal_price_and_amount_across_venues_resolves_deterministically() {
        let binance = book_from(&[("0.0314", "1.0")], &[("0.0316", "1.0")]);
        let bitstamp = book_from(&[("0.0314", "1.0")], &[("0.0316", "1.0")]);
        let venues = BTreeMap::from([(Venue::Binance, &binance), (Venue::Bitstamp, &bitstamp)]);

        let summary = merge(&venues).expect("two live venues yield Some(summary)");

        assert_eq!(summary.bids[0].exchange, "binance");
        assert_eq!(summary.asks[0].exchange, "binance");
    }

    /// A crossed book (one venue's best ask sits below the other venue's
    /// best bid) produces a negative spread and doesn't panic — catches an
    /// `abs()` or a clamp added by someone who mistakes this for a bug.
    #[test]
    fn crossed_book_produces_negative_spread_without_panicking() {
        let binance = book_from(&[("0.0320", "1.0")], &[("0.0330", "1.0")]);
        let bitstamp = book_from(&[("0.0310", "1.0")], &[("0.0315", "1.0")]);
        let venues = BTreeMap::from([(Venue::Binance, &binance), (Venue::Bitstamp, &bitstamp)]);

        let summary = merge(&venues).expect("two live venues yield Some(summary)");

        assert!(summary.spread < 0.0);
    }

    /// Only one venue present in the map still merges correctly — catches
    /// N=1 being treated as a special case that produces wrong or panicking
    /// output.
    #[test]
    fn single_venue_still_merges() {
        let binance = book_from(&[("0.0314", "1.0")], &[("0.0316", "1.0")]);
        let venues = BTreeMap::from([(Venue::Binance, &binance)]);

        let summary = merge(&venues).expect("one live venue yields Some(summary)");

        assert_eq!(summary.bids.len(), 1);
        assert_eq!(summary.asks.len(), 1);
        assert!((summary.spread - (0.0316 - 0.0314)).abs() < 1e-12);
    }

    /// An empty map returns `None` — catches a fabricated empty `Summary`
    /// being returned instead of `None`.
    #[test]
    fn no_venues_returns_none() {
        assert_eq!(merge(&BTreeMap::new()), None);
    }

    /// A book with fewer than 10 levels on a side returns what exists, not
    /// padded to 10 — catches invented price levels.
    #[test]
    fn six_levels_returns_six_not_padded_to_ten() {
        let bids: Vec<(&str, &str)> = (0..6).map(|_| ("0.0314", "1.0")).collect();
        let asks: Vec<(&str, &str)> = (0..6).map(|_| ("0.0316", "1.0")).collect();
        let binance = book_from(&bids, &asks);
        let venues = BTreeMap::from([(Venue::Binance, &binance)]);

        let summary = merge(&venues).expect("Some(book) yields Some(summary)");

        assert_eq!(summary.bids.len(), 6);
        assert_eq!(summary.asks.len(), 6);
    }
}
