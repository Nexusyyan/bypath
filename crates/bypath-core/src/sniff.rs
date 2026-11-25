/// Result of protocol sniffing on the initial payload of a TCP or UDP flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SniffResult {
    /// Hostname extracted from TLS ClientHello Server Name Indication (SNI).
    TlsSni(String),
    /// Hostname extracted from an HTTP/1.x `Host` request header.
    HttpHost(String),
    /// RFC 9000 QUIC Long Header Initial packet detected.
    QuicInitial,
}

/// Attempts to sniff a domain name from the first payload bytes of a TCP stream.
pub fn sniff_tcp_payload(data: &[u8]) -> Option<SniffResult> {
    if let Some(sni) = sniff_tls_sni(data) {
        return Some(SniffResult::TlsSni(sni));
    }
    if let Some(host) = sniff_http_host(data) {
        return Some(SniffResult::HttpHost(host));
    }
    None
}

/// Inspects a UDP datagram payload to detect QUIC Initial packets (RFC 9000 / v1 & v2).
pub fn sniff_udp_payload(data: &[u8]) -> Option<SniffResult> {
    if is_quic_initial(data) {
        return Some(SniffResult::QuicInitial);
    }
    None
}

/// Extracts the SNI hostname from a TLS 1.0-1.3 ClientHello record.
pub fn sniff_tls_sni(data: &[u8]) -> Option<String> {
    // TLS record header: 5 bytes
    // byte 0: ContentType = Handshake (0x16)
    // bytes 1..3: legacy_record_version (0x03 0x00..0x04)
    // bytes 3..5: record length
    if data.len() < 5 || data[0] != 0x16 || data[1] != 0x03 {
        return None;
    }
    let record_len = u16::from_be_bytes([data[3], data[4]]) as usize;
    let payload_end = (5 + record_len).min(data.len());
    let hs = &data[5..payload_end];

    // Handshake header: 4 bytes (HandshakeType = ClientHello = 0x01, 24-bit length)
    if hs.len() < 4 || hs[0] != 0x01 {
        return None;
    }
    let mut pos = 4usize;

    // ClientHello:
    // client_version (2) + random (32)
    if hs.len() < pos + 34 {
        return None;
    }
    pos += 34;

    // session_id (1-byte length + bytes)
    if hs.len() < pos + 1 {
        return None;
    }
    let session_id_len = hs[pos] as usize;
    pos += 1 + session_id_len;

    // cipher_suites (2-byte length + bytes)
    if hs.len() < pos + 2 {
        return None;
    }
    let cipher_len = u16::from_be_bytes([hs[pos], hs[pos + 1]]) as usize;
    pos += 2 + cipher_len;

    // compression_methods (1-byte length + bytes)
    if hs.len() < pos + 1 {
        return None;
    }
    let comp_len = hs[pos] as usize;
    pos += 1 + comp_len;

    // extensions (2-byte total length + extension entries)
    if hs.len() < pos + 2 {
        return None;
    }
    let ext_total_len = u16::from_be_bytes([hs[pos], hs[pos + 1]]) as usize;
    pos += 2;

    let ext_end = (pos + ext_total_len).min(hs.len());
    while pos + 4 <= ext_end {
        let ext_type = u16::from_be_bytes([hs[pos], hs[pos + 1]]);
        let ext_len = u16::from_be_bytes([hs[pos + 2], hs[pos + 3]]) as usize;
        pos += 4;
        if pos + ext_len > ext_end {
            break;
        }
        // Extension type 0x0000 = server_name
        if ext_type == 0x0000 && ext_len >= 5 {
            let ext = &hs[pos..pos + ext_len];
            let list_len = u16::from_be_bytes([ext[0], ext[1]]) as usize;
            let mut sni_pos = 2usize;
            let list_end = (2 + list_len).min(ext.len());
            while sni_pos + 3 <= list_end {
                let name_type = ext[sni_pos];
                let name_len = u16::from_be_bytes([ext[sni_pos + 1], ext[sni_pos + 2]]) as usize;
                sni_pos += 3;
                if sni_pos + name_len > list_end {
                    break;
                }
                if name_type == 0x00 {
                    let raw = &ext[sni_pos..sni_pos + name_len];
                    if let Ok(host) = std::str::from_utf8(raw) {
                        if is_valid_hostname(host) {
                            return Some(host.to_ascii_lowercase());
                        }
                    }
                }
                sni_pos += name_len;
            }
        }
        pos += ext_len;
    }
    None
}

/// Extracts the `Host` header value from an HTTP/1.x request.
pub fn sniff_http_host(data: &[u8]) -> Option<String> {
    const METHODS: &[&[u8]] = &[
        b"GET ",
        b"POST ",
        b"PUT ",
        b"DELETE ",
        b"HEAD ",
        b"OPTIONS ",
        b"PATCH ",
        b"CONNECT ",
        b"TRACE ",
    ];
    if !METHODS.iter().any(|m| data.starts_with(m)) {
        return None;
    }

    let text = std::str::from_utf8(&data[..data.len().min(4096)]).ok()?;
    for line in text.lines().skip(1) {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("host") {
                let val = v.trim();
                // Strip optional `:port` while keeping bracketed IPv6 safe
                let host = if val.starts_with('[') {
                    val.split(']').next()?.trim_start_matches('[')
                } else {
                    val.split(':').next()?
                };
                if is_valid_hostname(host) {
                    return Some(host.to_ascii_lowercase());
                }
            }
        }
    }
    None
}

