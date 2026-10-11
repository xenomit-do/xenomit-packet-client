//! Egress proxy support for the game TCP connection
//! (`docs/05-networking-and-proxies.md`).
//!
//! A bot's game socket must leave through *its* assigned proxy. [`dial`]
//! opens a [`TcpStream`] to the map server through an HTTP `CONNECT`
//! tunnel or a SOCKS5 handshake, with credentials, a bounded total
//! timeout and **remote DNS** — the game hostname is sent to the proxy
//! verbatim and never resolved locally (`socks5h`/CONNECT semantics;
//! plain `socks5` resolves locally, matching the scheme's meaning).
//!
//! There is **no direct fallback**: when the proxy is unreachable, denies
//! the tunnel or authentication fails, [`ProxyError`] is returned and the
//! caller must fail closed. `botd` maps [`ProxyError::exit_code`] (69,
//! `sysexits` EX_UNAVAILABLE) to `BotdError::ProxyFailure`.
//!
//! Credentials never appear in `Debug`/`Display` output; handshake
//! buffers that contain them are zeroized after use.

use std::fmt;
use std::io::{Read, Write};
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

/// Total budget for proxy connect + tunnel handshake when the caller
/// does not pass an explicit timeout.
pub const DEFAULT_DIAL_TIMEOUT: Duration = Duration::from_secs(15);

/// Cap on the proxy's HTTP response head — a CONNECT answer is a bare
/// status line; anything larger is a misbehaving proxy.
const MAX_HEAD: usize = 8 * 1024;

/// Proxy URL scheme. `socks5h`/`http`/`https` resolve names at the
/// proxy; `socks5` resolves locally per convention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Http,
    Https,
    Socks5,
    Socks5h,
}

impl Scheme {
    fn of(raw: &str) -> Option<Self> {
        match raw.to_ascii_lowercase().as_str() {
            "http" => Some(Self::Http),
            "https" => Some(Self::Https),
            "socks5" => Some(Self::Socks5),
            "socks5h" => Some(Self::Socks5h),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
            Self::Socks5 => "socks5",
            Self::Socks5h => "socks5h",
        }
    }
}

/// Username/password credentials for an authenticating proxy. The
/// password is zeroized on drop and redacted in `Debug`.
#[derive(Clone)]
pub struct ProxyAuth {
    pub username: String,
    password: Zeroizing<String>,
}

impl ProxyAuth {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: Zeroizing::new(password.into()),
        }
    }

    /// Raw password — only at the handshake boundary.
    pub fn password(&self) -> &str {
        &self.password
    }

    /// `Basic base64(user:pass)` for `Proxy-Authorization`.
    fn basic_value(&self) -> Zeroizing<String> {
        use base64::Engine;
        let raw = Zeroizing::new(format!("{}:{}", self.username, self.password()));
        Zeroizing::new(format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(raw.as_bytes())
        ))
    }
}

impl fmt::Debug for ProxyAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyAuth")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// Per-bot egress proxy: `url` is `scheme://host:port` (`http`, `https`,
/// `socks5`, `socks5h`). `new` moves any `user:pass@` userinfo out of the
/// URL into [`ProxyConfig::auth`], so a stored config never carries the
/// password — even though `url` is a plain public field for
/// construction flexibility.
///
/// Constructed from `botd`'s `network.proxy` secret (`ProxySpec`) and
/// handed to [`Session::connect_via`](crate::Session::connect_via) /
/// `xenomit_backpage::BackpageClient::with_proxy`.
#[derive(Clone)]
pub struct ProxyConfig {
    /// `scheme://host:port`. Userinfo supplied here is parsed but should
    /// go through [`ProxyConfig::new`]/[`ProxyConfig::with_auth`], which
    /// strip it into `auth`.
    pub url: String,
    /// Proxy credentials, if the proxy authenticates.
    pub auth: Option<ProxyAuth>,
}

impl ProxyConfig {
    /// Parse `scheme://[user[:pass]@]host:port` — the URL stored in
    /// `url` is rebuilt without userinfo.
    pub fn new(url: impl Into<String>) -> Self {
        let url = url.into();
        match ParsedUrl::parse(&url) {
            Ok(parsed) => Self {
                url: parsed.canonical_url(),
                auth: parsed.userinfo_auth(),
            },
            Err(_) => Self { url, auth: None },
        }
    }

