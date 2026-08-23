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
    // Unread this step — step 6 adds the staleness check that reads this to
    // exclude a venue whose last update is too old from the merge. Written
    // now so that step doesn't also need to add the field.
    #[allow(dead_code, reason = "read starting step 6's staleness check")]
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

        let venues: BTreeMap<Venue, &Book> = aggregator
            .venues
            .iter()
            .map(|(v, s)| (*v, &s.book))
            .collect();
        let summary = merge::merge(&venues);
        if let Some(summary) = summary {
            // `send` only fails once every receiver has been dropped —
            // nothing left to publish to, not worth logging or propagating.
            let _ = tx.send(Some(Arc::new(summary)));
        }
    }
}
