//! The `OrderbookAggregator` gRPC service, plus the seam that both
//! `main.rs` (phase 3) and `tests/grpc.rs` (phase 4) build on: `router()`
//! constructs the full set of registered services without binding to a
//! port, so both callers share the exact same construction code and differ
//! only in what address they serve on.
//!
//! This phase's watch channel is fed by [`run_fake_writer`] — a
//! once-a-second placeholder producer. Step 3 of the build order deletes it
//! and points the same `watch::Sender` at the real aggregator instead; none
//! of the plumbing in this file changes when that happens.

use std::pin::Pin;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::interval;
use tokio_stream::wrappers::WatchStream;
use tokio_stream::{Stream, StreamExt};
use tonic::{Request, Response, Status};

use crate::orderbook::orderbook_aggregator_server::{
    OrderbookAggregator, OrderbookAggregatorServer,
};
use crate::orderbook::{Empty, Level, Summary};

/// Emitted by `build.rs` alongside the generated Rust types — the raw bytes
/// `tonic-reflection` needs to answer `list`/`describe` calls without a
/// client-side `.proto` file. Same `OUT_DIR` derivation `build.rs` used to
/// write it, so the two stay in sync automatically.
const FILE_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/orderbook_descriptor.bin"));

/// The `OrderbookAggregator` implementation. Holds only the receiving half
/// of the watch channel — it never constructs a `Summary` itself, that's
/// [`run_fake_writer`]'s (and, from step 3 on, the real aggregator's) job.
struct AggregatorService {
    rx: watch::Receiver<Option<Summary>>,
}

#[tonic::async_trait]
impl OrderbookAggregator for AggregatorService {
    // The generated trait's associated type requires `Stream<Item =
    // Result<Summary, Status>> + Send + 'static`. `WatchStream::new(rx)`
    // adapted with `.filter_map(...)` embeds an unnameable closure type in
    // its own type parameter, so the return position here erases it behind
    // a trait object — the same reason `Box<dyn Trait>` erases any other
    // per-call-site-varying type. `Pin` is required in addition because
    // `Stream::poll_next` needs a stable address across polls.
    type BookSummaryStream = Pin<Box<dyn Stream<Item = Result<Summary, Status>> + Send>>;

    async fn book_summary(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<Self::BookSummaryStream>, Status> {
        // The watch channel carries `Option<Summary>` because nothing exists
        // to publish before the first tick lands. `filter_map` discards the
        // `None` case entirely rather than substituting a default/zeroed
        // `Summary` — a zero-spread, empty-book message reads to a client as
        // a specific (false) claim about market state, not an honest "no
        // data yet." A client connecting before the first tick sees nothing
        // until the first `Some` arrives; one connecting after sees the
        // watch's current value immediately, since `WatchStream::new`
        // yields the current value on first poll rather than waiting for a
        // change.
        let stream = WatchStream::new(self.rx.clone()).filter_map(|opt| opt.map(Ok));
        Ok(Response::new(Box::pin(stream)))
    }
}

/// Builds the full set of registered gRPC services — the aggregator and
/// reflection — without binding to a port or serving. `main.rs` and
/// `tests/grpc.rs` both call this and only differ in the address they
/// `.serve(addr)` on, so a test proves the exact construction path the real
/// binary runs.
///
/// # Panics
///
/// Panics if the embedded file descriptor set (emitted by `build.rs` at
/// compile time, never runtime input) fails to parse — an invariant that
/// can only break by editing `build.rs` or the proto itself, not by
/// anything a caller passes in.
pub fn router(rx: watch::Receiver<Option<Summary>>) -> tonic::transport::server::Router {
    let aggregator = AggregatorService { rx };

    let reflection = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(FILE_DESCRIPTOR_SET)
        .build_v1()
        .expect("embedded file descriptor set is produced by build.rs at compile time");

    tonic::transport::Server::builder()
        .add_service(OrderbookAggregatorServer::new(aggregator))
        .add_service(reflection)
}

/// Placeholder producer for this step: once a second, builds a `Summary`
/// with exactly 10 `Level`s per side clustered around the ETHBTC scale
/// (`0.0315`) and a small positive spread, and sends it into `tx`. Every
/// `Level.exchange` is the literal `"fake"` — never `"binance"` — so
/// `grpcurl` output is unmistakably placeholder data, and so a later step
/// can assert `"fake"` never appears once this task is deleted and the
/// watch is fed by the real aggregator instead.
///
/// Structured as a standalone `async fn` (not spawned here) so `main.rs`
/// can `tokio::spawn` it directly, alongside the feed and server tasks,
/// under one `select!`.
pub async fn run_fake_writer(tx: watch::Sender<Option<Summary>>) {
    let mut ticker = interval(Duration::from_secs(1));
    loop {
        ticker.tick().await;

        let bids: Vec<Level> = (0..10)
            .map(|i| Level {
                exchange: "fake".to_string(),
                price: 0.0315 - (i as f64) * 0.0001,
                amount: 1.0 + (i as f64) * 0.1,
            })
            .collect();
        let asks: Vec<Level> = (0..10)
            .map(|i| Level {
                exchange: "fake".to_string(),
                price: 0.0316 + (i as f64) * 0.0001,
                amount: 1.0 + (i as f64) * 0.1,
            })
            .collect();
        // Best bid 0.0315, best ask 0.0316 — matches the fixed spacing
        // above exactly, rather than recomputing it from the vectors.
        let spread = 0.0316 - 0.0315;

        let summary = Summary { spread, bids, asks };

        // `send` only fails once every receiver (every connected client,
        // plus main's own held copy) has been dropped — nothing left to
        // publish to, not a condition worth logging or propagating.
        if tx.send(Some(summary)).is_err() {
            break;
        }
    }
}
