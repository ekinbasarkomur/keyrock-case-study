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

use hdrhistogram::Histogram;
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

/// How often the periodic latency/throughput report (parse, merge+publish,
/// and total span p50/p99/p99.9, sustained update rate, duplicate
/// percentage) is logged. See `specs/011-measurement/spec.md`.
const REPORT_INTERVAL: Duration = Duration::from_secs(30);

/// Significant figures kept by each latency histogram. 3 is `hdrhistogram`'s
/// own "if you're not sure, use 3" recommendation — plenty of resolution for
/// logging p50/p99/p99.9 in whole microseconds off nanosecond samples.
const HISTOGRAM_SIGFIG: u8 = 3;

/// The last book received from one venue, plus when it arrived.
struct VenueState {
    book: Book,
    last_update: Instant,
}

/// Per-venue state the aggregator owns across the lifetime of the task. An
/// empty map is the "nothing to publish yet" state — no venue has sent a
/// message.
///
/// The three histograms, `update_count`, `duplicate_count`, and
/// `last_published` are the latency/dedup instrumentation added in
/// 011-measurement — same single-owner pattern as `venues`, no
/// `Arc<Mutex<_>>`, since this task is still the only writer and reader.
struct Aggregator {
    venues: BTreeMap<Venue, VenueState>,
    /// `book.parsed_at - book.parse_started_at`, recorded for every received
    /// book regardless of whether it's ultimately published. **Windowed, not
    /// lifetime**: `log_report` resets this after every read, so at any
    /// moment it holds only samples from the current, still-in-progress
    /// `REPORT_INTERVAL` window (see 013-rolling-histogram).
    parse_histogram: Histogram<u64>,
    /// `published_at - book.parsed_at` — includes queueing behind other
    /// `mpsc` messages and the `fresh_venues` staleness filter, not just
    /// `merge()`'s own comparisons (see spec.md Piece 1). Recorded only when
    /// a book actually reaches a `tx.send`. **Windowed, not lifetime** — see
    /// `parse_histogram`'s doc comment.
    merge_publish_histogram: Histogram<u64>,
    /// `published_at - book.parse_started_at`, a fresh `Instant::now()` call
    /// distinct from the one behind `merge_publish_histogram` — percentiles
    /// aren't additive, so this is a real third measurement, not derived by
    /// summing the other two. **Windowed, not lifetime** — see
    /// `parse_histogram`'s doc comment.
    total_histogram: Histogram<u64>,
    /// How many merges actually reached a `tx.send` (i.e. produced a
    /// `Summary` that differed from the last published one).
    update_count: u64,
    /// How many merges produced a `Summary` identical to the previously
    /// published one and were skipped rather than sent (see the ~30%
    /// measured threshold in `specs/011-measurement/spec.md` Piece 2).
    /// `update_count + duplicate_count` is the true total merge count —
    /// `update_count` alone no longer is, since a duplicate merge doesn't
    /// increment it.
    duplicate_count: u64,
    /// The last merged `Summary`, updated on every merge regardless of
    /// whether it was a duplicate — the comparison base for the next tick's
    /// duplicate check, and the contract a later phase's dedup skip depends
    /// on.
    last_published: Option<Arc<Summary>>,
}

impl Aggregator {
    fn new() -> Self {
        Aggregator {
            venues: BTreeMap::new(),
            // `Histogram::new` auto-resizes as values arrive, so there's no
            // upper bound to guess at construction time — a `expect` here is
            // provably local (a constant, valid sigfig can't fail).
            parse_histogram: Histogram::new(HISTOGRAM_SIGFIG)
                .expect("HISTOGRAM_SIGFIG is a constant, in-range value"),
            merge_publish_histogram: Histogram::new(HISTOGRAM_SIGFIG)
                .expect("HISTOGRAM_SIGFIG is a constant, in-range value"),
            total_histogram: Histogram::new(HISTOGRAM_SIGFIG)
                .expect("HISTOGRAM_SIGFIG is a constant, in-range value"),
            update_count: 0,
            duplicate_count: 0,
            last_published: None,
        }
    }
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
    let mut aggregator = Aggregator::new();
    let started_at = Instant::now();

