//! Price/amount newtypes and the exchange-agnostic `Book`.
//!
//! `Price`/`Amount` wrap `f64` so the compiler rejects mixing them up, and
//! `Ord` via `f64::total_cmp` gives a well-defined sort order.

use std::cmp::Ordering;
use std::fmt;
use std::time::Instant;

/// A price, parsed directly from the exchange's decimal string.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Price(f64);

/// An amount (order size), parsed directly from the exchange's decimal
/// string.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Amount(f64);

impl Price {
    /// Parses an exchange decimal string (e.g. `"0.03150000"`). Returns
    /// `None` if the string isn't a valid number.
    pub fn parse(s: &str) -> Option<Self> {
        s.parse::<f64>().ok().map(Self)
    }
}

impl Amount {
    /// Parses an exchange decimal string (e.g. `"5.00000000"`). Returns
    /// `None` if the string isn't a valid number.
    pub fn parse(s: &str) -> Option<Self> {
        s.parse::<f64>().ok().map(Self)
    }
}

impl Eq for Price {}

impl PartialOrd for Price {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Price {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl Eq for Amount {}

impl PartialOrd for Amount {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Amount {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl fmt::Display for Price {
    /// Renders to 8 decimal places, e.g. `0.03150000`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.8}", self.0)
    }
}

impl fmt::Display for Amount {
    /// Renders to 8 decimal places, e.g. `5.00000000`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.8}", self.0)
    }
}

// The gRPC schema fixes Level.price/amount as double — this is the one
// conversion point where Price/Amount become a bare f64.
impl From<Price> for f64 {
    fn from(price: Price) -> Self {
        price.0
    }
}

impl From<Amount> for f64 {
    fn from(amount: Amount) -> Self {
        amount.0
    }
}

/// An exchange-agnostic order book snapshot: bids and asks, each already
/// ordered best-first, plus the venue's own update sequence number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Book {
    pub bids: Vec<(Price, Amount)>,
    pub asks: Vec<(Price, Amount)>,
    pub last_update_id: u64,
    /// When parsing began — stamped before serde_json::from_str runs.
    pub parse_started_at: Instant,
    /// When parsing succeeded — stamped just before returning Some(Book).
    pub parsed_at: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `"0.03150000"` parses to a `Price` that `Display`s back to the same
    /// decimal string.
    #[test]
    fn price_round_trips_through_display() {
        let price = Price::parse("0.03150000").expect("valid decimal string");
        assert_eq!(price.to_string(), "0.03150000");
    }

    /// Out-of-order price strings sort ascending by value under `.sort()`.
    #[test]
    fn prices_sort_ascending_by_value() {
        let mut prices: Vec<Price> = ["0.0320", "0.0100", "0.0315", "0.0001"]
            .iter()
            .map(|s| Price::parse(s).expect("valid decimal string"))
            .collect();
        prices.sort();
        let sorted: Vec<String> = prices.iter().map(Price::to_string).collect();
        assert_eq!(
            sorted,
            vec!["0.00010000", "0.01000000", "0.03150000", "0.03200000"]
        );
    }

    /// Two Prices parsed from the same string compare Equal.
    #[test]
    fn equal_prices_compare_equal() {
        let a = Price::parse("0.03150000").expect("valid decimal string");
        let b = Price::parse("0.03150000").expect("valid decimal string");
        assert_eq!(a.cmp(&b), Ordering::Equal);
        assert!(!(a < b));
        assert!(!(b < a));
    }

    /// Deliberate: parse reports what the venue sent, doesn't validate
    /// domain rules like "price must be positive."
    #[test]
    fn a_negative_price_string_is_accepted_not_rejected() {
        assert!(Price::parse("-0.001").is_some());
    }
}
