//! The aggregator task: owns the last-seen book per venue, calls the pure
//! [`crate::merge::merge`], and publishes into the `watch` channel that
//! `src/server.rs` streams to gRPC clients.
//!
//! Single-owner task-local state, not `Arc<Mutex<_>>` — this task is the only
//! writer and reader of [`Aggregator`], so a lock would protect against a
//! concurrent access that structurally cannot happen (see
//! `specs/005-aggregator/spec.md` decision 2).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{mpsc, watch};

use crate::exchange::Venue;
use crate::merge;
use crate::model::Book;
use crate::orderbook::Summary;

/// The last book received from one venue, plus when it arrived.
struct VenueState {
    book: Book,
    last_update: Instant,
}

/// Per-venue state the aggregator owns across the lifetime of the task. An
/// empty map is the "nothing to publish yet" state — no venue has sent a
/// message.
struct Aggregator {
    venues: BTreeMap<Venue, VenueState>,
}

/// Receives `(Venue, Book)` pairs off the feed's `mpsc`, updates the
/// corresponding venue slot, and publishes a fresh [`Summary`] into `tx`
/// whenever [`merge::merge`] produces one.
///
/// Returns when `rx.recv()` yields `None` — the feed's `Sender` was dropped,
/// so there's nothing left to aggregate. No explicit shutdown signalling: the
/// caller's `select!` already ends the whole process when any one task ends,
/// the same supervision shape step 2 established.
pub async fn run(mut rx: mpsc::Receiver<(Venue, Book)>, tx: watch::Sender<Option<Arc<Summary>>>) {
    let mut aggregator = Aggregator {
        venues: BTreeMap::new(),
    };

    while let Some((venue, book)) = rx.recv().await {
        aggregator.venues.insert(
            venue,
            VenueState {
                book,
                last_update: Instant::now(),
            },
        );

        let fresh = fresh_venues(&aggregator.venues, Instant::now());
        let summary = merge::merge(&fresh);
        if let Some(summary) = summary {
            // `send` only fails once every receiver has been dropped —
            // nothing left to publish to, not worth logging or propagating.
            let _ = tx.send(Some(Arc::new(summary)));
        }
    }
}

/// Pre-filters `venues` down to the venues that are still fresh as of `now`,
/// producing exactly the `BTreeMap<Venue, &Book>` shape [`merge::merge`]
/// expects. This is where staleness lives — `merge()` itself stays pure, with
/// no clock and no notion of "stale" (see spec.md Piece 5).
///
/// `now` is a parameter, not `Instant::now()` called internally, for two
/// reasons: it's what makes this unit-testable with a fixed clock, and — the
/// reason that actually matters at runtime — every venue in the same pass
/// must be judged against the identical instant, not risk one venue reading
/// fresh and another stale within what should be the same tick.
fn fresh_venues(venues: &BTreeMap<Venue, VenueState>, now: Instant) -> BTreeMap<Venue, &Book> {
    venues
        .iter()
        .filter(|(venue, state)| {
            now.duration_since(state.last_update) < venue.staleness_threshold()
        })
        .map(|(venue, state)| (*venue, &state.book))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::model::{Amount, Price};

    /// A minimal but valid `Book` — only its presence/absence in the filtered
    /// map matters for these tests, not its contents.
    fn empty_book() -> Book {
        Book {
            bids: vec![(
                Price::parse("1.0").expect("valid literal"),
                Amount::parse("1.0").expect("valid literal"),
            )],
            asks: vec![(
                Price::parse("2.0").expect("valid literal"),
                Amount::parse("1.0").expect("valid literal"),
            )],
            last_update_id: 1,
        }
    }

    /// Catches staleness not being wired into the aggregator at all: a venue
    /// whose last update is past its threshold must not appear in what gets
    /// handed to `merge()`.
    #[test]
    fn stale_venue_excluded_from_merge() {
        let now = Instant::now();
        let mut venues = BTreeMap::new();
        venues.insert(
            Venue::Binance,
            VenueState {
                book: empty_book(),
                // Binance's threshold is 1.5s — 2s of silence is past it.
                last_update: now - Duration::from_secs(2),
            },
        );

        let fresh = fresh_venues(&venues, now);
        assert!(!fresh.contains_key(&Venue::Binance));
    }

    /// Catches a threshold set too tight (dropping everything): a venue
    /// updated just under its threshold must survive the filter.
    #[test]
    fn fresh_venue_included() {
        let now = Instant::now();
        let mut venues = BTreeMap::new();
        venues.insert(
            Venue::Binance,
            VenueState {
                book: empty_book(),
                // Binance's threshold is 1.5s — 1s of silence is still fresh.
                last_update: now - Duration::from_secs(1),
            },
        );

        let fresh = fresh_venues(&venues, now);
        assert!(fresh.contains_key(&Venue::Binance));
    }
}
