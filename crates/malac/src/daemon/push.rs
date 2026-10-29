//! Push notifications for paired phones. A phone registers the Web Push
//! endpoint its UnifiedPush distributor (ntfy, for one) gave it, with the
//! keys that encrypt messages to it. When an approval has waited
//! `push.wait_seconds` unanswered, every registered phone gets the request
//! ids, encrypted end to end (RFC 8291, so the push service sees nothing)
//! and signed with the daemon's VAPID key (RFC 8292); the phone fetches the
//! details over its own connection. A pushed request that is then answered
//! sends another push, so phones take the notification down.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, ensure};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use malac_core::config;
use reqwest::{StatusCode, Url};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use ring::{aead, agreement, hkdf};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// How long a push service keeps a message for a phone that is offline.
const TTL_SECONDS: u64 = 3600;
/// VAPID tokens may last a day at most.
const VAPID_LIFETIME_SECONDS: i64 = 12 * 3600;
/// Who runs this sender, for push services that need to reach someone.
const VAPID_SUBJECT: &str = "https://malac.io";
/// The aes128gcm record size; one record carries the whole message.
const RECORD_SIZE: usize = 4096;
/// A push service replaces an undelivered message with a newer one of the
/// same topic, so an offline phone gets only the latest.
const TOPIC: &str = "malac";

/// Where to send one device's pushes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subscription {
    pub endpoint: String,
    /// The device's P-256 public key, uncompressed, base64url.
    pub p256dh: String,
    /// Its 16-byte authentication secret, base64url.
    pub auth: String,
    #[serde(default)]
    pub kind: Receiver,
    pub registered: DateTime<Utc>,
}

/// What receives a device's pushes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Receiver {
    /// An app, which may take a push without showing anything.
    #[default]
    App,
    /// A browser's service worker. Each push must show a notification, or
    /// the browser (Safari above all) takes the permission back, so these
    /// get no follow-ups for answered requests.
    Browser,
}

#[derive(Default, Serialize, Deserialize)]
struct PushFile {
    /// The VAPID private key, PKCS#8, base64url.
    vapid: String,
    /// By device id.
    #[serde(default)]
    subscriptions: BTreeMap<String, Subscription>,
}

#[derive(Default)]
struct Requests {
    /// Waiting to be pushed when the timer fires.
    queued: Vec<String>,
    timer: bool,
    /// Pushed and not yet answered.
    pushed: HashSet<String>,
}

pub struct Push {
    /// `push.json`; `None` keeps everything in memory.
    path: Option<PathBuf>,
    pkcs8: Vec<u8>,
    vapid: EcdsaKeyPair,
    subscriptions: Mutex<BTreeMap<String, Subscription>>,
    requests: Mutex<Requests>,
    http: reqwest::Client,
    rng: SystemRandom,
    /// Signed VAPID headers by push service origin, with when they expire:
    /// reused, since Apple wants a token refreshed at most once an hour.
    tokens: Mutex<BTreeMap<String, (String, i64)>>,
}

impl Push {
    /// The key and registrations saved in `path`, with a new key (saved at
    /// once, so the phones' registrations stay valid) when there is none.
    pub fn load(path: PathBuf) -> Self {
        let saved = std::fs::read_to_string(&path).ok().and_then(|text| {
            serde_json::from_str::<PushFile>(&text)
                .map_err(|err| tracing::warn!("ignoring {}: {err}", path.display()))
                .ok()
        });
        let push = match saved {
            Some(file) => match URL_SAFE_NO_PAD.decode(&file.vapid) {
                Ok(pkcs8) => Self::new(Some(path), Some(pkcs8), file.subscriptions),
                Err(err) => {
                    tracing::warn!("ignoring the push key in {}: {err}", path.display());
                    Self::new(Some(path), None, BTreeMap::new())
                }
            },
            None => Self::new(Some(path), None, BTreeMap::new()),
        };
        push.save();
        push
    }

    /// Registrations that last as long as this process.
    pub fn in_memory() -> Self {
        Self::new(None, None, BTreeMap::new())
    }

