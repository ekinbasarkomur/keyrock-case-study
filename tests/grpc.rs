//! Integration test for the gRPC surface — real server, real client, real
//! TCP/HTTP2 connection, no mocking.

use std::sync::Arc;

use keyrock_case_study::exchange::Venue;
use keyrock_case_study::model::{Amount, Book, Price};
use keyrock_case_study::orderbook::Empty;
use keyrock_case_study::orderbook::Summary;
use keyrock_case_study::orderbook::orderbook_aggregator_client::OrderbookAggregatorClient;
use keyrock_case_study::{aggregator, server};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::TcpListenerStream;

/// Builds a 20-level `Book` offset by `price_offset`, so two calls produce
/// provably distinct books.
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
        parse_started_at: std::time::Instant::now(),
        parsed_at: std::time::Instant::now(),
    }
}

/// Catches a server that silently downgrades BookSummary to a single-shot
/// response. Reads exactly two consecutive messages off the stream,
/// proving it actually streams, and checks their content is real (10
/// bids/asks, positive spread, correct exchange label).
#[tokio::test]
async fn book_summary_streams_multiple_updates_not_a_single_shot_response() {
    // Port 0 — tests run concurrently, a fixed port would flake.
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding to an OS-assigned port cannot fail");
    let addr = listener
        .local_addr()
        .expect("a bound listener always has a local address");

    // Real pipeline: aggregator, merge, and watch channel — no mocking.
    let (tx, rx) = watch::channel::<Option<Arc<Summary>>>(None);
    let (feed_tx, feed_rx) = mpsc::channel::<(Venue, Book)>(32);
    tokio::spawn(aggregator::run(feed_rx, tx, "ethbtc".to_string()));

    // Sent before the client subscribes — WatchStream yields the current
    // value on first poll, so this is what the first read observes.
    feed_tx
        .send((Venue::Binance, book_with_offset(0.0)))
        .await
        .expect("aggregator's receiver is still alive");

    // serve_with_incoming reuses the already-bound listener, since the
    // port was claimed up front to read its OS-assigned number.
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

    // Two messages, not one — proves it's actually streaming.
    let first = stream
        .message()
        .await
        .expect("stream should not error")
        .expect("stream should yield a first Summary");

    // Sent only after the first read — watch only carries the latest
    // value, so sending both up front would collapse them into one.
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

/// Catches a BookSummary impl that moves the shared watch::Receiver
/// instead of cloning it — would break silently with a second subscriber.
/// Connects two clients, drops one, confirms the survivor still receives
/// a later publish.
#[tokio::test]
async fn a_second_client_keeps_streaming_after_the_first_one_leaves() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding to an OS-assigned port cannot fail");
    let addr = listener
        .local_addr()
        .expect("a bound listener always has a local address");

    let (tx, rx) = watch::channel::<Option<Arc<Summary>>>(None);
    let (feed_tx, feed_rx) = mpsc::channel::<(Venue, Book)>(32);
    tokio::spawn(aggregator::run(feed_rx, tx, "ethbtc".to_string()));

    feed_tx
        .send((Venue::Binance, book_with_offset(0.0)))
        .await
        .expect("aggregator's receiver is still alive");

    tokio::spawn(async move {
        server::router(rx)
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
    });

    let mut client_a = OrderbookAggregatorClient::connect(format!("http://{addr}"))
        .await
        .expect("server task is already spawned and listening on this address");
    let mut client_b = OrderbookAggregatorClient::connect(format!("http://{addr}"))
        .await
        .expect("a second independent connection to the same server should succeed");

    let mut stream_a = client_a
        .book_summary(Empty {})
        .await
        .expect("BookSummary call should succeed against a live server")
        .into_inner();
    let mut stream_b = client_b
        .book_summary(Empty {})
        .await
        .expect("BookSummary call should succeed against a live server")
        .into_inner();

    // Both must see the value published before either subscribed.
    stream_a
        .message()
        .await
        .expect("stream should not error")
        .expect("client A should receive the first Summary");
    stream_b
        .message()
        .await
        .expect("stream should not error")
        .expect("client B should receive the first Summary");

    // Client A leaves. Client B must be unaffected.
    drop(stream_a);
    drop(client_a);

    // Give the server a moment to notice before publishing the next update.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    feed_tx
        .send((Venue::Binance, book_with_offset(1.0)))
        .await
        .expect("aggregator's receiver is still alive");

    let second = stream_b
        .message()
        .await
        .expect("stream should not error")
        .expect("client B should still receive a second Summary after client A left");

    assert_eq!(second.bids.len(), 10);
    assert_eq!(second.asks.len(), 10);
}
