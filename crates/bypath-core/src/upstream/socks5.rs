use crate::config::ProxyConfig;
use crate::upstream::TargetAddr;
use anyhow::{anyhow, bail, Context, Result};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

const SOCKS5_VERSION: u8 = 0x05;
const AUTH_NONE: u8 = 0x00;
const AUTH_USERPASS: u8 = 0x02;
const AUTH_NO_ACCEPTABLE: u8 = 0xFF;

const CMD_CONNECT: u8 = 0x01;
const CMD_UDP_ASSOCIATE: u8 = 0x03;

const ATYP_IPV4: u8 = 0x01;
const ATYP_DOMAIN: u8 = 0x03;
const ATYP_IPV6: u8 = 0x04;

/// Establishes a TCP tunnel through an RFC 1928 SOCKS5 proxy.
pub async fn connect_tcp(proxy: &ProxyConfig, target: &TargetAddr) -> Result<TcpStream> {
    let mut stream = TcpStream::connect(&proxy.address)
        .await
        .with_context(|| format!("failed to connect to SOCKS5 proxy {}", proxy.address))?;
    let _ = stream.set_nodelay(true);

    negotiate_auth(&mut stream, proxy).await?;
    send_request(&mut stream, CMD_CONNECT, target).await?;
    let _bound = read_reply(&mut stream).await?;
    Ok(stream)
}

/// Active SOCKS5 UDP ASSOCIATE session (control TCP socket must stay alive).
pub struct Socks5UdpAssociation {
    pub _control_stream: TcpStream,
    pub relay_addr: SocketAddr,
    pub udp_socket: UdpSocket,
}

/// Establishes a SOCKS5 UDP ASSOCIATE session per RFC 1928 Section 7.
pub async fn associate_udp(proxy: &ProxyConfig) -> Result<Socks5UdpAssociation> {
    let mut stream = TcpStream::connect(&proxy.address)
        .await
        .with_context(|| format!("failed to connect to SOCKS5 proxy {}", proxy.address))?;
    let _ = stream.set_nodelay(true);
    let proxy_peer_ip = stream.peer_addr()?.ip();

    negotiate_auth(&mut stream, proxy).await?;

    let bind_target = match proxy_peer_ip {
        IpAddr::V4(_) => TargetAddr::Ip(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)),
        IpAddr::V6(_) => TargetAddr::Ip(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)),
    };
    send_request(&mut stream, CMD_UDP_ASSOCIATE, &bind_target).await?;
    let mut relay_addr = read_reply(&mut stream).await?;

    // Many SOCKS5 servers return 0.0.0.0:<port> or [::]:<port> meaning "same host as proxy TCP".
    if relay_addr.ip().is_unspecified() {
        relay_addr.set_ip(proxy_peer_ip);
    }

    let local_bind: SocketAddr = match relay_addr {
        SocketAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        SocketAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
    };
    let udp_socket = UdpSocket::bind(local_bind).await?;

    Ok(Socks5UdpAssociation {
        _control_stream: stream,
        relay_addr,
        udp_socket,
    })
}

/// Encapsulates a UDP payload with the RFC 1928 SOCKS5 UDP request header.
pub fn encode_udp_packet(dst: &TargetAddr, payload: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(32 + payload.len());
    out.extend_from_slice(&[0x00, 0x00, 0x00]); // RSV(2) + FRAG(1)=0
    write_target_addr(&mut out, dst)?;
    out.extend_from_slice(payload);
    Ok(out)
}

/// Decapsulates an RFC 1928 SOCKS5 UDP reply packet into `(source_addr, payload_slice)`.
pub fn decode_udp_packet(packet: &[u8]) -> Result<(SocketAddr, &[u8])> {
    if packet.len() < 4 {
        bail!("SOCKS5 UDP packet too short");
    }
    if packet[2] != 0x00 {
        bail!("fragmented SOCKS5 UDP packets are not supported");
    }
    let atyp = packet[3];
    let (ip, next_pos) = match atyp {
        ATYP_IPV4 => {
            if packet.len() < 10 {
                bail!("truncated SOCKS5 UDP IPv4 header");
            }
            let ip = IpAddr::V4(Ipv4Addr::new(packet[4], packet[5], packet[6], packet[7]));
            (ip, 8)
        }
        ATYP_IPV6 => {
            if packet.len() < 22 {
                bail!("truncated SOCKS5 UDP IPv6 header");
            }
            let octets: [u8; 16] = packet[4..20].try_into()?;
            (IpAddr::V6(Ipv6Addr::from(octets)), 20)
        }
        ATYP_DOMAIN => {
            if packet.len() < 5 {
                bail!("truncated SOCKS5 UDP domain length");
            }
            let len = packet[4] as usize;
            if packet.len() < 5 + len + 2 {
                bail!("truncated SOCKS5 UDP domain header");
            }
            let host = std::str::from_utf8(&packet[5..5 + len])?;
            let ip = host
                .parse::<IpAddr>()
                .map_err(|_| anyhow!("SOCKS5 UDP reply used non-IP domain '{host}'"))?;
            (ip, 5 + len)
        }
        other => bail!("invalid SOCKS5 UDP ATYP {other:#x}"),
    };
    let port = u16::from_be_bytes([packet[next_pos], packet[next_pos + 1]]);
    Ok((SocketAddr::new(ip, port), &packet[next_pos + 2..]))
}

