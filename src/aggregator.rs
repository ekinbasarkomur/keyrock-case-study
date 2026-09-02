//! Aggregator task: owns the last book per venue, calls `merge::merge`,
//! and publishes into the `watch` channel that `src/server.rs` streams.
//!
//! No `Arc<Mutex<_>>` — only this task reads/writes `Aggregator`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hdrhistogram::Histogram;
use tokio::sync::{mpsc, watch};
use tracing::error;

use crate::exchange::Venue;
use crate::merge;
use crate::model::Book;
use crate::orderbook::Summary;

/// Max wait for any venue's first book before exiting. Covers one full
/// backoff cycle (1+2+4+8+16+30 = 61s).
const GRACE: Duration = Duration::from_secs(60);

/// How often the grace period is checked while waiting for messages.
const GRACE_CHECK_INTERVAL: Duration = Duration::from_secs(5);

/// How often the latency/throughput report is logged.
const REPORT_INTERVAL: Duration = Duration::from_secs(30);

/// hdrhistogram's own recommended default when unsure.
const HISTOGRAM_SIGFIG: u8 = 3;

/// The last book received from one venue, plus when it arrived.
struct VenueState {
    book: Book,
    last_update: Instant,
}

/// Per-venue state plus latency/dedup counters. Single-owner, no
/// `Arc<Mutex<_>>`.
struct Aggregator {
    venues: BTreeMap<Venue, VenueState>,
    /// Parse duration per book. Reset every report tick (rolling window).
    parse_histogram: Histogram<u64>,
    /// Time from parsed to published. Reset every report tick.
    merge_publish_histogram: Histogram<u64>,
    /// Time from parse start to published. Reset every report tick.
    total_histogram: Histogram<u64>,
    /// Merges that were actually published.
    update_count: u64,
    /// Merges identical to the last publish, skipped instead of sent.
    duplicate_count: u64,
    /// Last merged summary, updated on every merge (duplicate or not).
    last_published: Option<Arc<Summary>>,
}

impl Aggregator {
    fn new() -> Self {
        Aggregator {
            venues: BTreeMap::new(),
            // HISTOGRAM_SIGFIG is a constant valid value, can't fail.
            parse_histogram: Histogram::new(HISTOGRAM_SIGFIG).expect("valid sigfig"),
            merge_publish_histogram: Histogram::new(HISTOGRAM_SIGFIG).expect("valid sigfig"),
            total_histogram: Histogram::new(HISTOGRAM_SIGFIG).expect("valid sigfig"),
            update_count: 0,
            duplicate_count: 0,
            last_published: None,
        }
    }
}

