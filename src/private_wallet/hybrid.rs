//! Direcciones y salidas híbridas de Zoka Quantic (etapa 3, experimental).
//!
//! Especificación completa: `docs/ETAPA3-direcciones-hibridas.md`. En corto:
//!
//! - Dirección v2 (`zkq1…`): versión, la clave pública de gasto de siempre
//!   (Ristretto, la misma de la dirección `zka1`) y una clave pública X-Wing
//!   (X25519 + ML-KEM-768). Unos 1.715 caracteres.
//! - Cada salida lleva un ciphertext X-Wing (1.120 bytes) en lugar de la clave
//!   efímera Ristretto de 32 bytes. El secreto compartido, ligado al
//!   ciphertext y al compromiso de la salida, deriva la clave de un solo uso y
//!   el ciego del monto.
//! - Reconocer una salida o leer su monto exige romper X25519 **y**
//!   ML-KEM-768. La autorización de gasto sigue siendo clásica (Ristretto):
//!   eso es la etapa 5.
//!
//! Las reglas de consenso de la salida v2 (formato y activación) viven en el
//! módulo padre y están en todas las builds; acá están el armado y el
//! escaneo, que necesitan X-Wing. La activación en Zenith depende de
//! `economics::PqFeature::HybridAddresses`, hoy sin altura.

use super::{
    constant_time_eq_bytes, derive_spending_key_from_recovery_seed, hash_to_scalar,
    scalar_from_secret_bytes, HYBRID_PAYMENT_ADDRESS_PREFIX,
};
use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT;
use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use curve25519_dalek::scalar::Scalar;
use curve25519_dalek::traits::Identity;
use sha2::{Digest, Sha256};
use std::fmt;
use zeroize::Zeroize;
use zoka_pq::{
    XWingCiphertext, XWingPublicKey, XWingSecretKey, X_WING_CIPHERTEXT_BYTES,
    X_WING_PUBLIC_KEY_BYTES,
};

pub const HYBRID_ADDRESS_PREFIX: &str = HYBRID_PAYMENT_ADDRESS_PREFIX;
pub const HYBRID_ADDRESS_VERSION: u8 = 2;
pub const HYBRID_KEM_CIPHERTEXT_BYTES: usize = X_WING_CIPHERTEXT_BYTES;
// El consenso valida el largo sin depender de X-Wing; tienen que coincidir.
const _: () = assert!(HYBRID_KEM_CIPHERTEXT_BYTES == super::HYBRID_OUTPUT_KEM_CIPHERTEXT_BYTES);
/// Ninguna dirección válida llega a este largo. Se corta antes de decodificar
/// Base58 para no gastar CPU en entradas gigantes.
pub const HYBRID_ADDRESS_MAX_CHARS: usize = 1800;

const SPEND_PUBLIC_BYTES: usize = 32;
const PAYLOAD_BYTES: usize = 1 + SPEND_PUBLIC_BYTES + X_WING_PUBLIC_KEY_BYTES;
const CHECKSUM_BYTES: usize = 4;

// Etiquetas de dominio. Cada derivación tiene la suya: un mismo secreto
// compartido nunca produce dos valores intercambiables.
const ADDRESS_CHECKSUM_DOMAIN: &[u8] = b"zoka-hybrid-address-v2";
const SCAN_SEED_DOMAIN: &[u8] = b"zoka-hybrid-scan-seed-v2";
const OUTPUT_CONTEXT_DOMAIN: &[u8] = b"zoka-hybrid-output-ctx-v2";
const OUTPUT_KEY_DOMAIN: &[u8] = b"zoka-hybrid-output-key-v2";
const AMOUNT_BLIND_DOMAIN: &[u8] = b"zoka-hybrid-amount-blind-v2";
const STEALTH_TAG_DOMAIN: &[u8] = b"zoka-hybrid-stealth-v2";

/// Los errores nunca incluyen claves, secretos ni el texto de la dirección.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HybridError {
    TooLong,
    Prefix,
    Base58,
    Length { expected: usize, actual: usize },
    Checksum,
    Version(u8),
    SpendKey,
    KemKey,
    RootSeed,
}

