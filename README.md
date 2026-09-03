# rust-crypto-orderbook

A Rust service that merges live market data from Binance, Bitstamp, and
Kraken into a single order book, streaming the spread and the top 10 bids
and asks over gRPC.

## Quick Start

```sh
# Run via Docker
docker compose up --build
docker compose run --rm client

# Run via Cargo
cargo run --bin rust-crypto-orderbook -- --pair btcusd --port 12345
```

## System Behavior

| Mechanism | Implementation Details |
| --- | --- |
| Streaming | Broadcasts via a `watch` channel. Slower clients receive the current state, avoiding queued backlogs. |
| Reconnection | Jittered exponential backoff (1s to 30s), constrained by exchange-specific rate limits. |
| Staleness | Drops inactive venues before merging. Silence thresholds: Binance (1.5s), Bitstamp (8s), Kraken (12s). |
| Data Parsing | Binance and Bitstamp updates are stateless. Kraken updates are delta-based and require holding state in a `Mutex`. |
| Tie-breaking | Identical prices are sorted by larger volume. Full ties default to the first declared venue. |

## Performance and Load

All figures reflect real-world execution, not synthetic benchmarks.
Deduplication is enabled by default to prevent redundant broadcasts.

| Metric | Measured Result |
| --- | --- |
| Latency (Release) | p50 76 to 85µs from ingest to publish. |
| Duplicate Rate | 35 to 49% redundant messages filtered. |
| Load: 100 Clients | 298 msg/s at 1.5% CPU. |
| Load: 500 Clients | 843 msg/s at 3.4% CPU. |
| Load: 1000 Clients | 2519 msg/s at 11.1% CPU. |

## Configuration

Settings accept a CLI flag or an `ORDERBOOK_`-prefixed environment
variable. Flags take priority.

| Setting | Flag | Default | Notes |
| --- | --- | --- | --- |
| Pair | `--pair` | `ethbtc` | Defines the active trading pair. |
| Port | `--port` | `50051` | The gRPC service port. |
| Host | None | `127.0.0.1` | Set to `0.0.0.0` when running in a container. |
| Logging | None | `info` | Overridden entirely by `RUST_LOG` if present. |

## Production Gaps

| Current Limitation | Required Architecture Fix |
| --- | --- |
| Single pair per process | Accept the target trading pair in the gRPC request. |
| Hardcoded tick sizes | Fetch per-pair tick sizes dynamically from each venue. |
| Unbounded gRPC clients | Implement a `tower` concurrency-limit layer. |
| Redundant encoding | Encode the protocol buffer once and distribute raw bytes to subscribers. |
| Guessed venue health | Expose individual exchange connection statuses in the gRPC schema. |
