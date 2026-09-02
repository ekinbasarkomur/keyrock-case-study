//! The `OrderbookAggregator` gRPC service. `router()` builds the full set
//! of services without binding a port, so `main.rs` and `tests/grpc.rs`
//! share the same construction code.

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

/// Emitted by `build.rs` — the raw bytes tonic-reflection needs to answer
/// list/describe calls without a client-side .proto file.
const FILE_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/orderbook_descriptor.bin"));

/// Holds only the receiving half of the watch channel — never constructs
/// a Summary itself, that's the aggregator task's job.
struct AggregatorService {
    rx: watch::Receiver<Option<Arc<Summary>>>,
}

#[tonic::async_trait]
impl OrderbookAggregator for AggregatorService {
    // filter_map's closure type is unnameable, so erase it behind a
    // trait object. Pin because poll_next needs a stable address.
    type BookSummaryStream = Pin<Box<dyn Stream<Item = Result<Summary, Status>> + Send>>;

    async fn book_summary(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<Self::BookSummaryStream>, Status> {
        // filter_map drops the None case (nothing published yet) instead of
        // sending a fake zero-spread Summary. Arc::clone happens under the
        // watch's lock; the deep Summary clone happens after, outside it.
        let stream =
            WatchStream::new(self.rx.clone()).filter_map(|opt| opt.map(|arc| Ok((*arc).clone())));
        Ok(Response::new(Box::pin(stream)))
    }
}

/// Builds the aggregator and reflection services without binding a port.
///
/// # Panics
///
/// Panics if the embedded file descriptor set fails to parse — only
/// possible by editing build.rs or the proto itself.
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