    fn new(
        path: Option<PathBuf>,
        pkcs8: Option<Vec<u8>>,
        subscriptions: BTreeMap<String, Subscription>,
    ) -> Self {
        let rng = SystemRandom::new();
        let parsed = pkcs8.and_then(|pkcs8| {
            let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &pkcs8, &rng);
            key.ok().map(|key| (pkcs8, key))
        });
        let (pkcs8, vapid) = parsed.unwrap_or_else(|| {
            let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
                .expect("the system random number generator works")
                .as_ref()
                .to_vec();
            let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &pkcs8, &rng)
                .expect("a freshly generated key parses");
            (pkcs8, key)
        });
        Self {
            path,
            pkcs8,
            vapid,
            subscriptions: Mutex::new(subscriptions),
            requests: Mutex::default(),
            tokens: Mutex::default(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("an HTTP client with a timeout builds"),
            rng,
        }
    }

    /// The VAPID public key phones pass to their distributor, base64url.
    pub fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.vapid.public_key().as_ref())
    }

    pub fn is_registered(&self, device: &str) -> bool {
        self.subscriptions.lock().unwrap().contains_key(device)
    }

    pub fn has_subscriptions(&self) -> bool {
        !self.subscriptions.lock().unwrap().is_empty()
    }

    /// Registers (or replaces) a device's endpoint and keys.
    pub fn subscribe(
        &self,
        device: &str,
        endpoint: &str,
        p256dh: &str,
        auth: &str,
        kind: Receiver,
    ) -> Result<()> {
        let url = Url::parse(endpoint).context("the endpoint is not a URL")?;
        ensure!(
            matches!(url.scheme(), "https" | "http") && url.host().is_some(),
            "the endpoint must be an http or https URL"
        );
        let key = URL_SAFE_NO_PAD
            .decode(p256dh)
            .context("p256dh is not base64url")?;
        ensure!(
            key.len() == 65 && key[0] == 4,
            "p256dh must be an uncompressed P-256 point"
        );
        let secret = URL_SAFE_NO_PAD
            .decode(auth)
            .context("auth is not base64url")?;
        ensure!(secret.len() == 16, "auth must be 16 bytes");
        self.subscriptions.lock().unwrap().insert(
            device.to_owned(),
            Subscription {
                endpoint: endpoint.to_owned(),
                p256dh: p256dh.to_owned(),
                auth: auth.to_owned(),
                kind,
                registered: Utc::now(),
            },
        );
        self.save();
        Ok(())
    }

    /// Forgets a device's endpoint; `false` when it had none.
    pub fn unsubscribe(&self, device: &str) -> bool {
        let removed = self.subscriptions.lock().unwrap().remove(device).is_some();
        if removed {
            self.save();
        }
        removed
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        let file = PushFile {
            vapid: URL_SAFE_NO_PAD.encode(&self.pkcs8),
            subscriptions: self.subscriptions.lock().unwrap().clone(),
        };
        let write = || -> Result<()> {
            let text = serde_json::to_string_pretty(&file)?;
            let tmp = path.with_extension("json.tmp");
            config::write_private(&tmp, &text)?;
            std::fs::rename(&tmp, path)?;
            Ok(())
        };
        if let Err(err) = write() {
            tracing::warn!("cannot save {}: {err:#}", path.display());
        }
    }

    /// Queues a new request; `true` when the caller should start the timer
    /// that pushes the queue.
    pub fn request(&self, id: &str) -> bool {
        let mut requests = self.requests.lock().unwrap();
        requests.queued.push(id.to_owned());
        !std::mem::replace(&mut requests.timer, true)
    }

    /// The queued requests that still wait (`waiting` tells), now pushed.
    pub fn take_queue(&self, waiting: impl Fn(&str) -> bool) -> Vec<String> {
        let mut requests = self.requests.lock().unwrap();
        requests.timer = false;
        let due: Vec<String> = std::mem::take(&mut requests.queued)
            .into_iter()
            .filter(|id| waiting(id))
            .collect();
        requests.pushed.extend(due.iter().cloned());
        due
    }

    /// An answered request; `true` when it had been pushed.
    pub fn resolved(&self, id: &str) -> bool {
        let mut requests = self.requests.lock().unwrap();
        requests.queued.retain(|queued| queued != id);
        requests.pushed.remove(id)
    }

    /// Sends `payload` to every registered device, in the background; to
    /// apps only unless it is meant to show (`visible`).
    pub fn send(self: &Arc<Self>, payload: Value, visible: bool) {
        let subscriptions = self.subscriptions.lock().unwrap().clone();
        let body = payload.to_string();
        for (device, subscription) in subscriptions {
            if !visible && subscription.kind == Receiver::Browser {
                continue;
            }
            let push = self.clone();
            let body = body.clone();
            tokio::spawn(async move {
                match push.deliver(&subscription, body.as_bytes()).await {
                    Ok(()) => {}
                    Err(Gone) => {
                        // The phone uninstalled the app or dropped the registration.
                        let current = push.subscriptions.lock().unwrap().get(&device).cloned();
                        if current.as_ref() == Some(&subscription) {
                            push.unsubscribe(&device);
                            tracing::info!("push endpoint of device {device} is gone; removed it");
                        }
                    }
                }
            });
        }
    }

    async fn deliver(&self, subscription: &Subscription, payload: &[u8]) -> Result<(), Gone> {
        let request = self
            .encrypt(subscription, payload)
            .and_then(|body| Ok((body, self.authorization(&subscription.endpoint)?)));
        let (body, authorization) = match request {
            Ok(request) => request,
            Err(err) => {
                tracing::warn!("cannot prepare a push: {err:#}");
                return Ok(());
            }
        };
        let response = self
            .http
            .post(&subscription.endpoint)
            .header("TTL", TTL_SECONDS.to_string())
            .header("Urgency", "high")
            .header("Topic", TOPIC)
            .header("Content-Encoding", "aes128gcm")
            .header("Content-Type", "application/octet-stream")
            .header("Authorization", authorization)
            .body(body)
            .send()
            .await;
        match response {
            Ok(response) if response.status().is_success() => Ok(()),
            Ok(response)
                if matches!(response.status(), StatusCode::NOT_FOUND | StatusCode::GONE) =>
            {
                Err(Gone)
            }
            Ok(response) => {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                tracing::warn!("push service refused a push ({status}): {}", text.trim());
                Ok(())
            }
            Err(err) => {
                tracing::warn!("cannot reach the push service: {err}");
                Ok(())
            }
        }
    }

    /// The message encrypted for one device: RFC 8291's `aes128gcm`
    /// content coding, in a single record.
    pub(crate) fn encrypt(&self, subscription: &Subscription, plaintext: &[u8]) -> Result<Vec<u8>> {
        let device_key = URL_SAFE_NO_PAD.decode(&subscription.p256dh)?;
        let auth = URL_SAFE_NO_PAD.decode(&subscription.auth)?;
        let private = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &self.rng)
            .map_err(|_| anyhow!("cannot make an encryption key"))?;
        let public = private
            .compute_public_key()
            .map_err(|_| anyhow!("cannot make an encryption key"))?;
        let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, &device_key);
        let shared = agreement::agree_ephemeral(private, &peer, |secret| secret.to_vec())
            .map_err(|_| anyhow!("the device's key is not a P-256 point"))?;

        let mut info = b"WebPush: info\0".to_vec();
        info.extend_from_slice(&device_key);
        info.extend_from_slice(public.as_ref());
        let ikm = derive(&auth, &shared, &info, 32);
        let mut salt = [0u8; 16];
        self.rng
            .fill(&mut salt)
            .map_err(|_| anyhow!("no random numbers"))?;
        let key = derive(&salt, &ikm, b"Content-Encoding: aes128gcm\0", 16);
        let nonce = derive(&salt, &ikm, b"Content-Encoding: nonce\0", 12);

        let mut record = plaintext.to_vec();
        // The delimiter of the last (and only) record.
        record.push(2);
        let header_len = 16 + 4 + 1 + public.as_ref().len();
        ensure!(
            header_len + record.len() + aead::AES_128_GCM.tag_len() <= RECORD_SIZE,
            "the message is too long for one push"
        );
        let key = aead::UnboundKey::new(&aead::AES_128_GCM, &key)
            .map(aead::LessSafeKey::new)
            .map_err(|_| anyhow!("cannot make the content key"))?;
        let nonce = aead::Nonce::try_assume_unique_for_key(&nonce)
            .map_err(|_| anyhow!("cannot make the nonce"))?;
        key.seal_in_place_append_tag(nonce, aead::Aad::empty(), &mut record)
            .map_err(|_| anyhow!("cannot encrypt the message"))?;

        let mut body = Vec::with_capacity(header_len + record.len());
        body.extend_from_slice(&salt);
        body.extend_from_slice(&(RECORD_SIZE as u32).to_be_bytes());
        body.push(public.as_ref().len() as u8);
        body.extend_from_slice(public.as_ref());
        body.extend_from_slice(&record);
        Ok(body)
    }

    /// The `Authorization: vapid` header for a push to `endpoint`: an ES256
    /// token for the push service's origin, and this daemon's public key.
    /// A token is reused until it has an hour left.
    pub(crate) fn authorization(&self, endpoint: &str) -> Result<String> {
        let url = Url::parse(endpoint)?;
        let audience = url.origin().ascii_serialization();
        let now = Utc::now().timestamp();
        if let Some((header, expires)) = self.tokens.lock().unwrap().get(&audience) {
            if expires - now > 3600 {
                return Ok(header.clone());
            }
        }
        let expires = now + VAPID_LIFETIME_SECONDS;
        let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = json!({
            "aud": audience,
            "exp": expires,
            "sub": VAPID_SUBJECT,
        });
        let claims = URL_SAFE_NO_PAD.encode(claims.to_string());
        let signing_input = format!("{header}.{claims}");
        let signature = self
            .vapid
            .sign(&self.rng, signing_input.as_bytes())
            .map_err(|_| anyhow!("cannot sign the VAPID token"))?;
        let header = format!(
            "vapid t={signing_input}.{}, k={}",
            URL_SAFE_NO_PAD.encode(signature.as_ref()),
            self.public_key()
        );
        self.tokens
            .lock()
            .unwrap()
            .insert(audience, (header.clone(), expires));
        Ok(header)
    }
}

