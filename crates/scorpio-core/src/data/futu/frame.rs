//! OpenD 44-byte frame header encode/decode + body SHA-1. Pure and socket-free.

use sha1::{Digest, Sha1};

use super::{MAX_BODY_LEN, PROTO_FMT_JSON, PROTO_VER};

/// Fixed OpenD frame-header length in bytes.
pub(crate) const FUTU_HEADER_LEN: usize = 44;
const HEADER_FLAG: [u8; 2] = *b"FT";

/// Decoded frame header (little-endian fields).
pub(crate) struct FrameHeader {
    pub proto_id: u32,
    pub proto_fmt: u8,
    pub proto_ver: u8,
    pub serial: u32,
    pub body_len: u32,
    pub body_sha1: [u8; 20],
}

/// SHA-1 of the JSON body bytes.
pub(crate) fn body_sha1(body: &[u8]) -> [u8; 20] {
    let mut hasher = Sha1::new();
    hasher.update(body);
    hasher.finalize().into()
}

/// Encode a full frame: 44-byte header + body appended verbatim.
pub(crate) fn encode_frame(proto_id: u32, serial: u32, body: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(FUTU_HEADER_LEN + body.len());
    buf.extend_from_slice(&HEADER_FLAG); // 0..2
    buf.extend_from_slice(&proto_id.to_le_bytes()); // 2..6
    buf.push(PROTO_FMT_JSON); // 6
    buf.push(PROTO_VER); // 7
    buf.extend_from_slice(&serial.to_le_bytes()); // 8..12
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes()); // 12..16
    buf.extend_from_slice(&body_sha1(body)); // 16..36
    buf.extend_from_slice(&[0u8; 8]); // 36..44 reserved
    buf.extend_from_slice(body); // 44..
    buf
}

/// Decode a 44-byte header, rejecting bad magic, short buffers, and oversized
/// `nBodyLen`.
pub(crate) fn decode_header(bytes: &[u8]) -> Result<FrameHeader, String> {
    if bytes.len() < FUTU_HEADER_LEN {
        return Err("OpenD frame header truncated".to_owned());
    }
    if bytes[0..2] != HEADER_FLAG {
        return Err("OpenD frame: bad magic".to_owned());
    }
    let proto_id = u32::from_le_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]);
    let proto_fmt = bytes[6];
    let proto_ver = bytes[7];
    let serial = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let body_len = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    if body_len > MAX_BODY_LEN {
        return Err(format!("OpenD frame: body too large ({body_len} bytes)"));
    }
    let mut body_sha1 = [0u8; 20];
    body_sha1.copy_from_slice(&bytes[16..36]);
    Ok(FrameHeader {
        proto_id,
        proto_fmt,
        proto_ver,
        serial,
        body_len,
        body_sha1,
    })
}

/// Validate a response header against the request it answers.
pub(crate) fn validate_response_header(
    header: &FrameHeader,
    expected_proto: u32,
    expected_serial: u32,
) -> Result<(), String> {
    if header.proto_id != expected_proto {
        return Err(format!(
            "OpenD frame: proto id mismatch (got {}, expected {expected_proto})",
            header.proto_id
        ));
    }
    if header.serial != expected_serial {
        return Err(format!(
            "OpenD frame: serial mismatch (got {}, expected {expected_serial})",
            header.serial
        ));
    }
    if header.proto_fmt != PROTO_FMT_JSON {
        return Err(format!(
            "OpenD frame: unexpected format {}",
            header.proto_fmt
        ));
    }
    if header.proto_ver != PROTO_VER {
        return Err(format!(
            "OpenD frame: unexpected version {}",
            header.proto_ver
        ));
    }
    Ok(())
}

