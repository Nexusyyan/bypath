//! Exact WinDivert 2.2 C ABI definitions verified against official `include/windivert.h`.

use std::ffi::c_void;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub type Handle = *mut c_void;
pub const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;

// WINDIVERT_LAYER enum values
pub const WINDIVERT_LAYER_NETWORK: u32 = 0;
pub const WINDIVERT_LAYER_NETWORK_FORWARD: u32 = 1;
pub const WINDIVERT_LAYER_FLOW: u32 = 2;
pub const WINDIVERT_LAYER_SOCKET: u32 = 3;
pub const WINDIVERT_LAYER_REFLECT: u32 = 4;

// WINDIVERT_EVENT enum values
pub const WINDIVERT_EVENT_NETWORK_PACKET: u8 = 0;
pub const WINDIVERT_EVENT_FLOW_ESTABLISHED: u8 = 1;
pub const WINDIVERT_EVENT_FLOW_DELETED: u8 = 2;
pub const WINDIVERT_EVENT_SOCKET_BIND: u8 = 3;
pub const WINDIVERT_EVENT_SOCKET_CONNECT: u8 = 4;
pub const WINDIVERT_EVENT_SOCKET_LISTEN: u8 = 5;
pub const WINDIVERT_EVENT_SOCKET_ACCEPT: u8 = 6;
pub const WINDIVERT_EVENT_SOCKET_CLOSE: u8 = 7;

// WINDIVERT_FLAG_* bitmask
pub const WINDIVERT_FLAG_SNIFF: u64 = 0x0001;
pub const WINDIVERT_FLAG_DROP: u64 = 0x0002;
pub const WINDIVERT_FLAG_RECV_ONLY: u64 = 0x0004;
pub const WINDIVERT_FLAG_SEND_ONLY: u64 = 0x0008;
pub const WINDIVERT_FLAG_NO_INSTALL: u64 = 0x0010;
pub const WINDIVERT_FLAG_FRAGMENTS: u64 = 0x0020;

// WINDIVERT_PARAM enum values
pub const WINDIVERT_PARAM_QUEUE_LENGTH: u32 = 0;
pub const WINDIVERT_PARAM_QUEUE_TIME: u32 = 1;
pub const WINDIVERT_PARAM_QUEUE_SIZE: u32 = 2;
pub const WINDIVERT_PARAM_VERSION_MAJOR: u32 = 3;
pub const WINDIVERT_PARAM_VERSION_MINOR: u32 = 4;

// WINDIVERT_SHUTDOWN enum values
pub const WINDIVERT_SHUTDOWN_RECV: u32 = 0x1;
pub const WINDIVERT_SHUTDOWN_SEND: u32 = 0x2;
pub const WINDIVERT_SHUTDOWN_BOTH: u32 = 0x3;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct WinDivertDataNetwork {
    pub if_idx: u32,
    pub sub_if_idx: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct WinDivertDataFlow {
    pub endpoint_id: u64,
    pub parent_endpoint_id: u64,
    pub process_id: u32,
    pub local_addr: [u32; 4],
    pub remote_addr: [u32; 4],
    pub local_port: u16,
    pub remote_port: u16,
    pub protocol: u8,
}

pub type WinDivertDataSocket = WinDivertDataFlow;

#[repr(C)]
#[derive(Clone, Copy)]
pub union WinDivertDataUnion {
    pub network: WinDivertDataNetwork,
    pub flow: WinDivertDataFlow,
    pub socket: WinDivertDataSocket,
    pub reserved3: [u8; 64],
}

impl Default for WinDivertDataUnion {
    fn default() -> Self {
        Self { reserved3: [0u8; 64] }
    }
}

/// Exact 80-byte `WINDIVERT_ADDRESS` layout matching WinDivert 2.2 `include/windivert.h`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct WinDivertAddress {
    pub timestamp: i64,
    /// Bitfields packed into little-endian UINT32:
    /// - bits  0..=7 : Layer
    /// - bits  8..=15: Event
    /// - bit   16    : Sniffed
    /// - bit   17    : Outbound
    /// - bit   18    : Loopback
    /// - bit   19    : Impostor
    /// - bit   20    : IPv6
    /// - bit   21    : IPChecksum
    /// - bit   22    : TCPChecksum
    /// - bit   23    : UDPChecksum
    /// - bits 24..=31: Reserved1
    pub flags: u32,
    pub reserved2: u32,
    pub data: WinDivertDataUnion,
}

impl WinDivertAddress {
    #[inline]
    pub fn layer(&self) -> u8 {
        (self.flags & 0xFF) as u8
    }

    #[inline]
    pub fn set_layer(&mut self, layer: u8) {
        self.flags = (self.flags & !0xFF) | (layer as u32);
    }

    #[inline]
    pub fn event(&self) -> u8 {
        ((self.flags >> 8) & 0xFF) as u8
    }

    #[inline]
    pub fn sniffed(&self) -> bool {
        (self.flags & (1 << 16)) != 0
    }

    #[inline]
    pub fn outbound(&self) -> bool {
        (self.flags & (1 << 17)) != 0
    }

