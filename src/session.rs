//! Blocking session: TCP framing, handshake state machine, event pump.
//!
//! Wire sequence (from `SocketManager.as` / `ConnectionManager.as`):
//!
//! ```text
//! connect → write single 0x00 byte
//!         → VersionRequest(hash)
//!         ← VersionCommand(hash, equal)
//!         → LoginRequest(...)
//!         ← LoginResponse(status == 0)
//!           or, when the server skips that ack, ShipInitializationCommand
//!         → ReadyRequest(view, MAP_LOADED_*) + ReadyRequest(view, UI_READY)
//!         → LegacyModule("w")
//!         ← world init traffic …
//! ```

use crate::error::{ClientError, CodecError};
use crate::frame::{FrameDecoder, frame_packet, stream as fstream};
use crate::model::WorldState;
use crate::modules::{self, VERSION_HASH, id};
use crate::packet::{Packet, RawPacket, decode_body};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// Credentials needed for `handshake`.
///
/// All fields come from the authenticated FlashVars the launcher page
/// generates (`flashAPI/loadingScreen.php`, keyed by the `dosid`
/// cookie). `FlashVarsParser.as` maps them as:
///
/// | FlashVars key | LoginInfo field    |
/// |---------------|--------------------|
/// | `userID`      | `user_id`          |
/// | `sessionID`   | `session_id`       |
/// | `factionID`   | `faction_id`       |
/// | `pid`/`projectID` | `instance_id`  |
/// | `platform`    | `is_mini_client` = `platform == "client"` |
///
/// The game server endpoint (`Map.getServerIP(mapID)`) is *not* part of
/// this struct — pass it to `Session::connect`/`connect_and_login`.
#[derive(Clone, Debug)]
pub struct LoginInfo {
    pub user_id: i32,
    pub faction_id: i16,
    pub session_id: String,
    pub instance_id: i32,
    /// Map view identifier echoed in `ReadyRequest`; `"2D"` style
    /// display-path strings are what the stock client produces.
    pub view_data: String,
    pub version: String,
    pub is_mini_client: bool,
}

impl LoginInfo {
    pub fn new(user_id: i32, session_id: impl Into<String>) -> Self {
        Self {
            user_id,
            faction_id: 0,
            session_id: session_id.into(),
            instance_id: 0,
            view_data: "2D".into(),
            version: VERSION_HASH.into(),
            is_mini_client: false,
        }
    }
}

