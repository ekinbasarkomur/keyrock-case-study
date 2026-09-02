//! Per-exchange feed code, plus the `Exchange` trait that abstracts over
//! what varies per venue so `src/feed.rs` can drive any venue with one
//! generic loop.

use std::fmt;
use std::time::Duration;

use crate::model::Book;

pub mod binance;
pub mod bitstamp;
pub mod kraken;

/// Which venue a `Book` or published `Level` came from. An enum, not a
/// string, so adding a venue is a compile error everywhere it needs
/// updating. Declaration order matters — it's `BTreeMap<Venue, _>`'s
/// iteration order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Venue {
    Binance,
    Bitstamp,
    Kraken,
}

impl fmt::Display for Venue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Venue::Binance => write!(f, "binance"),
            Venue::Bitstamp => write!(f, "bitstamp"),
            Venue::Kraken => write!(f, "kraken"),
        }
    }
}

impl Venue {
    /// `(capacity, tokens_per_second)` for this venue's reconnect token
    /// bucket — an absolute ceiling on connection attempts.
    pub fn connect_rate(self) -> (f64, f64) {
        match self {
            // Binance: 300 attempts/5min documented, ~1 token/sec. Capacity
            // kept small (5) so a burst can't spend the whole allowance.
            Venue::Binance => (5.0, 1.0),
            // Bitstamp/Kraken: no documented limit, conservative guess.
            Venue::Bitstamp => (5.0, 0.5),
            Venue::Kraken => (5.0, 0.5),
        }
    }

    /// How long a venue may go silent before it's excluded from the merge.
    pub fn staleness_threshold(self) -> Duration {
        match self {
            // Binance pushes a full snapshot every ~100ms regardless of
            // change, so silence itself means dead. 1.5s covers a few
            // dropped frames without flapping.
            Venue::Binance => Duration::from_secs_f64(1.5),
            // Measured live 2026-08-24, ~5.25min: max gap 1.795s. Bitstamp
            // only pushes on change, so threshold is ~4x the observed max.
            Venue::Bitstamp => Duration::from_secs(8),
            // Measured live 2026-08-26, 300.6s: max gap 2.914s, same ~4x rule.
            Venue::Kraken => Duration::from_secs(12),
        }
    }
}

/// What varies per venue, as data rather than control flow. Synchronous on
/// purpose — the async driving loop lives once in `src/feed.rs::run_feed`,
/// shared by every venue.
pub trait Exchange {
    /// Which venue this implementation is.
    fn venue(&self) -> Venue;

    /// The websocket URL to connect to for `pair`.
    fn connect_url(&self, pair: &str) -> String;

    /// Message to send right after connecting, if this venue needs an
    /// explicit subscribe (Bitstamp does; Binance returns None).
    fn subscribe_message(&self, pair: &str) -> Option<String>;

    /// Parses a raw text message into a `Book`. `None`, never `Err`, for
    /// anything that isn't a book payload.
    fn parse(&self, raw: &str) -> Option<Book>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three venues' staleness thresholds must not collapse to one value.
    #[test]
    fn thresholds_differ_per_venue() {
        assert_ne!(
            Venue::Binance.staleness_threshold(),
            Venue::Bitstamp.staleness_threshold()
        );
        assert_ne!(
            Venue::Bitstamp.staleness_threshold(),
            Venue::Kraken.staleness_threshold()
        );
        assert_ne!(
            Venue::Binance.staleness_threshold(),
            Venue::Kraken.staleness_threshold()
        );
    }

    /// Venue's Display string ends up verbatim in the wire Level.exchange
    /// field — a casing change here silently changes what clients receive.
    #[test]
    fn venue_display_matches_the_wire_contracts_lowercase_strings() {
        assert_eq!(Venue::Binance.to_string(), "binance");
        assert_eq!(Venue::Bitstamp.to_string(), "bitstamp");
        assert_eq!(Venue::Kraken.to_string(), "kraken");
    }
}
