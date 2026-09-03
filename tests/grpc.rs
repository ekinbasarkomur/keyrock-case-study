//! Integration test for `src/server.rs`'s gRPC surface — real server, real
//! client, real TCP/HTTP2 connection, no mocking, per the standing
//! convention recorded in `specs/002-binance-feed/revisions.md` entry 3.

use std::sync::Arc;

use rust_crypto_orderbook::exchange::Venue;
use rust_crypto_orderbook::model::{Amount, Book, Price};
use rust_crypto_orderbook::orderbook::Empty;
use rust_crypto_orderbook::orderbook::Summary;
use rust_crypto_orderbook::orderbook::orderbook_aggregator_client::OrderbookAggregatorClient;
use rust_crypto_orderbook::{aggregator, server};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::TcpListenerStream;

/// Builds a 20-level `Book` whose bid/ask prices are offset by `price_offset`
/// from a fixed base — used to hand-build two *distinct* books so the two
/// published `Summary` values are provably different, not two sends of the
/// same content.
fn book_with_offset(price_offset: f64) -> Book {
    let bids = (0..20)
        .map(|i| {
            let price = Price::parse(&format!("{:.8}", 1.0 + price_offset - i as f64 * 0.01))
                .expect("well-formed decimal string");
            let amount = Amount::parse("1.00000000").expect("well-formed decimal string");
            (price, amount)
        })
        .collect();
    let asks = (0..20)
        .map(|i| {
            let price = Price::parse(&format!("{:.8}", 1.01 + price_offset + i as f64 * 0.01))
                .expect("well-formed decimal string");
            let amount = Amount::parse("1.00000000").expect("well-formed decimal string");
            (price, amount)
        })
        .collect();
    Book {
        bids,
        asks,
        last_update_id: 1,
    }
}

/// Bug this catches: a server that silently downgraded `BookSummary` from a
/// real stream to a single-shot response (e.g. an accidental `.take(1)`, or
/// a future that resolves and drops the connection after the first message)
/// would still pass a test that only reads one message. Reading exactly two
/// consecutive messages off the stream is the one thing that actually
/// exercises the schema's `returns (stream Summary)` contract rather than
/// just "the RPC call returned something." The content assertions on both
/// messages (10 bids, 10 asks, positive spread, `exchange == "binance"`)
/// drive the real `aggregator` -> `summarise` -> `watch` pipeline with two
/// distinct hand-built `Book`s (real code, no mock), catching a wrong shape,
/// a crossed/zero spread, or a stale `"fake"` label surviving a forgotten
/// `run_fake_writer` deletion — not just "did anything arrive."
#[tokio::test]
async fn book_summary_streams_multiple_updates_not_a_single_shot_response() {
    // Port 0 — never a fixed port. `cargo test` runs tests in the same
    // binary concurrently, so a fixed port is a flake waiting to happen
    // against another test in the same run.
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding to an OS-assigned port cannot fail");
    let addr = listener
        .local_addr()
        .expect("a bound listener always has a local address");

    // Real pipeline: the aggregator task reads (Venue, Book) pairs off the
    // same bounded mpsc `main.rs` uses, calls the real `summarise`, and
    // writes into the same watch channel `src/server.rs` streams from — no
    // mock of any of those three pieces.
    let (tx, rx) = watch::channel::<Option<Arc<Summary>>>(None);
    let (feed_tx, feed_rx) = mpsc::channel::<(Venue, Book)>(32);
    tokio::spawn(aggregator::run(feed_rx, tx));

    // Send the first book before the client subscribes — `WatchStream`
    // yields whatever the current value is on first poll, so this is what
    // the client's first read below observes.
    feed_tx
        .send((Venue::Binance, book_with_offset(0.0)))
        .await
        .expect("aggregator's receiver is still alive");

    // Serve on the already-bound listener via `serve_with_incoming`, since
    // the port was claimed up front to read back its OS-assigned number —
    // `Router::serve(addr)` would need to rebind the same address itself.
    tokio::spawn(async move {
        server::router(rx)
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
    });

    let mut client = OrderbookAggregatorClient::connect(format!("http://{addr}"))
        .await
        .expect("server task is already spawned and listening on this address");

    let mut stream = client
        .book_summary(Empty {})
        .await
        .expect("BookSummary call should succeed against a live server")
        .into_inner();

    // Exactly two messages, before any assertion — one only proves the call
    // returned; two proves it's actually streaming.
    let first = stream
        .message()
        .await
        .expect("stream should not error")
        .expect("stream should yield a first Summary");

    // The second, distinct book is sent only *after* the first read — a
    // `watch` channel only carries the latest value, so publishing both
    // books before the client ever subscribes would collapse them into one
    // observable value and this second read would hang forever. Sending it
    // here, after the client has already seen the first value, is what
    // proves the aggregator publishes a genuine second update rather than
    // just replaying its startup state.
    feed_tx
        .send((Venue::Binance, book_with_offset(1.0)))
        .await
        .expect("aggregator's receiver is still alive");

    let second = stream
        .message()
        .await
        .expect("stream should not error")
        .expect("stream should yield a second Summary");

    for summary in [&first, &second] {
        assert_eq!(
            summary.bids.len(),
            10,
            "summarise() takes the top 10 bid levels from a 20-level book"
        );
        assert_eq!(
            summary.asks.len(),
            10,
            "summarise() takes the top 10 ask levels from a 20-level book"
        );
        assert!(
            summary.spread > 0.0,
            "the hand-built fixtures always have a positive best-ask-minus-best-bid spread"
        );
        assert!(
            summary
                .bids
                .iter()
                .chain(summary.asks.iter())
                .all(|level| level.exchange == "binance"),
            "every Level must carry the real venue label, never a leftover \"fake\" placeholder from run_fake_writer",
        );
    }
}
