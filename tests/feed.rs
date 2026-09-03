//! Integration tests for `run_feed`'s backoff, reconnect loop, and
//! per-connection subscribe — driven against a real local TcpListener
//! rather than a real exchange.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use rust_crypto_orderbook::exchange::{Exchange, Venue};
use rust_crypto_orderbook::feed::run_feed;
use rust_crypto_orderbook::model::Book;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// A fake `Exchange` that counts `subscribe_message` calls and points
/// `run_feed` at a local listener instead of a real venue.
struct LocalExchange {
    port: u16,
    subscribe_calls: Arc<AtomicU32>,
}

impl Exchange for LocalExchange {
    fn venue(&self) -> Venue {
        Venue::Binance
    }

    fn connect_url(&self, _pair: &str) -> String {
        format!("ws://127.0.0.1:{}", self.port)
    }

    /// Always subscribes so every reconnect is observable via the counter.
    fn subscribe_message(&self, _pair: &str) -> Option<String> {
        self.subscribe_calls.fetch_add(1, Ordering::SeqCst);
        Some("subscribe".to_string())
    }

    fn parse(&self, _raw: &str) -> Option<Book> {
        None
    }
}

/// Catches subscribe_message being hoisted outside the reconnect loop —
/// would silently break Bitstamp, whose subscription is per-connection.
/// Drives three real connect cycles and asserts it fires every time.
#[tokio::test]
async fn resubscribes_on_every_reconnect() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding to an OS-assigned port cannot fail");
    let addr = listener
        .local_addr()
        .expect("a bound listener always has a local address");

    // Complete the WS handshake, then close, forcing a reconnect — 3 times.
    let server = tokio::spawn(async move {
        for _ in 0..3 {
            let (stream, _) = listener.accept().await.expect("accept succeeds");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("ws handshake completes");
            let _ = futures_util::SinkExt::close(&mut ws).await;
        }
    });

    let subscribe_calls = Arc::new(AtomicU32::new(0));
    let exchange = LocalExchange {
        port: addr.port(),
        subscribe_calls: subscribe_calls.clone(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let feed = tokio::spawn(run_feed(exchange, "ethbtc".to_string(), tx));

    // run_feed never returns — timeout so a regression fails fast.
    tokio::time::timeout(Duration::from_secs(20), async {
        while subscribe_calls.load(Ordering::SeqCst) < 3 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("subscribe_message should have fired 3 times (initial connect + 2 reconnects) well within 20s of backoff");

    feed.abort();
    server.abort();

    assert_eq!(
        subscribe_calls.load(Ordering::SeqCst),
        3,
        "subscribe_message must fire exactly once per connection attempt, not once total"
    );
}

/// Catches a tight-spin reconnect loop with no backoff. Times the gap
/// between two connection attempts and asserts it's at least 0.5s (the
/// jittered lower bound of the 1s nominal backoff). No upper bound, so
/// jitter can't make this flaky.
#[tokio::test]
async fn reconnects_after_waiting_not_in_a_tight_spin() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding to an OS-assigned port cannot fail");
    let addr = listener
        .local_addr()
        .expect("a bound listener always has a local address");

    // Accept and close immediately, before any WS handshake — a simple
    // "connection failed" case. Records the accept time to measure the gap.
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("first accept succeeds");
        let t0 = Instant::now();
        drop(stream);

        let (stream2, _) = listener.accept().await.expect("second accept succeeds");
        let t1 = Instant::now();
        drop(stream2);

        (t0, t1)
    });

    let subscribe_calls = Arc::new(AtomicU32::new(0));
    let exchange = LocalExchange {
        port: addr.port(),
        subscribe_calls,
    };
    let (tx, _rx) = mpsc::channel(8);
    let feed = tokio::spawn(run_feed(exchange, "ethbtc".to_string(), tx));

    let (t0, t1) = tokio::time::timeout(Duration::from_secs(20), server)
        .await
        .expect("run_feed should come back for a second connection attempt well within 20s")
        .expect("server task should not panic");

    feed.abort();

    let gap = t1.duration_since(t0);
    assert!(
        gap >= Duration::from_millis(500),
        "expected at least ~0.5s between reconnect attempts (a tight-spin \
         reconnect loop would show a near-zero gap here), got {gap:?}"
    );
}
