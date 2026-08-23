//! Per-exchange feed code. No `Exchange` trait yet — deliberately deferred
//! until a second venue exists to show what actually varies (see
//! `specs/002-binance-feed/spec.md`).

use std::fmt;

pub mod binance;

/// Which venue a `Book` (or a published `Level`) came from. An enum, not a
/// string, so adding Bitstamp later makes every place that needs updating
/// fail to compile instead of silently doing nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Venue {
    Binance,
}

impl fmt::Display for Venue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Venue::Binance => write!(f, "binance"),
        }
    }
}