impl fmt::Display for HybridError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong => f.write_str("hybrid address is longer than any valid one"),
            Self::Prefix => write!(f, "hybrid address must start with {HYBRID_ADDRESS_PREFIX}"),
            Self::Base58 => f.write_str("hybrid address is not valid Base58"),
            Self::Length { expected, actual } => {
                write!(f, "hybrid address has {actual} bytes, expected {expected}")
            }
            Self::Checksum => f.write_str("hybrid address checksum mismatch"),
            Self::Version(version) => write!(f, "unsupported hybrid address version {version}"),
            Self::SpendKey => f.write_str("hybrid address spend key is not a valid point"),
            Self::KemKey => f.write_str("hybrid address X-Wing key is invalid"),
            Self::RootSeed => f.write_str("wallet root seed must be 16 or 32 bytes"),
        }
    }
}

impl std::error::Error for HybridError {}

/// Dirección v2: clave de gasto Ristretto + clave pública X-Wing.
#[derive(Clone)]
pub struct HybridAddress {
    spend_public: RistrettoPoint,
    kem_public: XWingPublicKey,
}

impl HybridAddress {
    pub fn spend_public(&self) -> RistrettoPoint {
        self.spend_public
    }

    pub fn kem_public_bytes(&self) -> [u8; X_WING_PUBLIC_KEY_BYTES] {
        self.kem_public.to_bytes()
    }

    pub fn encode(&self) -> String {
        let mut bytes = Vec::with_capacity(PAYLOAD_BYTES + CHECKSUM_BYTES);
        bytes.push(HYBRID_ADDRESS_VERSION);
        bytes.extend_from_slice(self.spend_public.compress().as_bytes());
        bytes.extend_from_slice(&self.kem_public.to_bytes());
        let checksum = address_checksum(&bytes);
        bytes.extend_from_slice(&checksum);
        format!(
            "{HYBRID_ADDRESS_PREFIX}{}",
            bs58::encode(bytes).into_string()
        )
    }

    /// Valida todo antes de aceptar: largo, prefijo, Base58, checksum,
    /// versión, que la clave de gasto sea un punto válido distinto de la
    /// identidad y que la clave X-Wing tenga el formato canónico.
    pub fn decode(text: &str) -> Result<Self, HybridError> {
        let text = text.trim();
        if text.len() > HYBRID_ADDRESS_MAX_CHARS {
            return Err(HybridError::TooLong);
        }
        let encoded = text
            .strip_prefix(HYBRID_ADDRESS_PREFIX)
            .ok_or(HybridError::Prefix)?;
        let bytes = bs58::decode(encoded)
            .into_vec()
            .map_err(|_| HybridError::Base58)?;
        if bytes.len() != PAYLOAD_BYTES + CHECKSUM_BYTES {
            return Err(HybridError::Length {
                expected: PAYLOAD_BYTES + CHECKSUM_BYTES,
                actual: bytes.len(),
            });
        }
        let (payload, checksum) = bytes.split_at(PAYLOAD_BYTES);
        if checksum != address_checksum(payload) {
            return Err(HybridError::Checksum);
        }
        if payload[0] != HYBRID_ADDRESS_VERSION {
            return Err(HybridError::Version(payload[0]));
        }
        let mut spend_bytes = [0u8; SPEND_PUBLIC_BYTES];
        spend_bytes.copy_from_slice(&payload[1..1 + SPEND_PUBLIC_BYTES]);
        let spend_public = CompressedRistretto(spend_bytes)
            .decompress()
            .filter(|point| *point != RistrettoPoint::identity())
            .ok_or(HybridError::SpendKey)?;
        let kem_public = XWingPublicKey::from_bytes(&payload[1 + SPEND_PUBLIC_BYTES..])
            .map_err(|_| HybridError::KemKey)?;
        Ok(Self {
            spend_public,
            kem_public,
        })
    }
}

impl fmt::Debug for HybridAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let spend = hex::encode(self.spend_public.compress().as_bytes());
        f.debug_struct("HybridAddress")
            .field("spend_public", &spend)
            .field("kem_public_bytes", &X_WING_PUBLIC_KEY_BYTES)
            .finish()
    }
}

