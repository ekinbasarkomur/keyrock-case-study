//! Per-exchange feed code, plus the `Exchange` trait that abstracts over
//! what varies per venue (a connect URL, an optional subscribe message, and
//! a parse function) so `src/feed.rs` can drive any venue with one generic
//! loop instead of one hand-written loop per exchange.

use std::fmt;

use crate::model::Book;

pub mod binance;
pub mod bitstamp;

/// Which venue a `Book` (or a published `Level`) came from. An enum, not a
/// string, so adding another venue makes every place that needs updating
/// fail to compile instead of silently doing nothing. `Binance` must stay
/// the first variant — `BTreeMap<Venue, _>` iteration order (and this
/// step's "first entry wins" `summarise` selection) depends on declaration
/// order, not insertion order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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
