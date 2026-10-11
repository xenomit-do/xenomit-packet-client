# xenomit-packet-client

Packet-level DarkOrbit game client: speaks the real wire protocol over an
established TCP connection, decodes every inbound module, encodes outbound
requests with the client's own obfuscation (bit rotations + mod-3/mod-8
coordinate checksums), and maintains a live world model for a bot.

Wire format is extracted from the decompiled Flash client
(`DOBOT/game_resources/scripts`); all 1,351 registered module schemas are
embedded in `src/generated/schema.rs` and were validated byte-exactly
against a 2,317-message live capture.

## Wire protocol

```
[0x00 greeting byte]            -- once, immediately after TCP connect
[u24 BE body len][body]         -- every frame afterwards
body := i16 module id + fields  -- big-endian; ints may be bit-rotated
```

Login flow: `VersionRequest(hash) → VersionCommand(hash, equal) →
LoginRequest(userId, factionId, sessionId, version, instanceId,
isMiniClient) → LoginResponse(status==0)`, then `ReadyRequest(MAP_LOADED)`,
`ReadyRequest(UI_READY)`, `LegacyModule("w")`.

### Session acquisition

The game socket itself has no challenge/response auth — `LoginRequest`
carrying a valid `sessionID` **is** the authentication. Session material
is minted off-band over HTTPS: a `dosid` cookie +
`BigpointClient/1.6.9` UA → `indexInternal.es?action=internalMapRevolution`
→ `flashAPI/loadingScreen.php` → the page emits an authenticated
`<embed>` whose FlashVars contain `userID`, `sessionID`, `factionID`,
`pid` (→ `instanceId`), `mapID`, `platform`. Feed those into
`LoginInfo`; the game endpoint is the map server for `mapID`
(`host:port`, caller-supplied). See `FlashVarsParser.as` for the exact
key mapping.

## Usage

```rust
use xenomit_packet_client::session::{LoginInfo, Session};

let login = LoginInfo::new(user_id, "SESSION_ID");
let mut s = Session::connect_and_login("server:6063", &login)?;
s.wait_hero_init(1024)?;        // ShipInitializationCommand applied
s.complete_ready("2D")?;        // ReadyRequests + login flag

// send actions
s.send(&xenomit_packet_client::modules::move_request(s.world.hero_pos().unwrap(), (4200, 5200)))?;

// pump events (folds into s.world)
while let Some(ev) = s.poll(std::time::Duration::from_millis(50))? {
    // Event::Packet / Event::Raw / Event::Malformed
}
```

For an already-established connection (e.g. forwarded through an
authenticating proxy), use `Session::new(stream)` — it wraps any
`Read + Write` transport and performs no I/O until asked.

## Egress proxy (`docs/05-networking-and-proxies.md`)

Every bot's game socket must leave through *its* assigned proxy —
never directly, never falling back. `src/proxy.rs` implements the
dialer; `Session::connect_via*` wraps it into a ready session:

```rust
use xenomit_packet_client::{ProxyConfig, Session};

let proxy = ProxyConfig::new("socks5h://user:pass@127.0.0.1:1080");
// or: ProxyConfig::with_auth("http://127.0.0.1:3128", "user", "pass")

let mut s = Session::connect_via_and_login(&proxy, "1.2.3.4:5001", &login)?;
// or the low-level form — a fully established tunnel, Session::new-ready:
let stream = xenomit_packet_client::proxy::dial(&proxy, "1.2.3.4:5001")?;
let s2 = Session::new(stream);
```

| URL scheme | Transport | DNS |
|---|---|---|
| `http` | `CONNECT host:port` tunnel | at the proxy |
| `socks5h` | SOCKS5 CONNECT, domain ATYP | at the proxy |
| `socks5` | SOCKS5 CONNECT, numeric ATYP | locally (scheme semantics — prefer `socks5h`) |
| `https` | **rejected** (`ProxyError::UnsupportedScheme`) — TLS to the proxy is not implemented | — |

Properties:

- **Fail closed**: any proxy failure returns `ClientError::Proxy` — a
  typed `ProxyError` (`Connect`, `Auth`, `Refused`, `Timeout`,
  `UnsupportedScheme`, …). No code path ever dials the game directly;
  `proxy::dial` is the only entry and it cannot bypass the proxy.
  `ProxyError::exit_code()` is 69 (`sysexits` EX_UNAVAILABLE) — the
  `BotdError::ProxyFailure` category.
- **Timeouts**: `dial`/`connect_via` use `DEFAULT_DIAL_TIMEOUT` (15 s)
  covering TCP-connect *and* the tunnel handshake; `dial_timeout` /
  `connect_via_timeout` take an explicit budget.
