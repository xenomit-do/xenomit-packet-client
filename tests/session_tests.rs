//! Session handshake + world-model tests over in-memory duplex streams.

use std::collections::VecDeque;
use std::io::{Read, Result as IoResult, Write};
use xenomit_packet_client::codec::{Reader, Writer};
use xenomit_packet_client::error::ClientError;
use xenomit_packet_client::frame::{frame_body, stream as fstream};
use xenomit_packet_client::modules::{self, id};
use xenomit_packet_client::op::IntWidth;
use xenomit_packet_client::packet::decode_body;
use xenomit_packet_client::session::{Event, LoginInfo, Session};

/// In-memory duplex stream with a scripted inbound side.
struct MockStream {
    inbound: VecDeque<u8>,
    pub sent: Vec<u8>,
}

impl MockStream {
    fn new(inbound: Vec<u8>) -> Self {
        Self {
            inbound: inbound.into(),
            sent: Vec::new(),
        }
    }
}

impl Read for MockStream {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        let n = self.inbound.read(buf)?;
        Ok(n)
    }
}

impl Write for MockStream {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        self.sent.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> IoResult<()> {
        Ok(())
    }
}

fn framed(body: Vec<u8>) -> Vec<u8> {
    frame_body(&body).unwrap()
}

fn version_command_body(version: &str, equal: bool) -> Vec<u8> {
    let mut w = Writer::new();
    w.i16(id::VERSION_COMMAND);
    w.utf(version).unwrap();
    w.bool(equal);
    w.into_bytes()
}

fn login_response_body(status: i16) -> Vec<u8> {
    let mut w = Writer::new();
    w.i16(id::LOGIN_RESPONSE);
    w.i16(status);
    w.into_bytes()
}

#[test]
fn handshake_sends_greeting_version_then_login() {
    let inbound = [
        framed(version_command_body(modules::VERSION_HASH, true)),
        framed(login_response_body(0)),
    ]
    .concat();
    let mut s = Session::new(MockStream::new(inbound));
    let login = LoginInfo::new(99, "sess-token");
    s.handshake(&login).unwrap();

    // inspect bytes actually written: greeting, VersionRequest, LoginRequest
    let sent = &s.stream.sent;
    assert_eq!(sent[0], 0x00, "first byte must be the 0x00 greeting");
    // frame 1: VersionRequest
    let len1 = ((sent[1] as usize) << 16) | ((sent[2] as usize) << 8) | sent[3] as usize;
    let vr = decode_body(&sent[4..4 + len1], true).unwrap();
    assert_eq!(vr.id, id::VERSION_REQUEST);
    assert_eq!(vr.string("version").unwrap(), modules::VERSION_HASH);
    // frame 2: LoginRequest — decode with the write ops (server's view)
    let off2 = 4 + len1;
    let len2 =
        ((sent[off2] as usize) << 16) | ((sent[off2 + 1] as usize) << 8) | sent[off2 + 2] as usize;
    let lr_body = &sent[off2 + 3..off2 + 3 + len2];
    let mut r = Reader::new(lr_body);
    assert_eq!(r.i16().unwrap(), id::LOGIN_REQUEST);
    let schema = xenomit_packet_client::registry::schema(id::LOGIN_REQUEST).unwrap();
    let mut found_session = false;
    let mut found_user = false;
    for op in schema.write {
        if let xenomit_packet_client::op::Op::Utf { name: Some(n) } = op {
            let v = r.utf().unwrap();
            if *n == "sessionID" {
                assert_eq!(v, "sess-token");
                found_session = true;
            }
            if *n == "version" {
                assert_eq!(v, modules::VERSION_HASH);
            }
        } else if let xenomit_packet_client::op::Op::Int {
            name: Some(n),
            width,
            rot,
        } = op
        {
            let v = match rot {
                Some((d, b)) => r.int_rot(*width, *d, *b).unwrap(),
                None => r.int(*width).unwrap(),
            };
            if *n == "userID" {
                assert_eq!(v, 99);
                found_user = true;
            }
        } else if let xenomit_packet_client::op::Op::Bool { .. } = op {
            let _ = r.bool().unwrap();
        } else if let xenomit_packet_client::op::Op::Const { value, width } = op {
            assert_eq!(r.int(*width).unwrap(), *value as i64);
        }
    }
    assert!(r.is_empty());
    assert!(found_session && found_user);
    assert!(s.world.logged_in);
}

#[test]
fn handshake_rejects_version_mismatch() {
    let inbound = framed(version_command_body("other-hash", false));
    let mut s = Session::new(MockStream::new(inbound));
    let e = s.handshake(&LoginInfo::new(1, "x")).unwrap_err();
    assert!(matches!(e, ClientError::Protocol(_)));
}

#[test]
fn handshake_reports_login_rejection() {
    let inbound = [
        framed(version_command_body(modules::VERSION_HASH, true)),
        framed(login_response_body(6)), // InvalidSessionId
    ]
    .concat();
    let mut s = Session::new(MockStream::new(inbound));
    let e = s.handshake(&LoginInfo::new(1, "x")).unwrap_err();
    assert!(matches!(e, ClientError::LoginRejected(6)));
}