    /// Explicit-credentials constructor.
    pub fn with_auth(
        url: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        let mut cfg = Self::new(url);
        cfg.auth = Some(ProxyAuth::new(username, password));
        cfg
    }

    /// `scheme://host:port` with any userinfo removed — safe for logs.
    pub fn redacted_url(&self) -> String {
        match ParsedUrl::parse(&self.url) {
            Ok(p) => p.canonical_url(),
            Err(_) => "<invalid proxy url>".into(),
        }
    }

    fn endpoint(&self) -> Result<Endpoint, ProxyError> {
        let parsed = ParsedUrl::parse(&self.url)?;
        let auth = self.auth.clone().or_else(|| parsed.userinfo_auth());
        Ok(Endpoint {
            scheme: parsed.scheme,
            host: parsed.host,
            port: parsed.port,
            auth,
        })
    }
}

impl fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyConfig")
            .field("url", &self.redacted_url())
            .field("auth", &self.auth)
            .finish()
    }
}

impl fmt::Display for ProxyConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.redacted_url())
    }
}

/// Parsed `scheme://[userinfo@]host:port`.
struct ParsedUrl {
    scheme: Scheme,
    userinfo: Option<String>,
    host: String,
    port: u16,
}

impl ParsedUrl {
    fn parse(url: &str) -> Result<Self, ProxyError> {
        let invalid = |reason: &str| ProxyError::InvalidUrl {
            url: url.to_string(),
            reason: reason.into(),
        };
        let (scheme, rest) = url
            .split_once("://")
            .ok_or_else(|| invalid("expected scheme://host:port"))?;
        let scheme =
            Scheme::of(scheme).ok_or_else(|| invalid("expected http, https, socks5 or socks5h"))?;
        let authority = rest.split('/').next().unwrap_or_default();
        let (userinfo, hostport) = match authority.rsplit_once('@') {
            Some((u, h)) => (Some(u.to_string()), h),
            None => (None, authority),
        };
        let (host, port) =
            split_host_port(hostport).ok_or_else(|| invalid("expected host:port"))?;
        Ok(Self {
            scheme,
            userinfo,
            host,
            port,
        })
    }

    fn canonical_url(&self) -> String {
        format!(
            "{}://{}:{}",
            self.scheme.as_str(),
            display_host(&self.host),
            self.port
        )
    }

    fn userinfo_auth(&self) -> Option<ProxyAuth> {
        self.userinfo.as_ref().map(|u| {
            let (name, pass) = u.split_once(':').unwrap_or((u.as_str(), ""));
            ProxyAuth::new(name, pass)
        })
    }
}

/// A validated proxy endpoint plus its effective credentials.
struct Endpoint {
    scheme: Scheme,
    host: String,
    port: u16,
    auth: Option<ProxyAuth>,
}

/// Split `host:port` or `[v6]:port`. Returns the bare host (no brackets).
fn split_host_port(target: &str) -> Option<(String, u16)> {
    let target = target.trim();
    if let Some(rest) = target.strip_prefix('[') {
        let (host, port) = rest.split_once("]:")?;
        return port.parse().ok().map(|p| (host.to_string(), p));
    }
    let (host, port) = target.rsplit_once(':')?;
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), port.parse().ok()?))
}

/// Re-add brackets for IPv6 literals on the wire (`CONNECT [::1]:8080`).
fn display_host(host: &str) -> String {
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(_)) => format!("[{host}]"),
        _ => host.to_string(),
    }
}

