//! Per-exchange feed code, plus the `Exchange` trait that abstracts over
//! what varies per venue (a connect URL, an optional subscribe message, and
//! a parse function) so `src/feed.rs` can drive any venue with one generic
//! loop instead of one hand-written loop per exchange.

use std::fmt;
use std::time::Duration;

use crate::model::Book;

pub mod binance;
pub mod bitstamp;

/// Which venue a `Book` (or a published `Level`) came from. An enum, not a
/// string, so adding another venue makes every place that needs updating
/// fail to compile instead of silently doing nothing. `Binance` must stay
/// the first variant — `BTreeMap<Venue, _>` iteration order (and this
/// step's "first entry wins" `summarise` selection) depends on declaration
/// order, not insertion order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Venue {
    Binance,
    Bitstamp,
}

impl fmt::Display for Venue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Venue::Binance => write!(f, "binance"),
            Venue::Bitstamp => write!(f, "bitstamp"),
        }
    }
}

impl Venue {
    /// `(capacity, tokens_per_second)` for this venue's reconnect token
    /// bucket (`src/feed.rs`'s `TokenBucket`) — an absolute ceiling on
    /// connection attempts, separate from and composed with the backoff
    /// delay. Lives here, next to where `staleness_threshold` will land in
    /// step 7's next piece, so every per-venue fact stays in one place and
    /// both `match`es stay exhaustive.
    pub fn connect_rate(self) -> (f64, f64) {
        match self {
            // Binance documents 300 connection attempts per 5 minutes per
            // IP, i.e. one token per second on average. Capacity is
            // deliberately small (5, not 300) — a capacity of 300 would let
            // the very first burst spend the entire five-minute allowance in
            // one go, which defeats the point of having a ceiling at all.
            Venue::Binance => (5.0, 1.0),
            // Bitstamp publishes no documented connection-rate limit. This
            // is a conservative guess, not a real figure — half of
            // Binance's refill rate and the same small capacity — stated
            // here plainly rather than presented as fact.
            Venue::Bitstamp => (5.0, 0.5),
        }
    }

    /// How long a venue may go silent before its last-known book is excluded
    /// from the merge (`src/aggregator.rs`'s pre-filter — `merge()` itself
    /// never sees a clock). Lives next to `connect_rate` for the same reason:
    /// every per-venue fact stays in one place and both `match`es stay
    /// exhaustive.
    pub fn staleness_threshold(self) -> Duration {
        match self {
            // Binance's depth20@100ms stream pushes a full snapshot every
            // ~100ms whether or not the book changed, so silence itself
            // means the connection is dead, not a quiet market. 1.5s is
            // ~15 missed snapshots' worth of grace — enough to absorb a
            // couple of dropped/delayed frames without flapping, tight
            // enough to exclude a genuinely dead feed within a couple of
            // seconds.
            Venue::Binance => Duration::from_secs_f64(1.5),
            // Measured live, 2026-08-24, ETHBTC, ~5.25 minutes: 792
            // messages, max observed gap 1.795s. Bitstamp only publishes on
            // change, so silence can mean a genuinely quiet market rather
            // than a dead connection — threshold set to ~4x the observed
            // max (not the low end of the 3-4x range) since a 5-minute
            // sample is short and a genuinely quiet moment could plausibly
            // produce a longer natural gap than this window happened to
            // catch. See README for the full measurement.
            Venue::Bitstamp => Duration::from_secs(8),
        }
    }
}

/// What varies per venue, as data rather than control flow. Deliberately
/// synchronous — an `async fn connect` per implementation would give each
/// venue its own driver loop, and step 6's reconnection handling would then
/// need to land in two places instead of one shared loop
/// (`src/feed.rs::run_feed`). `async fn` in a trait also can't be used
/// behind `dyn Trait` (stable since 1.75), which is moot here anyway since
/// every call site uses a concrete generic `E: Exchange`, never a trait
/// object — the venue set is compile-time-known.
pub trait Exchange {
    /// Which venue this implementation is.
    fn venue(&self) -> Venue;

    /// The websocket URL to connect to for `pair`.
    fn connect_url(&self, pair: &str) -> String;

    /// The message to send immediately after connecting, if this venue
    /// needs an explicit subscribe (Bitstamp does; Binance's subscription is
    /// baked into its URL, so it returns `None`).
    fn subscribe_message(&self, pair: &str) -> Option<String>;

    /// Parses a raw websocket text message into a `Book`. `None` — never
    /// `Err` — for anything that isn't a book payload, so a stray
    /// control/lifecycle message never kills the read loop.
    fn parse(&self, raw: &str) -> Option<Book>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Catches the two venues' staleness thresholds collapsing to one shared
    /// value (e.g. a copy-paste bug) — the whole point of per-venue
    /// thresholds is that Binance's silence means something different from
    /// Bitstamp's.
    #[test]
    fn thresholds_differ_per_venue() {
        assert_ne!(
            Venue::Binance.staleness_threshold(),
            Venue::Bitstamp.staleness_threshold()
        );
    }

    /// Bug this catches: `Venue`'s `Display` string ends up verbatim in the
    /// wire `Level.exchange` field (see `src/merge.rs`). Nothing before this
    /// asserted the exact casing — a `Display` change (e.g. `"Binance"`)
    /// would compile cleanly and pass every other existing test while
    /// silently changing what every gRPC client receives.
    #[test]
    fn venue_display_matches_the_wire_contracts_lowercase_strings() {
        assert_eq!(Venue::Binance.to_string(), "binance");
        assert_eq!(Venue::Bitstamp.to_string(), "bitstamp");
    }
}
