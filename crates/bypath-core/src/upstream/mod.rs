pub mod http;
pub mod probe;
pub mod socks5;

use crate::config::{ProxyConfig, ProxyKind};
use anyhow::Result;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use tokio::net::TcpStream;

/// Destination address requested through an upstream proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetAddr {
    /// Raw IP address + port (used when sniffing yields no hostname).
    Ip(SocketAddr),
    /// Sniffed domain name + port (sent as SOCKS5 ATYP=0x03 or HTTP CONNECT host:port
    /// so the upstream proxy performs clean remote DNS resolution).
    Domain(String, u16),
}

impl TargetAddr {
    pub fn port(&self) -> u16 {
        match self {
            TargetAddr::Ip(sa) => sa.port(),
            TargetAddr::Domain(_, p) => *p,
        }
    }

    pub fn from_ip_port(ip: IpAddr, port: u16) -> Self {
        TargetAddr::Ip(SocketAddr::new(ip, port))
    }
}

impl fmt::Display for TargetAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TargetAddr::Ip(sa) => write!(f, "{sa}"),
            TargetAddr::Domain(host, port) => write!(f, "{host}:{port}"),
        }
    }
}

/// Connects a TCP tunnel to `target` via the configured upstream proxy.
pub async fn dial_tcp_via_proxy(proxy: &ProxyConfig, target: &TargetAddr) -> Result<TcpStream> {
    match proxy.kind {
        ProxyKind::Socks5 => socks5::connect_tcp(proxy, target).await,
        ProxyKind::Http => http::connect_tcp(proxy, target).await,
    }
}
