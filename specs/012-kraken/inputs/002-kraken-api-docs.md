# Kraken WebSocket API v2 — order book channel, researched

Fetched from `docs.kraken.com`'s own `llms-full.txt` dump (via the project's
configured HTTP CONNECT proxy — `docs.kraken.com` was unreachable directly
from this environment, same as Binance/Bitstamp). Every field below is
quoted or paraphrased from Kraken's own docs, not guessed. Source URLs are
inline per section.

## The one finding that changes the shape of this integration

**Binance's `depth20@100ms` and this project's chosen Bitstamp channel
(`order_book_<pair>`, not `diff_order_book_<pair>`) both send a full,
self-contained top-N snapshot on *every* message.** That's why
`Exchange::parse(&self, raw: &str) -> Option<Book>` can be a pure,
stateless function today — each message alone is a complete `Book`, nothing
carried over from the last one.

**Kraken's v2 `book` channel does not work that way.** Per
`docs.kraken.com/exchange/api-reference/spot-websocket-v2/book`:

- On subscribe, Kraken sends exactly **one** `type: "snapshot"` message —
  the full book at that instant.
- After that, every message is `type: "update"` — **only the price levels
  that changed**, not the full book. `"qty": 0` means "remove this price
  level." A level that falls out of the top N is *not* sent as `qty: 0` —
  the client is expected to truncate its own locally-maintained book to the
  subscribed depth after applying each update.
- I did not find a Kraken channel/mode that re-sends a full snapshot
  repeatedly the way Binance's `depth20@100ms` or Bitstamp's
  `order_book_<pair>` do. `snapshot: true` in the subscribe request only
  controls whether the *first* message is a snapshot.

**Consequence**: producing a correct `Book` from Kraken's stream requires
holding local, mutable state across messages (apply each update to a
locally-held copy of the book) — something Binance's and Bitstamp's
`parse` never need, and something the current `Exchange` trait's
`fn parse(&self, raw: &str) -> Option<Book>` signature (`&self`, not
`&mut self`) doesn't have anywhere to put. This is a real architectural
question, not a detail — see Open Questions in spec.md for how this gets
decided, but it needs deciding before implementation starts, not
discovered partway through.

## Connection

Source: `docs.kraken.com/exchange/guides/websockets/introduction`

| Environment | Public WS URL |
| --- | --- |
| Primary | `wss://ws.kraken.com/v2` |
| Beta | `wss://beta-ws.kraken.com/v2` |

No auth needed for the public `book` channel (auth is only for private
channels like `executions`/`balances`, which this project has no use for).

## Subscribe (book channel)

Source: `docs.kraken.com/exchange/api-reference/spot-websocket-v2/book`

Request:

```json
{
    "method": "subscribe",
    "params": {
        "channel": "book",
        "symbol": ["ETH/BTC"],
        "depth": 10
    }
}
```

- `symbol` is a **list** — Kraken lets one connection subscribe to
  multiple pairs at once. This project only ever needs one.
- `depth`: one of `10, 25, 100, 500, 1000`, default `10`. `10` matches
  this project's top-10 requirement exactly, so no over-fetching or
  trimming needed on this side (unlike Binance's `depth20`, which is
  already handed over-sized).
- `snapshot`: boolean, default `true` — leave it at the default.

Response — **one ack per symbol subscribed**, e.g.:

```json
{
    "method": "subscribe",
    "result": {
        "channel": "book",
        "depth": 10,
        "snapshot": true,
        "symbol": "ETH/BTC"
    },
    "success": true,
    "time_in": "2023-10-06T17:35:55.219022Z",
    "time_out": "2023-10-06T17:35:55.219067Z"
}
```

On failure: `"success": false`, and an `"error"` field with a message
string. No documented error-code enum found (unlike some of Kraken's other
error surfaces) — treat as an opaque string, same as this project already
does for Bitstamp's `bts:error`.

## Snapshot message

```json
{
    "channel": "book",
    "type": "snapshot",
    "data": [
        {
            "symbol": "ETH/BTC",
            "bids": [
                { "price": 0.5666, "qty": 4831.75496356 }
            ],
            "asks": [
                { "price": 0.5668, "qty": 4410.79769741 }
            ],
            "checksum": 2439117997,
            "timestamp": "2023-10-06T17:35:55.440295Z"
        }
    ]
}
```

