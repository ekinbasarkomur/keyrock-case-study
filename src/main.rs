//! keyrock-case-study — CLI entry point.
//!
//! Deliberately thin: parse arguments, initialise logging, delegate. Anything
//! worth testing belongs in the library crate (`src/lib.rs`), which the
//! integration tests in `tests/` can actually reach.
//!
//! This step's read loop is driven directly here, in the `main` task — no
//! `tokio::spawn`, no `.split()` on the websocket stream. A single feed, a
//! single task, is the entire concurrency story until step 3/4 add a second
//! venue (see `specs/002-binance-feed/spec.md`).

use anyhow::{Context, Result};
use clap::Parser;
use futures_util::StreamExt;
use keyrock_case_study::exchange::binance;
use keyrock_case_study::proxy::parse_proxy_addr;
use keyrock_case_study::{config::Config, telemetry};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{client_async_tls, connect_async};
use tracing::{debug, info, warn};

#[derive(Parser)]
#[command(name = "keyrock-case-study", version, about = "Keyrock case study")]
struct Cli {
    /// Traded pair to aggregate, e.g. "ethbtc". Overrides KEYROCK_PAIR.
    #[arg(long)]
    pair: Option<String>,

    /// Port the service binds to. Overrides KEYROCK_PORT.
    #[arg(long)]
    port: Option<u16>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut config = Config::from_env()?;

    // CLI flags are the more specific, closer-to-the-call-site input, so
    // they win over the env-sourced value when both are given.
    if let Some(pair) = cli.pair {
        config.pair = pair;
    }
    if let Some(port) = cli.port {
        config.port = port;
    }

    telemetry::init(&config.log_level);

    info!(pair = %config.pair, port = %config.port, "starting");

    // rustls 0.23+ no longer picks a `CryptoProvider` implicitly from Cargo
    // features alone — the process must install one before the first TLS
    // handshake, or `connect_async` panics deep inside rustls with an
    // unhelpful message. `tokio-tungstenite`'s `rustls-tls-webpki-roots`
    // feature resolves `ring` as the provider; this just activates it.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("no CryptoProvider installed yet — this is the first call");

    let url = binance::connect_url(&config.pair);
    let (mut ws, _response) = match proxy_addr() {
        Some((proxy_host, proxy_port)) => {
            info!(proxy = %format!("{proxy_host}:{proxy_port}"), "connecting to binance via HTTP CONNECT proxy");
            let tunnel =
                connect_through_proxy(&proxy_host, proxy_port, binance::HOST, binance::PORT)
                    .await
                    .context("failed to establish CONNECT tunnel to binance through proxy")?;
            client_async_tls(&url, tunnel)
                .await
                .with_context(|| format!("failed to connect to Binance at {url} via proxy"))?
        }
        None => connect_async(&url)
            .await
            .with_context(|| format!("failed to connect to Binance at {url}"))?,
    };
    info!(url = %url, "connected to binance");

    // Single `next()` loop over the bidirectional stream — no `.split()`,
    // no `tokio::spawn`. This step never writes anything itself beyond what
    // `tokio-tungstenite` answers automatically (pongs), so one task reading
    // is sufficient.
    while let Some(message) = ws.next().await {
        let message = message.context("websocket read failed")?;
        match message {
            Message::Text(text) => {
                if let Some(book) = binance::parse(&text)
                    && let (Some((bid_price, bid_amount)), Some((ask_price, ask_amount))) =
                        (book.bids.first(), book.asks.first())
                {
                    info!(
                        "binance {} | bid {} x {} | ask {} x {} | id {}",
                        config.pair,
                        bid_price,
                        bid_amount,
                        ask_price,
                        ask_amount,
                        book.last_update_id
                    );
                }
            }
            // `tokio-tungstenite` answers pings automatically; nothing to do
            // for either side of the ping/pong exchange here.
            Message::Ping(_) | Message::Pong(_) => {}
            // No reconnection in this step (that's step 6) — a close frame
            // simply ends the read loop and the process exits.
            Message::Close(_) => {
                info!("binance closed the connection");
                break;
            }
            Message::Binary(_) => {
                debug!("ignoring unexpected binary message");
            }
            Message::Frame(_) => {
                debug!("ignoring raw frame message");
            }
        }
    }

    Ok(())
}

/// Reads `HTTPS_PROXY` (preferred) or `HTTP_PROXY` and parses it into a
/// `(host, port)` pair, e.g. `"http://100.64.x.x:3128"` -> `("100.64.x.x",
/// 3128)`. Returns `None` if neither is set, or if the value is set but
/// doesn't parse — a malformed proxy env var is a reason to log a warning
/// and connect directly, not to crash a binary that would otherwise run
/// fine (see `specs/002-binance-feed/revisions.md`, entry 2).
fn proxy_addr() -> Option<(String, u16)> {
    let raw = std::env::var("HTTPS_PROXY")
        .or_else(|_| std::env::var("HTTP_PROXY"))
        .ok()?;
    // compose.yml's PROXY_HOST/PROXY_PORT defaults produce a literal
    // "http://:" (not an absent env var) when neither is set in `.env` —
    // treat that specific empty-template shape as "no proxy configured"
    // rather than warning on every default run.
    if raw.trim_end_matches('/').ends_with("://:") {
        return None;
    }
    match parse_proxy_addr(&raw) {
        Some(addr) => Some(addr),
        None => {
            warn!(value = %raw, "HTTPS_PROXY/HTTP_PROXY is set but not a parseable host:port — connecting directly");
            None
        }
    }
}

/// Opens a plain TCP connection to the proxy and issues an HTTP `CONNECT`
/// for `target_host:target_port`, returning the tunneled `TcpStream` once
/// the proxy answers `200`. The caller then hands this stream to
/// `client_async_tls`, which performs the real TLS handshake through the
/// tunnel — the proxy only ever sees opaque bytes after this point.
async fn connect_through_proxy(
    proxy_host: &str,
    proxy_port: u16,
    target_host: &str,
    target_port: u16,
) -> Result<TcpStream> {
    let mut stream = TcpStream::connect((proxy_host, proxy_port))
        .await
        .with_context(|| format!("failed to connect to proxy at {proxy_host}:{proxy_port}"))?;

    let request = format!(
        "CONNECT {target_host}:{target_port} HTTP/1.1\r\nHost: {target_host}:{target_port}\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .context("failed to write CONNECT request to proxy")?;

    let response = read_http_response_headers(&mut stream).await?;
    let status_line = response.lines().next().unwrap_or("");
    if !status_line.contains(" 200 ") {
        anyhow::bail!("proxy CONNECT to {target_host}:{target_port} failed: {status_line}");
    }

    Ok(stream)
}

/// Reads from `stream` one byte at a time until the `\r\n\r\n` header
/// terminator. A proxy's CONNECT response is a handful of header lines, not
/// a bulk transfer, so a full buffered HTTP parser would be overkill here.
async fn read_http_response_headers(stream: &mut TcpStream) -> Result<String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = stream
            .read(&mut byte)
            .await
            .context("failed to read proxy CONNECT response")?;
        if n == 0 {
            anyhow::bail!("proxy closed the connection before completing the CONNECT response");
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(buf).context("proxy CONNECT response was not valid UTF-8")
}
