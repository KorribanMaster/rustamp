// SPDX-License-Identifier: BSD-3-Clause
//! On-the-wire structures (`struct rpmsg_hdr`, `struct rpmsg_ns_msg`).
//! Little-endian, packed, identical to the Linux kernel definitions.

use crate::{NAME_SIZE, Name};

/// Size of [`Header`] on the wire.
pub const HDR_SIZE: usize = 16;
/// Size of [`NsMsg`] on the wire.
pub const NS_MSG_SIZE: usize = NAME_SIZE + 8;
/// [`NsMsg::flags`]: service created.
pub const NS_CREATE: u32 = 0;
/// [`NsMsg::flags`]: service destroyed.
pub const NS_DESTROY: u32 = 1;

/// Message header preceding every payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Source endpoint address.
    pub src: u32,
    /// Destination endpoint address.
    pub dst: u32,
    /// Reserved (OpenAMP uses it locally for buffer bookkeeping).
    pub reserved: u32,
    /// Payload length.
    pub len: u16,
    /// Flags (unused, zero).
    pub flags: u16,
}

impl Header {
    /// Encodes the header.
    pub fn to_bytes(&self) -> [u8; HDR_SIZE] {
        let mut b = [0u8; HDR_SIZE];
        b[0..4].copy_from_slice(&self.src.to_le_bytes());
        b[4..8].copy_from_slice(&self.dst.to_le_bytes());
        b[8..12].copy_from_slice(&self.reserved.to_le_bytes());
        b[12..14].copy_from_slice(&self.len.to_le_bytes());
        b[14..16].copy_from_slice(&self.flags.to_le_bytes());
        b
    }

    /// Decodes a header.
    pub fn from_bytes(b: &[u8; HDR_SIZE]) -> Self {
        let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        Header {
            src: u32_at(0),
            dst: u32_at(4),
            reserved: u32_at(8),
            len: u16::from_le_bytes([b[12], b[13]]),
            flags: u16::from_le_bytes([b[14], b[15]]),
        }
    }
}

/// Name-service announcement, sent to [`NS_EPT_ADDR`](crate::NS_EPT_ADDR).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NsMsg {
    /// Service name.
    pub name: Name,
    /// Address of the endpoint providing the service.
    pub addr: u32,
    /// [`NS_CREATE`] or [`NS_DESTROY`].
    pub flags: u32,
}

impl NsMsg {
    /// Encodes the message.
    pub fn to_bytes(&self) -> [u8; NS_MSG_SIZE] {
        let mut b = [0u8; NS_MSG_SIZE];
        b[..NAME_SIZE].copy_from_slice(&self.name.to_wire());
        b[NAME_SIZE..NAME_SIZE + 4].copy_from_slice(&self.addr.to_le_bytes());
        b[NAME_SIZE + 4..].copy_from_slice(&self.flags.to_le_bytes());
        b
    }

    /// Decodes a message; `None` if `b` has the wrong length (the C library
    /// ignores such messages too).
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() != NS_MSG_SIZE {
            return None;
        }
        let mut raw = [0u8; NAME_SIZE];
        raw.copy_from_slice(&b[..NAME_SIZE]);
        let w = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        Some(NsMsg {
            name: Name::from_wire(&raw),
            addr: w(NAME_SIZE),
            flags: w(NAME_SIZE + 4),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_layout_matches_kernel() {
        let h = Header {
            src: 0x0400,
            dst: 0x35,
            reserved: 0,
            len: 40,
            flags: 0,
        };
        let b = h.to_bytes();
        assert_eq!(
            b,
            [0, 4, 0, 0, 0x35, 0, 0, 0, 0, 0, 0, 0, 40, 0, 0, 0],
            "struct rpmsg_hdr {{ u32 src, dst, reserved; u16 len, flags; }}"
        );
        assert_eq!(Header::from_bytes(&b), h);
    }

    #[test]
    fn ns_msg_layout() {
        let m = NsMsg {
            name: Name::new("rpmsg-raw").unwrap(),
            addr: 0x401,
            flags: NS_DESTROY,
        };
        let b = m.to_bytes();
        assert_eq!(&b[..9], b"rpmsg-raw");
        assert!(b[9..32].iter().all(|&c| c == 0));
        assert_eq!(&b[32..], &[1, 4, 0, 0, 1, 0, 0, 0]);
        assert_eq!(NsMsg::from_bytes(&b), Some(m));
        assert_eq!(NsMsg::from_bytes(&b[..39]), None);
    }
}
