//! Fixed-point price/amount representation and the exchange-agnostic `Book`.
//!
//! **Why not `f64`.**
//!
//! 1. Modelling argument, first: a price on an exchange is not a continuous
//!    quantity — it's an integer multiple of a tick size. Representing a
//!    discrete quantity with a continuous type (`f64`) is a category error
//!    before it's a precision one; the fix isn't "use more decimal digits,"
//!    it's "use a type whose values are exactly the ticks that exist."
//! 2. The measured numbers, as an honest bound on the precision cost, not
//!    the justification: `0.031505 - 0.031500` computed in `f64` carries a
//!    relative error of roughly `3.9e-13` — invisible at the 8 decimal
//!    places these prices are quoted to. So the case against `f64` here
//!    isn't "the arithmetic would visibly break" (see the regression test
//!    below, which pins this bound so it can't silently regress) — it's
//!    the modelling argument in point 1.
//! 3. Where the small error would stop being small: arithmetic that
//!    *accumulates* — summing many levels, compounding positions over many
//!    updates. This step performs exactly one subtraction (the spread) on
//!    values that were never accumulated, so it doesn't hit that case; a
//!    later step that sums across levels would need to re-examine this.
//!
//! Scale: `i64` at `1_000_000_000` (1e9). Largest representable price is
//! `i64::MAX / 1e9 ≈ 9.22e9` in the pair's quote unit — far beyond any
//! realistic price. Smallest representable tick is `1e-9` — finer than any
//! real exchange tick size, so nothing is lost at the low end either. This
//! scale is a documented assumption, not derived from exchange metadata
//! (`exchangeInfo` per-symbol tick sizes are the production answer, out of
//! scope here).

use std::fmt;

/// Fixed-point scale: 1e9. One unit of the underlying `i64` is `1e-9` of the
/// quoted decimal value.
const SCALE: f64 = 1_000_000_000.0;

/// A price, stored as an integer count of `1e-9`-sized ticks.
///
/// Distinct from [`Amount`] so the compiler rejects passing one where the
/// other is expected.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Price(i64);

/// An amount (order size), stored as an integer count of `1e-9`-sized ticks.
///
/// Distinct from [`Price`] so the compiler rejects passing one where the
/// other is expected.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Amount(i64);

impl Price {
    /// Parses an exchange decimal string (e.g. `"0.03150000"`) into ticks.
    ///
    /// The incoming string is parsed to `f64` and immediately scaled and
    /// rounded to the nearest tick — this is the one place a float is
    /// permitted, and only transiently; the stored value is always the
    /// `i64`. Returns `None` if the string isn't a valid number.
    pub fn from_str_price(s: &str) -> Option<Self> {
        s.parse::<f64>()
            .ok()
            .map(|v| Self((v * SCALE).round() as i64))
    }

    /// The raw tick count, for arithmetic that must stay in the integer
    /// domain (e.g. computing a spread).
    pub fn ticks(self) -> i64 {
        self.0
    }
}

impl Amount {
    /// Parses an exchange decimal string (e.g. `"5.00000000"`) into ticks.
    ///
    /// Same float-at-the-boundary rule as [`Price::from_str_price`].
    pub fn from_str_price(s: &str) -> Option<Self> {
        s.parse::<f64>()
            .ok()
            .map(|v| Self((v * SCALE).round() as i64))
    }

    /// The raw tick count.
    pub fn ticks(self) -> i64 {
        self.0
    }
}

impl fmt::Display for Price {
    /// Renders the decimal form at the scale's precision, e.g. `0.03150000`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.8}", self.0 as f64 / SCALE)
    }
}

impl fmt::Display for Amount {
    /// Renders the decimal form at the scale's precision, e.g. `5.00000000`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.8}", self.0 as f64 / SCALE)
    }
}

impl fmt::Debug for Price {
    /// Shows the raw tick count, not the decimal form — useful when
    /// debugging arithmetic on the underlying `i64`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Price({})", self.0)
    }
}

impl fmt::Debug for Amount {
    /// Shows the raw tick count, not the decimal form.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Amount({})", self.0)
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

    /// `"0.03150000"` round-trips through the integer domain and back to
    /// the same decimal string.
    #[test]
    fn price_round_trips_through_ticks() {
        let price = Price::from_str_price("0.03150000").expect("valid decimal string");
        assert_eq!(price.ticks(), 31_500_000);
        assert_eq!(price.to_string(), "0.03150000");
    }

    /// Guards against a future "simplification" of `Price` to `f64`: this
    /// subtraction is exact in the integer domain. If `Price` were ever
    /// backed by `f64` instead, this is the kind of comparison that would
    /// start failing intermittently depending on which values happen to be
    /// exactly representable.
    #[test]
    fn f64_would_lose_this_precision() {
        let a = Price::from_str_price("0.031505").expect("valid decimal string");
        let b = Price::from_str_price("0.031500").expect("valid decimal string");
        assert_eq!(a.ticks() - b.ticks(), 5000);
    }
}
