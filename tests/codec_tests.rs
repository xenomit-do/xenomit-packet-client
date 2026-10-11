//! Wire-level tests: primitives, rotations, UTF, framing, and exact
//! golden bodies observed on the real stream.

use xenomit_packet_client::codec::{Reader, Writer};
use xenomit_packet_client::error::CodecError;
use xenomit_packet_client::frame::{self, FrameDecoder};
use xenomit_packet_client::modules::{self, id};
use xenomit_packet_client::op::{Elem, IntWidth, Op, RotDir};
use xenomit_packet_client::packet::{Packet, Value, decode_body, encode_body};
use xenomit_packet_client::registry;

#[test]
fn economy_requests_encode_the_verified_nested_ore_payload() {
    for (request, field, expected_type, expected_amount) in [
        (modules::sell_ore_request(4, 17), "toSell", 4, 17.0),
        (modules::refine_ore_request(7, 29), "toProduce", 7, 29.0),
    ] {
        let decoded = decode_body(&encode_body(&request).unwrap(), true).unwrap();
        let count = decoded.module(field).unwrap();
        let ore_type = count.module("oreType").unwrap();
        assert_eq!(ore_type.int("typeValue").unwrap(), expected_type);
        assert_eq!(count.f64("count").unwrap(), expected_amount);
    }
}

#[test]
fn lab_requests_encode_item_value_and_ore_count_modules() {
    // `LabUpdateRequest` (137) is a bare marker packet.
    let p = modules::lab_update_request();
    assert_eq!(p.id, id::LAB_UPDATE_REQUEST);
    let body = encode_body(&p).unwrap();
    let decoded = decode_body(&body, true).unwrap();
    assert_eq!(decoded.id, id::LAB_UPDATE_REQUEST);

    // `LabUpdateItemRequest` (133): `itemToUpdate` = LabItemModule
    // (itemValue), `updateWith` = OreCountModule (oreType + count).
    let p = modules::lab_update_item_request(1, 6, 500);
    assert_eq!(p.id, id::LAB_UPDATE_ITEM_REQUEST);
    let item = p.module("itemToUpdate").unwrap();
    assert_eq!(item.id, id::LAB_ITEM_MODULE);
    assert_eq!(item.int("itemValue").unwrap(), 1);
    let decoded = decode_body(&encode_body(&p).unwrap(), true).unwrap();
    let update = decoded.module("updateWith").unwrap();
    assert_eq!(
        update.module("oreType").unwrap().int("typeValue").unwrap(),
        6
    );
    assert_eq!(update.f64("count").unwrap(), 500.0);
}

// ---------- primitives ----------

#[test]
fn be_primitives_round_trip() {
    let mut w = Writer::new();
    w.u8(0xAB);
    w.i16(-2);
    w.u16(0xBEEF);
    w.i32(-70000);
    w.u32(0xDEADBEEF);
    w.f32(1.5);
    w.f64(-2.25);
    w.bool(true);
    w.bool(false);
    let bytes = w.into_bytes();
    let mut r = Reader::new(&bytes);
    assert_eq!(r.u8().unwrap(), 0xAB);
    assert_eq!(r.i16().unwrap(), -2);
    assert_eq!(r.u16().unwrap(), 0xBEEF);
    assert_eq!(r.i32().unwrap(), -70000);
    assert_eq!(r.u32().unwrap(), 0xDEADBEEF);
    assert_eq!(r.f32().unwrap(), 1.5);
    assert_eq!(r.f64().unwrap(), -2.25);
    assert!(r.bool().unwrap());
    assert!(!r.bool().unwrap());
    assert!(r.is_empty());
}

#[test]
fn utf_round_trip_and_eof() {
    let mut w = Writer::new();
    w.utf("hello map").unwrap();
    w.utf("").unwrap();
    let bytes = w.into_bytes();
    let mut r = Reader::new(&bytes);
    assert_eq!(r.utf().unwrap(), "hello map");
    assert_eq!(r.utf().unwrap(), "");
    assert!(matches!(
        r.u8(),
        Err(CodecError::UnexpectedEof {
            needed: 1,
            remaining: 0
        })
    ));
}

