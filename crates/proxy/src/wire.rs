// SPDX-License-Identifier: BSD-3-Clause
//! `struct rpmsg_rpc_syscall`: a 16-byte little-endian header followed by
//! `data_len` bytes of data.

/// `open`.
pub const OPEN_SYSCALL_ID: u32 = 0x1;
/// `close`.
pub const CLOSE_SYSCALL_ID: u32 = 0x2;
/// `write`.
pub const WRITE_SYSCALL_ID: u32 = 0x3;
/// `read`.
pub const READ_SYSCALL_ID: u32 = 0x4;
/// Acknowledgement.
pub const ACK_STATUS_ID: u32 = 0x5;
/// Host asks the remote to shut the proxy down.
pub const TERM_SYSCALL_ID: u32 = 0x6;

/// Size of [`Syscall`] on the wire.
pub const SYSCALL_SIZE: usize = 16;

/// Request or response header.
///
/// | call  | `int_field1` | `int_field2` | data |
/// |-------|--------------|--------------|------|
/// | open  | flags → fd   | mode         | path, NUL-terminated |
/// | close | fd → result  | –            | – |
/// | write | fd → written | length       | bytes (+NUL for fd 1) |
/// | read  | fd → read    | length       | – → bytes |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Syscall {
    /// Call id.
    pub id: u32,
    /// First argument / return value.
    pub int_field1: i32,
    /// Second argument.
    pub int_field2: i32,
    /// Bytes of data following the header.
    pub data_len: u32,
}

impl Syscall {
    /// Encodes the header.
    pub fn to_bytes(&self) -> [u8; SYSCALL_SIZE] {
        let mut b = [0u8; SYSCALL_SIZE];
        b[0..4].copy_from_slice(&self.id.to_le_bytes());
        b[4..8].copy_from_slice(&self.int_field1.to_le_bytes());
        b[8..12].copy_from_slice(&self.int_field2.to_le_bytes());
        b[12..16].copy_from_slice(&self.data_len.to_le_bytes());
        b
    }

    /// Decodes a header from the start of `b`.
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let h = b.get(..SYSCALL_SIZE)?;
        let w = |i: usize| [h[i], h[i + 1], h[i + 2], h[i + 3]];
        Some(Syscall {
            id: u32::from_le_bytes(w(0)),
            int_field1: i32::from_le_bytes(w(4)),
            int_field2: i32::from_le_bytes(w(8)),
            data_len: u32::from_le_bytes(w(12)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        let s = Syscall {
            id: WRITE_SYSCALL_ID,
            int_field1: 1,
            int_field2: -2,
            data_len: 6,
        };
        let b = s.to_bytes();
        assert_eq!(b, [3, 0, 0, 0, 1, 0, 0, 0, 0xfe, 0xff, 0xff, 0xff, 6, 0, 0, 0]);
        assert_eq!(Syscall::from_bytes(&b), Some(s));
        assert_eq!(Syscall::from_bytes(&b[..15]), None);
    }
}