fn address_checksum(payload: &[u8]) -> [u8; CHECKSUM_BYTES] {
    let mut hasher = Sha256::new();
    hasher.update(ADDRESS_CHECKSUM_DOMAIN);
    hasher.update([0x1f]);
    hasher.update(payload);
    let digest = hasher.finalize();
    let mut checksum = [0u8; CHECKSUM_BYTES];
    checksum.copy_from_slice(&digest[..CHECKSUM_BYTES]);
    checksum
}

/// Clave de escaneo v2: reconoce salidas propias y lee sus montos, pero no
/// autoriza gastos.
///
/// Sale de la "scan key" de la wallet, la misma de la dirección `zka1`, con
/// una etiqueta de dominio propia. En una wallet creada desde una frase esa
/// clave es la raíz (los 16 o 32 bytes de entropía); en una de claves al azar
/// o rotadas, son sus 32 bytes.
pub struct HybridScanKey {
    kem_secret: XWingSecretKey,
}

impl HybridScanKey {
    pub fn from_scan_key(scan_key: &[u8]) -> Result<Self, HybridError> {
        if scan_key.len() != 16 && scan_key.len() != 32 {
            return Err(HybridError::RootSeed);
        }
        let mut hasher = Sha256::new();
        hasher.update(SCAN_SEED_DOMAIN);
        hasher.update([0x1f]);
        hasher.update(scan_key);
        let mut scan_seed: [u8; 32] = hasher.finalize().into();
        let kem_secret = XWingSecretKey::from_seed(&scan_seed);
        scan_seed.zeroize();
        Ok(Self {
            kem_secret: kem_secret.map_err(|_| HybridError::KemKey)?,
        })
    }

    /// Dirección v2 de la wallet cuya clave de gasto pública es `spend_public`.
    pub fn address(&self, spend_public: RistrettoPoint) -> HybridAddress {
        HybridAddress {
            spend_public,
            kem_public: self.kem_secret.public_key(),
        }
    }

    /// Intenta reconocer una salida v2 dirigida a `spend_public`. `None` si no
    /// es de esta wallet o si el ciphertext está mal formado o es no
    /// contributivo: los tres casos se tratan igual, sin distinguir por qué.
    pub fn scan(
        &self,
        spend_public: &RistrettoPoint,
        kem_ciphertext: &[u8],
        commitment: &str,
        one_time_public_key_hex: &str,
    ) -> Option<HybridOwnedOutput> {
        let ciphertext = XWingCiphertext::from_bytes(kem_ciphertext).ok()?;
        let shared = self.kem_secret.decapsulate(&ciphertext).ok()?;
        let context = output_context(kem_ciphertext, commitment);
        let key_offset = hash_to_scalar(&[OUTPUT_KEY_DOMAIN, shared.expose_secret(), &context]);
        let expected = (spend_public + key_offset * RISTRETTO_BASEPOINT_POINT).compress();
        let claimed = hex::decode(one_time_public_key_hex).ok()?;
        if !constant_time_eq_bytes(expected.as_bytes(), &claimed) {
            return None;
        }
        let amount_blind = hash_to_scalar(&[AMOUNT_BLIND_DOMAIN, shared.expose_secret(), &context]);
        Some(HybridOwnedOutput {
            key_offset,
            amount_blind,
        })
    }

    /// Ciego del monto de una salida que ya se sabe propia. No comprueba
    /// pertenencia: por el rechazo implícito, un ciphertext ajeno también da
    /// un valor, que no abre nada. Para reconocer salidas, [`Self::scan`].
    pub fn amount_blind(&self, kem_ciphertext: &[u8], commitment: &str) -> Option<Scalar> {
        let ciphertext = XWingCiphertext::from_bytes(kem_ciphertext).ok()?;
        let shared = self.kem_secret.decapsulate(&ciphertext).ok()?;
        let context = output_context(kem_ciphertext, commitment);
        Some(hash_to_scalar(&[
            AMOUNT_BLIND_DOMAIN,
            shared.expose_secret(),
            &context,
        ]))
    }
}

impl fmt::Debug for HybridScanKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HybridScanKey([redacted])")
    }
}

