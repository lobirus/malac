//! Who may use the daemon. The master token in `~/.malac/daemon.json`
//! works only for requests from this machine. Every other client pairs
//! once: `malac remote pair` shows a short code, valid for ten minutes and
//! one device, which the device trades for a token of its own
//! (`POST /api/pair`). Device tokens are stored hashed in
//! `~/.malac/devices.json` and can be revoked one by one.

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use axum::http::{HeaderMap, header};
use chrono::{DateTime, Duration, Utc};
use malac_core::config;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;

use crate::protocol::DeviceView;

/// How long a pairing code stays valid.
pub const CODE_MINUTES: i64 = 10;
/// Wrong codes tolerated before every open code is withdrawn.
const MAX_FAILURES: u32 = 10;
/// Crockford's base32: no I, L, O or U, so codes read aloud and type well.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CODE_LEN: usize = 10;
/// `last_seen` is saved at most this often per device.
const SEEN_EVERY: Duration = Duration::minutes(1);
/// Headers a proxy or tunnel adds; their presence makes a request remote.
const FORWARDED: [&str; 5] = [
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-real-ip",
    "cf-connecting-ip",
];

/// Where a request comes from, as far as the daemon can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// This machine, addressed by a loopback name, not through a proxy. An
    /// SSH tunnel looks like this too, which is fine: it took an SSH login.
    Local,
    /// Another machine, a tunnel or proxy, or a page that resolved its own
    /// name to this machine (DNS rebinding).
    Remote,
}

/// Classifies a request by its peer address, `Host` and proxy headers.
pub fn reach(peer: IpAddr, headers: &HeaderMap) -> Reach {
    let loopback = match peer {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map_or(v6.is_loopback(), |v4| v4.is_loopback()),
    };
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    let local_name = matches!(host_name(host), "localhost" | "127.0.0.1" | "[::1]");
    let proxied = FORWARDED.iter().any(|name| headers.contains_key(*name));
    if loopback && local_name && !proxied {
        Reach::Local
    } else {
        Reach::Remote
    }
}

/// The host part of a `Host` header, without the port.
fn host_name(host: &str) -> &str {
    if host.starts_with('[') {
        return host.find(']').map_or(host, |end| &host[..=end]);
    }
    host.rsplit_once(':').map_or(host, |(name, _)| name)
}

/// Who made an authorized request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// The master token, from this machine.
    Owner,
    /// A paired device, by id.
    Device(String),
}

impl Caller {
    pub fn device(&self) -> Option<&str> {
        match self {
            Caller::Owner => None,
            Caller::Device(id) => Some(id),
        }
    }
}

/// The address to show for a request: what a proxy says it forwarded,
/// else the peer. For display only; anyone can send these headers.
pub fn display_address(peer: IpAddr, headers: &HeaderMap) -> IpAddr {
    let forwarded = ["cf-connecting-ip", "x-real-ip", "x-forwarded-for"]
        .iter()
        .filter_map(|name| headers.get(*name)?.to_str().ok())
        .find_map(|value| value.split(',').next()?.trim().parse().ok());
    forwarded.unwrap_or(peer)
}

/// Compares secrets in time that depends only on their length.
pub fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0, |acc, (x, y)| acc | (x ^ y))
            == 0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Device {
    id: String,
    name: String,
    kind: String,
    created: DateTime<Utc>,
    #[serde(default)]
    last_seen: Option<DateTime<Utc>>,
    #[serde(default)]
    address: Option<String>,
    /// SHA-256 of the token, hex.
    token_sha256: String,
}