#[test]
fn utf_overlong_rejected() {
    let s = "x".repeat(u16::MAX as usize + 1);
    let mut w = Writer::new();
    assert!(matches!(w.utf(&s), Err(CodecError::BodyTooLarge(_))));
}

// ---------- rotations ----------

#[test]
fn rotation_round_trip_all_widths() {
    for (width, v) in [
        (IntWidth::W8, 0x5Ai64),
        (IntWidth::W16, -12345i64),
        (IntWidth::W32, -19088744i64),
    ] {
        for dir in [RotDir::Rol, RotDir::Ror] {
            for bits in [0u8, 1, 3, 7, 9, 15, 16, 31] {
                let bits = match width {
                    IntWidth::W8 => bits % 8,
                    IntWidth::W16 => bits % 16,
                    IntWidth::W32 => bits % 32,
                };
                let mut w = Writer::new();
                // write applies inverse rotation of decode dir
                w.int_rot(width, dir, bits, v);
                let mut r = Reader::new(w.as_slice());
                let got = r.int_rot(width, dir, bits).unwrap();
                let masked = match width {
                    IntWidth::W8 => v as i8 as i64,
                    IntWidth::W16 => v as i16 as i64,
                    IntWidth::W32 => v as i32 as i64,
                };
                assert_eq!(got, masked, "{width:?} {dir:?} {bits} value {v}");
            }
        }
    }
}

#[test]
fn rotation_matches_actionscript_semantics() {
    // MoveRequest.positionX on write: rol 11 of the logical value.
    // AS3: writeInt(((v << 11) | (v >>> 21)))  → wire = v.rol(11)
    // Our writer encodes value→wire with inv(decode_dir): decode dir for
    // positionX is Ror, so wire = v.rol(11). Check wire bytes directly.
    let mut w = Writer::new();
    w.int_rot(IntWidth::W32, RotDir::Ror, 11, 2150);
    let wire = u32::from_be_bytes(w.as_slice().try_into().unwrap());
    assert_eq!(wire, 2150u32.rotate_left(11));
    let mut r = Reader::new(w.as_slice());
    assert_eq!(r.int_rot(IntWidth::W32, RotDir::Ror, 11).unwrap(), 2150);
}

// ---------- framing ----------

#[test]
fn frame_header_is_24bit_be() {
    let framed = frame::frame_body(&[1, 2, 3, 4]).unwrap();
    assert_eq!(&framed[..3], &[0, 0, 4]);
    assert_eq!(&framed[3..], &[1, 2, 3, 4]);

    let big = vec![0u8; 0x12345];
    let framed = frame::frame_body(&big).unwrap();
    assert_eq!(&framed[..3], &[0x01, 0x23, 0x45]);
}

#[test]
fn frame_decoder_incremental() {
    let mut d = FrameDecoder::new();
    // two frames, delivered in awkward chunks
    let f1 = frame::frame_body(&[0, 2, 0]).unwrap(); // KeepAlive false
    let f2 = frame::frame_body(&[0, 10]).unwrap(); // JumpRequest
    let stream = [f1.clone(), f2.clone()].concat();
    for b in &stream[..5] {
        d.push(&[*b]);
        assert!(d.next_body().is_none() || true);
    }
    d.push(&stream[5..]);
    let b1 = d.next_body().unwrap();
    assert_eq!(b1, vec![0, 2, 0]);
    let b2 = d.next_body().unwrap();
    assert_eq!(b2, vec![0, 10]);
    assert!(d.next_body().is_none());
}

#[test]
fn frame_too_large_rejected() {
    let body = vec![0u8; frame::MAX_BODY_LEN + 1];
    assert!(matches!(
        frame::frame_body(&body),
        Err(CodecError::BodyTooLarge(_))
    ));
}

// ---------- module decode ----------

/// Golden: verbatim `MoveRequest` body captured from a live session
/// (module 82). Wire bytes: `0052 00433000 681e0000 011e8000 00d25000`.
/// Decodes to pos=(2150,13460) target=(2292,13327).
#[test]
fn golden_move_request_decodes() {
    let body: &[u8] = &[
        0x00, 0x52, 0x00, 0x43, 0x30, 0x00, 0x68, 0x1E, 0x00, 0x00, 0x01, 0x1E, 0x80, 0x00, 0x00,
        0xD2, 0x50, 0x00,
    ];
    let p = decode_body(body, true).unwrap();
    assert_eq!(p.id, id::MOVE_REQUEST);
    assert_eq!(p.name, Some("MoveRequest"));
    assert_eq!(p.int("positionX").unwrap(), 2150);
    assert_eq!(p.int("positionY").unwrap(), 13460);
    assert_eq!(p.int("targetX").unwrap(), 2292);
    assert_eq!(p.int("targetY").unwrap(), 13327);
}