/// Claves de una wallet para direcciones v2: la de gasto de siempre y la de
/// escaneo X-Wing.
///
/// La clave de gasto es exactamente la de la dirección `zka1`, así que una
/// misma wallet gasta salidas v1 y v2 y la frase de siempre recupera ambas.
/// Una frase de 12 palabras da 128 bits de entropía; para Quantic se
/// recomiendan 24 (ver la especificación).
pub struct HybridWalletKeys {
    spend_secret: Scalar,
    spend_public: RistrettoPoint,
    scan_key: HybridScanKey,
}

impl HybridWalletKeys {
    /// Desde la raíz de una frase (16 o 32 bytes de entropía).
    pub fn from_root_seed(root: &[u8]) -> Result<Self, HybridError> {
        if root.len() != 16 && root.len() != 32 {
            return Err(HybridError::RootSeed);
        }
        let mut spending_key = derive_spending_key_from_recovery_seed(root);
        let keys = Self::from_wallet_keys(&spending_key, root);
        spending_key.zeroize();
        keys
    }

    /// Desde las claves guardadas de una wallet. Con una wallet creada desde
    /// una frase da lo mismo que [`Self::from_root_seed`]; también sirve para
    /// las de claves al azar o rotadas, cuya clave de gasto no sale de la raíz.
    pub fn from_wallet_keys(spending_key: &[u8], scan_key: &[u8]) -> Result<Self, HybridError> {
        let scan_key = HybridScanKey::from_scan_key(scan_key)?;
        let spend_secret = scalar_from_secret_bytes(spending_key);
        Ok(Self {
            spend_secret,
            spend_public: spend_secret * RISTRETTO_BASEPOINT_POINT,
            scan_key,
        })
    }

    pub fn address(&self) -> HybridAddress {
        self.scan_key.address(self.spend_public)
    }

    /// Ver [`HybridScanKey::scan`].
    pub fn scan(
        &self,
        kem_ciphertext: &[u8],
        commitment: &str,
        one_time_public_key_hex: &str,
    ) -> Option<HybridOwnedOutput> {
        self.scan_key.scan(
            &self.spend_public,
            kem_ciphertext,
            commitment,
            one_time_public_key_hex,
        )
    }

    /// Secreto de un solo uso de una salida propia: el mismo esquema que v1
    /// (clave de gasto + desplazamiento), con el desplazamiento derivado del
    /// secreto X-Wing.
    pub fn one_time_secret(&self, owned: &HybridOwnedOutput) -> Scalar {
        self.spend_secret + owned.key_offset
    }
}

impl Drop for HybridWalletKeys {
    fn drop(&mut self) {
        self.spend_secret.zeroize();
    }
}

impl fmt::Debug for HybridWalletKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HybridWalletKeys([redacted])")
    }
}

/// Lo que la wallet obtiene al reconocer una salida propia.
pub struct HybridOwnedOutput {
    key_offset: Scalar,
    pub amount_blind: Scalar,
}

impl Drop for HybridOwnedOutput {
    fn drop(&mut self) {
        self.key_offset.zeroize();
        self.amount_blind.zeroize();
    }
}

impl fmt::Debug for HybridOwnedOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HybridOwnedOutput([redacted])")
    }
}

/// Material público y privado de una salida v2 hacia `recipient`.
pub struct HybridOutputMaterial {
    pub kem_ciphertext: Vec<u8>,
    pub one_time_public_key: String,
    pub amount_blind: Scalar,
    pub stealth_address: String,
}

impl fmt::Debug for HybridOutputMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HybridOutputMaterial")
            .field("kem_ciphertext_bytes", &self.kem_ciphertext.len())
            .field("one_time_public_key", &self.one_time_public_key)
            .field("amount_blind", &"[redacted]")
            .field("stealth_address", &self.stealth_address)
            .finish()
    }
}

