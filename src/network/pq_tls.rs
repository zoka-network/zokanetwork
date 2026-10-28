//! TLS híbrido postcuántico para las conexiones de iroh (QUIC): el grupo
//! X25519MLKEM768 (draft-ietf-tls-ecdhe-mlkem, código 0x11EC) sobre el proveedor
//! de `ring`, que no lo trae (rustls 0.23 solo lo tiene con aws-lc-rs).
//!
//! Por qué: lo que se graba hoy de una conexión se podría descifrar con una
//! computadora cuántica (inventario en docs/QUANTIC-INVENTARIO-CRIPTOGRAFICO.md).
//! El contenido de un snapshot es público y va firmado, pero el tráfico dice
//! quién baja qué y cuándo. Con el grupo híbrido el secreto de la sesión es
//! `secreto ML-KEM-768 ‖ secreto X25519`: hace falta romper los dos.
//!
//! Formato, el del borrador (el mismo que rustls con aws-lc-rs):
//!
//! ```text
//! cliente:  clave de encapsulación ML-KEM-768 (1184) ‖ X25519 (32)    1216 bytes
//! servidor: texto cifrado ML-KEM-768 (1088)         ‖ X25519 (32)    1120 bytes
//! secreto:  secreto ML-KEM (32)                      ‖ secreto X25519 (32)
//! ```
//!
//! El proveedor de compatibilidad ofrece también X25519 solo. El preset de
//! Quantic usa `RequiredPostQuantumTls`: no ofrece esa mitad ni permite volver
//! a un grupo clásico. TLS conserva la identidad Ed25519 de iroh; la firma
//! híbrida de snapshots se verifica aparte y no depende de esa identidad.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use iroh::endpoint::presets::{self, Preset};
use iroh::endpoint::Builder;
use rustls::crypto::{ActiveKeyExchange, CompletedKeyExchange, CryptoProvider, SharedSecret, SupportedKxGroup};
use rustls::ffdhe_groups::FfdheGroup;
use rustls::{Error, NamedGroup, PeerMisbehaved, ProtocolVersion};
use zeroize::Zeroizing;
use zoka_pq::{
    MlKem768Ciphertext, MlKem768PublicKey, MlKem768SecretKey, ML_KEM_768_CIPHERTEXT_BYTES,
    ML_KEM_768_PUBLIC_KEY_BYTES,
};

const X25519_BYTES: usize = 32;
const CLIENT_SHARE_BYTES: usize = ML_KEM_768_PUBLIC_KEY_BYTES + X25519_BYTES;
const SERVER_SHARE_BYTES: usize = ML_KEM_768_CIPHERTEXT_BYTES + X25519_BYTES;
const INVALID_KEY_SHARE: Error = Error::PeerMisbehaved(PeerMisbehaved::InvalidKeyShare);

/// X25519MLKEM768 sobre las primitivas de `zoka-pq` (ML-KEM-768, FIPS 203) y el
/// X25519 de `ring`.
#[derive(Debug)]
pub struct X25519MlKem768;

pub static X25519_MLKEM768: &dyn SupportedKxGroup = &X25519MlKem768;

/// Intercambios híbridos que este proceso completó como servidor.
static HYBRID_ACCEPTED: AtomicU64 = AtomicU64::new(0);

/// Cuántas conexiones entrantes negociaron el grupo híbrido (para el registro
/// y para comprobar que de verdad se usa).
pub fn hybrid_handshakes_accepted() -> u64 {
    HYBRID_ACCEPTED.load(Ordering::Relaxed)
}

fn classical() -> &'static dyn SupportedKxGroup {
    rustls::crypto::ring::kx_group::X25519
}

fn concat(post_quantum: &[u8], classical: &[u8]) -> SharedSecret {
    let mut secret = Zeroizing::new(Vec::with_capacity(post_quantum.len() + classical.len()));
    secret.extend_from_slice(post_quantum);
    secret.extend_from_slice(classical);
    SharedSecret::from(&secret[..])
}

