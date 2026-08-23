//! Pure parsing for the optional `HTTPS_PROXY`/`HTTP_PROXY` env vars.
//!
//! Kept in the library (not `src/main.rs`) so it's reachable from a unit
//! test — see `src/lib.rs`'s doc comment on why logic in `main.rs` silently
//! stops being covered. The actual TCP connect and HTTP `CONNECT` handshake
//! stay in `main.rs`: they're I/O, not logic, and there's nothing to unit
//! test in them beyond what `parse_proxy_addr` already covers (see
//! `specs/002-binance-feed/revisions.md`, entry 2).

/// Strips an optional `scheme://` prefix and any trailing path from a proxy
/// URL, then splits the remainder on the last `:` to separate host from
/// port. Returns `None` for anything that doesn't parse — a malformed value
/// is the caller's cue to log a warning and connect directly, not to crash
/// a binary that would otherwise run fine.
pub fn parse_proxy_addr(raw: &str) -> Option<(String, u16)> {
    let without_scheme = raw.split_once("://").map_or(raw, |(_, rest)| rest);
    let host_port = without_scheme.split('/').next()?;
    let (host, port) = host_port.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scheme_host_and_port() {
        assert_eq!(
            parse_proxy_addr("http://100.64.1.2:3128"),
            Some(("100.64.1.2".to_string(), 3128))
        );
    }

    #[test]
    fn parses_without_a_scheme() {
        assert_eq!(
            parse_proxy_addr("proxy.local:8080"),
            Some(("proxy.local".to_string(), 8080))
        );
    }

    #[test]
    fn ignores_a_trailing_path() {
        assert_eq!(
            parse_proxy_addr("http://proxy.local:8080/"),
            Some(("proxy.local".to_string(), 8080))
        );
    }

    #[test]
    fn rejects_a_missing_port() {
        assert_eq!(parse_proxy_addr("http://proxy.local"), None);
    }

    #[test]
    fn rejects_a_non_numeric_port() {
        assert_eq!(parse_proxy_addr("http://proxy.local:notaport"), None);
    }

    #[test]
    fn rejects_an_empty_string() {
        assert_eq!(parse_proxy_addr(""), None);
    }
}