- **Credentials**: `ProxyAuth.password` is `zeroize`d on drop; `Debug` /
  `Display` of `ProxyConfig`/`ProxyAuth` redact credentials and
  `ProxyConfig::new` moves `user:pass@` out of the URL into `auth`, so a
  stored config never carries secrets. Handshake buffers holding the
  `Proxy-Authorization` value are zeroized after send.
- **DNS**: the game `host:port` is sent to the proxy verbatim (CONNECT
  request / SOCKS5 domain ATYP); only the proxy's own address resolves
  locally (infra traffic, direct by design).

### Wiring for `xenomit-botd` / `xenomit-bot-runtime`

`botd` resolves `network.proxy` into a `ProxySpec`
(`scheme, host, port, username, password`). At game-connect time the
runtime must convert it instead of calling `Session::connect`:

```rust
let cfg = spec.map(|s| {
    let mut c = ProxyConfig::new(format!("{}://{}:{}", s.scheme, s.host, s.port));
    if let Some(u) = &s.username {
        let pass = s.password.as_ref().map(|p| p.reveal()).unwrap_or("");
        c.auth = Some(ProxyAuth::new(u, pass));
    }
    c
});
let session = match &cfg {
    Some(p) => Session::connect_via_and_login(p, &endpoint, &login),
    None => Session::connect_and_login(&endpoint, &login),
};
match session {
    Err(ClientError::Proxy(e)) => BotdError::ProxyFailure(e.to_string()), // exit 69
    Err(ClientError::Codec(_) | ClientError::Io(_) | ClientError::Protocol(_)) => /* fatal */,
    other => other.map_err(|e| classify_attach_error(&e.to_string())),
}
```

Rules: honor `network.require_proxy` by refusing to arm when `cfg` is
`None`; never retry `ClientError::Proxy` directly; keep
`maps.php`/`hangar_list` fetches on the *proxied* `BackpageClient`
(see `xenomit-backpage` README) so every game-egress byte shares the
bot's exit IP.

## Coordinate checksums (anti-tamper)

The stock client scrambles coordinates with divisibility tricks; the
server rejects requests that don't satisfy them. Implemented exactly in
`modules.rs`:

| request             | invariant                                        |
|---------------------|--------------------------------------------------|
| `MoveRequest`       | `(posX+targetY)%3==0`, `(posY+targetX)%3!=0`       |
| `AttackLaserRequest`| `(targetX+targetY)%3!=0`                          |
| `AttackRocketRequest`| `(targetX+targetY)%3!=0`                         |
| `CollectBoxRequest` | `(boxY+posX+posY)%3!=0`                           |
| `ShipSelectRequest` | `(targetX+targetY+posX+posY)%8==0`                |

## Layout

- `src/codec.rs` — big-endian `Reader`/`Writer`, AS3 UTF, bit rotations
- `src/frame.rs` — 24-bit framing + incremental `FrameDecoder`
- `src/op.rs` — schema op vocabulary
- `src/packet.rs` — generic `Packet`/`Value` decode/encode via schema
- `src/registry.rs` — module-id → schema lookup (1,351 entries)
- `src/modules.rs` — typed builders/parsers for bot-relevant packets
- `src/model.rs` — `WorldState`: hero, entities, collectables, POIs,
  combat links, vitals
- `src/session.rs` — blocking `Session`: greeting, handshake, ready
  sequence, `recv`/`poll`/`pump`
- `src/proxy.rs` — egress `ProxyConfig`/`ProxyAuth`/`ProxyError` and the
  blocking CONNECT/SOCKS5 `dial` used by `Session::connect_via*`
- `src/gameapi.rs` — `GameClient` impl (feature `game-api`)
- `src/generated/schema.rs` — generated table; regenerate with
  `python tools/gen_schema_rs.py` after `tools/extract_modules.py`

## xenomit-game-api

Build with `--features game-api` to get `gameapi::PacketGameClient`, a
`GameClient` implementation over the packet session. `call_function`
supports `move_to`, `attack`, `attack_rocket`, `stop_attack`, `select`,
`collect`, `harvest`, `jump`, `keep_alive`, `legacy`, `pump`,
`send_packet`; `snapshot` maps the world model into `GameSnapshot`.

## Tests

```
cargo test                      # unit + golden + session tests
cargo test --features game-api  # everything incl. the adapter
```

`tests/capture_replay.rs` replays the DOBOT stream files in place when
`XPC_CAPTURE_DIR` (or the default `DOBOT/analysis` path) exists; it never
embeds capture data.
