# keyrock-case-study

A Rust project scaffold: library + CLI binary, tested, linted, and containerised.

## Requirements

- Rust 1.97.1 — `rust-toolchain.toml` pins it, and `rustup` will install it
  automatically on first build.
- Docker (optional, for the container path).

## Quick start

```sh
cargo run -- hello Keyrock
cargo run -- doctor
```

## Development

```sh
cargo test                                  # unit + integration
cargo clippy --all-targets -- -D warnings   # lints are errors
cargo fmt --check
```

## Layout

```
src/lib.rs        library root — everything testable lives here
src/config.rs     configuration, read from the environment
src/telemetry.rs  tracing setup (logs to stderr)
src/main.rs       CLI entry point — parses arguments and delegates
tests/cli.rs      integration tests: the real binary, as a subprocess
```

The library/binary split is intentional: integration tests under `tests/` can
only import a library crate, so logic placed directly in `main.rs` would not be
reachable from them.

## Configuration

All settings are read from the environment with the `KEYROCK_` prefix, and all
have defaults — the binary runs with no configuration at all. Copy
`.env.example` to `.env` to override.

| Variable | Default | Meaning |
| --- | --- | --- |
| `KEYROCK_LOG_LEVEL` | `info` | `RUST_LOG`-style filter. An explicit `RUST_LOG` wins. |
| `KEYROCK_HOST` | `127.0.0.1` | Bind address. Use `0.0.0.0` inside a container. |
| `KEYROCK_PORT` | `8080` | Port. An unparseable value is a startup error, not a silent fallback. |

Logs are written to stderr so stdout carries only program output and stays
pipeable.

## Docker

```sh
docker compose build
docker compose run --rm app hello Keyrock
docker compose run --rm app doctor
```

Two-stage build: the Rust toolchain compiles the binary, and a slim Debian
image runs it as a non-root user. There is no long-running service yet, so
there is nothing to `docker compose up`.
