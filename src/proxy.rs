//! Optional `HTTPS_PROXY`/`HTTP_PROXY` support: parse the env var, open the
//! TCP connection, run the HTTP CONNECT handshake.

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// Strips an optional scheme and trailing path from a proxy URL, then
/// splits host from port. `None` for anything that doesn't parse.
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

/// Opens a TCP connection to the proxy and issues an HTTP CONNECT for
/// `target_host:target_port`, returning the tunneled stream once the proxy
/// answers 200.
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

/// Reads header lines up to the blank-line terminator.
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

    /// A well-formed value parses with a scheme, without one, and with a
    /// trailing path.
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

    /// Malformed input (no port, non-numeric port, empty) returns None.
    #[test]
    fn rejects_unparseable_input() {
        assert_eq!(parse_proxy_addr("http://proxy.local"), None);
        assert_eq!(parse_proxy_addr("http://proxy.local:notaport"), None);
        assert_eq!(parse_proxy_addr(""), None);
    }
}
