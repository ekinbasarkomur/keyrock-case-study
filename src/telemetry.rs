//! Logging setup.
//!
//! `tracing` is initialised exactly once, from `main`. Library code never
//! installs a subscriber — doing so from a library makes the behaviour depend
//! on which crate happened to run first.

use tracing_subscriber::EnvFilter;

/// Install the global tracing subscriber.
///
/// `filter` is a `RUST_LOG`-style directive. An explicit `RUST_LOG` in the
/// environment wins, so operators can turn up logging without a rebuild.
pub fn init(filter: &str) {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter));

    // Logs go to STDERR so that stdout stays clean for machine-readable
    // output. A CLI that interleaves logs into stdout cannot be piped.
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
}
