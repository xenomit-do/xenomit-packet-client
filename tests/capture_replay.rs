//! Replays the reassembled streams from the DOBOT capture through the
//! real decoder. Skipped automatically when the capture directory is
//! not present (the streams contain session data and are never
//! committed to the repository).

use std::path::PathBuf;
use xenomit_packet_client::packet::decode_body;

fn capture_dir() -> Option<PathBuf> {
    let dir = std::env::var("XPC_CAPTURE_DIR")
        .unwrap_or_else(|_| r"C:\Users\Yagiz\Desktop\DOBOT\analysis".to_string());
    let p = PathBuf::from(dir);
    p.is_dir().then_some(p)
}

fn bodies(stream: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 3 <= stream.len() {
        let n = ((stream[pos] as usize) << 16)
            | ((stream[pos + 1] as usize) << 8)
            | stream[pos + 2] as usize;
        if pos + 3 + n > stream.len() {
            break;
        }
        out.push(&stream[pos + 3..pos + 3 + n]);
        pos += 3 + n;
    }
    out
}

#[test]
fn decodes_every_captured_message_strictly() {
    let Some(dir) = capture_dir() else {
        eprintln!("capture dir unavailable; skipping replay test");
        return;
    };
    let mut total = 0usize;
    let mut unknown = 0usize;
    for entry in [
        "flow_01_170.33.12.50_6063_to_192.168.1.101_53215.bin",
        "flow_01_192.168.1.101_53215_to_170.33.12.50_6063.bin",
    ] {
        let path = dir.join(entry);
        if !path.exists() {
            continue;
        }
        let stream = std::fs::read(&path).unwrap();
        for body in bodies(&stream) {
            total += 1;
            match decode_body(body, true) {
                Ok(_) => {}
                Err(xenomit_packet_client::CodecError::UnknownModule(_)) => {
                    unknown += 1;
                }
                Err(e) => panic!("{entry}: decode failed: {e}"),
            }
        }
    }
    if total == 0 {
        eprintln!("no capture streams found; skipping");
        return;
    }
    // The capture must decode fully; any UnknownModule means the
    // generated schema table is missing a registered module.
    assert_eq!(
        unknown, 0,
        "{unknown}/{total} bodies hit unknown module ids"
    );
    assert!(total >= 2317, "expected >= 2317 messages, saw {total}");
}
