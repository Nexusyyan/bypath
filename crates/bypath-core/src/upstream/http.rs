use crate::config::ProxyConfig;
use crate::upstream::TargetAddr;
use anyhow::{bail, Context, Result};
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Establishes a TCP tunnel through an HTTP/1.1 `CONNECT` proxy.
pub async fn connect_tcp(proxy: &ProxyConfig, target: &TargetAddr) -> Result<TcpStream> {
    let mut stream = TcpStream::connect(&proxy.address)
        .await
        .with_context(|| format!("failed to connect to HTTP proxy {}", proxy.address))?;
    let _ = stream.set_nodelay(true);

    let authority = match target {
        TargetAddr::Domain(host, port) => format!("{host}:{port}"),
        TargetAddr::Ip(SocketAddr::V4(v4)) => format!("{}:{}", v4.ip(), v4.port()),
        TargetAddr::Ip(SocketAddr::V6(v6)) => format!("[{}]:{}", v6.ip(), v6.port()),
    };

    let mut req = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: Keep-Alive\r\n"
    );
    if let Some(ref user) = proxy.username {
        let pass = proxy.password.as_deref().unwrap_or_default();
        let encoded = base64_encode(format!("{user}:{pass}").as_bytes());
        req.push_str(&format!("Proxy-Authorization: Basic {encoded}\r\n"));
    }
    req.push_str("\r\n");

    stream.write_all(req.as_bytes()).await?;

    // Read response header up to `\r\n\r\n` one byte at a time so we never over-read
    // payload bytes belonging to the tunneled protocol.
    let mut buf = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    while buf.len() < 8192 {
        stream.read_exact(&mut byte).await?;
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }

    let header_str = std::str::from_utf8(&buf).context("non-UTF8 HTTP proxy response")?;
    let status_line = header_str.lines().next().unwrap_or_default();
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);

    if !(200..300).contains(&status_code) {
        bail!("HTTP CONNECT to {authority} rejected: {status_line}");
    }

    Ok(stream)
}

/// Probes an HTTP proxy endpoint over an already-connected stream by sending a minimal
/// request and checking for a valid HTTP status line while flagging 407 auth failures.
pub async fn probe_handshake_stream(stream: &mut TcpStream, proxy: &ProxyConfig) -> Result<()> {
    let _ = stream.set_nodelay(true);

    let mut req = String::from(
        "OPTIONS http://bypath.probe.invalid/ HTTP/1.1\r\nHost: bypath.probe.invalid\r\nConnection: close\r\n",
    );
    if let Some(ref user) = proxy.username {
        let pass = proxy.password.as_deref().unwrap_or_default();
        let encoded = base64_encode(format!("{user}:{pass}").as_bytes());
        req.push_str(&format!("Proxy-Authorization: Basic {encoded}\r\n"));
    }
    req.push_str("\r\n");

    stream.write_all(req.as_bytes()).await?;

    let mut buf = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    while buf.len() < 4096 {
        match stream.read_exact(&mut byte).await {
            Ok(_) => {
                buf.push(byte[0]);
                if buf.ends_with(b"\r\n") || buf.ends_with(b"\n") {
                    break;
                }
            }
            Err(_) if !buf.is_empty() => break,
            Err(err) => return Err(err).context("HTTP proxy closed connection before response"),
        }
    }

    let status_line = std::str::from_utf8(&buf)
        .context("endpoint returned non-HTTP binary data")?
        .trim();
    if !status_line.starts_with("HTTP/") {
        bail!("endpoint did not speak HTTP proxy protocol");
    }

    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    if status_code == 407 {
        bail!("HTTP proxy authentication failed (407 Proxy Authentication Required)");
    }
    Ok(())
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i < input.len() {
        let b0 = input[i];
        let b1 = if i + 1 < input.len() { input[i + 1] } else { 0 };
        let b2 = if i + 2 < input.len() { input[i + 2] } else { 0 };

        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if i + 1 < input.len() {
            out.push(TABLE[(((b1 & 0x0F) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if i + 2 < input.len() {
            out.push(TABLE[(b2 & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        i += 3;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"alice:secret"), "YWxpY2U6c2VjcmV0");
    }
}
