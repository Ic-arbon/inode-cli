//! FFI bindings for `libopenconnect-h3c` (our v9.21 fork).
//!
//! M0 skeleton: only the API-version contract is declared. Bindgen-generated
//! bindings for the fork will replace this file in M2, driven by
//! `OPENCONNECT_H3C_DEV` (include dir + library dir) or a future `build.rs`.

/// Public libopenconnect API version we target (upstream v9.x, API 5.8).
pub const OPENCONNECT_API_VERSION_MAJOR: u32 = 5;
pub const OPENCONNECT_API_VERSION_MINOR: u32 = 8;

/// H3C data-plane frame types, measured against the live gateway.
pub mod h3c_frame {
    /// IPv4 payload frame (`01 00 | len BE | ipv4 packet`).
    pub const TYPE_IPV4: u16 = 1;
    /// Keepalive request (`02 00 00 00`).
    pub const TYPE_KEEPALIVE: u16 = 2;
}

/// Wire format of an H3C frame.
///
/// ```text
/// +--------+--------+----------...---+
/// | type   | len    | payload        |
/// | u16 LE | u16 BE | len bytes      |
/// +--------+--------+----------...---+
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H3cFrame {
    pub frame_type: u16,
    pub payload: Vec<u8>,
}

impl H3cFrame {
    pub fn new_ipv4(payload: impl Into<Vec<u8>>) -> Self {
        Self {
            frame_type: h3c_frame::TYPE_IPV4,
            payload: payload.into(),
        }
    }

    pub fn new_keepalive() -> Self {
        Self {
            frame_type: h3c_frame::TYPE_KEEPALIVE,
            payload: Vec::new(),
        }
    }

    /// Serialize exactly as observed on the wire: `type` little-endian,
    /// `len` big-endian.
    pub fn to_wire(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.payload.len());
        out.extend_from_slice(&self.frame_type.to_le_bytes());
        out.extend_from_slice(&(self.payload.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keepalive_wire_format_matches_capture() {
        assert_eq!(
            H3cFrame::new_keepalive().to_wire(),
            vec![0x02, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn ipv4_wire_format_matches_capture() {
        // Probe captured on the wire: 01 00 | 00 27 | 45 00 ...
        let frame = H3cFrame::new_ipv4(vec![0x45, 0x00]);
        let wire = frame.to_wire();
        assert_eq!(&wire[..4], &[0x01, 0x00, 0x00, 0x02]);
        assert_eq!(&wire[4..], &[0x45, 0x00]);
    }
}
