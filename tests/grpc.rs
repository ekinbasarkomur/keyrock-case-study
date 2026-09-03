//! Integration test for `src/server.rs`'s gRPC surface — real server, real
//! client, real TCP/HTTP2 connection, no mocking, per the standing
//! convention recorded in `specs/002-binance-feed/revisions.md` entry 3.

use rust_crypto_orderbook::orderbook::Empty;
use rust_crypto_orderbook::orderbook::orderbook_aggregator_client::OrderbookAggregatorClient;
use rust_crypto_orderbook::server;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_stream::wrappers::TcpListenerStream;

/// Bug this catches: a server that silently downgraded `BookSummary` from a
/// real stream to a single-shot response (e.g. an accidental `.take(1)`, or
/// a future that resolves and drops the connection after the first message)
/// would still pass a test that only reads one message. Reading exactly two
/// consecutive messages off the stream is the one thing that actually
/// exercises the schema's `returns (stream Summary)` contract rather than
/// just "the RPC call returned something." The content assertions on both
/// messages (10 bids, 10 asks, positive spread, `exchange == "fake"`) catch
/// a fake-generator regression that produces the wrong shape, a crossed/zero
/// spread, or a forgotten literal — not just "did anything arrive."
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

    // Real fake-writer task feeding the real watch channel — the exact same
    // production seam `main.rs` uses, not a test-only stand-in.
    let (tx, rx) = watch::channel(None);
    tokio::spawn(server::run_fake_writer(tx));

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
    // returned; two proves it's actually streaming. The fake writer ticks
    // once a second, so this can take a couple of seconds; that wait is
    // expected, not a bug to work around by speeding up the writer.
    let first = stream
        .message()
        .await
        .expect("stream should not error")
        .expect("stream should yield a first Summary");
    let second = stream
        .message()
        .await
        .expect("stream should not error")
        .expect("stream should yield a second Summary");

    for summary in [&first, &second] {
        assert_eq!(
            summary.bids.len(),
            10,
            "fake writer always emits 10 bid levels"
        );
        assert_eq!(
            summary.asks.len(),
            10,
            "fake writer always emits 10 ask levels"
        );
        assert!(
            summary.spread > 0.0,
            "fake writer's spread is always a small positive value"
        );
        assert!(
            summary
                .bids
                .iter()
                .chain(summary.asks.iter())
                .all(|level| level.exchange == "fake"),
            "every Level from this step's placeholder writer must be labeled \"fake\", never a real exchange name",
        );
    }
}
