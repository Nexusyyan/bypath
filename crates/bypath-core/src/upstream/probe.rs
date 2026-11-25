use crate::config::{ProxyConfig, ProxyKind};
use crate::upstream::socks5;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyProbeReport {
    pub address: String,
    pub kind: ProxyKind,
    pub tcp_reachable: bool,
    pub latency_ms: Option<u64>,
    pub udp_associate_supported: Option<bool>,
    pub error: Option<String>,
}

/// Probes an upstream proxy for TCP reachability and (for SOCKS5) UDP ASSOCIATE capability.
pub async fn probe_proxy(proxy: &ProxyConfig, timeout: Duration) -> ProxyProbeReport {
    let start = Instant::now();
    let tcp_res = tokio::time::timeout(timeout, TcpStream::connect(&proxy.address)).await;
    match tcp_res {
        Ok(Ok(_stream)) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            let udp_supported = if proxy.kind == ProxyKind::Socks5 {
                match tokio::time::timeout(timeout, socks5::associate_udp(proxy)).await {
                    Ok(Ok(_assoc)) => Some(true),
                    _ => Some(false),
                }
            } else {
                Some(false)
            };

            ProxyProbeReport {
                address: proxy.address.clone(),
                kind: proxy.kind,
                tcp_reachable: true,
                latency_ms: Some(latency_ms),
                udp_associate_supported: udp_supported,
                error: None,
            }
        }
        Ok(Err(e)) => ProxyProbeReport {
            address: proxy.address.clone(),
            kind: proxy.kind,
            tcp_reachable: false,
            latency_ms: None,
            udp_associate_supported: None,
            error: Some(e.to_string()),
        },
        Err(_) => ProxyProbeReport {
            address: proxy.address.clone(),
            kind: proxy.kind,
            tcp_reachable: false,
            latency_ms: None,
            udp_associate_supported: None,
            error: Some("connection timed out".to_string()),
        },
    }
}
