use crate::codec::{Reader, Writer};
use crate::error::CodecError;
use crate::op::{Elem, IntWidth, Op};
use crate::registry;

/// Decoded value of a single named field.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    F32(f32),
    F64(f64),
    Str(String),
    /// A self-describing nested module (may itself be unknown).
    Module(Box<Packet>),
    /// Null nested module (wire id `0`).
    Null,
    List(Vec<Value>),
}

/// A decoded module: its id plus the named fields defined by its schema.
///
/// Fields appear in wire order. Unknown trailing/inline data is never
/// produced because decode is exact: any bytes left unread raise
/// `CodecError::TrailingBytes` at the caller's option.
#[derive(Clone, Debug, PartialEq)]
pub struct Packet {
    pub id: i16,
    /// Schema type name when the id is registered, else `None`.
    pub name: Option<&'static str>,
    pub fields: Vec<(Option<&'static str>, Value)>,
}

impl Packet {
    /// First field with the given name.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields
            .iter()
            .find(|(n, _)| *n == Some(name))
            .map(|(_, v)| v)
    }

    pub fn set(&mut self, name: &str, value: Value) {
        for (n, v) in &mut self.fields {
            if *n == Some(name) {
                *v = value;
                return;
            }
        }
        self.fields.push((None, value));
    }

    pub fn int(&self, name: &str) -> Result<i64, CodecError> {
        match self.get(name) {
            Some(Value::Int(v)) => Ok(*v),
            Some(_) => Err(CodecError::WrongType {
                field: name.to_string(),
            }),
            None => Err(CodecError::MissingField(name.to_string())),
        }
    }

    pub fn string(&self, name: &str) -> Result<&str, CodecError> {
        match self.get(name) {
            Some(Value::Str(s)) => Ok(s),
            Some(_) => Err(CodecError::WrongType {
                field: name.to_string(),
            }),
            None => Err(CodecError::MissingField(name.to_string())),
        }
    }

    pub fn boolean(&self, name: &str) -> Result<bool, CodecError> {
        match self.get(name) {
            Some(Value::Bool(v)) => Ok(*v),
            Some(_) => Err(CodecError::WrongType {
                field: name.to_string(),
            }),
            None => Err(CodecError::MissingField(name.to_string())),
        }
    }

    pub fn f64(&self, name: &str) -> Result<f64, CodecError> {
        match self.get(name) {
            Some(Value::F64(v)) => Ok(*v),
            Some(Value::Int(v)) => Ok(*v as f64),
            Some(_) => Err(CodecError::WrongType {
                field: name.to_string(),
            }),
            None => Err(CodecError::MissingField(name.to_string())),
        }
    }

    pub fn module(&self, name: &str) -> Result<&Packet, CodecError> {
        match self.get(name) {
            Some(Value::Module(m)) => Ok(m),
            Some(Value::Null) => Err(CodecError::MissingField(name.to_string())),
            Some(_) => Err(CodecError::WrongType {
                field: name.to_string(),
            }),
            None => Err(CodecError::MissingField(name.to_string())),
        }
    }

    pub fn list(&self, name: &str) -> Result<&[Value], CodecError> {
        match self.get(name) {
            Some(Value::List(v)) => Ok(v),
            Some(_) => Err(CodecError::WrongType {
                field: name.to_string(),
            }),
            None => Err(CodecError::MissingField(name.to_string())),
        }
    }
}

/// A module body whose id has no registered schema. The raw bytes are
/// kept so callers can still inspect or forward them.
#[derive(Clone, Debug, PartialEq)]
pub struct RawPacket {
    pub id: i16,
    pub body: Vec<u8>,
}

/// Decode one module body (bytes after the frame's 3-byte length) into a
/// `Packet` using its registered schema. Unknown ids yield
/// `Err(CodecError::UnknownModule)`; use `decode_or_raw` to keep them.
///
/// `ops` are applied in wire order. `Op::Const` verifies the wire value.
pub fn decode_body(body: &[u8], strict_consumed: bool) -> Result<Packet, CodecError> {
    let mut r = Reader::new(body);
    let id = r.i16()?;
    let schema = registry::schema(id).ok_or(CodecError::UnknownModule(id))?;
    let packet = decode_fields(&mut r, schema.read, id)?;
    if strict_consumed && !r.is_empty() {
        return Err(CodecError::TrailingBytes {
            module_id: id,
            actual: r.remaining(),
        });
    }
    Ok(packet)
}