#[test]
fn complete_ready_sends_two_readies_and_flag() {
    let mut s = Session::new(MockStream::new(vec![]));
    s.complete_ready("2D").unwrap();
    let sent = &s.stream.sent;
    // parse three frames back
    let mut bodies = Vec::new();
    let mut pos = 0;
    while pos + 3 <= sent.len() {
        let n =
            ((sent[pos] as usize) << 16) | ((sent[pos + 1] as usize) << 8) | sent[pos + 2] as usize;
        bodies.push(&sent[pos + 3..pos + 3 + n]);
        pos += 3 + n;
    }
    assert_eq!(bodies.len(), 3);
    assert_eq!(
        i16::from_be_bytes([bodies[0][0], bodies[0][1]]),
        id::READY_REQUEST
    );
    assert_eq!(
        i16::from_be_bytes([bodies[1][0], bodies[1][1]]),
        id::READY_REQUEST
    );
    assert_eq!(i16::from_be_bytes([bodies[2][0], bodies[2][1]]), id::LEGACY);
}

#[test]
fn handshake_accepts_ship_init_when_login_response_is_absent() {
    let inbound = [
        framed(version_command_body(modules::VERSION_HASH, true)),
        framed(ship_initialization_body()),
    ]
    .concat();
    let mut s = Session::new(MockStream::new(inbound));
    s.handshake(&LoginInfo::new(777, "sess-token")).unwrap();
    assert!(s.world.logged_in);
    assert_eq!(s.world.hero.as_ref().unwrap().user_id, 777);
    assert_eq!(s.world.hero.as_ref().unwrap().user_name, "tester");
}

fn ship_initialization_body() -> Vec<u8> {
    let schema = xenomit_packet_client::registry::schema(id::SHIP_INITIALIZATION).unwrap();
    let mut w = Writer::new();
    w.i16(id::SHIP_INITIALIZATION);
    for op in schema.read {
        use xenomit_packet_client::op::Op::*;
        match op {
            Int { name, width, rot } => {
                let v: i64 = match name {
                    Some("userId") => 777,
                    Some("x") => 1000,
                    Some("y") => 2000,
                    Some("mapId") => 42,
                    Some("speed") => 550,
                    Some("level") => 17,
                    _ => 0,
                };
                match rot {
                    Some((d, b)) => w.int_rot(*width, *d, *b, v),
                    None => w.int(*width, v),
                }
            }
            Bool { .. } => w.bool(false),
            F32 { .. } => w.f32(0.0),
            F64 { name, .. } => {
                let v = match name {
                    Some("hitPoints") | Some("hitMax") => 9000.0,
                    Some("credits") => 123456.0,
                    _ => 0.0,
                };
                w.f64(v);
            }
            Utf { name, .. } => w
                .utf(match name {
                    Some("userName") => "tester",
                    Some("typeId") => "ship_goliath",
                    _ => "",
                })
                .unwrap(),
            Skip { width } => w.int(*width, 0),
            Nested { .. } => w.i16(0),
            Vec { count, .. } => w.int(*count, 0),
            Const { value, width } => w.int(*width, *value as i64),
        }
    }
    w.into_bytes()
}

#[test]
fn world_updates_from_ship_init_and_move() {
    let inbound = framed(ship_initialization_body());
    let mut s = Session::new(MockStream::new(inbound));
    match s.recv().unwrap() {
        Event::Packet(p) => assert_eq!(p.id, id::SHIP_INITIALIZATION),
        _ => panic!("expected packet"),
    }
    let h = s.world.hero.as_ref().unwrap();
    assert_eq!(h.user_id, 777);
    assert_eq!(h.user_name, "tester");
    assert_eq!((h.x, h.y, h.map_id), (1000, 2000, 42));
    assert_eq!(h.hitpoints, 9000.0);
    assert_eq!(h.credits, 123456.0);
    assert_eq!(h.speed, 550);

    // HeroMoveCommand updates position
    let mut w = Writer::new();
    w.i16(id::HERO_MOVE);
    let hm = xenomit_packet_client::registry::schema(id::HERO_MOVE).unwrap();
    for op in hm.read {
        if let xenomit_packet_client::op::Op::Int { name, width, rot } = op {
            let v = if *name == Some("x") { 1500 } else { 2600 };
            match rot {
                Some((d, b)) => w.int_rot(*width, *d, *b, v),
                None => w.int(*width, v),
            }
        }
    }
    s.stream.inbound.extend(framed(w.into_bytes()));
    s.recv().unwrap();
    assert_eq!(s.world.hero.as_ref().unwrap().x, 1500);
    assert_eq!(s.world.hero.as_ref().unwrap().y, 2600);
}

#[test]
fn unknown_frame_arrives_as_raw_event() {
    let mut body = vec![0xD8, 0xF1]; // id -9999, unregistered
    body.extend_from_slice(&[9, 9, 9]);
    let mut s = Session::new(MockStream::new(framed(body)));
    match s.recv().unwrap() {
        Event::Raw(r) => {
            assert_eq!(r.id, -9999);
            assert_eq!(r.body.len(), 5);
        }
        e => panic!("expected raw event, got {e:?}"),
    }
}

#[test]
fn malformed_frame_arrives_as_malformed_event() {
    // MoveRequest with trailing junk
    let mut w = Writer::new();
    w.i16(id::MOVE_REQUEST);
    for _ in 0..4 {
        w.int(IntWidth::W32, 0);
    }
    w.u8(0xAA);
    let mut s = Session::new(MockStream::new(framed(w.into_bytes())));
    match s.recv().unwrap() {
        Event::Malformed { .. } => {}
        e => panic!("expected malformed, got {e:?}"),
    }
}

#[test]
fn read_body_round_trip_over_stream() {
    let pkt = modules::move_request((1, 2), (3, 4));
    let mut wire = Vec::new();
    xenomit_packet_client::frame::stream::write_packet(&mut wire, &pkt).unwrap();
    let mut cur = std::io::Cursor::new(wire);
    let got = fstream::read_packet(&mut cur, true).unwrap();
    assert_eq!(got.id, id::MOVE_REQUEST);
    assert_eq!(got.int("targetX").unwrap(), 3);
}
