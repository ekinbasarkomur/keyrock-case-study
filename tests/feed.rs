//! Integration tests for `src/feed.rs`'s `run_feed` loop — the composition of
//! backoff, the reconnect loop, and the per-connection subscribe call, driven
//! against a real local `TcpListener` rather than a real exchange. See
//! `specs/010-test-gaps/spec.md`, tests 1 and 3, for why a local listener
//! (not a mock `Exchange::connect`) is the right harness: the `Exchange`
//! trait describes protocol data, not control flow, so making `connect()`
//! itself mockable would undo that separation.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use keyrock_case_study::exchange::{Exchange, Venue};
use keyrock_case_study::feed::run_feed;
use keyrock_case_study::model::Book;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// A fake `Exchange` whose only interesting behaviour is counting
/// `subscribe_message` calls — everything else is the minimum needed to
/// satisfy the trait and point `run_feed` at a local listener instead of a
/// real venue. `venue()` returns `Venue::Binance` purely so `connect_rate()`/
/// `staleness_threshold()` (methods on `Venue`, not on `Exchange`) resolve to
/// something; neither actually matters to these tests.
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

    /// Always subscribes (unlike the real Binance impl, which returns
    /// `None`) specifically so every reconnect is observable via the
    /// counter — that's the entire point of test 1 below.
    fn subscribe_message(&self, _pair: &str) -> Option<String> {
        self.subscribe_calls.fetch_add(1, Ordering::SeqCst);
        Some("subscribe".to_string())
    }

    fn parse(&self, _raw: &str) -> Option<Book> {
        None
    }
}

/// Bug this catches: a future refactor that moves the `subscribe_message`
/// call site outside `run_feed`'s reconnect loop (e.g. hoisting it to run
/// once before the loop starts, as an "optimization"). That would compile,
/// would pass every existing unit test (none of them drive a real reconnect
/// cycle), and would silently break Bitstamp — whose subscription is
/// per-connection, not baked into the URL like Binance's — the socket would
/// keep opening successfully and no data would ever arrive again. This test
/// drives three real connect cycles (initial connect + two forced
/// disconnects) against a real local listener that completes the WebSocket
/// handshake each time, and asserts `subscribe_message` fires on every one.
#[tokio::test]
async fn resubscribes_on_every_reconnect() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding to an OS-assigned port cannot fail");
    let addr = listener
        .local_addr()
        .expect("a bound listener always has a local address");

    // Server side: complete the WS handshake (run_feed needs to get past it
    // to reach subscribe_message), then send a close frame, forcing a
    // reconnect — three times over.
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

    // run_feed never returns — wrap the wait in a timeout so a real
    // regression (subscribe never firing again) fails fast instead of
    // hanging the test suite.
    tokio::time::timeout(Duration::from_secs(20), async {
        while subscribe_calls.load(Ordering::SeqCst) < 3 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("subscribe_message should have fired 3 times (initial connect + 2 reconnects) well within 20s of backoff");

    // Stop both background tasks before asserting — nothing left to
    // interact with them, and leaving them running would just keep spending
    // the runtime's time budget for no reason.
    feed.abort();
    server.abort();

    assert_eq!(
        subscribe_calls.load(Ordering::SeqCst),
        3,
        "subscribe_message must fire exactly once per connection attempt, not once total"
    );
}

/// Bug this catches: a tight-spin reconnect loop — one that retries
/// instantly on every connection failure with no backoff at all. That would
/// be *worse* than not reconnecting: it's exactly the rate-limit failure
/// piece 3/4 of `009-resilience` (the stability-gated backoff reset, the
/// token bucket) exist to prevent. Proving `run_feed` merely retries isn't
/// enough on its own — this test also times the gap between two connection
/// attempts and asserts it's at least the low end of the first jittered
/// backoff delay (nominal 1s, jittered 0.5x-1.5x per `Backoff::jittered` in
/// `src/feed.rs`, so 0.5s is a safe one-sided lower bound). No upper bound,
/// so jitter and scheduler variance can't make this flaky.
#[tokio::test]
async fn reconnects_after_waiting_not_in_a_tight_spin() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding to an OS-assigned port cannot fail");
    let addr = listener
        .local_addr()
        .expect("a bound listener always has a local address");

    // Server side: accept a raw TCP connection and close it immediately,
    // before any WebSocket handshake — a legitimate "connection failed"
    // case, and the simplest one to force from the server end. Records the
    // wall-clock time of each accept so the gap between them can be
    // measured.
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