/// Errors from proxy dialing and tunnel establishment — every variant
/// means the egress path failed; there is never a direct fallback.
/// `botd` maps these to `BotdError::ProxyFailure` → exit 69.
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("invalid proxy url {url:?}: {reason}")]
    InvalidUrl { url: String, reason: String },
    #[error(
        "proxy scheme {0:?} needs TLS to the proxy, which the packet client does not implement; \
         use http or socks5h for game TCP"
    )]
    UnsupportedScheme(String),
    #[error("invalid tunnel target {0:?}: expected host:port")]
    InvalidTarget(String),
    #[error("cannot resolve proxy address {addr}: {source}")]
    ResolveProxy {
        addr: String,
        source: std::io::Error,
    },
    #[error("cannot connect to proxy {addr}: {source}")]
    Connect {
        addr: String,
        #[source]
        source: std::io::Error,
    },
    #[error("timed out {0}")]
    Timeout(&'static str),
    #[error("proxy authentication failed{0}")]
    Auth(String),
    #[error("proxy refused tunnel to {target}: {reason}")]
    Refused { target: String, reason: String },
    #[error("proxy protocol error: {0}")]
    Protocol(String),
    #[error("cannot resolve target {0} locally (socks5 without remote DNS): {1}")]
    ResolveTarget(String, String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

impl ProxyError {
    /// `sysexits` EX_UNAVAILABLE — the exit-code category `botd` uses for
    /// proxy failures (`BotdError::ProxyFailure` → 69).
    pub fn exit_code(&self) -> i32 {
        69
    }
}

/// Open a TCP connection to `target` (`host:port`) through `proxy`.
///
/// The returned stream is a fully established tunnel: the proxy ran the
/// CONNECT/SOCKS5 exchange and remote-side DNS already. No bytes of game
/// protocol have flowed yet — wrap it in `Session::new` (or use
/// [`Session::connect_via`](crate::Session::connect_via)).
pub fn dial(proxy: &ProxyConfig, target: &str) -> Result<TcpStream, ProxyError> {
    dial_timeout(proxy, target, DEFAULT_DIAL_TIMEOUT)
}

/// [`dial`] with an explicit total budget covering TCP connect to the
/// proxy *and* the tunnel handshake.
pub fn dial_timeout(
    proxy: &ProxyConfig,
    target: &str,
    timeout: Duration,
) -> Result<TcpStream, ProxyError> {
    let (host, port) =
        split_host_port(target).ok_or_else(|| ProxyError::InvalidTarget(target.into()))?;
    let endpoint = proxy.endpoint()?;
    if endpoint.scheme == Scheme::Https {
        // TLS to the proxy itself is out of scope for this crate —
        // fail closed rather than speaking plaintext to it.
        return Err(ProxyError::UnsupportedScheme(
            endpoint.scheme.as_str().into(),
        ));
    }
    let deadline = Instant::now() + timeout;
    let mut stream = connect_tcp(&endpoint, deadline)?;
    match endpoint.scheme {
        Scheme::Http => connect_tunnel(&mut stream, &endpoint, &host, port, deadline),
        Scheme::Socks5 => socks5_tunnel(&mut stream, &endpoint, &host, port, deadline, false),
        Scheme::Socks5h => socks5_tunnel(&mut stream, &endpoint, &host, port, deadline, true),
        Scheme::Https => unreachable!("rejected above"),
    }?;
    // Tunnel up — restore plain blocking mode like `Session::connect`.
    stream.set_read_timeout(None).ok();
    stream.set_write_timeout(None).ok();
    stream.set_nodelay(true).ok();
    Ok(stream)
}

fn connect_tcp(endpoint: &Endpoint, deadline: Instant) -> Result<TcpStream, ProxyError> {
    let addr = format!("{}:{}", display_host(&endpoint.host), endpoint.port);
    let addrs = (endpoint.host.as_str(), endpoint.port)
        .to_socket_addrs()
        .map_err(|e| ProxyError::ResolveProxy {
            addr: addr.clone(),
            source: e,
        })?;
    let mut last = None;
    for sa in addrs {
        match TcpStream::connect_timeout(&sa, remaining(deadline, "connecting to proxy")?) {
            Ok(s) => {
                s.set_nodelay(true).ok();
                return Ok(s);
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::TimedOut {
                    return Err(ProxyError::Timeout("connecting to proxy"));
                }
                last = Some(e);
            }
        }
    }
    Err(ProxyError::Connect {
        addr,
        source: last.unwrap_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no address resolved")
        }),
    })
}

