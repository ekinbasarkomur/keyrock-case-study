//! Configuration, read from the environment.
//!
//! One prefix (`KEYROCK_`), one accessor, no global mutable state. Every
//! setting has a default so the binary runs with an empty environment — a
//! config error at startup should be impossible, not merely unlikely.

use std::env;

/// Environment-variable prefix for every setting this crate reads.
pub const ENV_PREFIX: &str = "KEYROCK_";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// `RUST_LOG`-style filter, e.g. "info" or "keyrock_case_study=debug".
    pub log_level: String,
    /// Address the service binds to.
    ///
    /// MUST be 0.0.0.0 inside a container: 127.0.0.1 in there is the
    /// container's own loopback, so the published port would refuse every
    /// connection while the logs look perfectly healthy. Exposure is limited
    /// by the `127.0.0.1:` prefix on compose's `ports:` line, not by this.
    pub host: String,
    pub port: u16,
    /// Traded pair the aggregator streams, e.g. "ethbtc".
    pub pair: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            log_level: "info".to_string(),
            host: "127.0.0.1".to_string(),
            port: 50051,
            pair: "ethbtc".to_string(),
        }
    }
}

impl Config {
    /// Read configuration from the process environment, falling back to
    /// [`Default`] for anything absent.
    ///
    /// An unparseable port is a hard error rather than a silent fallback:
    /// starting on a port nobody asked for is worse than refusing to start.
    pub fn from_env() -> Result<Self, ConfigError> {
        let defaults = Self::default();
        let port = match env::var(format!("{ENV_PREFIX}PORT")) {
            Ok(raw) => raw
                .parse::<u16>()
                .map_err(|_| ConfigError::InvalidPort(raw))?,
            Err(_) => defaults.port,
        };
        Ok(Self {
            log_level: env::var(format!("{ENV_PREFIX}LOG_LEVEL")).unwrap_or(defaults.log_level),
            host: env::var(format!("{ENV_PREFIX}HOST")).unwrap_or(defaults.host),
            pair: env::var(format!("{ENV_PREFIX}PAIR")).unwrap_or(defaults.pair),
            port,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{ENV_PREFIX}PORT is not a valid port number: {0:?}")]
    InvalidPort(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_usable_with_no_environment() {
        let c = Config::default();
        assert_eq!(c.port, 50051);
        assert_eq!(c.host, "127.0.0.1");
        assert_eq!(c.pair, "ethbtc");
    }

    #[test]
    fn invalid_port_is_an_error_not_a_fallback() {
        // SAFETY: single-threaded test; see the note in tests/cli.rs about why
        // env-mutating tests are kept out of the parallel suite where possible.
        unsafe { env::set_var("KEYROCK_PORT", "not-a-number") };
        let err = Config::from_env().unwrap_err();
        unsafe { env::remove_var("KEYROCK_PORT") };
        assert!(matches!(err, ConfigError::InvalidPort(_)));
    }
}