fn decode_fields(r: &mut Reader<'_>, ops: &[Op], module_id: i16) -> Result<Packet, CodecError> {
    let mut fields = Vec::with_capacity(ops.len());
    for op in ops {
        let name = op.name();
        match op {
            Op::Int { width, rot, .. } => {
                let v = match rot {
                    Some((dir, bits)) => r.int_rot(*width, *dir, *bits)?,
                    None => r.int(*width)?,
                };
                fields.push((name, Value::Int(v)));
            }
            Op::Bool { .. } => fields.push((name, Value::Bool(r.bool()?))),
            Op::F32 { .. } => fields.push((name, Value::F32(r.f32()?))),
            Op::F64 { .. } => fields.push((name, Value::F64(r.f64()?))),
            Op::Utf { .. } => fields.push((name, Value::Str(r.utf()?))),
            Op::Skip { width } => {
                r.skip(width.bytes())?;
            }
            Op::Nested { .. } => fields.push((name, decode_nested(r)?)),
            Op::Vec { count, elem, .. } => {
                fields.push((name, Value::List(decode_vec(r, *count, *elem)?)))
            }
            Op::Const { value, width } => {
                let actual = r.int(*width)?;
                if actual != *value as i64 {
                    return Err(CodecError::ConstMismatch {
                        expected: *value as i64,
                        actual,
                    });
                }
            }
        }
    }
    Ok(Packet {
        id: module_id,
        name: registry::schema(module_id).map(|s| s.name),
        fields,
    })
}

/// Decode a nested module slot: i16 id then that module's body ops.
/// `0` decodes as `Value::Null`. Unknown ids consume nothing further
/// (callers should treat the payload as opaque and stop).
fn decode_nested(r: &mut Reader<'_>) -> Result<Value, CodecError> {
    let id = r.i16()?;
    if id == 0 {
        return Ok(Value::Null);
    }
    match registry::schema(id) {
        Some(schema) => {
            let inner = decode_fields(r, schema.read, id)?;
            Ok(Value::Module(Box::new(inner)))
        }
        // Unknown nested module: no length information exists on the
        // wire, so we cannot skip it safely. Surface it as an error;
        // all currently observed schemas are registered.
        None => Err(CodecError::UnknownModule(id)),
    }
}

fn decode_vec(r: &mut Reader<'_>, count: IntWidth, elem: Elem) -> Result<Vec<Value>, CodecError> {
    let n = r.int(count)?;
    if n < 0 || n as usize > r.remaining() {
        return Err(CodecError::VecTooLong(n.max(0) as usize));
    }
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        out.push(decode_elem(r, elem)?);
    }
    Ok(out)
}

fn decode_elem(r: &mut Reader<'_>, elem: Elem) -> Result<Value, CodecError> {
    Ok(match elem {
        Elem::Int { width, rot } => Value::Int(match rot {
            Some((dir, bits)) => r.int_rot(width, dir, bits)?,
            None => r.int(width)?,
        }),
        Elem::Bool => Value::Bool(r.bool()?),
        Elem::F32 => Value::F32(r.f32()?),
        Elem::F64 => Value::F64(r.f64()?),
        Elem::Utf => Value::Str(r.utf()?),
        Elem::Nested => decode_nested(r)?,
    })
}

/// Encode a `Packet` to a module body using the schema's write ops.
/// Fields are pulled by name; ops without names (`Skip`) write zeroes,
/// `Const` writes its literal.
pub fn encode_body(packet: &Packet) -> Result<Vec<u8>, CodecError> {
    let schema = registry::schema(packet.id).ok_or(CodecError::UnknownModule(packet.id))?;
    let mut w = Writer::new();
    w.i16(packet.id);
    encode_fields(&mut w, schema.write, packet)?;
    Ok(w.into_bytes())
}

fn encode_fields(w: &mut Writer, ops: &[Op], packet: &Packet) -> Result<(), CodecError> {
    for op in ops {
        match op {
            Op::Int { name, width, rot } => {
                let v = int_field(packet, *name)?;
                match rot {
                    Some((dir, bits)) => w.int_rot(*width, *dir, *bits, v),
                    None => w.int(*width, v),
                }
            }
            Op::Bool { name } => w.bool(bool_field(packet, *name)?),
            Op::F32 { name } => w.f32(float_field(packet, *name)? as f32),
            Op::F64 { name } => w.f64(float_field(packet, *name)?),
            Op::Utf { name } => {
                let s = match name.and_then(|n| packet.get(n)) {
                    Some(Value::Str(s)) => s.clone(),
                    Some(_) => {
                        return Err(CodecError::WrongType {
                            field: name.unwrap().to_string(),
                        });
                    }
                    None => return Err(CodecError::MissingField(name.unwrap().to_string())),
                };
                w.utf(&s)?;
            }
            Op::Skip { width } => w.int(*width, 0),
            Op::Nested { name } => encode_nested(w, packet, *name)?,
            Op::Vec {
                name, count, elem, ..
            } => encode_vec(w, packet, *name, *count, *elem)?,
            Op::Const { value, width } => w.int(*width, *value as i64),
        }
    }
    Ok(())
}