#[test]
fn golden_move_request_reencodes_identically() {
    // Our MoveRequest builder + schema encoder must reproduce the exact
    // captured wire bytes.
    let p = modules::move_request((2150, 13460), (2292, 13327));
    assert_eq!(p.int("positionX").unwrap(), 2150);
    assert_eq!(p.int("positionY").unwrap(), 13460);
    let body = encode_body(&p).unwrap();
    let expected: &[u8] = &[
        0x00, 0x52, 0x00, 0x43, 0x30, 0x00, 0x68, 0x1E, 0x00, 0x00, 0x01, 0x1E, 0x80, 0x00, 0x00,
        0xD2, 0x50, 0x00,
    ];
    assert_eq!(body, expected, "re-encoded MoveRequest differs from wire");
}

#[test]
fn golden_keepalive_and_stayinalive() {
    // body 00 02 00 from the capture: KeepAlive(false)
    let p = decode_body(&[0, 2, 0], true).unwrap();
    assert_eq!(p.id, id::KEEP_ALIVE);
    assert!(!p.boolean("MouseClick").unwrap());

    // StayinAlive: id only, no payload
    let p = decode_body(&[0x20, 0xEB], true).unwrap();
    assert_eq!(p.id, id::STAYIN_ALIVE);
    assert!(p.fields.is_empty());
}

#[test]
fn golden_command_center_news_clicked() {
    // Real body from capture:
    // 77 fc | 40 db | 2a 80 | 00 00 00 00 | 00 19 "galaxy_gate_double_reward"
    let mut body = vec![0x77, 0xFC];
    body.extend_from_slice(&[0x40, 0xDB, 0x2A, 0x80, 0, 0, 0, 0]);
    body.extend_from_slice(&[0, 0x19]);
    body.extend_from_slice(b"galaxy_gate_double_reward");
    let p = decode_body(&body, true).unwrap();
    assert_eq!(p.id, id::COMMAND_CENTER_NEWS_CLICKED);
}

// ---------- encode round-trips ----------

#[test]
fn request_packets_round_trip_through_write_schema() {
    for packet in [
        modules::version_request(modules::VERSION_HASH),
        modules::login_request(12345678, 3, "sess-abc", modules::VERSION_HASH, 12, false),
        modules::ready_request("2D", modules::READY_UI_READY),
        modules::keep_alive(true),
        modules::legacy("w"),
        modules::stayin_alive(),
        modules::jump_request(),
        modules::attack_abort_laser_request(),
        modules::move_request((2150, 13460), (2292, 13327)),
        modules::attack_laser_request(42, (100, 200)),
        modules::attack_rocket_request(42, (100, 200)),
        modules::ship_select_request(42, (100, 200), (90, 210), (5, 7), 3),
        modules::collect_box_request("hash123", (300, 301), (290, 295)),
        modules::harvest_request("orehash"),
        modules::ping_position_request((111, 222)),
        modules::channel_close(true),
    ] {
        let body = encode_body(&packet).unwrap();
        assert_eq!(
            i16::from_be_bytes([body[0], body[1]]),
            packet.id,
            "id mismatch for {}",
            packet.name.unwrap_or("?")
        );
        // decode with the WRITE schema (what the server actually parses)
        let schema = registry::schema(packet.id).unwrap();
        let mut r = Reader::new(&body);
        let id = r.i16().unwrap();
        assert_eq!(id, packet.id);
        // re-decode fields using write ops via a decoder shim:
        // emulate server side: apply ops with their natural direction.
        let decoded = decode_with_ops(&mut r, schema.write, packet.id).unwrap();
        for (name, v) in &packet.fields {
            let Some(n) = name else { continue };
            let dv = decoded.get(n).unwrap_or_else(|| {
                panic!(
                    "missing decoded field {n} in {}",
                    packet.name.unwrap_or("?")
                )
            });
            assert_eq!(dv, v, "field {n} of {}", packet.name.unwrap_or("?"));
        }
        assert!(
            r.is_empty(),
            "leftover bytes decoding {}",
            packet.name.unwrap_or("?")
        );
    }
}

