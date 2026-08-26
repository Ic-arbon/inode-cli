//! Golden protocol fixtures captured from the live gateway on 2026-08-26.
//!
//! See `docs/architecture.md` appendix A for the capture methodology.

use inode_openconnect_sys::{H3cFrame, H3cFrameStream};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{FIXTURES}/{name}")).unwrap()
}

#[test]
fn golden_keepalive_request_wire_bytes() {
    assert_eq!(
        fixture("h3c-keepalive-request.bin"),
        H3cFrame::new_keepalive().to_wire()
    );
}

#[test]
fn golden_keepalive_response_parses() {
    let mut stream = H3cFrameStream::new();
    let frames = stream.push(&fixture("h3c-keepalive-response.bin")).unwrap();
    assert_eq!(frames, vec![H3cFrame::KeepaliveResponse]);
}

#[test]
fn golden_ipv4_probe_round_trip() {
    let wire = fixture("h3c-ipv4-probe.bin");
    let mut stream = H3cFrameStream::new();
    let frames = stream.push(&wire).unwrap();
    let expected = H3cFrame::new_ipv4(wire[4..].to_vec());
    assert_eq!(frames, vec![expected]);
    assert_eq!(stream.push(&wire).unwrap()[0].to_wire(), wire);
}
