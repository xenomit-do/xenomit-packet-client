//! `xenomit-packet-client` — a packet-level DarkOrbit game client.
//!
//! # Protocol summary
//!
//! * Transport: plain TCP, big-endian.
//! * Framing: `[u24 BE body length][body]`; body starts with the signed
//!   `i16` module id.
//! * On connect, the client writes a single `0x00` greeting byte before
//!   the first frame.
//! * Fields may be obfuscated with fixed-width bit rotations; all
//!   schemas are extracted from the decompiled client and embedded in
//!   [`generated::schema::SCHEMAS`].
//!
//! # Usage
//!
//! ```no_run
//! use xenomit_packet_client::session::{LoginInfo, Session};
//!
//! let login = LoginInfo::new(12345, "SESSION_ID_FROM_AUTH");
//! let mut s = Session::connect_and_login("127.0.0.1:6063", &login)?;
//! s.wait_hero_init(512)?;
//! s.complete_ready("2D")?;
//! # Ok::<(), xenomit_packet_client::error::ClientError>(())
//! ```

pub mod codec;
pub mod error;
pub mod frame;
pub mod generated;
pub mod model;
pub mod modules;
pub mod op;
pub mod packet;
pub mod proxy;
pub mod registry;
pub mod session;

#[cfg(feature = "game-api")]
pub mod gameapi;

pub use error::{ClientError, CodecError};
pub use packet::{Packet, RawPacket, Value};
pub use proxy::{ProxyAuth, ProxyConfig, ProxyError};
pub use session::{Event, LoginInfo, Phase, Session};
