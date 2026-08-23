//! The optional `HTTPS_PROXY`/`HTTP_PROXY` proxy support: parsing the env
//! var, opening the TCP connection, and running the HTTP `CONNECT` handshake.
//!
//! Kept in the library (not `src/main.rs`) so it's reachable from a unit
//! test — see `src/lib.rs`'s doc comment on why logic in `main.rs` silently
//! stops being covered.

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

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

/// Opens a plain TCP connection to the proxy and issues an HTTP `CONNECT`
/// for `target_host:target_port`, returning the tunneled `TcpStream` once
/// the proxy answers `200`. The caller then hands this stream to
/// `client_async_tls`, which performs the real TLS handshake through the
/// tunnel — the proxy only ever sees opaque bytes after this point.
pub async fn connect_through_proxy(
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

/// Reads header lines from `stream` up to the blank-line (`\r\n\r\n`)
/// terminator. A proxy's CONNECT response is a handful of header lines, not
/// a bulk transfer, so a full buffered HTTP parser would be overkill — but
/// reading via `read_until` a line at a time is still one syscall per line
/// rather than one per byte.
async fn read_http_response_headers(stream: &mut TcpStream) -> Result<String> {
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        let n = reader
            .read_until(b'\n', &mut buf)
            .await
            .context("failed to read proxy CONNECT response")?;
        if n == 0 {
            anyhow::bail!("proxy closed the connection before completing the CONNECT response");
        }
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(buf).context("proxy CONNECT response was not valid UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A well-formed proxy value parses correctly with a scheme, without a
    /// scheme, and with a trailing path — the three "this should parse"
    /// shapes folded into one test since they all exercise the same
    /// success path through `parse_proxy_addr` and would fail together for
    /// the same reason (e.g. a broken `rsplit_once(':')` or `split('/')`).
    #[test]
    fn parses_well_formed_values() {
        assert_eq!(
            parse_proxy_addr("http://100.64.1.2:3128"),
            Some(("100.64.1.2".to_string(), 3128))
        );
        assert_eq!(
            parse_proxy_addr("proxy.local:8080"),
            Some(("proxy.local".to_string(), 8080))
        );
        assert_eq!(
            parse_proxy_addr("http://proxy.local:8080/"),
            Some(("proxy.local".to_string(), 8080))
        );
    }

    /// Unparseable input returns `None` rather than panicking, across the
    /// three ways a proxy value can be malformed (no port, non-numeric
    /// port, empty string) — folded into one test since all three assert
    /// the same "reject, don't crash" property and would fail together for
    /// the same reason.
    #[test]
    fn rejects_unparseable_input() {
        assert_eq!(parse_proxy_addr("http://proxy.local"), None);
        assert_eq!(parse_proxy_addr("http://proxy.local:notaport"), None);
        assert_eq!(parse_proxy_addr(""), None);
    }
}
