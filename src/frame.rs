//! 24-bit big-endian frame layer.
//!
//! Every application message on the wire is
//! `[3-byte unsigned BE body length][body]`, where the body starts with
//! the signed i16 module id. This mirrors `SocketManager.as` which
//! writes `(len >> 16) & 0xFF` then `len & 0xFFFF`.

use crate::error::CodecError;
use crate::packet::{Packet, RawPacket, decode_body, encode_body};
use std::io::{Read, Write};

pub const MAX_BODY_LEN: usize = 0x00FF_FFFF;
const HEADER_LEN: usize = 3;

/// Encode a packet body and prepend the 3-byte frame header.
pub fn frame_packet(packet: &Packet) -> Result<Vec<u8>, CodecError> {
    let body = encode_body(packet)?;
    frame_body(&body)
}

/// Wrap an already-serialized body in the frame header.
pub fn frame_body(body: &[u8]) -> Result<Vec<u8>, CodecError> {
    if body.len() > MAX_BODY_LEN {
        return Err(CodecError::BodyTooLarge(body.len()));
    }
    let mut out = Vec::with_capacity(HEADER_LEN + body.len());
    let len = body.len() as u32;
    out.push(((len >> 16) & 0xFF) as u8);
    out.push(((len >> 8) & 0xFF) as u8);
    out.push((len & 0xFF) as u8);
    out.extend_from_slice(body);
    Ok(out)
}

/// Parse a frame header.
pub fn parse_header(header: &[u8; 3]) -> usize {
    ((header[0] as usize) << 16) | ((header[1] as usize) << 8) | header[2] as usize
}

/// Incremental frame decoder fed with raw stream bytes.
///
/// Feed with `push`, then drain with `next_body` until `None`.
#[derive(Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Pop the next complete message body, if one is buffered.
    pub fn next_body(&mut self) -> Option<Vec<u8>> {
        if self.buf.len() < HEADER_LEN {
            return None;
        }
        let len = parse_header(self.buf[..3].try_into().unwrap());
        if self.buf.len() < HEADER_LEN + len {
            return None;
        }
        let body = self.buf[HEADER_LEN..HEADER_LEN + len].to_vec();
        self.buf.drain(..HEADER_LEN + len);
        Some(body)
    }

    /// Decode the next complete buffered frame into a packet.
    pub fn next_packet(&mut self, strict: bool) -> Option<Result<Packet, CodecError>> {
        self.next_body().map(|b| decode_body(&b, strict))
    }

    /// Decode the next frame into a `RawPacket` without schema lookup.
    pub fn next_raw(&mut self) -> Option<RawPacket> {
        self.next_body().map(|b| {
            let id = if b.len() >= 2 {
                i16::from_be_bytes([b[0], b[1]])
            } else {
                0
            };
            RawPacket { id, body: b }
        })
    }

    pub fn buffered(&self) -> usize {
        self.buf.len()
    }
}

/// Blocking helpers for stream-based sessions.
pub mod stream {
    use super::*;

    /// Read exactly one frame body from a blocking stream.
    pub fn read_body(r: &mut impl Read) -> Result<Vec<u8>, CodecError> {
        let mut header = [0u8; 3];
        r.read_exact(&mut header)?;
        let len = parse_header(&header);
        let mut body = vec![0u8; len];
        r.read_exact(&mut body)?;
        Ok(body)
    }

    /// Read one frame and decode it.
    pub fn read_packet(r: &mut impl Read, strict: bool) -> Result<Packet, CodecError> {
        decode_body(&read_body(r)?, strict)
    }

    /// Encode and write one framed packet.
    pub fn write_packet(w: &mut impl Write, packet: &Packet) -> Result<(), CodecError> {
        w.write_all(&frame_packet(packet)?)?;
        Ok(())
    }
}