/// Arma una salida v2: encapsula hacia la clave X-Wing del destinatario y
/// deriva la clave de un solo uso y el ciego del monto del secreto
/// compartido, ligado al ciphertext y al compromiso de la salida.
///
/// Como en el crate base, si el generador de aleatoriedad del sistema falla el
/// encapsulado entra en pánico; no exponer esto como servicio remoto.
pub fn hybrid_output_material(recipient: &HybridAddress, commitment: &str) -> HybridOutputMaterial {
    let (ciphertext, shared) = recipient.kem_public.encapsulate();
    let kem_ciphertext = ciphertext.as_bytes().to_vec();
    let context = output_context(&kem_ciphertext, commitment);
    let key_offset = hash_to_scalar(&[OUTPUT_KEY_DOMAIN, shared.expose_secret(), &context]);
    let amount_blind = hash_to_scalar(&[AMOUNT_BLIND_DOMAIN, shared.expose_secret(), &context]);
    let one_time_public_key = hex::encode(
        (recipient.spend_public + key_offset * RISTRETTO_BASEPOINT_POINT)
            .compress()
            .as_bytes(),
    );
    let stealth_address = stealth_tag(&kem_ciphertext, commitment, &one_time_public_key);
    HybridOutputMaterial {
        kem_ciphertext,
        one_time_public_key,
        amount_blind,
        stealth_address,
    }
}

fn output_context(kem_ciphertext: &[u8], commitment: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(OUTPUT_CONTEXT_DOMAIN);
    hasher.update([0x1f]);
    hasher.update(kem_ciphertext);
    hasher.update([0x1f]);
    hasher.update(commitment.as_bytes());
    hasher.finalize().into()
}