`data` is an **array** (Kraken batches per-symbol payloads in one message
when multiple symbols are subscribed) — for this project's one-symbol
subscription it will always be a one-element array, but the parser has to
index into it, not assume a bare object.

## Update message

```json
{
    "channel": "book",
    "type": "update",
    "data": [
        {
            "symbol": "ETH/BTC",
            "bids": [
                { "price": 0.5657, "qty": 1098.3947558 }
            ],
            "asks": [],
            "checksum": 2114181697,
            "timestamp": "2023-10-06T17:35:55.440295Z"
        }
    ]
}
```

Only the levels that changed appear — `asks: []` above means no ask
changed in this update. "Note, it is possible to have multiple updates to
the same price level in a single update message. Updates should always be
processed in sequence."

## Price/qty representation — a documentation inconsistency, flagged not resolved

The `book` channel's own field reference
(`spot-websocket-v2/book`) types `price`/`qty` as **`float`**, and the
worked examples above show them as **bare JSON numbers**
(`"price": 0.5666`), not strings.

But the separate checksum guide
(`exchange/guides/websockets/book-checksum-v2`) shows a worked example with
`price`/`qty` as **strings** (`"price": "45283.5", "qty": "0.10000000"`)
and explicitly warns: *"Parse `price` and `qty` fields using a decimal or
string decoder to preserve full precision through deserialisation."*

