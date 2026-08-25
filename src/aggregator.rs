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
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use tracing::error;

use crate::exchange::Venue;
use crate::merge;
use crate::model::Book;
use crate::orderbook::Summary;

/// How long the aggregator waits for *any* venue to produce its first book
/// before treating the pair as unsupported and exiting. 60s, confirmed (see
/// spec.md 009-resilience, Open Questions #2): it covers one full backoff
/// cycle (`1+2+4+8+16+30 = 61s`), so a venue that is genuinely mid-reconnect
/// isn't killed before it gets a fair attempt — only a pair that has never
/// produced data from either venue trips this.
const GRACE: Duration = Duration::from_secs(60);

/// How often the aggregator's receive loop checks the grace period while
/// waiting for messages. Must be well under `GRACE` so the check fires
/// promptly once the deadline passes, even if no venue ever sends anything —
/// `rx.recv()` alone would block forever in that case, which is exactly the
/// bug this polling interval exists to close.
const GRACE_CHECK_INTERVAL: Duration = Duration::from_secs(5);

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
/// Returns in two cases: `rx.recv()` yields `None` (the feed's `Sender` was
/// dropped, nothing left to aggregate), or the grace period elapses with no
/// venue ever having produced a book (`pair` was never valid on either
/// venue — see `GRACE`'s doc comment). Both are fatal to the whole process
/// under the `JoinSet` supervisor in `src/main.rs`: a feed reconnecting
/// forever is not fatal, but the aggregator ending is — this is what makes
/// "no data at all" detectable and actionable rather than a silent hang.
pub async fn run(
    mut rx: mpsc::Receiver<(Venue, Book)>,
    tx: watch::Sender<Option<Arc<Summary>>>,
    pair: String,
) {
    let mut aggregator = Aggregator {
        venues: BTreeMap::new(),
    };
    let started_at = Instant::now();

    // A plain `while let Some(...) = rx.recv().await` loop can't notice time
    // passing while it's blocked waiting for a first message that never
    // arrives (Bitstamp accepts any channel name without validation, so an
    // invalid pair produces exactly that: two connected, permanently silent
    // feeds). `select!` against a periodic tick is what lets the grace-period
    // check fire even when nothing is ever received.
    let mut grace_check = tokio::time::interval(GRACE_CHECK_INTERVAL);
    grace_check.tick().await; // first tick fires immediately; consume it up front

    loop {
        tokio::select! {
            maybe_msg = rx.recv() => {
                match maybe_msg {
                    Some((venue, book)) => {
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
                            // `send` only fails once every receiver has been
                            // dropped — nothing left to publish to, not worth
                            // logging or propagating.
                            let _ = tx.send(Some(Arc::new(summary)));
                        }
                    }
                    None => return,
                }
            }
            _ = grace_check.tick() => {
                if past_grace(started_at, Instant::now(), aggregator.venues.is_empty()) {
                    error!(
                        pair = %pair,
                        "no data from any venue after {GRACE:?} — check the pair name"
                    );
                    return;
                }
            }
        }
    }
}

/// The pure decision behind piece 6: has the grace period elapsed with no
/// venue ever having produced a book? Takes `now` and `venues_empty` as
/// parameters, rather than reading `Instant::now()` or `self.venues`
/// internally, so it's testable with a fixed clock and no async runtime.
fn past_grace(started_at: Instant, now: Instant, venues_empty: bool) -> bool {
    now.duration_since(started_at) > GRACE && venues_empty
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
    use std::collections::BTreeSet;
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

    /// Piece 6's core guarantee: an empty venue map past the grace period
    /// must signal exit. Catches the silent-nothing-forever case — a bad
    /// pair name (e.g. Bitstamp accepting `xyzabc` without validating it)
    /// producing two permanently silent feeds and a process that never ends.
    #[test]
    fn empty_map_past_grace_exits() {
        let started_at = Instant::now();
        let now = started_at + GRACE + Duration::from_secs(1);

        assert!(past_grace(started_at, now, true));
    }

    /// Bug this catches: staleness filtering being correct in isolation
    /// (as in the two tests above, which only assert on `fresh_venues`'s
    /// return value) while never actually reaching what gets *published* —
    /// e.g. a future refactor that calls `merge::merge` on the unfiltered
    /// `venues` map by mistake instead of on `fresh_venues`'s output. Drives
    /// the same `fresh_venues` + `merge::merge` sequence `run`'s loop body
    /// performs, against a real `watch::channel`, with a hand-advanced clock
    /// (see `fresh_venues`'s doc comment above for why `now` is a parameter
    /// rather than `Instant::now()`) — `tokio::time::pause` isn't available
    /// here (this crate's `tokio` dependency uses the `full` feature without
    /// `test-util`).
    ///
    /// CAUTION, load-bearing: every `.send()` below is immediately followed
    /// by a `.changed().await` before the next `.send()`. `watch` only ever
    /// holds the *latest* value — batching two sends before the first read
    /// would collapse them into one observable value, and the second read
    /// would then hang forever waiting for a change that already happened.
    /// This exact bug hit `005-aggregator`'s implementation for real (see
    /// `specs/005-aggregator/revisions.md` entry 3).
    #[tokio::test]
    async fn a_venue_going_stale_narrows_the_published_summary() {
        let (tx, mut rx) = watch::channel::<Option<Arc<Summary>>>(None);
        let mut venues: BTreeMap<Venue, VenueState> = BTreeMap::new();

        // Both venues report a book at the same instant.
        let now0 = Instant::now();
        venues.insert(
            Venue::Binance,
            VenueState {
                book: empty_book(),
                last_update: now0,
            },
        );
        venues.insert(
            Venue::Bitstamp,
            VenueState {
                book: empty_book(),
                last_update: now0,
            },
        );

        let fresh = fresh_venues(&venues, now0);
        let summary =
            merge::merge(&fresh).expect("both venues fresh, merge should produce a summary");
        tx.send(Some(Arc::new(summary)))
            .expect("receiver still alive");

        rx.changed().await.expect("sender still alive");
        let first = rx.borrow().clone().expect("a Summary was published");
        let exchanges_in_first: BTreeSet<&str> = first
            .bids
            .iter()
            .chain(first.asks.iter())
            .map(|level| level.exchange.as_str())
            .collect();
        assert!(exchanges_in_first.contains("binance"));
        assert!(exchanges_in_first.contains("bitstamp"));

        // Advance the clock past Binance's 1.5s staleness threshold, but
        // stay under Bitstamp's 8s one — and only refresh Bitstamp's
        // last_update, mirroring "Bitstamp keeps publishing, Binance goes
        // quiet."
        let now1 = now0 + Duration::from_millis(1_600);
        venues.insert(
            Venue::Bitstamp,
            VenueState {
                book: empty_book(),
                last_update: now1,
            },
        );

        let fresh = fresh_venues(&venues, now1);
        let summary = merge::merge(&fresh).expect("bitstamp alone should still merge");
        tx.send(Some(Arc::new(summary)))
            .expect("receiver still alive");

        rx.changed().await.expect("sender still alive");
        let second = rx.borrow().clone().expect("a second Summary was published");
        let exchanges_in_second: BTreeSet<&str> = second
            .bids
            .iter()
            .chain(second.asks.iter())
            .map(|level| level.exchange.as_str())
            .collect();
        assert_eq!(
            exchanges_in_second,
            BTreeSet::from(["bitstamp"]),
            "a stale Binance must not appear in the published Summary once it's excluded from the merge"
        );
    }
}
