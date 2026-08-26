//! FFI bindings and wire helpers for `libopenconnect-h3c` (our v9.21 fork).
//!
//! M0 provided the wire-format constants. M1 adds a pure-Rust frame codec
//! used for golden tests and later daemon diagnostics. Bindgen-generated
//! bindings for the fork arrive in M2, driven by `OPENCONNECT_H3C_DEV`.

/// Public libopenconnect API version we target (upstream v9.x, API 5.8).
pub const OPENCONNECT_API_VERSION_MAJOR: u32 = 5;
pub const OPENCONNECT_API_VERSION_MINOR: u32 = 8;

/// Maximum accepted H3C frame payload length, matching the fork's
/// `H3C_MAX_FRAME_LEN`.
pub const H3C_MAX_FRAME_LEN: u16 = 16384;

/// H3C data-plane frame types, measured against the live gateway.
pub mod h3c_frame {
    /// IPv4 payload frame (`01 00 | len BE | ipv4 packet`).
    pub const TYPE_IPV4: u16 = 1;
    /// Keepalive request (`02 00 00 00`).
    pub const TYPE_KEEPALIVE: u16 = 2;
    /// Keepalive response (`02 02 00 00`).
    pub const KEEPALIVE_RESPONSE: [u8; 4] = [0x02, 0x02, 0x00, 0x00];
}

/// Wire format of an H3C frame:
///
/// ```text
/// +--------+--------+----------...---+
/// | type   | len    | payload        |
/// | u16 LE | u16 BE | len bytes      |
/// +--------+--------+----------...---+
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum H3cFrame {
    Ipv4(Vec<u8>),
    KeepaliveRequest,
    KeepaliveResponse,
}

impl H3cFrame {
    pub fn new_ipv4(payload: impl Into<Vec<u8>>) -> Self {
        Self::Ipv4(payload.into())
    }

    pub fn new_keepalive() -> Self {
        Self::KeepaliveRequest
    }

    /// Serialize exactly as observed on the wire: `type` little-endian,
    /// `len` big-endian.
    pub fn to_wire(&self) -> Vec<u8> {
        match self {
            Self::Ipv4(payload) => {
                let mut out = Vec::with_capacity(4 + payload.len());
                out.extend_from_slice(&h3c_frame::TYPE_IPV4.to_le_bytes());
                out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
                out.extend_from_slice(payload);
                out
            }
            Self::KeepaliveRequest => vec![0x02, 0x00, 0x00, 0x00],
            Self::KeepaliveResponse => h3c_frame::KEEPALIVE_RESPONSE.to_vec(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// Not enough bytes for the header yet.
    Incomplete,
    /// Not enough bytes for the advertised payload yet.
    IncompletePayload { have: usize, need: usize },
    /// Advertised length exceeds H3C_MAX_FRAME_LEN.
    TooLarge(u16),
}

/// Streaming parser mirroring the C fork's `h3c_parse_frames()`.
///
/// Push arbitrary TLS record chunks; every push returns the complete frames
/// contained in those bytes. Leftover partial frames stay buffered.
#[derive(Debug, Default)]
pub struct H3cFrameStream {
    buffer: Vec<u8>,
}

impl H3cFrameStream {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    pub fn push(&mut self, data: &[u8]) -> Result<Vec<H3cFrame>, FrameError> {
        self.buffer.extend_from_slice(data);
        let mut frames = Vec::new();
        loop {
            if self.buffer.len() < 4 {
                return Ok(frames);
            }
            let len = u16::from_be_bytes([self.buffer[2], self.buffer[3]]) as usize;
            if len as u16 > H3C_MAX_FRAME_LEN {
                return Err(FrameError::TooLarge(len as u16));
            }
            let total = 4 + len;
            if self.buffer.len() < total {
                return Err(FrameError::IncompletePayload {
                    have: self.buffer.len() - 4,
                    need: len,
                });
            }

            let frame = &self.buffer[..total];
            if frame == h3c_frame::KEEPALIVE_RESPONSE {
                frames.push(H3cFrame::KeepaliveResponse);
            } else {
                let frame_type = u16::from_le_bytes([frame[0], frame[1]]);
                if frame_type == h3c_frame::TYPE_IPV4 {
                    frames.push(H3cFrame::Ipv4(frame[4..].to_vec()));
                }
                // Unknown types are dropped, matching the C parser.
            }
            self.buffer.drain(..total);
        }
    }

    pub fn buffered_len(&self) -> usize {
        self.buffer.len()
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
        assert_eq!(
            H3cFrame::KeepaliveResponse.to_wire(),
            h3c_frame::KEEPALIVE_RESPONSE
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

    #[test]
    fn parses_two_frames_from_one_record() {
        let mut stream = H3cFrameStream::new();
        let mut wire = H3cFrame::new_ipv4(vec![0x45, 0x00]).to_wire();
        wire.extend_from_slice(&H3cFrame::new_ipv4(vec![0x45, 0x01, 0x02]).to_wire());
        let frames = stream.push(&wire).unwrap();
        assert_eq!(
            frames,
            vec![
                H3cFrame::Ipv4(vec![0x45, 0x00]),
                H3cFrame::Ipv4(vec![0x45, 0x01, 0x02])
            ]
        );
        assert_eq!(stream.buffered_len(), 0);
    }

    #[test]
    fn keeps_half_frame_buffered() {
        let mut stream = H3cFrameStream::new();
        let wire = H3cFrame::new_ipv4(vec![0xde, 0xad, 0xbe, 0xef]).to_wire();
        match stream.push(&wire[..5]) {
            Err(FrameError::IncompletePayload { have: 1, need: 4 }) => {}
            other => panic!("expected IncompletePayload, got {other:?}"),
        }
        assert_eq!(stream.buffered_len(), 5);
        let frames = stream.push(&wire[5..]).unwrap();
        assert_eq!(frames, vec![H3cFrame::Ipv4(vec![0xde, 0xad, 0xbe, 0xef])]);
    }

    #[test]
    fn recognizes_server_keepalive_response() {
        let mut stream = H3cFrameStream::new();
        let frames = stream.push(&h3c_frame::KEEPALIVE_RESPONSE).unwrap();
        assert_eq!(frames, vec![H3cFrame::KeepaliveResponse]);
    }

    #[test]
    fn drops_unknown_frame_types() {
        let mut stream = H3cFrameStream::new();
        // type=6 LE (06 00), len=0
        let frames = stream.push(&[0x06, 0x00, 0x00, 0x00]).unwrap();
        assert!(frames.is_empty());
        assert_eq!(stream.buffered_len(), 0);
    }

    #[test]
    fn rejects_oversized_frame() {
        let mut stream = H3cFrameStream::new();
        let err = stream
            .push(&[0x01, 0x00, 0xff, 0xff])
            .expect_err("16384+ payload must be rejected");
        assert_eq!(err, FrameError::TooLarge(0xffff));
    }
}
