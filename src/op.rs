/// Primitive field widths used by the protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntWidth {
    W8,
    W16,
    W32,
}

impl IntWidth {
    pub const fn bytes(self) -> usize {
        match self {
            Self::W8 => 1,
            Self::W16 => 2,
            Self::W32 => 4,
        }
    }
}

/// Rotation direction, expressed in the DECODE direction:
/// `value = wire.rotate(dir, bits)` when reading. Encoding applies the
/// inverse rotation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RotDir {
    Rol,
    Ror,
}

impl RotDir {
    pub const fn invert(self) -> Self {
        match self {
            Self::Rol => Self::Ror,
            Self::Ror => Self::Rol,
        }
    }
}

/// A single wire-level operation inside a module body.
///
/// All ops appear in wire order. For `Op::Int`, `rot` describes how the
/// raw wire integer transforms into the logical value on decode; `None`
/// means the value is stored verbatim.
#[derive(Clone, Debug)]
pub enum Op {
    /// Integer field, optionally obfuscated with a bit rotation.
    Int {
        name: Option<&'static str>,
        width: IntWidth,
        rot: Option<(RotDir, u8)>,
    },
    Bool {
        name: Option<&'static str>,
    },
    F32 {
        name: Option<&'static str>,
    },
    F64 {
        name: Option<&'static str>,
    },
    /// AS3 `readUTF`/`writeUTF`: u16 length prefix + bytes.
    Utf {
        name: Option<&'static str>,
    },
    /// Fixed-width bytes that the reference client reads and discards.
    Skip {
        width: IntWidth,
    },
    /// A self-describing nested module: `[i16 module id][module body]`.
    /// `0` encodes a null module on write.
    Nested {
        name: Option<&'static str>,
    },
    /// Count-prefixed vector of homogeneous elements.
    Vec {
        name: Option<&'static str>,
        count: IntWidth,
        elem: Elem,
    },
    /// Constant written by the reference client (decode verifies it).
    Const {
        value: i32,
        width: IntWidth,
    },
}

impl Op {
    pub const fn name(&self) -> Option<&'static str> {
        match self {
            Self::Int { name, .. }
            | Self::Bool { name }
            | Self::F32 { name }
            | Self::F64 { name }
            | Self::Utf { name }
            | Self::Nested { name }
            | Self::Vec { name, .. } => *name,
            Self::Skip { .. } | Self::Const { .. } => None,
        }
    }
}

/// Element type inside a `Vec` op.
#[derive(Clone, Copy, Debug)]
pub enum Elem {
    Int {
        width: IntWidth,
        rot: Option<(RotDir, u8)>,
    },
    Bool,
    F32,
    F64,
    /// u16 length-prefixed string per element.
    Utf,
    /// Each element is a self-describing nested module.
    Nested,
}