I did not resolve this discrepancy from the docs alone — it needs a real
captured message to settle, the same way this project settled Bitstamp's
envelope shape by capturing a real payload before writing
`src/exchange/bitstamp.rs`. What it means either way: Binance and
Bitstamp's `Depth20`/`Data` structs borrow `&str` price/qty pairs
(`Vec<[&'a str; 2]>`) straight out of the source text — if Kraken really
sends bare JSON floats, `Kraken`'s equivalent struct needs `Vec<Level<'a>>`
with typed `f64`/`Decimal`-ish fields instead of a borrowed-string pair,
which is a real (small) difference in the parsing shape, not just a
different field name.

## Checksum (CRC32) — optional, not used by Binance/Bitstamp today

Source: `exchange/guides/websockets/book-checksum-v2`

Every snapshot/update carries a `checksum` field: CRC32 over the top 10
bids (high→low) + top 10 asks (low→high), each level's `price`+`qty`
digit-string (dot removed, leading zeros stripped) concatenated, asks
first then bids. Verification is explicitly **optional** — "provides an
additional check that the client copy has been constructed correctly and
is synchronised to the exchange," not required to consume the feed.
Neither `binance.rs` nor `bitstamp.rs` implements any equivalent integrity
check today — whether Kraken's gets implemented is a real scope question,
not a given, since it's the first per-message integrity check in this
codebase if built.

## Heartbeat and status — the two control/lifecycle messages to route to `None`

Source: `spot-websocket-v2/heartbeat`, `spot-websocket-v2/status`

- **`heartbeat`**: sent ~once per second **whenever no other channel
  update is due to be sent** — not opt-in, generated automatically once
  any channel is subscribed. Payload is just `{"channel": "heartbeat"}`,
  no other data. This is Kraken's rough equivalent of "the stream is
  alive" — closer to what this project already infers from Binance's
  100ms cadence than to an explicit ping/pong.
- **`status`**: sent automatically right after the connection is
  established, and again whenever the trading engine's status changes
  (`online`/`maintenance`/`cancel_only`/`post_only`). Example:
  ```json
  {"channel":"status","type":"update","data":[{"api_version":"v2","connection_id":13834774380200032777,"system":"online","version":"2.0.0"}]}
  ```
  `maintenance` is documented as "stop sending orders" — not directly
  relevant to a read-only book feed, but still a message `parse` needs to
  recognize and return `None` for, same as Binance's `serverShutdown` or
  Bitstamp's `bts:subscription_succeeded`.

Both arrive on the same connection, interleaved with `book` messages — the
parser needs to branch on the top-level `"channel"` field (`"book"` vs
`"heartbeat"` vs `"status"`) before it even looks at `type`.

## Ping — client-initiated, not exchange-initiated

Source: `spot-websocket-v2/ping`, and the "Connection management" section
of the WebSocket overview page.

"Send a `ping` at least every 60 seconds to keep the connection open." This
is the **opposite direction** from Binance (Binance's server pings the
client every 20s; `tokio-tungstenite` already auto-answers that with no
code needed) and from Bitstamp (no documented ping requirement at all).
For Kraken, *this project's client* would be the one expected to send
`{"method":"ping"}` periodically if the connection were otherwise idle for
that long — in practice, a subscribed `book` channel on a liquid pair plus
the automatic ~1s heartbeat almost certainly keeps traffic flowing well
under 60s, so this may never matter in practice, but it's a real,
documented expectation this project doesn't currently have machinery for
(no proactive client-side ping exists in `src/feed.rs` today).

## Reconnection guidance

Source: `exchange/guides/websockets/reconnection`

- "Use exponential backoff with jitter to avoid thundering-herd
  reconnection storms" — the example code resets its delay to 1.0s on
  every successful connect, uncapped comparison to this project's
  stability-gated reset (`009-resilience`'s `Backoff::reset_if_stable`,
  which only resets after 30s *held* stable, not on connect alone). Worth
  noting as a place Kraken's own sample code is less careful than what
  this project already does for Binance/Bitstamp — not a reason to weaken
  this project's approach.
- "After reconnecting you must re-subscribe to all channels. Kraken does
  not automatically restore subscriptions" — same shape as Bitstamp
  (per-connection subscribe), unlike Binance (baked into the URL).
- "On reconnect: discard your existing book, wait for the snapshot,
  rebuild from scratch. Do not attempt to merge deltas from before and
  after the reconnect gap." Directly relevant given the
  snapshot-then-incremental model above — whatever local book state a
  Kraken integration ends up holding must be thrown away and rebuilt on
  every reconnect, not resumed.
- "If \[checksum] fails, unsubscribe and re-subscribe to force a fresh
  snapshot" — the checksum's one concrete documented use, if it gets
  built.
- No specific numeric backoff schedule is mandated — Kraken's own sample
  uses 1s→60s exponential, close to but not identical to this project's
  existing 1/2/4/8/16/30s-capped schedule.

## Rate limits — nothing specific found for public book-channel connects

Searched the general rate-limits guide (`exchange/guides/general/ratelimits`)
and the derivatives-specific one
(`exchange/guides/futures/ratelimits`, which documents 100 concurrent
connections / 100 requests-per-second — but that page is explicitly scoped
to **Derivatives**, not Spot, per its own header note). The Spot trading
rate-limit page covers order-placement rate counters (adds/amends/cancels),
which don't apply to a read-only public book subscription. **No documented
figure for how often a client may open new public spot WebSocket
connections was found** — same situation this project already documented
for Bitstamp's connect-rate ("Bitstamp publishes no documented limit... a
stated guess, not fact").

## Symbol format

Kraken v2's examples all use slash-separated, uppercase pairs:
`"BTC/USD"`, `"ETH/BTC"` would be the equivalent for this project's default
pair. Note this project's `--pair`/`ORDERBOOK_PAIR` today is a single
lowercase concatenated token (`"ethbtc"`, matching Binance's URL path
convention) — Kraken's `subscribe_message` can't mechanically derive
`"ETH/BTC"` from `"ethbtc"` without knowing where the base/quote boundary
is (fine for a hardcoded `ethbtc`/`ETH/BTC` case, not a general solution
for an arbitrary pair string the same way Binance/Bitstamp's lowercasing
is).

## Symbol identity — no legacy `XBT` in v2

Kraken's older REST/v1 APIs and Futures product IDs use `XBT` for
Bitcoin (`XBT/USD`, `PF_XBTUSD`). The v2 spot WebSocket examples
consistently use `BTC` (`"BTC/USD"`) — the legacy `XBT` naming appears to
be a v1/Futures-only quirk, not something v2 spot inherits. Worth
confirming against a live capture rather than trusting this fully, per
this project's own "measure, don't guess" convention, but nothing in the
v2 docs suggests `XBT` is needed.
