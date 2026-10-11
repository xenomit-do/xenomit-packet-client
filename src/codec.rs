use crate::error::CodecError;
use crate::op::{IntWidth, RotDir};

/// Bounds-checked big-endian reader over a single module body.
///
/// Every accessor mirrors an AS3 `IDataInput` call: `readByte`,
/// `readShort`, `readInt`, `readFloat`, `readDouble`, `readBoolean`,
/// `readUTF`.
#[derive(Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub const fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub const fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub const fn position(&self) -> usize {
        self.pos
    }

    pub const fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], CodecError> {
        if self.remaining() < n {
            return Err(CodecError::UnexpectedEof {
                needed: n,
                remaining: self.remaining(),
            });
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn skip(&mut self, n: usize) -> Result<(), CodecError> {
        self.take(n).map(|_| ())
    }

    pub fn u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.take(1)?[0])
    }

    pub fn i8(&mut self) -> Result<i8, CodecError> {
        Ok(self.u8()? as i8)
    }

    pub fn u16(&mut self) -> Result<u16, CodecError> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub fn i16(&mut self) -> Result<i16, CodecError> {
        Ok(self.u16()? as i16)
    }

    pub fn u32(&mut self) -> Result<u32, CodecError> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn i32(&mut self) -> Result<i32, CodecError> {
        Ok(self.u32()? as i32)
    }

    pub fn f32(&mut self) -> Result<f32, CodecError> {
        Ok(f32::from_bits(self.u32()?))
    }

    pub fn f64(&mut self) -> Result<f64, CodecError> {
        Ok(f64::from_bits(u64::from_be_bytes(
            self.take(8)?.try_into().unwrap(),
        )))
    }

    /// AS3 `readBoolean`: one byte, `!= 0` is true.
    pub fn bool(&mut self) -> Result<bool, CodecError> {
        Ok(self.u8()? != 0)
    }

    /// AS3 `readUTF`: u16 length prefix followed by modified-UTF-8 bytes.
    /// The game emits plain UTF-8 for its data; a length of `0xFFFF`
    /// means a null string in some builds.
    pub fn utf(&mut self) -> Result<String, CodecError> {
        let len = self.u16()? as usize;
        let bytes = self.take(len)?;
        Ok(std::str::from_utf8(bytes)?.to_owned())
    }

    /// Read an integer of the given width, preserving sign.
    pub fn int(&mut self, width: IntWidth) -> Result<i64, CodecError> {
        match width {
            IntWidth::W8 => Ok(self.i8()? as i64),
            IntWidth::W16 => Ok(self.i16()? as i64),
            IntWidth::W32 => Ok(self.i32()? as i64),
        }
    }

    /// Read an integer then rotate the raw wire value into the logical
    /// value (`value = wire.rotate(dir, bits)` at the field's width).
    pub fn int_rot(&mut self, width: IntWidth, dir: RotDir, bits: u8) -> Result<i64, CodecError> {
        let wire = match width {
            IntWidth::W8 => self.u8()? as u64,
            IntWidth::W16 => self.u16()? as u64,
            IntWidth::W32 => self.u32()? as u64,
        };
        let b = bits as u32;
        let v = match (width, dir) {
            (IntWidth::W8, RotDir::Rol) => (wire as u8).rotate_left(b) as u64,
            (IntWidth::W8, RotDir::Ror) => (wire as u8).rotate_right(b) as u64,
            (IntWidth::W16, RotDir::Rol) => (wire as u16).rotate_left(b) as u64,
            (IntWidth::W16, RotDir::Ror) => (wire as u16).rotate_right(b) as u64,
            (IntWidth::W32, RotDir::Rol) => (wire as u32).rotate_left(b) as u64,
            (IntWidth::W32, RotDir::Ror) => (wire as u32).rotate_right(b) as u64,
        };
        Ok(match width {
            IntWidth::W8 => v as u8 as i8 as i64,
            IntWidth::W16 => v as u16 as i16 as i64,
            IntWidth::W32 => v as u32 as i32 as i64,
        })
    }
}

/// Big-endian writer accumulating one module body.
#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn i8(&mut self, v: i8) {
        self.u8(v as u8);
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn i16(&mut self, v: i16) {
        self.u16(v as u16);
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.u32(v as u32);
    }

    pub fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }

    pub fn f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_bits().to_be_bytes());
    }

    pub fn bool(&mut self, v: bool) {
        self.u8(v as u8);
    }

    /// AS3 `writeUTF`: u16 byte length then the UTF-8 bytes.
    pub fn utf(&mut self, s: &str) -> Result<(), CodecError> {
        let len = s.len();
        if len > u16::MAX as usize {
            return Err(CodecError::BodyTooLarge(len));
        }
        self.u16(len as u16);
        self.buf.extend_from_slice(s.as_bytes());
        Ok(())
    }

    /// Write an integer truncated to `width`.
    pub fn int(&mut self, width: IntWidth, v: i64) {
        match width {
            IntWidth::W8 => self.u8(v as u8),
            IntWidth::W16 => self.u16(v as u16),
            IntWidth::W32 => self.u32(v as u32),
        }
    }

    /// Apply the inverse of a decode rotation then write the result:
    /// `wire = value.rotate(dir.invert(), bits)`.
    pub fn int_rot(&mut self, width: IntWidth, dir: RotDir, bits: u8, v: i64) {
        let b = bits as u32;
        let inv = dir.invert();
        match (width, inv) {
            (IntWidth::W8, RotDir::Rol) => self.u8((v as u8).rotate_left(b)),
            (IntWidth::W8, RotDir::Ror) => self.u8((v as u8).rotate_right(b)),
            (IntWidth::W16, RotDir::Rol) => self.u16((v as u16).rotate_left(b)),
            (IntWidth::W16, RotDir::Ror) => self.u16((v as u16).rotate_right(b)),
            (IntWidth::W32, RotDir::Rol) => self.u32((v as u32).rotate_left(b)),
            (IntWidth::W32, RotDir::Ror) => self.u32((v as u32).rotate_right(b)),
        }
    }
}