    #[inline]
    pub fn set_outbound(&mut self, val: bool) {
        if val {
            self.flags |= 1 << 17;
        } else {
            self.flags &= !(1 << 17);
        }
    }

    #[inline]
    pub fn loopback(&self) -> bool {
        (self.flags & (1 << 18)) != 0
    }

    #[inline]
    pub fn ipv6(&self) -> bool {
        (self.flags & (1 << 20)) != 0
    }

    #[inline]
    pub fn set_ipv6(&mut self, val: bool) {
        if val {
            self.flags |= 1 << 20;
        } else {
            self.flags &= !(1 << 20);
        }
    }

    #[inline]
    pub fn network(&self) -> WinDivertDataNetwork {
        unsafe { self.data.network }
    }

    #[inline]
    pub fn set_network(&mut self, net: WinDivertDataNetwork) {
        self.data.network = net;
    }

    #[inline]
    pub fn socket(&self) -> WinDivertDataSocket {
        unsafe { self.data.socket }
    }

    #[inline]
    pub fn flow(&self) -> WinDivertDataFlow {
        unsafe { self.data.flow }
    }
}

/// Converts a WinDivert 2.2 host-order 128-bit address (`[u32; 4]`) into a standard `IpAddr`.
///
/// Verified against `WinDivertByteSwap128` in `dll/windivert_helper.c`:
/// - `raw[0]` is the least significant 32 bits in host byte order.
/// - For IPv4 (`!is_ipv6`) or IPv4-mapped IPv6 (`::ffff:a.b.c.d`), `raw[0]` is the IPv4 address.
/// - For IPv6, network-order bytes are `[raw[3].to_be_bytes(), raw[2].to_be_bytes(), raw[1].to_be_bytes(), raw[0].to_be_bytes()]`.
pub fn decode_windivert_addr(raw: [u32; 4], is_ipv6: bool) -> IpAddr {
    if !is_ipv6 {
        return IpAddr::V4(Ipv4Addr::from(raw[0]));
    }
    if raw[3] == 0 && raw[2] == 0 && raw[1] == 0x0000_FFFF {
        return IpAddr::V4(Ipv4Addr::from(raw[0]));
    }
    let mut bytes = [0u8; 16];
    bytes[0..4].copy_from_slice(&raw[3].to_be_bytes());
    bytes[4..8].copy_from_slice(&raw[2].to_be_bytes());
    bytes[8..12].copy_from_slice(&raw[1].to_be_bytes());
    bytes[12..16].copy_from_slice(&raw[0].to_be_bytes());
    IpAddr::V6(Ipv6Addr::from(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    #[test]
    fn windivert_address_abi_matches_header() {
        assert_eq!(size_of::<WinDivertDataNetwork>(), 8);
        assert_eq!(size_of::<WinDivertDataFlow>(), 64);
        assert_eq!(size_of::<WinDivertDataUnion>(), 64);
        assert_eq!(size_of::<WinDivertAddress>(), 80);
        assert_eq!(align_of::<WinDivertAddress>(), 8);

        assert_eq!(offset_of!(WinDivertAddress, timestamp), 0);
        assert_eq!(offset_of!(WinDivertAddress, flags), 8);
        assert_eq!(offset_of!(WinDivertAddress, reserved2), 12);
        assert_eq!(offset_of!(WinDivertAddress, data), 16);

        assert_eq!(offset_of!(WinDivertDataFlow, endpoint_id), 0);
        assert_eq!(offset_of!(WinDivertDataFlow, parent_endpoint_id), 8);
        assert_eq!(offset_of!(WinDivertDataFlow, process_id), 16);
        assert_eq!(offset_of!(WinDivertDataFlow, local_addr), 20);
        assert_eq!(offset_of!(WinDivertDataFlow, remote_addr), 36);
        assert_eq!(offset_of!(WinDivertDataFlow, local_port), 52);
        assert_eq!(offset_of!(WinDivertDataFlow, remote_port), 54);
        assert_eq!(offset_of!(WinDivertDataFlow, protocol), 56);
    }

    #[test]
    fn bitfield_accessors_match_spec() {
        let mut addr = WinDivertAddress::default();
        addr.set_layer(WINDIVERT_LAYER_NETWORK as u8);
        addr.set_outbound(true);
        addr.set_ipv6(true);
        assert_eq!(addr.layer(), 0);
        assert!(addr.outbound());
        assert!(addr.ipv6());
        assert!(!addr.loopback());
        assert_eq!(addr.flags, (1 << 17) | (1 << 20));
    }

    #[test]
    fn decodes_ipv4_and_ipv6_host_order_addrs() {
        let v4 = decode_windivert_addr([0x8EFA_BE4E, 0, 0, 0], false);
        assert_eq!(v4, IpAddr::V4(Ipv4Addr::new(142, 250, 190, 78)));

        let mapped = decode_windivert_addr([0x0101_0101, 0x0000_FFFF, 0, 0], true);
        assert_eq!(mapped, IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)));

        let v6 = decode_windivert_addr([0x0000_8888, 0, 0x4860_0000, 0x2001_4860], true);
        assert_eq!(v6, IpAddr::V6("2001:4860:4860::8888".parse().unwrap()));
    }
}
