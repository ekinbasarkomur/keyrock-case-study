//! Bitstamp `order_book_<pair>` feed: the connect URL, the explicit
//! subscribe message Bitstamp requires (unlike Binance, whose subscription
//! is baked into the URL), and the pure `parse` function, behind the
//! `Exchange` trait. No websocket connection is opened here — that's
//! `src/feed.rs::run_feed`'s job.

use crate::exchange::{Exchange, Venue};
use crate::model::{Amount, Book, Price};
use serde::Deserialize;

/// Bitstamp wraps every message in an envelope (`{"event":...,
/// "channel":..., "data":{...}}`), unlike Binance's flat `depth20` payload —
/// only the `"data"` event carries a book; the other three are
/// lifecycle/control messages (see `parse` below).
#[derive(Deserialize)]
struct Envelope<'a> {
    event: &'a str,
    #[serde(borrow, default)]
    data: Option<Data<'a>>,
}

/// The book payload nested inside a `"data"`-event envelope. Borrows
/// `bids`/`asks` straight out of the source JSON text, same as Binance's
/// `Depth20<'a>` — the borrow doesn't outlive `parse()`, since the `Book`
/// that survives past this function holds only `Price`/`Amount`, never a
/// `&str`.
#[derive(Deserialize)]
struct Data<'a> {
    #[serde(borrow)]
    bids: Vec<[&'a str; 2]>,
    #[serde(borrow)]
    asks: Vec<[&'a str; 2]>,
}

/// Bitstamp's `Exchange` implementation. A unit struct — everything it
/// needs is free-standing in this module already; the struct exists only to
/// carry the trait impl.
pub struct Bitstamp;

impl Exchange for Bitstamp {
    fn venue(&self) -> Venue {
        Venue::Bitstamp
    }

    /// Bitstamp's websocket endpoint has nothing pair-specific in the path
    /// — the pair only shows up in the subscribe channel name
    /// (`subscribe_message` below). `pair` is unused here on purpose.
    fn connect_url(&self, _pair: &str) -> String {
        "wss://ws.bitstamp.net".to_string()
    }

    /// Unlike Binance, Bitstamp's subscription is per-connection, not baked
    /// into the URL — this message must be sent once, right after
    /// connecting, or the connection sits open and silent.
    fn subscribe_message(&self, pair: &str) -> Option<String> {
        Some(format!(
            r#"{{"event":"bts:subscribe","data":{{"channel":"order_book_{}"}}}}"#,
            pair.to_lowercase()
        ))
    }

    /// Parses a raw websocket text message into a [`Book`].
    ///
    /// Returns `None` — never `Result` — for anything that isn't a `"data"`
    /// event: malformed JSON, or one of Bitstamp's three lifecycle events
    /// (`bts:subscription_succeeded`, `bts:request_reconnect`, `bts:error`).
    /// A stray control message must not kill the read loop — same
    /// discipline as `binance.rs::parse`.
    fn parse(&self, raw: &str) -> Option<Book> {
        let envelope: Envelope = match serde_json::from_str(raw) {
            Ok(envelope) => envelope,
            Err(err) => {
                tracing::debug!(%err, "not a recognizable bitstamp envelope, skipping");
                return None;
            }
        };

        match envelope.event {
            "data" => {
                let data = envelope.data?;
                let bids = parse_levels(&data.bids)?;
                let asks = parse_levels(&data.asks)?;
                Some(Book {
                    bids,
                    asks,
                    // Bitstamp has no `lastUpdateId` equivalent — `Book`'s
                    // field is framed around Binance's sequence-number
                    // semantics, and Bitstamp's own `microtimestamp` isn't
                    // wired into `Book` yet (no consumer for it this step,
                    // per specs/006-bitstamp/spec.md). 0 is a placeholder,
                    // not a claim that Bitstamp sent this update first.
                    last_update_id: 0,
                })
            }
            "bts:subscription_succeeded" => {
                tracing::info!("bitstamp subscription succeeded");
                None
            }
            "bts:request_reconnect" => {
                // Step 6 owns turning this into an actual reconnect trigger
                // — this step only logs it.
                tracing::info!("bitstamp requested a reconnect");
                None
            }
            "bts:error" => {
                // The one event that means something is actually wrong —
                // must not be logged at the same level as the benign ones.
                tracing::warn!(raw, "bitstamp reported an error");
                None
            }
            other => {
                tracing::debug!(event = other, "unrecognized bitstamp event, skipping");
                None
            }
        }
    }
}

/// Converts `[price_str, amount_str]` pairs into `(Price, Amount)` levels.
/// Returns `None` if any level fails to parse, rather than silently
/// dropping a level and publishing a book with a hole in it. A direct copy
/// of `binance.rs::parse_levels`'s shape — factoring this out further would
/// fight the borrow checker for no real benefit at two implementations.
fn parse_levels(levels: &[[&str; 2]]) -> Option<Vec<(Price, Amount)>> {
    levels
        .iter()
        .map(|[price, amount]| {
            let price = Price::parse(price)?;
            let amount = Amount::parse(amount)?;
            Some((price, amount))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // TODO(user): capture a real Bitstamp "data" fixture from
    // wss://ws.bitstamp.net — attempted from this implementation
    // environment on 2026-08-23 both directly and through the configured
    // HTTP CONNECT proxy, using the project's actual tokio-tungstenite +
    // rustls-tls-webpki-roots stack; both attempts (three retries each)
    // failed identically with a TLS handshake EOF
    // (`Io(Custom { kind: UnexpectedEof, error: "tls handshake eof" })`)
    // immediately after the ClientHello, while `curl`/`openssl s_client`
    // against the same host from the same machine completed the TLS
    // handshake and a full websocket upgrade without issue — consistent
    // with the host rejecting this rustls client's TLS fingerprint rather
    // than an actual network-reachability problem. See
    // specs/006-bitstamp/plan.md, "Expected Drift Triggers" for this
    // outcome. The "data"-fixture parse test below is skipped, not
    // fabricated, until a real capture is available.

    /// `bts:subscription_succeeded` is a benign lifecycle message, not a
    /// parse failure worth propagating — and its payload has no
    /// `bids`/`asks` at all, so this also catches a panic on that shape.
    #[test]
    fn bts_subscription_succeeded_parses_to_none_without_panicking() {
        assert!(
            Bitstamp
                .parse(r#"{"event":"bts:subscription_succeeded","channel":"order_book_ethbtc","data":{}}"#)
                .is_none()
        );
    }

    /// Confirms `bts:request_reconnect` is recognized specifically, rather
    /// than falling through the generic "unknown event" branch silently —
    /// step 6 depends on this event being distinguishable from the other
    /// three.
    #[test]
    fn bts_request_reconnect_parses_to_none_without_panicking() {
        assert!(
            Bitstamp
                .parse(r#"{"event":"bts:request_reconnect","channel":"","data":{}}"#)
                .is_none()
        );
    }

    /// `bts:error` must not panic on a shape with no `bids`/`asks` either —
    /// distinct from the other lifecycle tests only in which event string
    /// it exercises, kept separate because this one is the log-level-`warn`
    /// case, worth its own name in `cargo test`'s output.
    #[test]
    fn bts_error_parses_to_none_without_panicking() {
        assert!(
            Bitstamp
                .parse(r#"{"event":"bts:error","channel":"","data":{"code":null,"message":"bad subscription"}}"#)
                .is_none()
        );
    }

    /// An unhandled `serde_json` error must convert to `None`, not
    /// propagate — a stray malformed frame must not kill the read loop.
    #[test]
    fn malformed_json_parses_to_none_without_panicking() {
        assert!(Bitstamp.parse("not valid json {{{").is_none());
    }

    /// Bug caught: a wrong channel name is a silent failure — Bitstamp
    /// accepts the subscription and then sends nothing, indistinguishable
    /// from "no messages yet" from the outside.
    #[test]
    fn bitstamp_subscribe_message_contains_the_configured_pairs_channel_name() {
        let msg = Bitstamp
            .subscribe_message("ethbtc")
            .expect("bitstamp subscribes");
        assert!(msg.contains("order_book_ethbtc"));
    }
}