    // A plain `while let Some(...) = rx.recv().await` loop can't notice time
    // passing while it's blocked waiting for a first message that never
    // arrives (Bitstamp accepts any channel name without validation, so an
    // invalid pair produces exactly that: two connected, permanently silent
    // feeds). `select!` against a periodic tick is what lets the grace-period
    // check fire even when nothing is ever received.
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

/// Records a book's parse-span sample, updates the venue's state, merges,
/// and — if the merge produces a `Summary` — compares it against the last
/// published one, updating the duplicate counter and `last_published`
/// either way. A duplicate is **not** sent (see the ~30%-measured decision
/// in `specs/011-measurement/spec.md` Piece 2); a genuine change is sent and
/// its merge+publish/total spans are recorded.
///
/// This is the exact `fresh_venues`/`merge::merge`/record sequence `run()`'s
/// loop body performs, factored out so it's callable with a hand-picked
/// `now` in tests — same reasoning as `fresh_venues` taking `now` as a
/// parameter rather than calling `Instant::now()` internally.
///
/// A book that never gets published — filtered out by `fresh_venues` before
/// reaching `merge`, `merge` returns `None`, or the merged `Summary` is a
/// duplicate of the last one — still contributes its parse-span sample
/// (recorded unconditionally, before the venue state is even updated), but
/// no sample to `merge_publish_histogram` or `total_histogram`: neither span
/// describes work that led to an actual publish, and a duplicate's
/// "publish" never happened at all.
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

    // Compare against the merged Summary, not a per-venue lastUpdateId — a
    // change 15 levels deep may not move the published top 10 (see spec.md
    // Piece 2). `last_published` updates on every merge, duplicate or not,
    // so the *next* comparison is always against the true last-merged
    // state, not the last-*sent* one.
    let is_duplicate = aggregator
        .last_published
        .as_deref()
        .is_some_and(|last| *last == summary);
    let summary = Arc::new(summary);
    aggregator.last_published = Some(Arc::clone(&summary));

    if is_duplicate {
        // Measured at ~36-49% across debug and release runs against real
        // Binance+Bitstamp connections — comfortably above the ~30%
        // threshold spec.md set for actually implementing the skip (see
        // specs/011-measurement/spec.md Piece 2 and README.md's
        // Measurement section for the numbers). `last_published` was
        // already updated above, so the *next* comparison is still against
        // the true last-merged state, not the last-*sent* one.
        aggregator.duplicate_count += 1;
        return;
    }

    // `send` only fails once every receiver has been dropped — nothing left
    // to publish to, not worth logging or propagating.
    let _ = tx.send(Some(summary));

    record_duration(
        &mut aggregator.merge_publish_histogram,
        Instant::now().duration_since(parsed_at),
        "merge+publish span",
    );
    // A fresh `Instant::now()` call, not `parsed_at`'s or the merge+publish
    // span's captured instant — percentiles aren't additive, so the total is
    // its own real measurement (see spec.md Piece 1).
    record_duration(
        &mut aggregator.total_histogram,
        Instant::now().duration_since(parse_started_at),
        "total span",
    );
    aggregator.update_count += 1;
}

/// Records `duration` (as nanoseconds) into `histogram`, logging rather than
/// silently swallowing the rare case `hdrhistogram` rejects a value (an
/// auto-resizing histogram only fails this on a value that overflows its
/// internal representation, not on ordinary latency samples).
fn record_duration(histogram: &mut Histogram<u64>, duration: Duration, label: &str) {
    if let Err(err) = histogram.record(duration.as_nanos() as u64) {
        tracing::warn!(%err, label, "failed to record latency sample");
    }
}

