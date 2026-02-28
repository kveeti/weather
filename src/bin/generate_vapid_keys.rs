/// Run: cargo run --bin generate-vapid-keys
fn main() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use p256::{ecdsa::SigningKey, EncodedPoint};
    use rand::rngs::OsRng;

    let signing_key = SigningKey::random(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    let pub_ep = EncodedPoint::from(verifying_key);
    let pub_bytes = pub_ep.to_bytes(); // uncompressed, 65 bytes

    let priv_bytes = signing_key.to_bytes();

    let pub_b64 = URL_SAFE_NO_PAD.encode(pub_bytes);
    let priv_b64 = URL_SAFE_NO_PAD.encode(priv_bytes);

    println!("Add these to your .env file:\n");
    println!("VAPID_PUBLIC_KEY={}", pub_b64);
    println!("VAPID_PRIVATE_KEY={}", priv_b64);
}