impl SupportedKxGroup for X25519MlKem768 {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        let post_quantum = MlKem768SecretKey::generate().map_err(|_| Error::FailedToGetRandomBytes)?;
        let classical = classical().start()?;
        let mut pub_key = post_quantum.public_key().to_bytes().to_vec();
        pub_key.extend_from_slice(classical.pub_key());
        Ok(Box::new(ActiveX25519MlKem768 {
            post_quantum,
            classical,
            pub_key,
        }))
    }

    fn start_and_complete(&self, client_share: &[u8]) -> Result<CompletedKeyExchange, Error> {
        if client_share.len() != CLIENT_SHARE_BYTES {
            return Err(INVALID_KEY_SHARE);
        }
        let (post_quantum_share, classical_share) = client_share.split_at(ML_KEM_768_PUBLIC_KEY_BYTES);
        let encapsulation_key = MlKem768PublicKey::from_bytes(post_quantum_share).map_err(|_| INVALID_KEY_SHARE)?;
        let (ciphertext, post_quantum_secret) = encapsulation_key.encapsulate();
        let classical = classical().start_and_complete(classical_share)?;
        let mut pub_key = ciphertext.as_bytes().to_vec();
        pub_key.extend_from_slice(&classical.pub_key);
        HYBRID_ACCEPTED.fetch_add(1, Ordering::Relaxed);
        Ok(CompletedKeyExchange {
            group: self.name(),
            pub_key,
            secret: concat(post_quantum_secret.expose_secret(), classical.secret.secret_bytes()),
        })
    }

    fn ffdhe_group(&self) -> Option<FfdheGroup<'static>> {
        None
    }

    fn name(&self) -> NamedGroup {
        NamedGroup::X25519MLKEM768
    }

    fn usable_for_version(&self, version: ProtocolVersion) -> bool {
        version == ProtocolVersion::TLSv1_3
    }
}

struct ActiveX25519MlKem768 {
    post_quantum: MlKem768SecretKey,
    classical: Box<dyn ActiveKeyExchange>,
    pub_key: Vec<u8>,
}

impl ActiveKeyExchange for ActiveX25519MlKem768 {
    fn complete(self: Box<Self>, server_share: &[u8]) -> Result<SharedSecret, Error> {
        if server_share.len() != SERVER_SHARE_BYTES {
            return Err(INVALID_KEY_SHARE);
        }
        let (post_quantum_share, classical_share) = server_share.split_at(ML_KEM_768_CIPHERTEXT_BYTES);
        let ciphertext = MlKem768Ciphertext::from_bytes(post_quantum_share).map_err(|_| INVALID_KEY_SHARE)?;
        let post_quantum_secret = self.post_quantum.decapsulate(&ciphertext);
        let classical_secret = self.classical.complete(classical_share)?;
        Ok(concat(post_quantum_secret.expose_secret(), classical_secret.secret_bytes()))
    }

    fn hybrid_component(&self) -> Option<(NamedGroup, &[u8])> {
        Some((self.classical.group(), self.classical.pub_key()))
    }

    fn complete_hybrid_component(self: Box<Self>, peer_pub_key: &[u8]) -> Result<SharedSecret, Error> {
        self.classical.complete(peer_pub_key)
    }

    fn pub_key(&self) -> &[u8] {
        &self.pub_key
    }

    fn ffdhe_group(&self) -> Option<FfdheGroup<'static>> {
        None
    }

    fn group(&self) -> NamedGroup {
        NamedGroup::X25519MLKEM768
    }
}

/// El proveedor de `ring` con el grupo híbrido primero (el preferido) y los
/// clásicos detrás.
pub fn provider() -> CryptoProvider {
    let mut provider = rustls::crypto::ring::default_provider();
    provider.kx_groups.insert(0, X25519_MLKEM768);
    provider
}

/// A Quantic peer must never silently negotiate only the classical half.
/// Keep the compatibility provider above for explicit legacy tools/tests.
#[derive(Debug)]
struct RequiredHybridGroup;