/// Of the 3993 bytes one push carries, what the request list may take.
const BUDGET: usize = 3600;

/// The push for waiting requests, each given as its details and a brief
/// form (ids only). Requests get their details while they fit in one push,
/// then the brief form; past that the message says `more`, and the phone
/// fetches the rest.
pub fn approvals_message(entries: impl IntoIterator<Item = (Value, Value)>) -> Value {
    let mut requests = Vec::new();
    let mut used = r#"{"type":"approvals","requests":[],"more":true}"#.len();
    let mut more = false;
    for (detailed, brief) in entries {
        let fitting = [detailed, brief]
            .into_iter()
            .map(|entry| {
                let len = entry.to_string().len() + 1;
                (entry, len)
            })
            .find(|(_, len)| used + len <= BUDGET);
        match fitting {
            Some((entry, len)) => {
                used += len;
                requests.push(entry);
            }
            None => more = true,
        }
    }
    let mut message = json!({ "type": "approvals", "requests": requests });
    if more {
        message["more"] = true.into();
    }
    message
}

/// The push service no longer knows the endpoint.
struct Gone;

/// HKDF-SHA256 with one `info`, `len` bytes long.
fn derive(salt: &[u8], ikm: &[u8], info: &[u8], len: usize) -> Vec<u8> {
    struct Len(usize);
    impl hkdf::KeyType for Len {
        fn len(&self) -> usize {
            self.0
        }
    }
    let mut out = vec![0; len];
    hkdf::Salt::new(hkdf::HKDF_SHA256, salt)
        .extract(ikm)
        .expand(&[info], Len(len))
        .and_then(|okm| okm.fill(&mut out))
        .expect("HKDF output this short is always valid");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};

    /// A device's side: its key pair (used once) and auth secret.
    fn device() -> (agreement::EphemeralPrivateKey, Subscription) {
        let rng = SystemRandom::new();
        let private =
            agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
        let public = private.compute_public_key().unwrap();
        let mut auth = [0u8; 16];
        rng.fill(&mut auth).unwrap();
        let subscription = Subscription {
            endpoint: "https://push.example.com/up/abc".into(),
            p256dh: URL_SAFE_NO_PAD.encode(public.as_ref()),
            auth: URL_SAFE_NO_PAD.encode(auth),
            kind: Receiver::App,
            registered: Utc::now(),
        };
        (private, subscription)
    }

    /// What the device does with a push: RFC 8291 decryption.
    fn decrypt(
        private: agreement::EphemeralPrivateKey,
        subscription: &Subscription,
        body: &[u8],
    ) -> Vec<u8> {
        let salt = &body[..16];
        assert_eq!(u32::from_be_bytes(body[16..20].try_into().unwrap()), 4096);
        let key_len = body[20] as usize;
        let sender = &body[21..21 + key_len];
        let mut record = body[21 + key_len..].to_vec();
        let device_key = URL_SAFE_NO_PAD.decode(&subscription.p256dh).unwrap();
        let auth = URL_SAFE_NO_PAD.decode(&subscription.auth).unwrap();
        let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, sender);
        let shared = agreement::agree_ephemeral(private, &peer, |s| s.to_vec()).unwrap();
        let mut info = b"WebPush: info\0".to_vec();
        info.extend_from_slice(&device_key);
        info.extend_from_slice(sender);
        let ikm = derive(&auth, &shared, &info, 32);
        let key = derive(salt, &ikm, b"Content-Encoding: aes128gcm\0", 16);
        let nonce = derive(salt, &ikm, b"Content-Encoding: nonce\0", 12);
        let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &key).unwrap());
        let nonce = aead::Nonce::try_assume_unique_for_key(&nonce).unwrap();
        let plain = key
            .open_in_place(nonce, aead::Aad::empty(), &mut record)
            .unwrap();
        assert_eq!(
            plain.last(),
            Some(&2),
            "ends with the last-record delimiter"
        );
        plain[..plain.len() - 1].to_vec()
    }

    #[test]
    fn encrypts_for_the_device() {
        let push = Push::in_memory();
        let (private, subscription) = device();
        let message = br#"{"type":"approvals","requests":["r1"]}"#;
        let body = push.encrypt(&subscription, message).unwrap();
        assert_eq!(decrypt(private, &subscription, &body), message);
        assert!(
            push.encrypt(&subscription, &[b'x'; 4000]).is_err(),
            "one record at most"
        );
    }

    #[test]
    fn signs_a_vapid_token_for_the_push_origin() {
        let push = Push::in_memory();
        let header = push
            .authorization("https://ntfy.example.com:8443/upAbc?up=1")
            .unwrap();
        let (token, key) = header
            .strip_prefix("vapid t=")
            .unwrap()
            .split_once(", k=")
            .unwrap();
        assert_eq!(key, push.public_key());
        let (signed, signature) = token.rsplit_once('.').unwrap();
        let public = URL_SAFE_NO_PAD.decode(key).unwrap();
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &public)
            .verify(
                signed.as_bytes(),
                &URL_SAFE_NO_PAD.decode(signature).unwrap(),
            )
            .expect("a valid ES256 signature");
        let claims: Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(signed.split_once('.').unwrap().1)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(claims["aud"], "https://ntfy.example.com:8443");
        assert_eq!(claims["sub"], VAPID_SUBJECT);
        assert!(claims["exp"].as_i64().unwrap() > Utc::now().timestamp());
    }

    #[test]
    fn reuses_a_token_per_push_service() {
        let push = Push::in_memory();
        let apple = push
            .authorization("https://web.push.apple.com/abc")
            .unwrap();
        assert_eq!(
            push.authorization("https://web.push.apple.com/def")
                .unwrap(),
            apple,
            "the same origin, the same token"
        );
        assert_ne!(
            push.authorization("https://fcm.googleapis.com/fcm/send/x")
                .unwrap(),
            apple
        );
    }

    #[test]
    fn checks_registrations() {
        let push = Push::in_memory();
        let (_, good) = device();
        push.subscribe(
            "d1",
            &good.endpoint,
            &good.p256dh,
            &good.auth,
            Receiver::App,
        )
        .unwrap();
        assert!(push.is_registered("d1"));
        assert!(
            push.subscribe("d1", "ftp://x", &good.p256dh, &good.auth, Receiver::App)
                .is_err()
        );
        assert!(
            push.subscribe("d1", &good.endpoint, "AAAA", &good.auth, Receiver::App)
                .is_err()
        );
        assert!(
            push.subscribe("d1", &good.endpoint, &good.p256dh, "AAAA", Receiver::App)
                .is_err()
        );
        assert!(push.unsubscribe("d1"));
        assert!(!push.unsubscribe("d1"));
    }

    #[test]
    fn keeps_its_key_and_registrations() {
        let dir = std::env::temp_dir().join(format!("malac-push-{}", std::process::id()));
        let path = dir.join("push.json");
        let (_, subscription) = device();
        let key = {
            let push = Push::load(path.clone());
            push.subscribe(
                "d1",
                &subscription.endpoint,
                &subscription.p256dh,
                &subscription.auth,
                Receiver::Browser,
            )
            .unwrap();
            push.public_key()
        };
        let again = Push::load(path);
        assert_eq!(again.public_key(), key);
        assert!(again.is_registered("d1"));
        assert_eq!(
            again.subscriptions.lock().unwrap()["d1"].kind,
            Receiver::Browser
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn many_requests_still_fit_one_push() {
        let id = |n: usize| format!("019a3b4c-5d6e-7f80-91a2-b3c4d5e6{n:04}");
        let entries = (0..40).map(|n| {
            let detailed = json!({
                "id": id(n),
                "agent": id(9999),
                "agent_label": "é".repeat(40),
                "tool": "m".repeat(60),
                "summary": "ő".repeat(160),
                "tab": id(9998),
                "place": "ű".repeat(60),
            });
            (detailed, json!({ "id": id(n), "agent": id(9999) }))
        });
        let message = approvals_message(entries);
        let requests = message["requests"].as_array().unwrap();
        assert!(requests[0]["summary"].is_string(), "the first ones in full");
        assert!(
            requests.last().unwrap().get("summary").is_none(),
            "then ids"
        );
        assert_eq!(message["more"], true, "and the rest left to fetch");
        let (_, subscription) = device();
        Push::in_memory()
            .encrypt(&subscription, message.to_string().as_bytes())
            .expect("fits one push");

        let one =
            approvals_message([(json!({ "id": "r1", "tool": "bash" }), json!({ "id": "r1" }))]);
        assert_eq!(
            one,
            json!({ "type": "approvals", "requests": [{ "id": "r1", "tool": "bash" }] })
        );
    }

    #[test]
    fn batches_requests_and_follows_up_on_answered_ones() {
        let push = Push::in_memory();
        assert!(push.request("r1"), "the first request starts the timer");
        assert!(!push.request("r2"), "later ones join it");
        assert!(!push.request("r3"));
        assert!(
            !push.resolved("r3"),
            "answered before its push: nothing to take down"
        );
        assert_eq!(push.take_queue(|id| id != "r2"), vec!["r1"]);
        assert!(push.request("r4"), "the next one starts a new timer");
        assert!(push.resolved("r1"), "pushed, then answered");
        assert!(!push.resolved("r1"));
    }
}