/// One inbound frame, decoded when a schema exists.
#[derive(Debug)]
pub enum Event {
    Packet(Packet),
    /// Unknown module id; raw body retained (starts with i16 id).
    Raw(RawPacket),
    /// Body decoded but had trailing bytes or a const mismatch —
    /// surfaced rather than dropped so schema drift is visible.
    Malformed {
        error: CodecError,
        body: Vec<u8>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Connected,
    VersionSent,
    LoginSent,
    Ready,
}

pub struct Session<S> {
    pub stream: S,
    decoder: FrameDecoder,
    pub world: WorldState,
    pub phase: Phase,
    /// Credentials from the last `handshake` — retained because
    /// `KillScreenRepairRequest` embeds a fresh `LoginRequest` and
    /// `S|CFG` legacy commands carry the user/session pair.
    pub login: Option<LoginInfo>,
    /// Bounded replay journal of decoded events, consumed by
    /// `GameClient::poll_events` (only with the `game-api` feature).
    #[cfg(feature = "game-api")]
    pub journal: xenomit_game_api::features::ClientEventJournal,
}

impl<S: Read + Write> Session<S> {
    /// Wrap an already-established stream. No bytes are written until
    /// `handshake`/`send` are called.
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            decoder: FrameDecoder::new(),
            world: WorldState::default(),
            phase: Phase::Connected,
            login: None,
            #[cfg(feature = "game-api")]
            journal: xenomit_game_api::features::ClientEventJournal::default(),
        }
    }

    pub fn into_inner(self) -> S {
        self.stream
    }

    /// Encode and write one framed packet.
    pub fn send(&mut self, packet: &Packet) -> Result<(), ClientError> {
        let buf = frame_packet(packet)?;
        self.stream.write_all(&buf)?;
        self.stream.flush()?;
        Ok(())
    }

    /// Read exactly one frame, decode it, and fold it into the world
    /// model. Blocks until a full frame arrives.
    pub fn recv(&mut self) -> Result<Event, ClientError> {
        let body = fstream::read_body(&mut self.stream)?;
        Ok(self.decode_event(body))
    }

    fn decode_event(&mut self, body: Vec<u8>) -> Event {
        let event = match decode_body(&body, true) {
            Ok(p) => {
                self.world.apply(&p);
                Event::Packet(p)
            }
            Err(CodecError::UnknownModule(_)) => Event::Raw(RawPacket {
                id: if body.len() >= 2 {
                    i16::from_be_bytes([body[0], body[1]])
                } else {
                    0
                },
                body,
            }),
            Err(e) => Event::Malformed { error: e, body },
        };
        #[cfg(feature = "game-api")]
        self.push_journal(&event);
        event
    }

    /// Record a decoded event in the replay journal. The journal stays
    /// narrow on purpose: legacy console/log lines plus session,
    /// map-change, jump, death and kill-screen lifecycle packets —
    /// high-rate traffic (movement, attribute ticks) would swamp the
    /// bounded buffer without adding information.
    #[cfg(feature = "game-api")]
    fn push_journal(&mut self, event: &Event) {
        use serde_json::json;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        match event {
            Event::Packet(p) => {
                if p.id == id::LEGACY {
                    if let Ok(m) = p.string("message") {
                        self.journal
                            .push(now, "log", Some(m.to_owned()), json!({"id": p.id}));
                    }
                    return;
                }
                if p.id == id::LOG_MESSAGE {
                    if let Some(text) = modules::parse_log_message(p) {
                        self.journal
                            .push(now, "log", Some(text), json!({"id": p.id}));
                    }
                    return;
                }
                let kind = match p.id {
                    id::MAP_CHANGED => "map",
                    id::JUMP_INITIATED | id::JUMP_DENIED | id::JUMP_TO_SERVER | id::RELOGIN => {
                        "jump"
                    }
                    id::SHIP_DESTROYED => "death",
                    id::SHIP_INITIALIZATION | id::LOGIN_RESPONSE | id::LOGIN_ERROR => "session",
                    id::KILL_SCREEN_POST | id::KILL_SCREEN_UPDATE => "killscreen",
                    _ => return,
                };
                self.journal.push(
                    now,
                    kind,
                    p.name.map(str::to_owned),
                    json!({"id": p.id, "name": p.name}),
                );
            }
            Event::Malformed { error, .. } => {
                self.journal
                    .push(now, "malformed", Some(error.to_string()), json!({}));
            }
            Event::Raw(_) => {}
        }
    }

    /// Full login handshake over a fresh connection: greeting byte,
    /// version check, login request, then either `LoginResponse` or the
    /// ship-init packet current servers send in its place.
    pub fn handshake(&mut self, login: &LoginInfo) -> Result<(), ClientError> {
        self.stream.write_all(&[0u8])?;
        self.stream.flush()?;
        self.send(&modules::version_request(&login.version))?;
        self.phase = Phase::VersionSent;

        let mut attempts = 0u8;
        loop {
            attempts += 1;
            if attempts > 64 {
                return Err(ClientError::Protocol(
                    "no VersionCommand within 64 frames".into(),
                ));
            }
            match self.recv()? {
                Event::Packet(p) if p.id == id::VERSION_COMMAND => {
                    if let Some((_v, equal)) = modules::parse_version_command(&p) {
                        if !equal {
                            return Err(ClientError::Protocol(
                                "version hash rejected by server".into(),
                            ));
                        }
                    }
                    break;
                }
                Event::Malformed { error, .. } => return Err(error.into()),
                _ => continue,
            }
        }

        self.send(&modules::login_request(
            login.user_id,
            login.faction_id,
            &login.session_id,
            &login.version,
            login.instance_id,
            login.is_mini_client,
        ))?;
        self.phase = Phase::LoginSent;

        let mut attempts = 0u16;
        loop {
            attempts += 1;
            if attempts > 512 {
                return Err(ClientError::Protocol(
                    "no LoginResponse or ShipInitialization within 512 frames".into(),
                ));
            }
            match self.recv()? {
                Event::Packet(p) if p.id == id::LOGIN_RESPONSE => {
                    let status = p.int("status").unwrap_or(-1) as i16;
                    if status != 0 {
                        return Err(ClientError::LoginRejected(status));
                    }
                    self.finish_login(login);
                    return Ok(());
                }
                Event::Packet(p) if p.id == id::LOGIN_ERROR => {
                    let code = p.int("errorCode").unwrap_or(-1) as i16;
                    return Err(ClientError::LoginRejected(code));
                }
                // Live servers accept the login and stream settings plus
                // `ShipInitializationCommand` without a `LoginResponse`.
                // The ship-init packet is the session proof; `world.apply`
                // has already stored the hero.
                Event::Packet(p) if p.id == id::SHIP_INITIALIZATION => {
                    self.finish_login(login);
                    return Ok(());
                }
                Event::Malformed { error, .. } => return Err(error.into()),
                _ => continue,
            }
        }
    }

    fn finish_login(&mut self, login: &LoginInfo) {
        self.world.logged_in = true;
        self.phase = Phase::Ready;
        // Keep the credentials — kill-screen repair re-logins and
        // `S|CFG` legacy commands need them.
        self.login = Some(login.clone());
    }

    /// Post-login readiness sequence the stock client sends once the
    /// hero and map are loaded: `ReadyRequest(MAP_LOADED)`,
    /// `ReadyRequest(UI_READY)`, then the `w` login flag through
    /// `LegacyModule`.
    pub fn complete_ready(&mut self, view_data: &str) -> Result<(), ClientError> {
        let map_loaded = if view_data == "3D" {
            modules::READY_MAP_LOADED_3D
        } else {
            modules::READY_MAP_LOADED_2D
        };
        self.send(&modules::ready_request(view_data, map_loaded))?;
        self.send(&modules::ready_request(view_data, modules::READY_UI_READY))?;
        self.send(&modules::legacy("w"))?;
        Ok(())
    }

    /// Convenience: block until `ShipInitializationCommand` has been
    /// applied (hero known) or `max` frames pass. Returns whether the
    /// hero is initialized.
    pub fn wait_hero_init(&mut self, max_frames: usize) -> Result<bool, ClientError> {
        for _ in 0..max_frames {
            if self.world.hero.is_some() {
                return Ok(true);
            }
            self.recv()?;
        }
        Ok(self.world.hero.is_some())
    }
}