impl Device {
    fn view(&self, current: Option<&str>) -> DeviceView {
        DeviceView {
            id: self.id.clone(),
            name: self.name.clone(),
            kind: self.kind.clone(),
            created: self.created,
            last_seen: self.last_seen,
            address: self.address.clone(),
            current: current == Some(self.id.as_str()),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct DeviceFile {
    devices: Vec<Device>,
}

struct Code {
    /// Normalized: ten characters of [`ALPHABET`].
    code: String,
    expires: DateTime<Utc>,
}

#[derive(Default)]
struct State {
    devices: Vec<Device>,
    codes: Vec<Code>,
    failures: u32,
}

/// Why a pairing attempt failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairError {
    /// Unknown, used or expired.
    Invalid,
    /// Too many wrong codes: every open code was withdrawn.
    TooMany,
}

impl std::fmt::Display for PairError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PairError::Invalid => "the pairing code is wrong, used or expired",
            PairError::TooMany => {
                "too many wrong pairing codes; create a new one with `malac remote pair`"
            }
        })
    }
}

/// Paired devices and open pairing codes.
pub struct Access {
    /// `devices.json`; `None` keeps devices in memory only.
    path: Option<PathBuf>,
    state: Mutex<State>,
    revoked: broadcast::Sender<String>,
}

impl Access {
    /// Devices saved in `path`, which is created when the first one pairs.
    pub fn load(path: PathBuf) -> Self {
        let devices = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| match serde_json::from_str::<DeviceFile>(&text) {
                Ok(file) => Some(file.devices),
                Err(err) => {
                    tracing::warn!("ignoring {}: {err}", path.display());
                    None
                }
            })
            .unwrap_or_default();
        Self::new(Some(path), devices)
    }

    /// Devices that last as long as this process, for a terminal session's server.
    pub fn in_memory() -> Self {
        Self::new(None, Vec::new())
    }

    fn new(path: Option<PathBuf>, devices: Vec<Device>) -> Self {
        let (revoked, _) = broadcast::channel(16);
        Self {
            path,
            state: Mutex::new(State {
                devices,
                ..Default::default()
            }),
            revoked,
        }
    }

    /// The device a token belongs to, noting when and from where it was seen.
    pub fn authenticate(&self, token: &str, address: Option<IpAddr>) -> Option<String> {
        let hash = sha256(token);
        let mut state = self.state.lock().unwrap();
        let device = state
            .devices
            .iter_mut()
            .find(|d| same_secret(&d.token_sha256, &hash))?;
        let id = device.id.clone();
        let now = Utc::now();
        let address = address.map(|a| a.to_string());
        let stale = device.last_seen.is_none_or(|seen| now - seen > SEEN_EVERY);
        if stale || (address.is_some() && device.address != address) {
            device.last_seen = Some(now);
            if address.is_some() {
                device.address = address;
            }
            self.save(&state.devices);
        }
        Some(id)
    }

    /// A new one-time code, valid for [`CODE_MINUTES`].
    pub fn create_code(&self) -> (String, DateTime<Utc>) {
        let code = random_code();
        let expires = Utc::now() + Duration::minutes(CODE_MINUTES);
        let mut state = self.state.lock().unwrap();
        let now = Utc::now();
        state.codes.retain(|c| c.expires > now);
        state.failures = 0;
        state.codes.push(Code {
            code: code.clone(),
            expires,
        });
        (format_code(&code), expires)
    }

    /// Trades a code for a device token. The code is used up either way.
    pub fn pair(
        &self,
        code: &str,
        name: &str,
        kind: &str,
        address: Option<IpAddr>,
    ) -> Result<(String, DeviceView), PairError> {
        let code = normalize_code(code);
        let mut state = self.state.lock().unwrap();
        let now = Utc::now();
        state.codes.retain(|c| c.expires > now);
        let Some(index) = state.codes.iter().position(|c| same_secret(&c.code, &code)) else {
            state.failures += 1;
            if state.failures >= MAX_FAILURES {
                state.codes.clear();
                tracing::warn!("too many wrong pairing codes; withdrew the open ones");
                return Err(PairError::TooMany);
            }
            return Err(PairError::Invalid);
        };
        state.codes.remove(index);
        let token = crate::web::random_token();
        let id = loop {
            let id = uuid::Uuid::new_v4().simple().to_string()[..8].to_owned();
            if !state.devices.iter().any(|d| d.id == id) {
                break id;
            }
        };
        let clean = |text: &str, fallback: &str, max: usize| {
            let text: String = text.trim().chars().filter(|c| !c.is_control()).collect();
            match text.is_empty() {
                true => fallback.to_owned(),
                false => malac_core::tools::truncate_chars(&text, max),
            }
        };
        let device = Device {
            id,
            name: clean(name, "device", 60),
            kind: clean(kind, "client", 20),
            created: now,
            last_seen: Some(now),
            address: address.map(|a| a.to_string()),
            token_sha256: sha256(&token),
        };
        let view = device.view(Some(&device.id));
        state.devices.push(device);
        self.save(&state.devices);
        Ok((token, view))
    }

    /// Every paired device, oldest first; `current` marks the caller's.
    pub fn devices(&self, current: Option<&str>) -> Vec<DeviceView> {
        let state = self.state.lock().unwrap();
        state.devices.iter().map(|d| d.view(current)).collect()
    }

    /// A paired device's name.
    pub fn device_name(&self, id: &str) -> Option<String> {
        let state = self.state.lock().unwrap();
        state
            .devices
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.name.clone())
    }

    pub fn device_count(&self) -> usize {
        self.state.lock().unwrap().devices.len()
    }

    /// Forgets a device; its token stops working at once and its open
    /// connections close.
    pub fn revoke(&self, id: &str) -> Result<DeviceView> {
        let mut state = self.state.lock().unwrap();
        let index = state
            .devices
            .iter()
            .position(|d| d.id == id)
            .with_context(|| format!("no device {id}"))?;
        let device = state.devices.remove(index);
        self.save(&state.devices);
        drop(state);
        let _ = self.revoked.send(device.id.clone());
        Ok(device.view(None))
    }

    /// Ids of revoked devices, as they are revoked.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.revoked.subscribe()
    }

    fn save(&self, devices: &[Device]) {
        let Some(path) = &self.path else { return };
        let write = || -> Result<()> {
            let text = serde_json::to_string_pretty(&DeviceFile {
                devices: devices.to_vec(),
            })?;
            let tmp = path.with_extension("json.tmp");
            config::write_private(&tmp, &text)?;
            std::fs::rename(&tmp, path)?;
            Ok(())
        };
        if let Err(err) = write() {
            tracing::warn!("cannot save {}: {err:#}", path.display());
        }
    }
}

fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Ten characters from 50 random bits of a v4 UUID (the OS generator).
fn random_code() -> String {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    // Bytes 0 to 5 are random; byte 6 carries the version.
    let mut bits = bytes[..6]
        .iter()
        .chain(&bytes[9..11])
        .fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
    let mut code = String::with_capacity(CODE_LEN);
    for _ in 0..CODE_LEN {
        code.push(char::from(ALPHABET[(bits & 31) as usize]));
        bits >>= 5;
    }
    code
}

/// `ABCDE-FGHJK`.
fn format_code(code: &str) -> String {
    let (a, b) = code.split_at(code.len() / 2);
    format!("{a}-{b}")
}

/// Uppercase, without separators, with the letters people mistype for
/// digits mapped the way Crockford's base32 does.
pub fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect()
}

/// Checks the shape of a code before it travels anywhere.
pub fn check_code(code: &str) -> Result<String> {
    let normalized = normalize_code(code);
    if normalized.len() != CODE_LEN || !normalized.bytes().all(|b| ALPHABET.contains(&b)) {
        bail!("a pairing code has ten letters and digits, like ABCDE-FGHJK");
    }
    Ok(format_code(&normalized))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(host: &str, extra: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        map.insert(header::HOST, HeaderValue::from_str(host).unwrap());
        for (name, value) in extra {
            map.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn only_loopback_by_name_without_a_proxy_is_local() {
        let lo: IpAddr = "127.0.0.1".parse().unwrap();
        let lo6: IpAddr = "::1".parse().unwrap();
        let mapped: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        let lan: IpAddr = "192.168.1.20".parse().unwrap();
        assert_eq!(reach(lo, &headers("127.0.0.1:7700", &[])), Reach::Local);
        assert_eq!(reach(lo, &headers("localhost:7700", &[])), Reach::Local);
        assert_eq!(reach(lo6, &headers("[::1]:7700", &[])), Reach::Local);
        assert_eq!(reach(mapped, &headers("[::1]", &[])), Reach::Local);
        assert_eq!(reach(lan, &headers("127.0.0.1:7700", &[])), Reach::Remote);
        assert_eq!(
            reach(lo, &headers("evil.example:7700", &[])),
            Reach::Remote,
            "a rebound name"
        );
        assert_eq!(
            reach(lo, &headers("abc.trycloudflare.com", &[])),
            Reach::Remote
        );
        assert_eq!(
            reach(
                lo,
                &headers("127.0.0.1:7700", &[("x-forwarded-for", "1.2.3.4")])
            ),
            Reach::Remote,
            "a proxy that kept the Host"
        );
        assert_eq!(reach(lo, &HeaderMap::new()), Reach::Remote);
    }

    #[test]
    fn codes_pair_one_device_once() {
        let access = Access::in_memory();
        let (code, expires) = access.create_code();
        assert_eq!(code.len(), 11);
        assert_eq!(&code[5..6], "-");
        assert!(expires > Utc::now() + Duration::minutes(9));
        assert_eq!(check_code(&code.to_lowercase()).unwrap(), code);
        assert!(check_code("short").is_err());

        let peer: IpAddr = "192.168.1.20".parse().unwrap();
        let typed = code.to_lowercase().replace('-', " ");
        let (token, device) = access.pair(&typed, "laptop", "cli", Some(peer)).unwrap();
        assert!(device.current);
        assert_eq!(device.address.as_deref(), Some("192.168.1.20"));
        assert_eq!(
            access.pair(&code, "again", "cli", None).unwrap_err(),
            PairError::Invalid,
            "used up"
        );
        assert_eq!(access.authenticate(&token, None), Some(device.id.clone()));
        assert_eq!(access.authenticate("nope", None), None);

        let mut revoked = access.subscribe();
        access.revoke(&device.id).unwrap();
        assert_eq!(revoked.try_recv().unwrap(), device.id);
        assert_eq!(access.authenticate(&token, None), None);
        assert!(access.revoke(&device.id).is_err());
    }

    #[test]
    fn wrong_codes_withdraw_the_open_ones() {
        let access = Access::in_memory();
        let (code, _) = access.create_code();
        for _ in 0..MAX_FAILURES - 1 {
            assert_eq!(
                access.pair("AAAAA-AAAAA", "x", "cli", None).unwrap_err(),
                PairError::Invalid
            );
        }
        assert_eq!(
            access.pair("AAAAA-AAAAA", "x", "cli", None).unwrap_err(),
            PairError::TooMany
        );
        assert_eq!(
            access.pair(&code, "x", "cli", None).unwrap_err(),
            PairError::TooMany,
            "the real code went with them, until a new one is made"
        );
        let (fresh, _) = access.create_code();
        assert!(access.pair(&fresh, "x", "cli", None).is_ok());
    }

    #[test]
    fn devices_persist_hashed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let access = Access::load(path.clone());
        let (code, _) = access.create_code();
        let (token, device) = access.pair(&code, " phone\n", "", None).unwrap();
        assert_eq!(device.name, "phone");
        assert_eq!(device.kind, "client");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains(&token), "only the hash is stored");
        let again = Access::load(path);
        assert_eq!(again.authenticate(&token, None), Some(device.id));
        assert_eq!(again.device_count(), 1);
    }

    #[test]
    fn secrets_compare_by_content() {
        assert!(same_secret("abc", "abc"));
        assert!(!same_secret("abc", "abd"));
        assert!(!same_secret("abc", "abcd"));
    }
}