/// Verify the body matches the header's SHA-1.
pub(crate) fn verify_body_sha1(header: &FrameHeader, body: &[u8]) -> Result<(), String> {
    if body_sha1(body) != header.body_sha1 {
        return Err("OpenD frame: body SHA-1 mismatch".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::futu::{PROTO_GET_ACC_LIST, PROTO_GET_POSITION_LIST, PROTO_INIT_CONNECT};

    #[test]
    fn encodes_header_with_exact_byte_layout() {
        let body = br#"{"c2s":{}}"#;
        let frame = encode_frame(PROTO_INIT_CONNECT, 1, body);
        assert_eq!(&frame[0..2], b"FT");
        assert_eq!(&frame[2..6], &1001u32.to_le_bytes()); // nProtoID
        assert_eq!(frame[6], PROTO_FMT_JSON);
        assert_eq!(frame[7], PROTO_VER);
        assert_eq!(&frame[8..12], &1u32.to_le_bytes()); // nSerialNo
        assert_eq!(&frame[12..16], &(body.len() as u32).to_le_bytes()); // nBodyLen
        assert_eq!(&frame[36..44], &[0u8; 8]); // reserved
        assert_eq!(&frame[44..], &body[..]); // body appended verbatim
        assert_eq!(frame.len(), FUTU_HEADER_LEN + body.len());
    }

    #[test]
    fn body_sha1_matches_known_vector() {
        // SHA-1("abc") = a9993e364706816aba3e25717850c26c9cd0d89d
        let digest = body_sha1(b"abc");
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn header_and_body_round_trip() {
        let body = br#"{"retType":0}"#;
        let frame = encode_frame(PROTO_GET_POSITION_LIST, 7, body);
        let header = decode_header(&frame[..FUTU_HEADER_LEN]).expect("decode");
        assert_eq!(header.proto_id, PROTO_GET_POSITION_LIST);
        assert_eq!(header.serial, 7);
        assert_eq!(header.body_len as usize, body.len());
        verify_body_sha1(&header, body).expect("sha1 must match");
    }

    #[test]
    fn decode_rejects_bad_magic() {
        let mut frame = encode_frame(PROTO_INIT_CONNECT, 1, b"{}");
        frame[0] = b'X';
        assert!(decode_header(&frame[..FUTU_HEADER_LEN]).is_err());
    }

    #[test]
    fn decode_rejects_short_buffer() {
        assert!(decode_header(&[0u8; 10]).is_err());
    }

    #[test]
    fn decode_rejects_oversized_body_len() {
        let mut frame = encode_frame(PROTO_INIT_CONNECT, 1, b"{}");
        frame[12..16].copy_from_slice(&(MAX_BODY_LEN + 1).to_le_bytes());
        assert!(decode_header(&frame[..FUTU_HEADER_LEN]).is_err());
    }

    #[test]
    fn verify_body_sha1_rejects_tampered_body() {
        let frame = encode_frame(PROTO_INIT_CONNECT, 1, b"{}");
        let header = decode_header(&frame[..FUTU_HEADER_LEN]).unwrap();
        assert!(verify_body_sha1(&header, b"{ }").is_err());
    }

    #[test]
    fn response_validation_rejects_mismatched_proto_and_serial() {
        let frame = encode_frame(PROTO_GET_ACC_LIST, 7, b"{}");
        let header = decode_header(&frame[..FUTU_HEADER_LEN]).unwrap();
        assert!(validate_response_header(&header, PROTO_INIT_CONNECT, 7).is_err());
        assert!(validate_response_header(&header, PROTO_GET_ACC_LIST, 8).is_err());
        assert!(validate_response_header(&header, PROTO_GET_ACC_LIST, 7).is_ok());
    }

    #[test]
    fn response_validation_rejects_unexpected_format_or_version() {
        let frame = encode_frame(PROTO_INIT_CONNECT, 1, b"{}");
        let mut wrong_fmt = decode_header(&frame[..FUTU_HEADER_LEN]).unwrap();
        wrong_fmt.proto_fmt = PROTO_FMT_JSON + 1; // e.g. protobuf
        assert!(validate_response_header(&wrong_fmt, PROTO_INIT_CONNECT, 1).is_err());

        let mut wrong_ver = decode_header(&frame[..FUTU_HEADER_LEN]).unwrap();
        wrong_ver.proto_ver = PROTO_VER + 9;
        assert!(validate_response_header(&wrong_ver, PROTO_INIT_CONNECT, 1).is_err());
    }
}
