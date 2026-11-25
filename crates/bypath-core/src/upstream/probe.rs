use crate::config::{ProxyConfig, ProxyKind};
use crate::upstream::{http, socks5};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyProbeReport {
    pub address: String,
    pub kind: ProxyKind,
    pub tcp_reachable: bool,
    #[serde(default)]
    pub handshake_ok: Option<bool>,
    #[serde(default)]
    pub auth_ok: Option<bool>,
    pub latency_ms: Option<u64>,
    pub udp_associate_supported: Option<bool>,
    pub error: Option<String>,
}

/// Probes an upstream proxy for TCP reachability, protocol handshake/auth validity,
/// and (for SOCKS5) UDP ASSOCIATE capability.
pub async fn probe_proxy(proxy: &ProxyConfig, timeout: Duration) -> ProxyProbeReport {
    let start = Instant::now();
    let tcp_res = tokio::time::timeout(timeout, TcpStream::connect(&proxy.address)).await;
    match tcp_res {
        Ok(Ok(mut stream)) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            match proxy.kind {
                ProxyKind::Socks5 => {
                    let hs =
                        tokio::time::timeout(timeout, socks5::probe_handshake_stream(&mut stream, proxy))
                            .await;
                    drop(stream);
                    match hs {
                        Ok(Ok(())) => {
                            let udp_supported = match tokio::time::timeout(
                                timeout,
                                socks5::associate_udp(proxy),
                            )
                            .await
                            {
                                Ok(Ok(_assoc)) => Some(true),
                                _ => Some(false),
                            };
                            ProxyProbeReport {
                                address: proxy.address.clone(),
                                kind: proxy.kind,
                                tcp_reachable: true,
                                handshake_ok: Some(true),
                                auth_ok: proxy.username.as_ref().map(|_| true),
                                latency_ms: Some(latency_ms),
                                udp_associate_supported: udp_supported,
                                error: None,
                            }
                        }
                        Ok(Err(err)) => {
                            let msg = err.to_string();
                            let auth_failed = msg.to_ascii_lowercase().contains("authentication");
                            ProxyProbeReport {
                                address: proxy.address.clone(),
                                kind: proxy.kind,
                                tcp_reachable: true,
                                handshake_ok: Some(false),
                                auth_ok: if auth_failed { Some(false) } else { None },
                                latency_ms: Some(latency_ms),
                                udp_associate_supported: Some(false),
                                error: Some(msg),
                            }
                        }
                        Err(_) => ProxyProbeReport {
                            address: proxy.address.clone(),
                            kind: proxy.kind,
                            tcp_reachable: true,
                            handshake_ok: Some(false),
                            auth_ok: None,
                            latency_ms: Some(latency_ms),
                            udp_associate_supported: Some(false),
                            error: Some("SOCKS5 handshake timed out".to_string()),
                        },
                    }
                }
                ProxyKind::Http => {
                    let hs =
                        tokio::time::timeout(timeout, http::probe_handshake_stream(&mut stream, proxy))
                            .await;
                    drop(stream);
                    match hs {
                        Ok(Ok(())) => ProxyProbeReport {
                            address: proxy.address.clone(),
                            kind: proxy.kind,
                            tcp_reachable: true,
                            handshake_ok: Some(true),
                            auth_ok: proxy.username.as_ref().map(|_| true),
                            latency_ms: Some(latency_ms),
                            udp_associate_supported: Some(false),
                            error: None,
                        },
                        Ok(Err(err)) => {
                            let msg = err.to_string();
                            let auth_failed = msg.contains("407");
                            ProxyProbeReport {
                                address: proxy.address.clone(),
                                kind: proxy.kind,
                                tcp_reachable: true,
                                handshake_ok: Some(false),
                                auth_ok: if auth_failed { Some(false) } else { None },
                                latency_ms: Some(latency_ms),
                                udp_associate_supported: Some(false),
                                error: Some(msg),
                            }
                        }
                        Err(_) => ProxyProbeReport {
                            address: proxy.address.clone(),
                            kind: proxy.kind,
                            tcp_reachable: true,
                            handshake_ok: Some(false),
                            auth_ok: None,
                            latency_ms: Some(latency_ms),
                            udp_associate_supported: Some(false),
                            error: Some("HTTP proxy handshake timed out".to_string()),
                        },
                    }
                }
            }
        }
        Ok(Err(e)) => ProxyProbeReport {
            address: proxy.address.clone(),
            kind: proxy.kind,
            tcp_reachable: false,
            handshake_ok: Some(false),
            auth_ok: None,
            latency_ms: None,
            udp_associate_supported: None,
            error: Some(e.to_string()),
        },
        Err(_) => ProxyProbeReport {
            address: proxy.address.clone(),
            kind: proxy.kind,
            tcp_reachable: false,
            handshake_ok: Some(false),
            auth_ok: None,
            latency_ms: None,
            udp_associate_supported: None,
            error: Some("connection timed out".to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn socks5_probe_detects_non_socks_tcp_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            if let Ok((mut s, _)) = listener.accept().await {
                let mut buf = [0u8; 16];
                let _ = s.read(&mut buf).await;
                let _ = s.write_all(b"SSH-2.0-OpenSSH\r\n").await;
            }
        });

        let cfg = ProxyConfig {
            kind: ProxyKind::Socks5,
            address: addr.to_string(),
            username: None,
            password: None,
            udp: true,
        };
        let report = probe_proxy(&cfg, Duration::from_secs(2)).await;
        assert!(report.tcp_reachable);
        assert_eq!(report.handshake_ok, Some(false));
        assert!(report.error.is_some());
        server.abort();
    }

    #[tokio::test]
    async fn http_probe_flags_407_auth_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                if let Ok((mut s, _)) = listener.accept().await {
                    let mut buf = [0u8; 256];
                    let _ = s.read(&mut buf).await;
                    let _ = s
                        .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n")
                        .await;
                }
            }
        });

        let cfg = ProxyConfig {
            kind: ProxyKind::Http,
            address: addr.to_string(),
            username: Some("alice".into()),
            password: Some("wrong".into()),
            udp: false,
        };
        let report = probe_proxy(&cfg, Duration::from_secs(2)).await;
        assert!(report.tcp_reachable);
        assert_eq!(report.handshake_ok, Some(false));
        assert_eq!(report.auth_ok, Some(false));
        server.abort();
    }
}