impl Session<TcpStream> {
    /// Connect TCP, then (optionally) run `handshake` via the caller.
    pub fn connect(addr: &str) -> Result<Self, ClientError> {
        let stream = TcpStream::connect(addr)?;
        stream.set_nodelay(true)?;
        Ok(Self::new(stream))
    }

    /// Connect and perform the whole login handshake.
    pub fn connect_and_login(addr: &str, login: &LoginInfo) -> Result<Self, ClientError> {
        let mut s = Self::connect(addr)?;
        s.handshake(login)?;
        Ok(s)
    }

    /// Connect to `addr` (`host:port`) through an egress proxy instead of
    /// dialing directly — HTTP `CONNECT` or SOCKS5, DNS resolved at the
    /// proxy, no direct fallback. Failures come back as the typed
    /// [`ClientError::Proxy`] (exit-code category 69). The returned
    /// session wraps the tunnel exactly like [`Session::connect`] wraps
    /// a direct socket.
    pub fn connect_via(proxy: &crate::proxy::ProxyConfig, addr: &str) -> Result<Self, ClientError> {
        Self::connect_via_timeout(proxy, addr, crate::proxy::DEFAULT_DIAL_TIMEOUT)
    }

    /// [`connect_via`](Self::connect_via) with an explicit budget for the
    /// proxy-connect + tunnel-handshake phase.
    pub fn connect_via_timeout(
        proxy: &crate::proxy::ProxyConfig,
        addr: &str,
        timeout: Duration,
    ) -> Result<Self, ClientError> {
        let stream = crate::proxy::dial_timeout(proxy, addr, timeout)?;
        stream.set_nodelay(true)?;
        Ok(Self::new(stream))
    }

    /// Connect through the proxy and perform the whole login handshake.
    pub fn connect_via_and_login(
        proxy: &crate::proxy::ProxyConfig,
        addr: &str,
        login: &LoginInfo,
    ) -> Result<Self, ClientError> {
        let mut s = Self::connect_via(proxy, addr)?;
        s.handshake(login)?;
        Ok(s)
    }

    /// Try to read one frame, waiting up to `timeout`. Returns `None`
    /// on timeout. The socket's read timeout is preserved.
    pub fn poll(&mut self, timeout: Duration) -> Result<Option<Event>, ClientError> {
        // Drain anything already buffered first.
        if let Some(body) = self.decoder.next_body() {
            return Ok(Some(self.decode_event(body)));
        }
        self.stream.set_read_timeout(Some(timeout))?;
        let mut chunk = [0u8; 8192];
        let deadline = Instant::now() + timeout;
        let result = loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(ClientError::Protocol("connection closed".into())),
                Ok(n) => {
                    self.decoder.push(&chunk[..n]);
                    if let Some(body) = self.decoder.next_body() {
                        break Some(body);
                    }
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break None;
                }
                Err(e) => return Err(e.into()),
            }
            if Instant::now() >= deadline {
                break None;
            }
        };
        self.stream.set_read_timeout(None)?;
        Ok(result.map(|b| self.decode_event(b)))
    }

    /// Drain all immediately available frames without blocking.
    pub fn pump(&mut self) -> Result<Vec<Event>, ClientError> {
        let mut out = Vec::new();
        self.stream.set_nonblocking(true)?;
        let mut chunk = [0u8; 8192];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    self.stream.set_nonblocking(false)?;
                    return Err(ClientError::Protocol("connection closed".into()));
                }
                Ok(n) => self.decoder.push(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    self.stream.set_nonblocking(false)?;
                    return Err(e.into());
                }
            }
        }
        self.stream.set_nonblocking(false)?;
        while let Some(body) = self.decoder.next_body() {
            out.push(self.decode_event(body));
        }
        Ok(out)
    }
}
