// SPDX-License-Identifier: BSD-3-Clause
use core::fmt;

use crate::{Error, NAME_SIZE, Result};

/// An endpoint or service name: up to 31 bytes, NUL-padded on the wire.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Name {
    bytes: [u8; NAME_SIZE],
    len: u8,
}

impl Name {
    /// The empty name (an anonymous endpoint, never announced).
    pub const EMPTY: Name = Name {
        bytes: [0; NAME_SIZE],
        len: 0,
    };

    /// A name from a string of at most 31 bytes without NULs.
    pub fn new(s: &str) -> Result<Self> {
        let b = s.as_bytes();
        if b.len() >= NAME_SIZE || b.contains(&0) {
            return Err(Error::InvalidParam);
        }
        let mut bytes = [0; NAME_SIZE];
        bytes[..b.len()].copy_from_slice(b);
        Ok(Name {
            bytes,
            len: b.len() as u8,
        })
    }

    /// Decodes a NUL-padded wire name (up to 32 bytes, NUL optional).
    pub fn from_wire(raw: &[u8; NAME_SIZE]) -> Self {
        let len = raw.iter().position(|&c| c == 0).unwrap_or(NAME_SIZE);
        let mut bytes = [0; NAME_SIZE];
        bytes[..len].copy_from_slice(&raw[..len]);
        Name {
            bytes,
            len: len as u8,
        }
    }

    /// The NUL-padded wire form.
    pub fn to_wire(&self) -> [u8; NAME_SIZE] {
        self.bytes
    }

    /// The name's bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// The name as a string, if it is valid UTF-8.
    pub fn as_str(&self) -> Option<&str> {
        core::str::from_utf8(self.as_bytes()).ok()
    }

    /// Whether the name is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.as_str() {
            Some(s) => fmt::Debug::fmt(s, f),
            None => fmt::Debug::fmt(self.as_bytes(), f),
        }
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for c in self.as_bytes().utf8_chunks() {
            f.write_str(c.valid())?;
            if !c.invalid().is_empty() {
                f.write_str("\u{fffd}")?;
            }
        }
        Ok(())
    }
}

impl PartialEq<str> for Name {
    fn eq(&self, other: &str) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_limits() {
        let n = Name::new("rpmsg-tty").unwrap();
        assert_eq!(&n, "rpmsg-tty");
        assert_eq!(Name::from_wire(&n.to_wire()), n);
        assert!(Name::new(&"x".repeat(31)).is_ok());
        assert_eq!(Name::new(&"x".repeat(32)), Err(Error::InvalidParam));
        assert_eq!(Name::new("a\0b"), Err(Error::InvalidParam));
        // A full 32-byte wire name without NUL is accepted on receive.
        let full = Name::from_wire(&[b'z'; NAME_SIZE]);
        assert_eq!(full.as_bytes().len(), 32);
    }
}
