/// Errors produced while decoding or encoding module bodies.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("unexpected end of body: needed {needed} bytes, {remaining} remaining")]
    UnexpectedEof { needed: usize, remaining: usize },
    #[error("{actual} unconsumed bytes after decoding module {module_id}")]
    TrailingBytes { module_id: i16, actual: usize },
    #[error("invalid UTF-8 string: {0}")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("invalid boolean value {0}")]
    InvalidBool(u8),
    #[error("unknown module id {0} and no raw fallback requested")]
    UnknownModule(i16),
    #[error("vector length {0} exceeds remaining body")]
    VecTooLong(usize),
    #[error("wire constant mismatch: expected {expected}, got {actual}")]
    ConstMismatch { expected: i64, actual: i64 },
    #[error("field '{0}' not present in packet")]
    MissingField(String),
    #[error("field '{field}' has unexpected type")]
    WrongType { field: String },
    #[error("body length {0} exceeds 24-bit frame maximum")]
    BodyTooLarge(usize),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

/// Errors surfaced by the session layer.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol violation: {0}")]
    Protocol(String),
    #[error("login rejected with status {0}")]
    LoginRejected(i16),
    #[error("connection is not in the required state")]
    BadState,
    /// Egress-proxy failure — fail closed, never retry direct. Maps to
    /// `botd`'s `ProxyFailure` exit-code category (69).
    #[error(transparent)]
    Proxy(#[from] crate::proxy::ProxyError),
}

pub type Result<T> = std::result::Result<T, ClientError>;
