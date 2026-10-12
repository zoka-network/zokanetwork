//! Comprueba que las primitivas de `crates/zoka-pq` funcionan compiladas dentro
//! del nodo, con las versiones que resuelve el Cargo.lock de la raiz. Los
//! vectores conocidos (NIST ACVP y X-Wing) viven en el propio crate; aca solo
//! se prueba que el enlace con el grafo de dependencias del nodo es correcto.
//!
//! Correr con: cargo test --features pq-experimental --test pq_experimental
#![cfg(feature = "pq-experimental")]

use zoka::pq;

#[test]
fn ml_kem_768_round_trips_inside_node_build() {
    let secret = pq::MlKem768SecretKey::generate().expect("os rng");
    let public =
        pq::MlKem768PublicKey::from_bytes(&secret.public_key().to_bytes()).expect("public key");
    let (ciphertext, sent) = public.encapsulate();
    let ciphertext = pq::MlKem768Ciphertext::from_bytes(ciphertext.as_bytes()).expect("ciphertext");
    let received = secret.decapsulate(&ciphertext);
    assert_eq!(sent.expose_secret(), received.expose_secret());
}

#[test]
fn x_wing_round_trips_from_exported_seed() {
    let secret = pq::XWingSecretKey::generate().expect("os rng");
    let restored =
        pq::XWingSecretKey::from_seed(secret.export_seed().expose_secret()).expect("seed");
    let (ciphertext, sent) = restored.public_key().encapsulate();
    let received = secret
        .decapsulate(&ciphertext)
        .expect("contributory ciphertext");
    assert_eq!(sent.expose_secret(), received.expose_secret());
    assert_eq!(
        pq::X_WING_PROFILE,
        "x-wing-draft06-rustcrypto-0.1.1-reject-noncontributory"
    );
}

#[test]
fn ml_dsa_65_signature_is_bound_to_message_and_context() {
    let secret = pq::MlDsa65SecretKey::generate().expect("os rng");
    let public = secret.public_key();
    let signature = secret.sign(b"zoka snapshot", b"zoka-test").expect("sign");
    public
        .verify(b"zoka snapshot", b"zoka-test", &signature)
        .expect("valid signature");
    assert!(public
        .verify(b"zoka snapshot", b"other", &signature)
        .is_err());
    assert!(public
        .verify(b"zoka snapshoT", b"zoka-test", &signature)
        .is_err());
}
