//! Price/amount newtypes and the exchange-agnostic `Book`.
//!
//! `Price`/`Amount` wrap `f64` rather than a bare `f64` for two reasons: the
//! newtype boundary makes the compiler reject passing an `Amount` where a
//! `Price` is expected, and `Ord` via [`f64::total_cmp`] gives a well-defined
//! total order for sorting without an ad-hoc comparator at each call site.
//! Fixed-point was measured (see `specs/002-binance-feed/revisions.md`) and
//! isn't earning its complexity at this scale — it would be the right choice
//! in a system whose arithmetic accumulates across many updates.

use std::cmp::Ordering;
use std::fmt;

/// A price, parsed directly from the exchange's decimal string.
///
/// Distinct from [`Amount`] so the compiler rejects passing one where the
/// other is expected.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Price(f64);

/// An amount (order size), parsed directly from the exchange's decimal
/// string.
///
/// Distinct from [`Price`] so the compiler rejects passing one where the
/// other is expected.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Amount(f64);

impl Price {
    /// Parses an exchange decimal string (e.g. `"0.03150000"`). Returns
    /// `None` if the string isn't a valid number.
    pub fn from_str_price(s: &str) -> Option<Self> {
        s.parse::<f64>().ok().map(Self)
    }
}

impl Amount {
    /// Parses an exchange decimal string (e.g. `"5.00000000"`). Returns
    /// `None` if the string isn't a valid number.
    pub fn from_str_price(s: &str) -> Option<Self> {
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

/// An exchange-agnostic order book snapshot: bids and asks, each already
/// ordered best-first, plus the venue's own update sequence number.
///
/// Nothing Binance-specific lives here — that's `src/exchange/binance.rs`'s
/// job (landing in the next phase).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Book {
    pub bids: Vec<(Price, Amount)>,
    pub asks: Vec<(Price, Amount)>,
    pub last_update_id: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `"0.03150000"` parses to a `Price` that `Display`s back to the same
    /// decimal string.
    #[test]
    fn price_round_trips_through_display() {
        let price = Price::from_str_price("0.03150000").expect("valid decimal string");
        assert_eq!(price.to_string(), "0.03150000");
    }
}