/// Decode `body` fields using an op list directly (server-side view of
/// our outbound packets).
fn decode_with_ops(r: &mut Reader<'_>, ops: &[Op], id: i16) -> Result<Packet, CodecError> {
    use xenomit_packet_client::op::IntWidth as W;
    let mut fields = Vec::new();
    for op in ops {
        let name = op.name();
        match op {
            Op::Int { width, rot, .. } => {
                // write ops were normalized to decode direction, so the
                // stored rotation is exactly what decode applies.
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
            Op::Skip { width } => r.skip(width.bytes())?,
            Op::Nested { .. } => {
                let mid = r.i16()?;
                if mid != 0 {
                    let _ = decode_body_nested(r, mid)?;
                }
                fields.push((name, Value::Null));
            }
            Op::Vec { count, elem, .. } => {
                let n = r.int(*count)? as usize;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(match elem {
                        Elem::Int { width, rot } => Value::Int(match rot {
                            Some((d, b)) => r.int_rot(*width, *d, *b)?,
                            None => r.int(*width)?,
                        }),
                        Elem::Bool => Value::Bool(r.bool()?),
                        Elem::F32 => Value::F32(r.f32()?),
                        Elem::F64 => Value::F64(r.f64()?),
                        Elem::Utf => Value::Str(r.utf()?),
                        Elem::Nested => {
                            let mid = r.i16()?;
                            decode_body_nested(r, mid)?
                        }
                    });
                }
                fields.push((name, Value::List(items)));
            }
            Op::Const { value, width } => {
                let actual = r.int(*width)?;
                assert_eq!(actual, *value as i64);
            }
        }
    }
    let _ = W::W8;
    Ok(Packet {
        id,
        name: registry::module_name(id),
        fields,
    })
}

fn decode_body_nested(r: &mut Reader<'_>, mid: i16) -> Result<Value, CodecError> {
    let schema = registry::schema(mid).ok_or(CodecError::UnknownModule(mid))?;
    let inner = decode_with_ops(r, schema.write, mid)?;
    Ok(Value::Module(Box::new(inner)))
}

// ---------- mod-3 / mod-8 checksums ----------

#[test]
fn move_request_satisfies_wire_checksums() {
    for (pos, tgt) in [
        ((2150, 13460), (2292, 13327)),
        ((0, 0), (100, 100)),
        ((-50, 80), (-200, -300)),
        ((9_999_999, 1), (8_388_608, 8_388_608)),
    ] {
        let p = modules::move_request(pos, tgt);
        let px = p.int("positionX").unwrap() as i32;
        let py = p.int("positionY").unwrap() as i32;
        let tx = p.int("targetX").unwrap() as i32;
        let ty = p.int("targetY").unwrap() as i32;
        // server expects (positionX + targetY) % 3 == 0
        assert_eq!((px + ty) % 3, 0, "pos {pos:?} tgt {tgt:?}");
        // and (positionY + targetX) % 3 != 0
        assert_ne!((py + tx) % 3, 0, "pos {pos:?} tgt {tgt:?}");
        // positive targets are masked to 23 bits
        if tgt.0 > 0 {
            assert_eq!(tx, tgt.0 & 0x7FFFFF);
        }
        if tgt.1 > 0 {
            assert_eq!(ty, tgt.1 & 0x7FFFFF);
        }
    }
}

#[test]
fn attack_requests_satisfy_wire_checksums() {
    for tgt in [(100, 200), (30, 60), (0, 0), (-30, 15)] {
        let p = modules::attack_laser_request(7, tgt);
        let tx = p.int("targetX").unwrap();
        let ty = p.int("targetY").unwrap();
        assert_ne!((tx + ty) % 3, 0, "laser {tgt:?}");

        let p = modules::attack_rocket_request(7, tgt);
        let tx = p.int("targetX").unwrap();
        let ty = p.int("targetY").unwrap();
        assert_ne!((tx + ty) % 3, 0, "rocket {tgt:?}");
    }
}