/// Receives `(Venue, Book)` off the feed's mpsc, updates venue state, and
/// publishes a fresh `Summary` whenever merge produces one. Returns when
/// the channel closes or the grace period elapses with no data at all.
pub async fn run(
    mut rx: mpsc::Receiver<(Venue, Book)>,
    tx: watch::Sender<Option<Arc<Summary>>>,
    pair: String,
) {
    let mut aggregator = Aggregator::new();
    let started_at = Instant::now();

    // select! against a periodic tick so the grace check fires even if
    // rx.recv() never resolves (e.g. an invalid pair on both venues).
    let mut grace_check = tokio::time::interval(GRACE_CHECK_INTERVAL);
    grace_check.tick().await; // first tick fires immediately; consume it up front

    let mut report_tick = tokio::time::interval(REPORT_INTERVAL);
    report_tick.tick().await; // first tick fires immediately; consume it up front
    let mut update_count_at_last_report: u64 = 0;

    loop {
        tokio::select! {
            maybe_msg = rx.recv() => {
                match maybe_msg {
                    Some((venue, book)) => {
                        record_and_publish(&mut aggregator, venue, book, &tx, Instant::now());
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
            _ = report_tick.tick() => {
                log_report(&mut aggregator, update_count_at_last_report, REPORT_INTERVAL);
                update_count_at_last_report = aggregator.update_count;
            }
        }
    }
}

/// Records the parse-span sample, updates venue state, merges, and — if the
/// result differs from the last published summary — sends it and records
/// its latency. A duplicate merge is not sent. `now` is a parameter so
/// tests can pick it.
fn record_and_publish(
    aggregator: &mut Aggregator,
    venue: Venue,
    book: Book,
    tx: &watch::Sender<Option<Arc<Summary>>>,
    now: Instant,
) {
    record_duration(
        &mut aggregator.parse_histogram,
        book.parsed_at.duration_since(book.parse_started_at),
        "parse span",
    );
    let parse_started_at = book.parse_started_at;
    let parsed_at = book.parsed_at;

    aggregator.venues.insert(
        venue,
        VenueState {
            book,
            last_update: now,
        },
    );

    let fresh = fresh_venues(&aggregator.venues, now);
    let Some(summary) = merge::merge(&fresh) else {
        return;
    };

    // Compare against the merged Summary, not per-venue lastUpdateId — a
    // change deep in the book may not move the published top 10.
    let is_duplicate = aggregator
        .last_published
        .as_deref()
        .is_some_and(|last| *last == summary);
    let summary = Arc::new(summary);
    aggregator.last_published = Some(Arc::clone(&summary));

    if is_duplicate {
        aggregator.duplicate_count += 1;
        return;
    }

    // send only fails once every receiver is dropped — nothing to log.
    let _ = tx.send(Some(summary));

    record_duration(
        &mut aggregator.merge_publish_histogram,
        Instant::now().duration_since(parsed_at),
        "merge+publish span",
    );
    // Fresh Instant::now() call — percentiles aren't additive, so total is
    // its own measurement, not a sum of the other two.
    record_duration(
        &mut aggregator.total_histogram,
        Instant::now().duration_since(parse_started_at),
        "total span",
    );
    aggregator.update_count += 1;
}

/// Records `duration` into `histogram`, logging (not swallowing) the rare
/// case a value is rejected.
fn record_duration(histogram: &mut Histogram<u64>, duration: Duration, label: &str) {
    if let Err(err) = histogram.record(duration.as_nanos() as u64) {
        tracing::warn!(%err, label, "failed to record latency sample");
    }
}

/// Logs p50/p99/p99.9 for total/parse/merge+publish spans, the update rate,
/// and duplicate percentage. Resets the histograms after reading them, so
/// each report is a rolling window, not the process's whole lifetime.
/// `duplicate_pct` stays cumulative — only the latency histograms window.
fn log_report(aggregator: &mut Aggregator, update_count_at_last_report: u64, window: Duration) {
    let updates_this_window = aggregator.update_count - update_count_at_last_report;
    let update_rate_per_sec = updates_this_window as f64 / window.as_secs_f64();
    let total_merges = aggregator.update_count + aggregator.duplicate_count;
    let duplicate_pct = if total_merges == 0 {
        0.0
    } else {
        aggregator.duplicate_count as f64 / total_merges as f64 * 100.0
    };

    let us = |ns: u64| ns as f64 / 1_000.0;

    tracing::info!(
        total_p50_us = us(aggregator.total_histogram.value_at_quantile(0.50)),
        total_p99_us = us(aggregator.total_histogram.value_at_quantile(0.99)),
        total_p999_us = us(aggregator.total_histogram.value_at_quantile(0.999)),
        parse_p50_us = us(aggregator.parse_histogram.value_at_quantile(0.50)),
        parse_p99_us = us(aggregator.parse_histogram.value_at_quantile(0.99)),
        parse_p999_us = us(aggregator.parse_histogram.value_at_quantile(0.999)),
        merge_publish_p50_us = us(aggregator.merge_publish_histogram.value_at_quantile(0.50)),
        merge_publish_p99_us = us(aggregator.merge_publish_histogram.value_at_quantile(0.99)),
        merge_publish_p999_us = us(aggregator.merge_publish_histogram.value_at_quantile(0.999)),
        update_rate_per_sec,
        duplicate_pct,
        "latency/throughput report"
    );

    aggregator.total_histogram.reset();
    aggregator.parse_histogram.reset();
    aggregator.merge_publish_histogram.reset();
}

/// Has the grace period elapsed with no venue ever producing a book?
fn past_grace(started_at: Instant, now: Instant, venues_empty: bool) -> bool {
    now.duration_since(started_at) > GRACE && venues_empty
}

/// Filters to venues still fresh as of `now`. This is where staleness
/// lives — `merge()` stays pure with no clock. `now` is a parameter so
/// every venue in one pass is judged against the same instant.
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

    /// A minimal valid `Book` for tests that only care about presence/absence.
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
            parse_started_at: Instant::now(),
            parsed_at: Instant::now(),
        }
    }

    /// A venue past its staleness threshold must not reach merge().
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

    /// A venue updated just under its threshold must survive the filter.
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

    /// An empty venue map past the grace period must signal exit.
    #[test]
    fn empty_map_past_grace_exits() {
        let started_at = Instant::now();
        let now = started_at + GRACE + Duration::from_secs(1);

        assert!(past_grace(started_at, now, true));
    }

    /// Confirms staleness actually reaches what gets published, not just
    /// `fresh_venues`'s return value in isolation.
    ///
    /// Each send below is read via `.changed().await` before the next send —
    /// `watch` only holds the latest value, so batching sends would collapse
    /// them and hang the second read.
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

        // Past Binance's 1.5s threshold, under Bitstamp's 8s one — only
        // Bitstamp refreshes, mirroring Binance going quiet.
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

    /// A published book records one sample in each histogram, and its
    /// Summary is actually observable on the watch channel.
    #[tokio::test]
    async fn a_published_book_records_both_parse_and_merge_publish_samples() {
        let (tx, mut rx) = watch::channel::<Option<Arc<Summary>>>(None);
        let mut aggregator = Aggregator::new();

        let now = Instant::now();
        let book = Book {
            parse_started_at: now - Duration::from_micros(37),
            parsed_at: now,
            ..empty_book()
        };

        record_and_publish(&mut aggregator, Venue::Binance, book, &tx, now);

        assert_eq!(aggregator.parse_histogram.len(), 1);
        assert_eq!(aggregator.merge_publish_histogram.len(), 1);
        assert_eq!(aggregator.total_histogram.len(), 1);
        assert_eq!(aggregator.update_count, 1);
        assert_eq!(aggregator.duplicate_count, 0);

        rx.changed().await.expect("sender still alive");
        assert!(
            rx.borrow().is_some(),
            "a Summary was actually published down the watch channel, not just counted"
        );
    }

    /// Regression: histograms used to never reset, so log_report read
    /// lifetime percentiles instead of the current window.
    #[test]
    fn a_histogram_reset_after_report_excludes_prior_samples() {
        let (tx, _rx) = watch::channel::<Option<Arc<Summary>>>(None);
        let mut aggregator = Aggregator::new();

        let now = Instant::now();
        let first_book = Book {
            parse_started_at: now - Duration::from_micros(37),
            parsed_at: now,
            ..empty_book()
        };
        record_and_publish(&mut aggregator, Venue::Binance, first_book, &tx, now);
        assert_eq!(aggregator.parse_histogram.len(), 1);

        log_report(&mut aggregator, 0, REPORT_INTERVAL);
        assert_eq!(
            aggregator.parse_histogram.len(),
            0,
            "log_report must reset the histogram after reading it"
        );

        // Much larger parse span — if the first sample survived, the
        // percentile would blend both instead of reflecting only this one.
        let now2 = now + Duration::from_secs(1);
        let second_book = Book {
            parse_started_at: now2 - Duration::from_millis(5),
            parsed_at: now2,
            ..empty_book()
        };
        record_and_publish(&mut aggregator, Venue::Binance, second_book, &tx, now2);

        assert_eq!(
            aggregator.parse_histogram.len(),
            1,
            "only the second sample should be present after the reset"
        );
        let p50_us = aggregator.parse_histogram.value_at_quantile(0.50) as f64 / 1_000.0;
        assert!(
            p50_us > 4_000.0,
            "reported p50 ({p50_us}us) should reflect the ~5ms second sample, \
             not a blend with the ~37us first one that was already reported and reset"
        );
    }

    /// An unpublished book still records its parse span, but no sample in
    /// the merge_publish/total histograms and no duplicate-count increment.
    /// Uses a stale Bitstamp entry alongside an empty incoming Binance book
    /// so merge() returns None once Bitstamp is filtered out.
    #[test]
    fn a_stale_book_records_no_merge_publish_or_total_sample() {
        let (tx, _rx) = watch::channel::<Option<Arc<Summary>>>(None);
        let mut aggregator = Aggregator::new();

        let now = Instant::now();
        aggregator.venues.insert(
            Venue::Bitstamp,
            VenueState {
                book: empty_book(),
                // Bitstamp's threshold is 8s — 9s of silence is past it.
                last_update: now - Duration::from_secs(9),
            },
        );

        let unpublishable_book = Book {
            bids: vec![],
            asks: vec![],
            last_update_id: 1,
            parse_started_at: now - Duration::from_micros(50),
            parsed_at: now,
        };

        record_and_publish(
            &mut aggregator,
            Venue::Binance,
            unpublishable_book,
            &tx,
            now,
        );

        assert_eq!(
            aggregator.parse_histogram.len(),
            1,
            "the parse span is recorded unconditionally, before merge is even attempted"
        );
        assert_eq!(aggregator.merge_publish_histogram.len(), 0);
        assert_eq!(aggregator.total_histogram.len(), 0);
        assert_eq!(aggregator.update_count, 0);
        assert_eq!(aggregator.duplicate_count, 0);
    }

    /// A merge identical to the last publish is not sent — the watch
    /// receiver must not wake a second time.
    #[tokio::test]
    async fn an_unchanged_tick_is_not_resent_down_the_watch_channel() {
        let (tx, mut rx) = watch::channel::<Option<Arc<Summary>>>(None);
        let mut aggregator = Aggregator::new();

        let now = Instant::now();
        let first_book = Book {
            parse_started_at: now - Duration::from_micros(10),
            parsed_at: now,
            ..empty_book()
        };
        record_and_publish(&mut aggregator, Venue::Binance, first_book, &tx, now);
        rx.changed().await.expect("sender still alive");
        assert!(rx.borrow().is_some(), "first Summary was published");

        // Same bids/asks as the first book — merge() ignores timestamps,
        // so this merges to an identical Summary despite the later now2.
        let now2 = now + Duration::from_millis(100);
        let second_book = Book {
            parse_started_at: now2 - Duration::from_micros(10),
            parsed_at: now2,
            ..empty_book()
        };
        record_and_publish(&mut aggregator, Venue::Binance, second_book, &tx, now2);

        assert_eq!(
            aggregator.duplicate_count, 1,
            "the second, identical merge must still be counted as a duplicate"
        );
        assert_eq!(
            aggregator.update_count, 1,
            "update_count must not increment for a skipped duplicate"
        );

        let woke_again = tokio::time::timeout(Duration::from_millis(50), rx.changed()).await;
        assert!(
            woke_again.is_err(),
            "a duplicate merge must not wake the watch receiver a second time"
        );
    }

    /// Merging identical input twice produces two Summary values that
    /// compare equal — the precondition the duplicate check relies on.
    #[test]
    fn two_structurally_identical_summaries_compare_equal() {
        let binance = empty_book();
        let bitstamp = empty_book();
        let venues: BTreeMap<Venue, &Book> =
            BTreeMap::from([(Venue::Binance, &binance), (Venue::Bitstamp, &bitstamp)]);

        let first = merge::merge(&venues).expect("two live venues yield Some(summary)");
        let second = merge::merge(&venues).expect("two live venues yield Some(summary)");

        assert_eq!(first, second);
    }

    /// A genuinely different book produces a Summary that compares unequal.
    #[test]
    fn a_changed_book_produces_a_summary_that_compares_unequal() {
        let binance = empty_book();
        let bitstamp = empty_book();
        let venues: BTreeMap<Venue, &Book> =
            BTreeMap::from([(Venue::Binance, &binance), (Venue::Bitstamp, &bitstamp)]);
        let before = merge::merge(&venues).expect("two live venues yield Some(summary)");

        let mut changed_binance = empty_book();
        changed_binance.bids[0].0 = Price::parse("1.5").expect("valid literal");
        let venues: BTreeMap<Venue, &Book> = BTreeMap::from([
            (Venue::Binance, &changed_binance),
            (Venue::Bitstamp, &bitstamp),
        ]);
        let after = merge::merge(&venues).expect("two live venues yield Some(summary)");

        assert_ne!(before, after);
    }
}