async fn negotiate_auth(stream: &mut TcpStream, proxy: &ProxyConfig) -> Result<()> {
    let has_creds = proxy.username.is_some();
    if has_creds {
        stream
            .write_all(&[SOCKS5_VERSION, 2, AUTH_NONE, AUTH_USERPASS])
            .await?;
    } else {
        stream.write_all(&[SOCKS5_VERSION, 1, AUTH_NONE]).await?;
    }

    let mut resp = [0u8; 2];
    stream.read_exact(&mut resp).await?;
    if resp[0] != SOCKS5_VERSION {
        bail!("unexpected SOCKS version {:#x}", resp[0]);
    }

    match resp[1] {
        AUTH_NONE => Ok(()),
        AUTH_USERPASS => {
            let user = proxy.username.as_deref().unwrap_or_default().as_bytes();
            let pass = proxy.password.as_deref().unwrap_or_default().as_bytes();
            if user.len() > 255 || pass.len() > 255 {
                bail!("SOCKS5 credentials exceed 255 bytes");
            }
            let mut auth_req = Vec::with_capacity(3 + user.len() + pass.len());
            auth_req.push(0x01); // RFC 1929 subnegotiation version
            auth_req.push(user.len() as u8);
            auth_req.extend_from_slice(user);
            auth_req.push(pass.len() as u8);
            auth_req.extend_from_slice(pass);
            stream.write_all(&auth_req).await?;

            let mut auth_resp = [0u8; 2];
            stream.read_exact(&mut auth_resp).await?;
            if auth_resp[1] != 0x00 {
                bail!("SOCKS5 username/password authentication rejected");
            }
            Ok(())
        }
        AUTH_NO_ACCEPTABLE => bail!("SOCKS5 proxy rejected offered authentication methods"),
        other => bail!("unsupported SOCKS5 auth method {other:#x}"),
    }
}

async fn send_request(stream: &mut TcpStream, cmd: u8, target: &TargetAddr) -> Result<()> {
    let mut req = Vec::with_capacity(32);
    req.push(SOCKS5_VERSION);
    req.push(cmd);
    req.push(0x00); // RSV
    write_target_addr(&mut req, target)?;
    stream.write_all(&req).await?;
    Ok(())
}

fn write_target_addr(buf: &mut Vec<u8>, target: &TargetAddr) -> Result<()> {
    match target {
        TargetAddr::Ip(SocketAddr::V4(v4)) => {
            buf.push(ATYP_IPV4);
            buf.extend_from_slice(&v4.ip().octets());
            buf.extend_from_slice(&v4.port().to_be_bytes());
        }
        TargetAddr::Ip(SocketAddr::V6(v6)) => {
            buf.push(ATYP_IPV6);
            buf.extend_from_slice(&v6.ip().octets());
            buf.extend_from_slice(&v6.port().to_be_bytes());
        }
        TargetAddr::Domain(host, port) => {
            let bytes = host.as_bytes();
            if bytes.is_empty() || bytes.len() > 255 {
                bail!("invalid SOCKS5 domain length: {}", bytes.len());
            }
            buf.push(ATYP_DOMAIN);
            buf.push(bytes.len() as u8);
            buf.extend_from_slice(bytes);
            buf.extend_from_slice(&port.to_be_bytes());
        }
    }
    Ok(())
}

async fn read_reply(stream: &mut TcpStream) -> Result<SocketAddr> {
    let mut hdr = [0u8; 4];
    stream.read_exact(&mut hdr).await?;
    if hdr[0] != SOCKS5_VERSION {
        bail!("unexpected SOCKS5 reply version {:#x}", hdr[0]);
    }
    if hdr[1] != 0x00 {
        bail!("SOCKS5 request failed with reply code {:#x}", hdr[1]);
    }

    let ip = match hdr[3] {
        ATYP_IPV4 => {
            let mut addr = [0u8; 4];
            stream.read_exact(&mut addr).await?;
            IpAddr::V4(Ipv4Addr::from(addr))
        }
        ATYP_IPV6 => {
            let mut addr = [0u8; 16];
            stream.read_exact(&mut addr).await?;
            IpAddr::V6(Ipv6Addr::from(addr))
        }
        ATYP_DOMAIN => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await?;
            let mut domain = vec![0u8; len[0] as usize];
            stream.read_exact(&mut domain).await?;
            let s = std::str::from_utf8(&domain)?;
            s.parse::<IpAddr>()
                .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
        }
        other => bail!("invalid SOCKS5 reply ATYP {other:#x}"),
    };

    let mut port_bytes = [0u8; 2];
    stream.read_exact(&mut port_bytes).await?;
    let port = u16::from_be_bytes(port_bytes);
    Ok(SocketAddr::new(ip, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn udp_encode_decode_roundtrip_ipv4_and_ipv6() {
        let dst4 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 53);
        let enc4 = encode_udp_packet(&TargetAddr::Ip(dst4), b"dns-query").unwrap();
        let (dec4, payload4) = decode_udp_packet(&enc4).unwrap();
        assert_eq!(dec4, dst4);
        assert_eq!(payload4, b"dns-query");

        let dst6 = SocketAddr::new(IpAddr::V6("2606:4700:4700::1111".parse().unwrap()), 443);
        let enc6 = encode_udp_packet(&TargetAddr::Ip(dst6), b"quic").unwrap();
        let (dec6, payload6) = decode_udp_packet(&enc6).unwrap();
        assert_eq!(dec6, dst6);
        assert_eq!(payload6, b"quic");
    }
}