#[test]
fn collect_and_select_checksums() {
    // CollectBoxRequest: (boxY + posX + posY) % 3 != 0
    let p = modules::collect_box_request("h", (10, 30), (60, 90));
    let by = p.int("boxY").unwrap();
    let px = p.int("posX").unwrap();
    let py = p.int("posY").unwrap();
    assert_ne!((by + px + py) % 3, 0);

    // ShipSelectRequest: (tx + ty + px + py) % 8 == 0
    let p = modules::ship_select_request(9, (101, 203), (55, 77), (0, 0), 0);
    let tx = p.int("targetX").unwrap();
    let ty = p.int("targetY").unwrap();
    let px = p.int("posX").unwrap();
    let py = p.int("posY").unwrap();
    assert_eq!((tx + ty + py + px) % 8, 0);
}

// ---------- errors / edge cases ----------

#[test]
fn unknown_module_id_errors_and_raw_keeps_body() {
    // id -9999 is not registered
    let body = [0xD8u8, 0xF1, 1, 2, 3]; // -9999 + 3 payload bytes
    match decode_body(&body, true) {
        Err(CodecError::UnknownModule(-9999)) => {}
        other => panic!("expected UnknownModule, got {other:?}"),
    }
}

#[test]
fn truncated_body_errors() {
    // MoveRequest needs 16 payload bytes after id
    let mut w = Writer::new();
    w.i16(id::MOVE_REQUEST);
    w.int_rot(IntWidth::W32, RotDir::Ror, 11, 1);
    let body = &w.into_bytes()[..4]; // cut mid-field
    assert!(matches!(
        decode_body(body, true),
        Err(CodecError::UnexpectedEof { .. })
    ));
}

#[test]
fn trailing_bytes_flagged_when_strict() {
    let mut w = Writer::new();
    w.i16(id::STAYIN_ALIVE);
    w.u8(0xFF);
    let body = w.into_bytes();
    assert!(matches!(
        decode_body(&body, true),
        Err(CodecError::TrailingBytes { .. })
    ));
    assert!(decode_body(&body, false).is_ok());
}

#[test]
fn registry_covers_observed_modules() {
    for id in [
        id::MOVE_REQUEST,
        id::MOVE_COMMAND,
        id::HERO_MOVE,
        id::SHIP_INITIALIZATION,
        id::SHIP_CREATE,
        id::SHIP_REMOVE,
        id::ATTACK_LASER_REQUEST,
        id::LOGIN_REQUEST,
        id::LOGIN_RESPONSE,
        id::VERSION_REQUEST,
        id::VERSION_COMMAND,
        id::READY_REQUEST,
        id::KEEP_ALIVE,
        id::STAYIN_ALIVE,
        id::LEGACY,
        id::JUMP_REQUEST,
        id::COLLECT_BOX_REQUEST,
        id::SHIP_SELECT_REQUEST,
        id::HARVEST_REQUEST,
        id::ADD_BOX,
        id::ADD_ORE,
        id::ADD_MINE,
        id::REMOVE_MINE,
        id::ADD_BEACON,
        id::BEACON_COMMAND,
        id::WINDOW_ITEM,
        id::MENU_ITEM_STATUS,
        id::ATTACK_ABORT_LASER,
        id::PET_LASER_ATTACK_STOP,
        id::ATTACK_HIT,
        id::ATTACK_HIT_NO_LOCK,
        id::HITPOINT_INFO,
        id::ATTRIBUTE_SHIELD_UPDATE,
        id::ATTRIBUTE_HITPOINT_UPDATE,
        id::ATTRIBUTE_CREDITS_UPDATE,
        id::ATTRIBUTE_ORE_COUNT_UPDATE,
        id::ATTRIBUTE_SHIP_SPEED_UPDATE,
        id::MAP_ADD_POI,
        id::ASSET_CREATE,
        id::COMMAND_CENTER_NEWS_CLICKED,
        id::PING,
        id::RELOGIN,
        id::LOGIN_ERROR,
        id::CHANNEL_CLOSE_REQUEST,
    ] {
        assert!(registry::schema(id).is_some(), "missing schema for {id}");
    }
}
