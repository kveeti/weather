use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes128Gcm, Nonce,
};
use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::Utc;
use hkdf::Hkdf;
use p256::{
    ecdh::EphemeralSecret,
    ecdsa::{signature::Signer, Signature, SigningKey},
    EncodedPoint, PublicKey,
};
use rand::rngs::OsRng;
use sha2::Sha256;

use crate::db::Subscription;

pub struct VapidConfig {
    pub subject: String,
    pub public_key_b64: String,
    pub private_key_b64: String,
}

fn make_client() -> reqwest::Client {
    reqwest::Client::builder()
        .use_rustls_tls()
        .build()
        .expect("reqwest client")
}

pub async fn send_all(
    subscriptions: &[Subscription],
    message: &str,
    vapid: &VapidConfig,
) -> Vec<Result<()>> {
    let client = make_client();
    let mut results = Vec::new();
    for sub in subscriptions {
        let r = send_one(&client, sub, message, vapid).await;
        results.push(r);
    }
    results
}

pub async fn send_one_sub(sub: &Subscription, message: &str, vapid: &VapidConfig) -> Result<()> {
    send_one(&make_client(), sub, message, vapid).await
}

async fn send_one(
    client: &reqwest::Client,
    sub: &Subscription,
    message: &str,
    vapid: &VapidConfig,
) -> Result<()> {
    let p256dh_bytes = URL_SAFE_NO_PAD.decode(&sub.p256dh)?;
    let auth_bytes = URL_SAFE_NO_PAD.decode(&sub.auth)?;

    let ua_pubkey =
        PublicKey::from_sec1_bytes(&p256dh_bytes).map_err(|e| anyhow!("Invalid p256dh: {e}"))?;

    let payload = encrypt_payload(message.as_bytes(), &ua_pubkey, &auth_bytes)?;

    let endpoint = &sub.endpoint;
    let origin = extract_origin(endpoint)?;
    let jwt = build_vapid_jwt(&origin, &vapid.subject, &vapid.private_key_b64)?;
    let auth_header = format!("vapid t={jwt}, k={}", vapid.public_key_b64);

    let resp = client
        .post(endpoint.as_str())
        .header("Content-Type", "application/octet-stream")
        .header("Content-Encoding", "aes128gcm")
        .header("Authorization", &auth_header)
        .header("TTL", "43200")
        .header("Urgency", "high")
        .body(payload)
        .send()
        .await?;

    let status = resp.status();
    tracing::debug!("response status {status}");
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Push endpoint returned {status}: {body}"));
    }

    Ok(())
}

/// Encrypt using RFC 8291 aes128gcm content encoding.
fn encrypt_payload(plaintext: &[u8], ua_pubkey: &PublicKey, auth_secret: &[u8]) -> Result<Vec<u8>> {
    // Generate ephemeral server key pair
    let server_secret = EphemeralSecret::random(&mut OsRng);
    let server_pubkey = server_secret.public_key();
    let server_pubkey_bytes = EncodedPoint::from(&server_pubkey).to_bytes().to_vec();

    // ECDH shared secret
    let ua_ep = EncodedPoint::from(ua_pubkey);
    let shared = server_secret.diffie_hellman(ua_pubkey);
    let shared_bytes = shared.raw_secret_bytes();

    // RFC 8291 key derivation
    // IKM via HKDF using auth_secret as salt
    let ua_pubkey_bytes = ua_ep.to_bytes();

    // info = "WebPush: info\x00" + ua_pubkey(65) + as_pubkey(65)
    let mut info = b"WebPush: info\x00".to_vec();
    info.extend_from_slice(&ua_pubkey_bytes);
    info.extend_from_slice(&server_pubkey_bytes);

    let hk = Hkdf::<Sha256>::new(Some(auth_secret), shared_bytes.as_slice());
    let mut ikm = [0u8; 32];
    hk.expand(&info, &mut ikm)
        .map_err(|_| anyhow!("HKDF expand failed for IKM"))?;

    // Random 16-byte salt
    let mut salt = [0u8; 16];
    rand::RngCore::fill_bytes(&mut OsRng, &mut salt);

    // Second HKDF using random salt and IKM
    let hk2 = Hkdf::<Sha256>::new(Some(&salt), &ikm);

    let mut cek = [0u8; 16];
    hk2.expand(b"Content-Encoding: aes128gcm\x00", &mut cek)
        .map_err(|_| anyhow!("HKDF expand failed for CEK"))?;

    let mut nonce_bytes = [0u8; 12];
    hk2.expand(b"Content-Encoding: nonce\x00", &mut nonce_bytes)
        .map_err(|_| anyhow!("HKDF expand failed for nonce"))?;

    // Encrypt: plaintext + 0x02 padding delimiter
    let mut msg = plaintext.to_vec();
    msg.push(0x02);

    let cipher = Aes128Gcm::new_from_slice(&cek)?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, msg.as_slice())
        .map_err(|_| anyhow!("AES-GCM encryption failed"))?;

    // Build aes128gcm payload:
    // salt(16) + rs(4, big-endian, record size = 4096) + idlen(1) + keyid(65) + ciphertext
    let rs: u32 = 4096;
    let mut payload = Vec::new();
    payload.extend_from_slice(&salt);
    payload.extend_from_slice(&rs.to_be_bytes());
    payload.push(server_pubkey_bytes.len() as u8); // idlen = 65
    payload.extend_from_slice(&server_pubkey_bytes);
    payload.extend_from_slice(&ciphertext);

    tracing::trace!("Encrypted payload: {} bytes", payload.len());
    Ok(payload)
}

/// Build a VAPID JWT (RFC 8292).
fn build_vapid_jwt(audience: &str, subject: &str, private_key_b64: &str) -> Result<String> {
    // Header
    let header = serde_json::json!({"alg": "ES256"});
    let header_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_string(&header)?);

    // Payload
    let exp = Utc::now().timestamp() + 3600;
    let payload = serde_json::json!({
        "aud": audience,
        "exp": exp,
        "sub": subject,
    });
    let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_string(&payload)?);

    let signing_input = format!("{header_b64}.{payload_b64}");

    // Sign with private key
    let priv_bytes = URL_SAFE_NO_PAD.decode(private_key_b64)?;
    let signing_key = SigningKey::from_bytes(priv_bytes.as_slice().into())
        .map_err(|e| anyhow!("Invalid VAPID private key: {e}"))?;

    let signature: Signature = signing_key.sign(signing_input.as_bytes());
    // Encode as fixed 64-byte r‖s (not DER)
    let sig_bytes = signature.to_bytes();
    let sig_b64 = URL_SAFE_NO_PAD.encode(sig_bytes);

    Ok(format!("{signing_input}.{sig_b64}"))
}

/// Extract origin (scheme + host) from a push endpoint URL.
fn extract_origin(endpoint: &str) -> Result<String> {
    let url = reqwest::Url::parse(endpoint)?;
    let origin = format!(
        "{}://{}",
        url.scheme(),
        url.host_str()
            .ok_or_else(|| anyhow!("No host in endpoint URL"))?
    );
    if let Some(port) = url.port() {
        Ok(format!("{}:{}", origin, port))
    } else {
        Ok(origin)
    }
}