/// Logs the periodic latency/throughput report: p50/p99/p99.9 in
/// microseconds for the total, parse, and merge+publish spans, the
/// sustained published-update rate over the last `window`, and the running
/// duplicate percentage (guarded against a zero total-merge count, which
/// would otherwise divide by zero and log `NaN`).
///
/// **The three histograms are reset immediately after being read here**, so
/// each report describes only the samples recorded since the *previous*
/// report — a rolling ~`REPORT_INTERVAL` window, not the process's entire
/// lifetime. Without this, one bad tick early in a long-running process
/// would dominate p999 in every report for the rest of that process's life,
/// which is exactly what a periodic "how are things going right now" line
/// must not do. The reset happens last, after every value in this function
/// has already been read — resetting first would report an empty/degenerate
/// window instead of the one that just elapsed. `duplicate_pct` stays
/// cumulative on purpose (`update_count`/`duplicate_count` are running
/// totals used elsewhere, not report-local); only the latency histograms
/// are windowed.
fn log_report(aggregator: &mut Aggregator, update_count_at_last_report: u64, window: Duration) {
    let updates_this_window = aggregator.update_count - update_count_at_last_report;
    let update_rate_per_sec = updates_this_window as f64 / window.as_secs_f64();
    // `update_count + duplicate_count` is the true total merge count — a
    // duplicate no longer increments `update_count` now that it's skipped
    // rather than sent (see `duplicate_count`'s doc comment).
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
            parse_started_at: Instant::now(),
            parsed_at: Instant::now(),
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

    /// A book that reaches a real `tx.send` records exactly one sample in
    /// both the parse and merge+publish histograms (and the total
    /// histogram), and its `Summary` is actually observable on the `watch`
    /// channel — not just that the counters moved, but that the thing they
    /// describe really happened.
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

    /// The bug this packet fixes (013-rolling-histogram): before this, the
    /// three histograms were never reset, so a `log_report` call read
    /// percentiles over the process's entire lifetime rather than the
    /// window since the previous report. Records one sample, calls
    /// `log_report` (which must reset afterward), records a second, larger
    /// sample, and asserts the histogram now holds exactly that one new
    /// sample — not two, and not still dominated by the first.
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

        // A second, later book — a much larger parse span, so if the first
        // sample were still present the reported percentiles would blend
        // the two rather than reflecting only this one.
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

    /// A book that never gets published contributes its parse-span sample
    /// (recorded unconditionally) but no sample to `merge_publish_histogram`
    /// or `total_histogram`, and no duplicate-count increment.
    ///
    /// Note on why this isn't literally the `stale_venue_excluded_from_merge`
    /// shape reused verbatim: `record_and_publish` always inserts the
    /// venue/book it was just handed with `last_update: now` (the same `now`
    /// it then filters with), so the venue *currently* producing a book can
    /// never be excluded by its own staleness check — only some *other*,
    /// non-updating venue can go stale. This test reproduces that other
    /// venue (a stale Bitstamp entry, same fixture shape as the tests above)
    /// alongside an incoming Binance book that is itself empty on both
    /// sides, so `merge::merge` returns `None` once Bitstamp is filtered out
    /// — the "merge::merge returns None" half of the contract documented on
    /// `record_and_publish`, and the real way an incoming book ends up
    /// unpublished in this design.
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

    /// The dedup decision itself (Piece 2, spec.md — implemented after the
    /// measured duplicate rate came back at ~36-49%, comfortably above the
    /// ~30% threshold): a merge producing a `Summary` identical to the last
    /// one published is not sent. Publishes a book, reads it, then feeds a
    /// second, merge-identical book and confirms the `watch` receiver never
    /// wakes a second time within a short real-time window — the actual
    /// behavior the decision put in place, not just that `duplicate_count`
    /// moved. Reuses the interleaved-send-and-read discipline
    /// `a_venue_going_stale_narrows_the_published_summary` establishes:
    /// `watch` only ever holds the latest value, so the first send is read
    /// before the second book is fed in, never batched.
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

        // Same venue, same bids/asks (empty_book()'s contents) — merge()
        // reads only bids/asks, never the timestamp fields, so this second
        // book merges to a Summary identical to the first despite the
        // later `now2`.
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

    /// The real precondition the duplicate counter's comparison depends on:
    /// merging the same venues/book contents twice produces two `Summary`
    /// values that compare equal via `Summary`'s derived `PartialEq` — not a
    /// restatement of `prost`'s own derive, but confirmation that *this*
    /// project's usage (parsed-through prices/amounts, spread rounded once
    /// to the 8-decimal tick) never disagrees with itself between two
    /// merges of identical input.
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

    /// The other half of the same precondition: a genuinely different book
    /// (one changed price) produces a `Summary` that compares unequal —
    /// confirms the comparison isn't vacuously true (e.g. from a derive that
    /// silently ignored a field).
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