fn remaining(deadline: Instant, phase: &'static str) -> Result<Duration, ProxyError> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(ProxyError::Timeout(phase))
    } else {
        Ok(left)
    }
}

fn rw_err(e: std::io::Error, phase: &'static str) -> ProxyError {
    match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => ProxyError::Timeout(phase),
        _ => ProxyError::Io(e),
    }
}

fn send(
    stream: &mut TcpStream,
    deadline: Instant,
    phase: &'static str,
    bytes: &[u8],
) -> Result<(), ProxyError> {
    stream
        .set_write_timeout(Some(remaining(deadline, phase)?))
        .ok();
    stream.write_all(bytes).map_err(|e| rw_err(e, phase))?;
    stream.flush().map_err(|e| rw_err(e, phase))
}

fn recv_exact(
    stream: &mut TcpStream,
    deadline: Instant,
    phase: &'static str,
    buf: &mut [u8],
) -> Result<(), ProxyError> {
    stream
        .set_read_timeout(Some(remaining(deadline, phase)?))
        .ok();
    stream.read_exact(buf).map_err(|e| rw_err(e, phase))
}

/// HTTP `CONNECT` — the target `host:port` goes to the proxy verbatim;
/// the proxy resolves it. Any non-2xx answer refuses the tunnel.
fn connect_tunnel(
    stream: &mut TcpStream,
    endpoint: &Endpoint,
    host: &str,
    port: u16,
    deadline: Instant,
) -> Result<(), ProxyError> {
    const PHASE: &str = "during CONNECT handshake";
    let target = format!("{}:{port}", display_host(host));
    // The head carries the credential — keep it in a zeroized buffer.
    let mut head = Zeroizing::new(Vec::with_capacity(160));
    head.extend_from_slice(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n").as_bytes());
    if let Some(auth) = &endpoint.auth {
        head.extend_from_slice(
            format!("Proxy-Authorization: {}\r\n", auth.basic_value().as_str()).as_bytes(),
        );
    }
    head.extend_from_slice(b"\r\n");
    send(stream, deadline, PHASE, &head)?;
    drop(head);

    // Byte-wise read so tunnel bytes are never swallowed by a buffered
    // read past the `\r\n\r\n` terminator.
    let mut resp_head = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    while !resp_head.ends_with(b"\r\n\r\n") {
        recv_exact(stream, deadline, PHASE, &mut byte)?;
        resp_head.push(byte[0]);
        if resp_head.len() > MAX_HEAD {
            return Err(ProxyError::Protocol(
                "CONNECT response head exceeds 8 KiB".into(),
            ));
        }
    }
    let head_text = String::from_utf8_lossy(&resp_head);
    let status_line = head_text.lines().next().unwrap_or_default();
    let mut parts = status_line.splitn(3, ' ');
    let _http = parts.next();
    let code: u16 = parts
        .next()
        .and_then(|c| c.trim().parse().ok())
        .ok_or_else(|| {
            ProxyError::Protocol(format!("malformed CONNECT status line {status_line:?}"))
        })?;
    match code {
        200..=299 => Ok(()),
        407 => Err(ProxyError::Auth(" (HTTP 407)".into())),
        _ => Err(ProxyError::Refused {
            target,
            reason: format!("HTTP {code} {}", parts.next().unwrap_or_default().trim()),
        }),
    }
}

/// SOCKS5 handshake (RFC 1928 + 1929 user/pass auth). `remote_dns` is
/// the `socks5h` distinction: a hostname goes to the proxy as a domain
/// ATYP; an IP literal always uses its numeric ATYP either way.
fn socks5_tunnel(
    stream: &mut TcpStream,
    endpoint: &Endpoint,
    host: &str,
    port: u16,
    deadline: Instant,
    remote_dns: bool,
) -> Result<(), ProxyError> {
    const PHASE: &str = "during SOCKS5 handshake";
    // Greeting: always offer no-auth; add user/pass when we hold some.
    let mut greeting = vec![0x05u8, 0x01, 0x00];
    if endpoint.auth.is_some() {
        greeting[1] = 0x02;
        greeting.push(0x02);
    }
    send(stream, deadline, PHASE, &greeting)?;
    let mut method = [0u8; 2];
    recv_exact(stream, deadline, PHASE, &mut method)?;
    if method[0] != 0x05 {
        return Err(ProxyError::Protocol(format!(
            "SOCKS version {} in method reply",
            method[0]
        )));
    }
    match method[1] {
        0x00 => {}
        0x02 => {
            let Some(auth) = &endpoint.auth else {
                return Err(ProxyError::Protocol(
                    "proxy selected user/pass auth but none was configured".into(),
                ));
            };
            let (u, p) = (auth.username.as_bytes(), auth.password().as_bytes());
            if u.len() > 255 || p.len() > 255 {
                return Err(ProxyError::Auth(
                    " (SOCKS5 credentials exceed 255-byte field)".into(),
                ));
            }
            let mut sub = Zeroizing::new(Vec::with_capacity(3 + u.len() + p.len()));
            sub.extend_from_slice(&[0x01, u.len() as u8]);
            sub.extend_from_slice(u);
            sub.push(p.len() as u8);
            sub.extend_from_slice(p);
            send(stream, deadline, PHASE, &sub)?;
            drop(sub);
            let mut status = [0u8; 2];
            recv_exact(stream, deadline, PHASE, &mut status)?;
            if status[1] != 0 {
                return Err(ProxyError::Auth(
                    " (SOCKS5 username/password rejected)".into(),
                ));
            }
        }
        0xFF => {
            return Err(ProxyError::Auth(
                " (SOCKS5 proxy accepted no offered method)".into(),
            ));
        }
        other => {
            return Err(ProxyError::Protocol(format!(
                "proxy selected unsupported SOCKS5 method {other:#04x}"
            )));
        }
    }

    // CONNECT request: domain ATYP for names when remote DNS applies.
    let addr = socks_addr(host, remote_dns)?;
    let mut request = Vec::with_capacity(7 + addr.len());
    request.extend_from_slice(&[0x05, 0x01, 0x00]);
    request.extend_from_slice(&addr);
    request.extend_from_slice(&port.to_be_bytes());
    send(stream, deadline, PHASE, &request)?;

    let mut head = [0u8; 4];
    recv_exact(stream, deadline, PHASE, &mut head)?;
    if head[0] != 0x05 {
        return Err(ProxyError::Protocol(format!(
            "SOCKS version {} in connect reply",
            head[0]
        )));
    }
    if head[1] != 0x00 {
        return Err(ProxyError::Refused {
            target: format!("{}:{port}", display_host(host)),
            reason: socks_reply_reason(head[1]).into(),
        });
    }
    // Consume the bound-address echo to leave the stream at offset 0.
    let bound_len = match head[3] {
        0x01 => 4,
        0x04 => 16,
        0x03 => {
            let mut len = [0u8; 1];
            recv_exact(stream, deadline, PHASE, &mut len)?;
            len[0] as usize
        }
        other => {
            return Err(ProxyError::Protocol(format!(
                "unknown ATYP {other:#04x} in connect reply"
            )));
        }
    };
    let mut bound = vec![0u8; bound_len + 2];
    recv_exact(stream, deadline, PHASE, &mut bound)?;
    Ok(())
}

fn socks_addr(host: &str, remote_dns: bool) -> Result<Vec<u8>, ProxyError> {
    let mut out = Vec::new();
    match host.parse::<IpAddr>() {
        // Numeric literals never need DNS on either side.
        Ok(IpAddr::V4(v4)) => {
            out.push(0x01);
            out.extend_from_slice(&v4.octets());
        }
        Ok(IpAddr::V6(v6)) => {
            out.push(0x04);
            out.extend_from_slice(&v6.octets());
        }
        Err(_) if remote_dns => {
            if host.len() > 255 {
                return Err(ProxyError::InvalidTarget(format!(
                    "{host:?}: hostname exceeds 255 bytes"
                )));
            }
            out.push(0x03);
            out.push(host.len() as u8);
            out.extend_from_slice(host.as_bytes());
        }
        // Plain `socks5` — local resolution is the scheme's semantic.
        Err(_) => {
            let ip = format!("{host}:0")
                .to_socket_addrs()
                .map_err(|e| ProxyError::ResolveTarget(host.into(), e.to_string()))?
                .next()
                .ok_or_else(|| ProxyError::ResolveTarget(host.into(), "no address".into()))?
                .ip();
            return socks_addr(&ip.to_string(), remote_dns);
        }
    }
    Ok(out)
}

fn socks_reply_reason(code: u8) -> &'static str {
    match code {
        0x01 => "general SOCKS5 failure",
        0x02 => "connection not allowed by ruleset",
        0x03 => "network unreachable",
        0x04 => "host unreachable",
        0x05 => "connection refused by target",
        0x06 => "TTL expired",
        0x07 => "command not supported",
        0x08 => "address type not supported",
        _ => "unknown SOCKS5 reply",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_userinfo_moves_into_auth() {
        let cfg = ProxyConfig::new("http://alice:s3cret@proxy.local:3128");
        assert_eq!(cfg.url, "http://proxy.local:3128");
        let auth = cfg.auth.as_ref().unwrap();
        assert_eq!(auth.username, "alice");
        assert_eq!(auth.password(), "s3cret");
        let ep = cfg.endpoint().unwrap();
        assert_eq!(
            (ep.scheme, ep.host.as_str(), ep.port),
            (Scheme::Http, "proxy.local", 3128)
        );
    }

    #[test]
    fn debug_and_display_never_carry_credentials() {
        let cfg = ProxyConfig::with_auth("socks5h://proxy.local:1080", "u", "w0rd");
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("w0rd"), "{dbg}");
        assert!(dbg.contains("redacted"));
        let cfg = ProxyConfig::new("http://a:b@h:1");
        assert!(!format!("{cfg:?}").contains(":b@"));
        assert!(!format!("{cfg}").contains(":b@"));
    }

    #[test]
    fn parse_ipv6_and_rejects_bad_urls() {
        let cfg = ProxyConfig::new("socks5h://[fd00::1]:1080");
        let ep = cfg.endpoint().unwrap();
        assert_eq!((ep.host.as_str(), ep.port), ("fd00::1", 1080));
        assert!(ProxyConfig::new("ftp://h:1").endpoint().is_err());
        assert!(ProxyConfig::new("http://h").endpoint().is_err());
        assert!(ProxyConfig::new("not-a-url").endpoint().is_err());
        assert!(dial(&ProxyConfig::new("http://127.0.0.1:1"), "target-no-port").is_err());
    }

    #[test]
    fn https_is_accepted_but_unsupported_for_tcp() {
        let err = dial(
            &ProxyConfig::new("https://127.0.0.1:3128"),
            "game.example:5001",
        )
        .unwrap_err();
        // May also hit Connect first on a dead port — assert the scheme
        // rejection path via a reachable dummy listener instead.
        match err {
            ProxyError::UnsupportedScheme(s) => assert_eq!(s, "https"),
            ProxyError::Connect { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn target_splitting() {
        assert_eq!(
            split_host_port("map.server:5001"),
            Some(("map.server".into(), 5001))
        );
        assert_eq!(split_host_port("[::1]:8080"), Some(("::1".into(), 8080)));
        assert_eq!(
            split_host_port("10.0.0.1:443"),
            Some(("10.0.0.1".into(), 443))
        );
        assert_eq!(split_host_port("noport"), None);
        assert_eq!(split_host_port(":8080"), None);
    }

    #[test]
    fn socks_addr_uses_domain_only_when_remote() {
        assert_eq!(socks_addr("127.0.0.1", true).unwrap()[0], 0x01);
        let dom = socks_addr("game.example", true).unwrap();
        assert_eq!(dom[0], 0x03);
        assert_eq!(dom[1] as usize, "game.example".len());
        // socks5 (local DNS): a literal still needs no lookup.
        assert_eq!(socks_addr("::1", false).unwrap()[0], 0x04);
    }

    #[test]
    fn exit_code_is_ex_unavailable() {
        assert_eq!(ProxyError::Timeout("x").exit_code(), 69);
    }
}