fn stealth_tag(kem_ciphertext: &[u8], commitment: &str, one_time_public_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(STEALTH_TAG_DOMAIN);
    hasher.update([0x1f]);
    hasher.update(kem_ciphertext);
    hasher.update([0x1f]);
    hasher.update(commitment.as_bytes());
    hasher.update([0x1f]);
    hasher.update(one_time_public_key.as_bytes());
    format!("stlth2_{}", hex::encode(&hasher.finalize()[..20]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    fn reencode_with(mutate: impl FnOnce(&mut Vec<u8>)) -> String {
        let address = HybridWalletKeys::from_root_seed(&root(7))
            .unwrap()
            .address();
        let mut bytes = bs58::decode(&address.encode()[HYBRID_ADDRESS_PREFIX.len()..])
            .into_vec()
            .unwrap();
        bytes.truncate(PAYLOAD_BYTES);
        mutate(&mut bytes);
        let checksum = address_checksum(&bytes);
        bytes.extend_from_slice(&checksum);
        format!(
            "{HYBRID_ADDRESS_PREFIX}{}",
            bs58::encode(bytes).into_string()
        )
    }

    #[test]
    fn address_round_trips_and_is_deterministic_from_the_root() {
        let keys = HybridWalletKeys::from_root_seed(&root(7)).unwrap();
        let encoded = keys.address().encode();
        assert!(encoded.starts_with(HYBRID_ADDRESS_PREFIX));
        assert!(
            (1700..=HYBRID_ADDRESS_MAX_CHARS).contains(&encoded.len()),
            "{}",
            encoded.len()
        );
        let decoded = HybridAddress::decode(&encoded).unwrap();
        assert_eq!(decoded.spend_public(), keys.address().spend_public());
        assert_eq!(
            decoded.kem_public_bytes(),
            keys.address().kem_public_bytes()
        );
        assert_eq!(decoded.encode(), encoded);

        let again = HybridWalletKeys::from_root_seed(&root(7)).unwrap();
        assert_eq!(again.address().encode(), encoded);
        let other = HybridWalletKeys::from_root_seed(&root(8)).unwrap();
        assert_ne!(other.address().encode(), encoded);
        assert!(HybridWalletKeys::from_root_seed(&[1u8; 16]).is_ok());
        assert_eq!(
            HybridWalletKeys::from_root_seed(&[1u8; 20]).err(),
            Some(HybridError::RootSeed)
        );
    }

    // La dirección v2 reutiliza la clave de gasto de la v1: una sola wallet
    // gasta las dos clases de salidas y la frase de siempre recupera ambas.
    #[test]
    fn spend_key_is_the_same_as_the_zka1_address() {
        let seed = root(9);
        let keys = HybridWalletKeys::from_root_seed(&seed).unwrap();
        let v1 = super::super::payment_address_from_scan_key(&seed).unwrap();
        let spend_hex = v1.rsplit(':').next().unwrap();
        assert_eq!(
            spend_hex,
            hex::encode(keys.address().spend_public().compress().as_bytes())
        );
    }

    #[test]
    fn recipient_recognizes_its_output_and_derives_the_same_secrets() {
        let keys = HybridWalletKeys::from_root_seed(&root(7)).unwrap();
        let material = hybrid_output_material(&keys.address(), "commitment-a");
        assert_eq!(material.kem_ciphertext.len(), HYBRID_KEM_CIPHERTEXT_BYTES);
        assert!(material.stealth_address.starts_with("stlth2_"));

        let owned = keys
            .scan(
                &material.kem_ciphertext,
                "commitment-a",
                &material.one_time_public_key,
            )
            .expect("own output");
        assert_eq!(owned.amount_blind, material.amount_blind);
        let one_time = keys.one_time_secret(&owned) * RISTRETTO_BASEPOINT_POINT;
        assert_eq!(
            hex::encode(one_time.compress().as_bytes()),
            material.one_time_public_key
        );
    }

    #[test]
    fn other_wallets_and_altered_outputs_are_not_recognized() {
        let keys = HybridWalletKeys::from_root_seed(&root(7)).unwrap();
        let stranger = HybridWalletKeys::from_root_seed(&root(8)).unwrap();
        let material = hybrid_output_material(&keys.address(), "commitment-a");
        let ct = &material.kem_ciphertext;
        let otk = &material.one_time_public_key;

        assert!(stranger.scan(ct, "commitment-a", otk).is_none());
        // El ciphertext queda ligado al compromiso de la salida.
        assert!(keys.scan(ct, "commitment-b", otk).is_none());
        // Parte ML-KEM (rechazo implícito) y parte X25519 del ciphertext.
        for index in [0, ct.len() - 1] {
            let mut tampered = ct.clone();
            tampered[index] ^= 0x01;
            assert!(keys.scan(&tampered, "commitment-a", otk).is_none());
        }
        assert!(keys
            .scan(&ct[..ct.len() - 1], "commitment-a", otk)
            .is_none());
        let other = hybrid_output_material(&keys.address(), "commitment-a");
        assert!(keys
            .scan(ct, "commitment-a", &other.one_time_public_key)
            .is_none());
        assert!(keys.scan(ct, "commitment-a", "not-hex").is_none());
    }

    #[test]
    fn each_output_uses_a_fresh_encapsulation() {
        let keys = HybridWalletKeys::from_root_seed(&root(7)).unwrap();
        let first = hybrid_output_material(&keys.address(), "commitment-a");
        let second = hybrid_output_material(&keys.address(), "commitment-a");
        assert_ne!(first.kem_ciphertext, second.kem_ciphertext);
        assert_ne!(first.one_time_public_key, second.one_time_public_key);
        assert_ne!(first.amount_blind, second.amount_blind);
    }

    #[test]
    fn malformed_addresses_are_rejected_with_a_precise_error() {
        let valid = HybridWalletKeys::from_root_seed(&root(7))
            .unwrap()
            .address()
            .encode();

        assert_eq!(
            HybridAddress::decode(&format!("zka1{}", &valid[4..])).err(),
            Some(HybridError::Prefix)
        );
        assert_eq!(
            HybridAddress::decode(&format!("{valid}0")).err(),
            Some(HybridError::Base58)
        );
        assert!(matches!(
            HybridAddress::decode(&valid[..valid.len() - 10]),
            Err(HybridError::Length { .. }) | Err(HybridError::Checksum)
        ));
        assert_eq!(
            HybridAddress::decode(&"z".repeat(HYBRID_ADDRESS_MAX_CHARS + 1)).err(),
            Some(HybridError::TooLong)
        );

        let mut typo = valid.clone().into_bytes();
        let at = typo.len() / 2;
        typo[at] = if typo[at] == b'2' { b'3' } else { b'2' };
        assert_eq!(
            HybridAddress::decode(std::str::from_utf8(&typo).unwrap()).err(),
            Some(HybridError::Checksum)
        );

        let wrong_version = reencode_with(|bytes| bytes[0] = 3);
        assert_eq!(
            HybridAddress::decode(&wrong_version).err(),
            Some(HybridError::Version(3))
        );
        let identity_spend = reencode_with(|bytes| bytes[1..33].fill(0));
        assert_eq!(
            HybridAddress::decode(&identity_spend).err(),
            Some(HybridError::SpendKey)
        );
    }

    #[test]
    fn domain_labels_are_distinct() {
        let labels = [
            ADDRESS_CHECKSUM_DOMAIN,
            SCAN_SEED_DOMAIN,
            OUTPUT_CONTEXT_DOMAIN,
            OUTPUT_KEY_DOMAIN,
            AMOUNT_BLIND_DOMAIN,
            STEALTH_TAG_DOMAIN,
        ];
        let unique: std::collections::BTreeSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len());
    }

    // Una wallet creada desde una frase da la misma dirección por los dos
    // caminos; una de claves sueltas (al azar o rotadas) tiene la suya y
    // también reconoce y gasta sus salidas.
    #[test]
    fn wallet_keys_match_the_root_for_phrase_wallets() {
        let seed = root(9);
        let spending_key = derive_spending_key_from_recovery_seed(&seed);
        let from_root = HybridWalletKeys::from_root_seed(&seed).unwrap();
        let from_keys = HybridWalletKeys::from_wallet_keys(&spending_key, &seed).unwrap();
        assert_eq!(from_root.address().encode(), from_keys.address().encode());

        let loose = HybridWalletKeys::from_wallet_keys(&[3u8; 32], &[4u8; 32]).unwrap();
        assert_ne!(loose.address().encode(), from_root.address().encode());
        let material = hybrid_output_material(&loose.address(), "commitment-a");
        let owned = loose
            .scan(
                &material.kem_ciphertext,
                "commitment-a",
                &material.one_time_public_key,
            )
            .expect("own output");
        let one_time = loose.one_time_secret(&owned) * RISTRETTO_BASEPOINT_POINT;
        assert_eq!(
            hex::encode(one_time.compress().as_bytes()),
            material.one_time_public_key
        );
    }

    // La clave de escaneo sola (una wallet de solo lectura) reconoce la
    // salida y lee el ciego del monto, pero solo para la clave de gasto
    // pública que corresponde.
    #[test]
    fn scan_key_alone_recognizes_outputs_and_reads_the_amount() {
        let keys = HybridWalletKeys::from_root_seed(&root(7)).unwrap();
        let scan = HybridScanKey::from_scan_key(&root(7)).unwrap();
        let spend_public = keys.address().spend_public();
        let material = hybrid_output_material(&keys.address(), "commitment-a");
        let ct = &material.kem_ciphertext;
        let otk = &material.one_time_public_key;

        let owned = scan
            .scan(&spend_public, ct, "commitment-a", otk)
            .expect("own output");
        assert_eq!(owned.amount_blind, material.amount_blind);
        assert_eq!(
            scan.amount_blind(ct, "commitment-a"),
            Some(material.amount_blind)
        );

        let other_spend = HybridWalletKeys::from_root_seed(&root(8))
            .unwrap()
            .address()
            .spend_public();
        assert!(scan.scan(&other_spend, ct, "commitment-a", otk).is_none());
        assert!(scan.amount_blind(&ct[..10], "commitment-a").is_none());
        assert_eq!(
            HybridScanKey::from_scan_key(&[1u8; 20]).err(),
            Some(HybridError::RootSeed)
        );
        assert_eq!(format!("{scan:?}"), "HybridScanKey([redacted])");
    }

    #[test]
    fn debug_output_redacts_secrets() {
        let keys = HybridWalletKeys::from_root_seed(&root(7)).unwrap();
        let material = hybrid_output_material(&keys.address(), "commitment-a");
        let owned = keys
            .scan(
                &material.kem_ciphertext,
                "commitment-a",
                &material.one_time_public_key,
            )
            .unwrap();
        assert_eq!(format!("{keys:?}"), "HybridWalletKeys([redacted])");
        assert_eq!(format!("{owned:?}"), "HybridOwnedOutput([redacted])");
        assert!(format!("{material:?}").contains("[redacted]"));
    }
}