/// Checks whether a UDP datagram matches RFC 9000 / RFC 9369 QUIC Initial Long Header format.
pub fn is_quic_initial(data: &[u8]) -> bool {
    // Minimum Initial packet header: 1 (flags) + 4 (version) + 1 (dcid_len) + 1 (scid_len)
    if data.len() < 16 {
        return false;
    }
    let first = data[0];
    // Long Header Form (bit 7 = 1) and Fixed Bit (bit 6 = 1)
    if (first & 0xC0) != 0xC0 {
        return false;
    }
    let version = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
    let long_type = (first >> 4) & 0x03;

    // QUIC v1 (RFC 9000): version 0x00000001, Initial type = 0b00
    // QUIC v2 (RFC 9369): version 0x6b3343cf, Initial type = 0b01
    let valid_version_type =
        (version == 0x0000_0001 && long_type == 0) || (version == 0x6b33_43cf && long_type == 1);
    if !valid_version_type {
        return false;
    }

    let dcid_len = data[5] as usize;
    if dcid_len > 20 || data.len() < 7 + dcid_len {
        return false;
    }
    let scid_len = data[6 + dcid_len] as usize;
    scid_len <= 20 && data.len() >= 7 + dcid_len + scid_len
}

fn is_valid_hostname(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    // Reject raw IP literals so we don't mistakenly treat an IP as a DNS hostname
    if host.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    host.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_client_hello_with_sni(hostname: &str) -> Vec<u8> {
        let host_bytes = hostname.as_bytes();
        // SNI extension body: list_len(2) + type(1) + name_len(2) + name
        let mut sni_ext = Vec::new();
        let list_len = (1 + 2 + host_bytes.len()) as u16;
        sni_ext.extend_from_slice(&list_len.to_be_bytes());
        sni_ext.push(0x00); // host_name
        sni_ext.extend_from_slice(&(host_bytes.len() as u16).to_be_bytes());
        sni_ext.extend_from_slice(host_bytes);

        let mut extensions = Vec::new();
        extensions.extend_from_slice(&0x0000u16.to_be_bytes()); // SNI ext type
        extensions.extend_from_slice(&(sni_ext.len() as u16).to_be_bytes());
        extensions.extend_from_slice(&sni_ext);

        let mut body = Vec::new();
        body.extend_from_slice(&[0x03, 0x03]); // TLS 1.2 legacy version
        body.extend_from_slice(&[0xAA; 32]); // Random
        body.push(0); // Session ID len = 0
        body.extend_from_slice(&[0x00, 0x02, 0x13, 0x01]); // 1 cipher suite
        body.extend_from_slice(&[0x01, 0x00]); // 1 compression method (null)
        body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
        body.extend_from_slice(&extensions);

        let mut handshake = Vec::new();
        handshake.push(0x01); // ClientHello
        let hs_len = body.len();
        handshake.push(((hs_len >> 16) & 0xFF) as u8);
        handshake.push(((hs_len >> 8) & 0xFF) as u8);
        handshake.push((hs_len & 0xFF) as u8);
        handshake.extend_from_slice(&body);

        let mut record = Vec::new();
        record.push(0x16); // Handshake
        record.extend_from_slice(&[0x03, 0x01]); // Record version
        record.extend_from_slice(&(handshake.len() as u16).to_be_bytes());
        record.extend_from_slice(&handshake);
        record
    }

    #[test]
    fn extracts_tls_sni() {
        let hello = build_client_hello_with_sni("cloudcode-pa.googleapis.com");
        assert_eq!(
            sniff_tcp_payload(&hello),
            Some(SniffResult::TlsSni(
                "cloudcode-pa.googleapis.com".to_string()
            ))
        );
    }

    #[test]
    fn survives_truncated_tls_packets() {
        let hello = build_client_hello_with_sni("example.com");
        for cut in 0..hello.len() {
            let _ = sniff_tls_sni(&hello[..cut]);
        }
    }

    #[test]
    fn extracts_http_host_without_port() {
        let req = b"GET /v1/models HTTP/1.1\r\nHost: api.example.com:8080\r\nUser-Agent: test\r\n\r\n";
        assert_eq!(
            sniff_tcp_payload(req),
            Some(SniffResult::HttpHost("api.example.com".to_string()))
        );
    }

    #[test]
    fn detects_quic_v1_and_v2_initial() {
        let mut pkt_v1 = vec![0u8; 32];
        pkt_v1[0] = 0xC3; // Long header + fixed bit + Initial (0)
        pkt_v1[1..5].copy_from_slice(&0x0000_0001u32.to_be_bytes());
        pkt_v1[5] = 8; // DCID len
        pkt_v1[14] = 0; // SCID len
        assert_eq!(sniff_udp_payload(&pkt_v1), Some(SniffResult::QuicInitial));

        let mut pkt_v2 = vec![0u8; 32];
        pkt_v2[0] = 0xD3; // Long header + fixed bit + Initial (1 for v2)
        pkt_v2[1..5].copy_from_slice(&0x6b33_43cfu32.to_be_bytes());
        pkt_v2[5] = 8;
        pkt_v2[14] = 4;
        assert_eq!(sniff_udp_payload(&pkt_v2), Some(SniffResult::QuicInitial));
    }
}