impl SupportedKxGroup for RequiredHybridGroup {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        Ok(Box::new(RequiredHybridExchange(X25519_MLKEM768.start()?)))
    }

    fn start_and_complete(&self, share: &[u8]) -> Result<CompletedKeyExchange, Error> {
        X25519_MLKEM768.start_and_complete(share)
    }

    fn ffdhe_group(&self) -> Option<FfdheGroup<'static>> { None }
    fn name(&self) -> NamedGroup { NamedGroup::X25519MLKEM768 }
    fn usable_for_version(&self, version: ProtocolVersion) -> bool {
        version == ProtocolVersion::TLSv1_3
    }
}

struct RequiredHybridExchange(Box<dyn ActiveKeyExchange>);

impl ActiveKeyExchange for RequiredHybridExchange {
    fn complete(self: Box<Self>, share: &[u8]) -> Result<SharedSecret, Error> {
        self.0.complete(share)
    }
    fn pub_key(&self) -> &[u8] { self.0.pub_key() }
    fn ffdhe_group(&self) -> Option<FfdheGroup<'static>> { None }
    fn group(&self) -> NamedGroup { NamedGroup::X25519MLKEM768 }
    fn complete_hybrid_component(self: Box<Self>, _share: &[u8]) -> Result<SharedSecret, Error> {
        Err(INVALID_KEY_SHARE)
    }
    // No classical component is offered, even in the first ClientHello.
}

