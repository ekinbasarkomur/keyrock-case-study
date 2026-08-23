//! The `OrderbookAggregator` gRPC service, plus the seam that both
//! `main.rs` (phase 3) and `tests/grpc.rs` (phase 4) build on: `router()`
//! constructs the full set of registered services without binding to a
//! port, so both callers share the exact same construction code and differ
//! only in what address they serve on.
//!
//! The watch channel is fed by the real aggregator task (`src/aggregator.rs`,
//! step 3 of the build order) — step 2's placeholder `run_fake_writer` has
//! been deleted.

use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::watch;
use tokio_stream::wrappers::WatchStream;
use tokio_stream::{Stream, StreamExt};
use tonic::{Request, Response, Status};

use crate::orderbook::orderbook_aggregator_server::{
    OrderbookAggregator, OrderbookAggregatorServer,
};
use crate::orderbook::{Empty, Summary};

/// Emitted by `build.rs` alongside the generated Rust types — the raw bytes
/// `tonic-reflection` needs to answer `list`/`describe` calls without a
/// client-side `.proto` file. Same `OUT_DIR` derivation `build.rs` used to
/// write it, so the two stay in sync automatically.
const FILE_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/orderbook_descriptor.bin"));

/// The `OrderbookAggregator` implementation. Holds only the receiving half
/// of the watch channel — it never constructs a `Summary` itself, that's
/// the aggregator task's (`src/aggregator.rs`) job.
struct AggregatorService {
    rx: watch::Receiver<Option<Arc<Summary>>>,
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
        // `Arc::clone` (cheap, atomic) happens under `WatchStream`'s internal
        // lock as it reads the current value; the deep `Summary` clone
        // (`tonic` needs a `Summary` by value, not an `Arc<Summary>`) happens
        // afterward, on the already-cloned `Arc`, outside that lock — per
        // spec.md decision 5, so a slow subscriber's deep clone never blocks
        // another subscriber's read of the current value.
        let stream =
            WatchStream::new(self.rx.clone()).filter_map(|opt| opt.map(|arc| Ok((*arc).clone())));
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
pub fn router(rx: watch::Receiver<Option<Arc<Summary>>>) -> tonic::transport::server::Router {
    let aggregator = AggregatorService { rx };

    let reflection = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(FILE_DESCRIPTOR_SET)
        .build_v1()
        .expect("embedded file descriptor set is produced by build.rs at compile time");

    tonic::transport::Server::builder()
        .add_service(OrderbookAggregatorServer::new(aggregator))
        .add_service(reflection)
}