fn int_field(packet: &Packet, name: Option<&'static str>) -> Result<i64, CodecError> {
    let name = name.ok_or_else(|| CodecError::MissingField(String::new()))?;
    match packet.get(name) {
        Some(Value::Int(v)) => Ok(*v),
        Some(Value::F64(v)) => Ok(*v as i64),
        Some(_) => Err(CodecError::WrongType {
            field: name.to_string(),
        }),
        None => Err(CodecError::MissingField(name.to_string())),
    }
}

fn bool_field(packet: &Packet, name: Option<&'static str>) -> Result<bool, CodecError> {
    let name = name.ok_or_else(|| CodecError::MissingField(String::new()))?;
    match packet.get(name) {
        Some(Value::Bool(v)) => Ok(*v),
        Some(_) => Err(CodecError::WrongType {
            field: name.to_string(),
        }),
        None => Err(CodecError::MissingField(name.to_string())),
    }
}

fn float_field(packet: &Packet, name: Option<&'static str>) -> Result<f64, CodecError> {
    let name = name.ok_or_else(|| CodecError::MissingField(String::new()))?;
    match packet.get(name) {
        Some(Value::F64(v)) => Ok(*v),
        Some(Value::F32(v)) => Ok(*v as f64),
        Some(Value::Int(v)) => Ok(*v as f64),
        Some(_) => Err(CodecError::WrongType {
            field: name.to_string(),
        }),
        None => Err(CodecError::MissingField(name.to_string())),
    }
}

fn encode_nested(
    w: &mut Writer,
    packet: &Packet,
    name: Option<&'static str>,
) -> Result<(), CodecError> {
    let name = name.ok_or_else(|| CodecError::MissingField(String::new()))?;
    match packet.get(name) {
        Some(Value::Module(m)) => {
            w.i16(m.id);
            let schema = registry::schema(m.id).ok_or(CodecError::UnknownModule(m.id))?;
            encode_fields(w, schema.write, m)
        }
        Some(Value::Null) | None => {
            // AS3 writes a zero module id for a missing nested module.
            w.i16(0);
            Ok(())
        }
        Some(_) => Err(CodecError::WrongType {
            field: name.to_string(),
        }),
    }
}

fn encode_vec(
    w: &mut Writer,
    packet: &Packet,
    name: Option<&'static str>,
    count: IntWidth,
    elem: Elem,
) -> Result<(), CodecError> {
    let items = match name.and_then(|n| packet.get(n)) {
        Some(Value::List(v)) => v.clone(),
        Some(_) => {
            return Err(CodecError::WrongType {
                field: name.unwrap().to_string(),
            });
        }
        None => Vec::new(),
    };
    w.int(count, items.len() as i64);
    for item in &items {
        encode_elem(w, elem, item)?;
    }
    Ok(())
}

fn encode_elem(w: &mut Writer, elem: Elem, v: &Value) -> Result<(), CodecError> {
    match (elem, v) {
        (Elem::Int { width, rot }, Value::Int(n)) => {
            match rot {
                Some((dir, bits)) => w.int_rot(width, dir, bits, *n),
                None => w.int(width, *n),
            }
            Ok(())
        }
        (Elem::Bool, Value::Bool(b)) => {
            w.bool(*b);
            Ok(())
        }
        (Elem::F32, Value::F32(f)) => {
            w.f32(*f);
            Ok(())
        }
        (Elem::F32, Value::F64(f)) => {
            w.f32(*f as f32);
            Ok(())
        }
        (Elem::F64, Value::F64(f)) => {
            w.f64(*f);
            Ok(())
        }
        (Elem::Utf, Value::Str(s)) => w.utf(s),
        (Elem::Nested, Value::Module(m)) => {
            w.i16(m.id);
            let schema = registry::schema(m.id).ok_or(CodecError::UnknownModule(m.id))?;
            encode_fields(w, schema.write, m)
        }
        (Elem::Nested, Value::Null) => {
            w.i16(0);
            Ok(())
        }
        _ => Err(CodecError::WrongType {
            field: "<elem>".to_string(),
        }),
    }
}