pub fn required_hybrid_provider() -> CryptoProvider {
    let mut provider = rustls::crypto::ring::default_provider();
    provider.kx_groups = vec![&RequiredHybridGroup];
    provider
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RequiredPostQuantumTls<P>(pub P);

impl<P: Preset> Preset for RequiredPostQuantumTls<P> {
    fn apply(self, builder: Builder) -> Builder {
        self.0.apply(builder).crypto_provider(Arc::new(required_hybrid_provider()))
    }
}

/// Un preset de iroh con el TLS híbrido encima de `inner`.
#[derive(Debug, Clone, Copy, Default)]
pub struct PostQuantumTls<P>(pub P);

impl<P: Preset> Preset for PostQuantumTls<P> {
    fn apply(self, builder: Builder) -> Builder {
        self.0.apply(builder).crypto_provider(Arc::new(provider()))
    }
}

/// El preset de producción del nodo: el de n0 (relays para atravesar NAT) y, en
/// un proceso que sigue una red de Zoka Quantic, el TLS híbrido. Zenith queda
/// como estaba.
#[derive(Debug, Clone, Copy, Default)]
pub struct NodePreset;

impl Preset for NodePreset {
    fn apply(self, builder: Builder) -> Builder {
        if crate::network::p2p::network_protocol_is_set() {
            RequiredPostQuantumTls(presets::N0).apply(builder)
        } else {
            presets::N0.apply(builder)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_provider_cannot_offer_a_classical_downgrade() {
        let provider = required_hybrid_provider();
        assert_eq!(provider.kx_groups.len(), 1);
        let exchange = provider.kx_groups[0].start().unwrap();
        assert_eq!(exchange.group(), NamedGroup::X25519MLKEM768);
        assert!(exchange.hybrid_component().is_none());
        assert!(exchange.complete_hybrid_component(&[1; 32]).is_err());
    }

    #[tokio::test]
    async fn required_iroh_tls_accepts_hybrid_and_rejects_classical_peers() {
        use crate::network::iroh_snapshot_transfer::{fetch_snapshot, serve_snapshot};
        use std::time::Duration;
        let payload = vec![42; 4096];
        let strict = || RequiredPostQuantumTls(presets::Minimal);
        let server = serve_snapshot(Arc::new(payload.clone()), strict()).await.unwrap();
        assert_eq!(fetch_snapshot(server.addr(), strict()).await.unwrap(), payload);
        let rejected = tokio::time::timeout(
            Duration::from_secs(10), fetch_snapshot(server.addr(), presets::Minimal)
        ).await;
        assert!(rejected.is_err() || rejected.unwrap().is_err());
        server.close().await;
        let classical = serve_snapshot(Arc::new(payload), presets::Minimal).await.unwrap();
        let rejected = tokio::time::timeout(
            Duration::from_secs(10), fetch_snapshot(classical.addr(), strict())
        ).await;
        assert!(rejected.is_err() || rejected.unwrap().is_err());
        classical.close().await;
    }

    #[test]
    fn both_sides_agree_on_a_secret_made_of_both_halves() {
        let client = X25519_MLKEM768.start().unwrap();
        assert_eq!(client.pub_key().len(), CLIENT_SHARE_BYTES);
        assert_eq!(client.group(), NamedGroup::X25519MLKEM768);
        let server = X25519_MLKEM768.start_and_complete(client.pub_key()).unwrap();
        assert_eq!(server.pub_key.len(), SERVER_SHARE_BYTES);
        let server_secret = server.secret.secret_bytes().to_vec();
        assert_eq!(server_secret.len(), 64, "32 de ML-KEM y 32 de X25519");
        let client_secret = client.complete(&server.pub_key).unwrap();
        assert_eq!(client_secret.secret_bytes(), &server_secret[..]);
    }

    #[test]
    fn a_share_of_the_wrong_size_or_a_changed_ciphertext_does_not_agree() {
        assert!(X25519_MLKEM768.start_and_complete(&[0u8; 32]).is_err());
        let client = X25519_MLKEM768.start().unwrap();
        let server = X25519_MLKEM768.start_and_complete(client.pub_key()).unwrap();
        let mut tampered = server.pub_key.clone();
        tampered[10] ^= 1;
        // ML-KEM rechaza en forma implícita: da otro secreto, no un error.
        let client_secret = client.complete(&tampered).unwrap();
        assert_ne!(client_secret.secret_bytes(), server.secret.secret_bytes());
        let client = X25519_MLKEM768.start().unwrap();
        assert!(client.complete(&server.pub_key[..100]).is_err());
    }

    #[test]
    fn a_classical_peer_can_still_answer_with_x25519_alone() {
        let client = X25519_MLKEM768.start().unwrap();
        let (group, classical_share) = client.hybrid_component().unwrap();
        assert_eq!(group, NamedGroup::X25519);
        let server = rustls::crypto::ring::kx_group::X25519
            .start_and_complete(classical_share)
            .unwrap();
        let client_secret = client.complete_hybrid_component(&server.pub_key).unwrap();
        assert_eq!(client_secret.secret_bytes(), server.secret.secret_bytes());
    }

    #[test]
    fn the_provider_prefers_the_hybrid_group_and_keeps_the_classical_ones() {
        let provider = provider();
        assert_eq!(provider.kx_groups[0].name(), NamedGroup::X25519MLKEM768);
        assert!(provider.kx_groups.iter().any(|group| group.name() == NamedGroup::X25519));
    }

    /// Por iroh de verdad (QUIC en la máquina, sin relays): con los dos lados
    /// híbridos se negocia el grupo híbrido, y con un lado clásico la conexión
    /// sigue andando.
    #[tokio::test]
    async fn an_iroh_transfer_uses_the_hybrid_group_and_still_talks_to_classical_peers() {
        use crate::network::iroh_snapshot_transfer::{fetch_snapshot, serve_snapshot};
        let payload: Vec<u8> = (0..64_000u32).map(|i| (i % 249) as u8).collect();
        let pq = || PostQuantumTls(presets::Minimal);

        let server = serve_snapshot(Arc::new(payload.clone()), pq()).await.unwrap();
        let before = hybrid_handshakes_accepted();
        assert_eq!(fetch_snapshot(server.addr(), pq()).await.unwrap(), payload);
        assert!(hybrid_handshakes_accepted() > before, "the hybrid group was negotiated");
        // Un cliente clásico contra un servidor híbrido.
        assert_eq!(fetch_snapshot(server.addr(), presets::Minimal).await.unwrap(), payload);
        server.close().await;

        // Un cliente híbrido contra un servidor clásico (nodos de antes).
        let classical = serve_snapshot(Arc::new(payload.clone()), presets::Minimal).await.unwrap();
        assert_eq!(fetch_snapshot(classical.addr(), pq()).await.unwrap(), payload);
        classical.close().await;
    }
}
