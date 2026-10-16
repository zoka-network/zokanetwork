//! Firma híbrida Ed25519 + ML-DSA-65 (etapa 6 del plan post-cuántico).
//!
//! Las dos firmas son obligatorias: falsificar una exige romper Ed25519 **y**
//! ML-DSA-65, así que una computadora cuántica sola no alcanza. Las dos firman
//! el mismo mensaje, `dominio ‖ 0x1f ‖ mensaje`, y ML-DSA usa además el dominio
//! como contexto (FIPS 204). La clave pública es una sola pieza: un verificador
//! nunca acepta una de las dos firmas por separado, así que no se puede
//! "pelar" la firma híbrida y presentar una mitad.
//!
//! La usan los checkpoints de la autoridad (versión 2) y los snapshots.

use std::fmt;
use std::path::Path;

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use zeroize::{Zeroize, Zeroizing};
use zoka_pq::{
    MlDsa65PublicKey, MlDsa65SecretKey, MlDsa65Signature, ML_DSA_65_PUBLIC_KEY_BYTES,
    ML_DSA_65_SEED_BYTES, ML_DSA_65_SIGNATURE_BYTES,
};

/// Versión de la codificación de la clave pública.
pub const HYBRID_KEY_VERSION: u8 = 1;
pub const HYBRID_PUBLIC_KEY_BYTES: usize = 1 + 32 + ML_DSA_65_PUBLIC_KEY_BYTES;
pub const HYBRID_SIGNATURE_BYTES: usize = 64 + ML_DSA_65_SIGNATURE_BYTES;
/// ML-DSA admite contextos de hasta 255 bytes.
pub const MAX_DOMAIN_BYTES: usize = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HybridSignatureError {
    Encoding(&'static str),
    Domain,
    Ed25519,
    MlDsa,
}

impl fmt::Display for HybridSignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encoding(what) => write!(f, "invalid hybrid {what} encoding"),
            Self::Domain => write!(f, "signing domain must have 1 to {MAX_DOMAIN_BYTES} bytes"),
            Self::Ed25519 => f.write_str("Ed25519 half of the hybrid signature is not valid"),
            Self::MlDsa => f.write_str("ML-DSA-65 half of the hybrid signature is not valid"),
        }
    }
}

impl std::error::Error for HybridSignatureError {}

fn signed_message(domain: &[u8], message: &[u8]) -> Result<Vec<u8>, HybridSignatureError> {
    if domain.is_empty() || domain.len() > MAX_DOMAIN_BYTES {
        return Err(HybridSignatureError::Domain);
    }
    let mut out = Vec::with_capacity(domain.len() + 1 + message.len());
    out.extend_from_slice(domain);
    out.push(0x1f);
    out.extend_from_slice(message);
    Ok(out)
}

/// Clave privada híbrida. `Debug` no muestra nada.
pub struct HybridSigningKey {
    ed25519: SigningKey,
    mldsa: MlDsa65SecretKey,
}

impl HybridSigningKey {
    /// La clave Ed25519 puede ser una que ya existía (la de la autoridad de
    /// Zenith); la ML-DSA sale de una semilla de 32 bytes al azar (FIPS 204).
    pub fn from_parts(
        ed25519: SigningKey,
        mldsa_seed: &[u8],
    ) -> Result<Self, HybridSignatureError> {
        let mldsa = MlDsa65SecretKey::from_seed(mldsa_seed)
            .map_err(|_| HybridSignatureError::Encoding("ML-DSA seed"))?;
        Ok(Self { ed25519, mldsa })
    }

    pub fn verifying_key(&self) -> HybridVerifyingKey {
        HybridVerifyingKey {
            ed25519: self.ed25519.verifying_key(),
            mldsa: self.mldsa.public_key(),
        }
    }

    pub fn ed25519(&self) -> &SigningKey {
        &self.ed25519
    }

    pub fn sign(
        &self,
        domain: &[u8],
        message: &[u8],
    ) -> Result<HybridSignature, HybridSignatureError> {
        let signed = signed_message(domain, message)?;
        let ed25519 = self.ed25519.sign(&signed);
        let mldsa = self
            .mldsa
            .sign(&signed, domain)
            .map_err(|_| HybridSignatureError::MlDsa)?;
        Ok(HybridSignature { ed25519, mldsa })
    }
}

impl fmt::Debug for HybridSigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HybridSigningKey([REDACTED])")
    }
}

#[derive(Clone)]
pub struct HybridVerifyingKey {
    ed25519: VerifyingKey,
    mldsa: MlDsa65PublicKey,
}

impl HybridVerifyingKey {
    pub fn from_parts(ed25519: VerifyingKey, mldsa: MlDsa65PublicKey) -> Self {
        Self { ed25519, mldsa }
    }

    pub fn ed25519(&self) -> &VerifyingKey {
        &self.ed25519
    }

    pub fn mldsa(&self) -> &MlDsa65PublicKey {
        &self.mldsa
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HYBRID_PUBLIC_KEY_BYTES);
        out.push(HYBRID_KEY_VERSION);
        out.extend_from_slice(self.ed25519.as_bytes());
        out.extend_from_slice(&self.mldsa.to_bytes());
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, HybridSignatureError> {
        if bytes.len() != HYBRID_PUBLIC_KEY_BYTES {
            return Err(HybridSignatureError::Encoding("public key length"));
        }
        if bytes[0] != HYBRID_KEY_VERSION {
            return Err(HybridSignatureError::Encoding("public key version"));
        }
        let ed: [u8; 32] = bytes[1..33].try_into().expect("exact slice");
        let ed25519 = VerifyingKey::from_bytes(&ed)
            .map_err(|_| HybridSignatureError::Encoding("Ed25519 public key"))?;
        let mldsa = MlDsa65PublicKey::from_bytes(&bytes[33..])
            .map_err(|_| HybridSignatureError::Encoding("ML-DSA public key"))?;
        Ok(Self { ed25519, mldsa })
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.to_bytes())
    }

    pub fn from_hex(text: &str) -> Result<Self, HybridSignatureError> {
        let bytes = hex::decode(text.trim()).map_err(|_| HybridSignatureError::Encoding("hex"))?;
        Self::from_bytes(&bytes)
    }

    /// Las dos mitades tienen que verificar. Ed25519 en modo estricto
    /// (rechaza claves y firmas de orden chico y escalares no canónicos).
    pub fn verify(
        &self,
        domain: &[u8],
        message: &[u8],
        signature: &HybridSignature,
    ) -> Result<(), HybridSignatureError> {
        let signed = signed_message(domain, message)?;
        self.ed25519
            .verify_strict(&signed, &signature.ed25519)
            .map_err(|_| HybridSignatureError::Ed25519)?;
        self.mldsa
            .verify(&signed, domain, &signature.mldsa)
            .map_err(|_| HybridSignatureError::MlDsa)
    }
}

impl PartialEq for HybridVerifyingKey {
    fn eq(&self, other: &Self) -> bool {
        self.to_bytes() == other.to_bytes()
    }
}

impl Eq for HybridVerifyingKey {}

impl fmt::Debug for HybridVerifyingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "HybridVerifyingKey(ed25519 {})",
            hex::encode(self.ed25519.as_bytes())
        )
    }
}

#[derive(Clone)]
pub struct HybridSignature {
    ed25519: ed25519_dalek::Signature,
    mldsa: MlDsa65Signature,
}

impl HybridSignature {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HYBRID_SIGNATURE_BYTES);
        out.extend_from_slice(&self.ed25519.to_bytes());
        out.extend_from_slice(&self.mldsa.to_bytes());
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, HybridSignatureError> {
        if bytes.len() != HYBRID_SIGNATURE_BYTES {
            return Err(HybridSignatureError::Encoding("signature length"));
        }
        let ed: [u8; 64] = bytes[..64].try_into().expect("exact slice");
        let mldsa = MlDsa65Signature::from_bytes(&bytes[64..])
            .map_err(|_| HybridSignatureError::Encoding("ML-DSA signature"))?;
        Ok(Self {
            ed25519: ed25519_dalek::Signature::from_bytes(&ed),
            mldsa,
        })
    }

    /// Las dos mitades por separado, en hex: así viajan en los JSON que ya
    /// tenían un campo de firma Ed25519.
    pub fn to_hex_parts(&self) -> (String, String) {
        (
            hex::encode(self.ed25519.to_bytes()),
            hex::encode(self.mldsa.to_bytes()),
        )
    }

    pub fn from_hex_parts(ed25519: &str, mldsa: &str) -> Result<Self, HybridSignatureError> {
        let mut bytes = hex::decode(ed25519).map_err(|_| HybridSignatureError::Encoding("hex"))?;
        if bytes.len() != 64 {
            return Err(HybridSignatureError::Encoding("Ed25519 signature length"));
        }
        bytes.extend(hex::decode(mldsa).map_err(|_| HybridSignatureError::Encoding("hex"))?);
        Self::from_bytes(&bytes)
    }
}

impl fmt::Debug for HybridSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HybridSignature(..)")
    }
}

/// Lee una semilla ML-DSA de 32 bytes en hex.
pub fn load_mldsa_seed(path: &Path) -> Result<Zeroizing<[u8; ML_DSA_65_SEED_BYTES]>, String> {
    let mut text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read ML-DSA key {}: {e}", path.display()))?;
    let decoded = hex::decode(text.trim());
    text.zeroize();
    let mut bytes = decoded.map_err(|_| "ML-DSA key is not hex".to_string())?;
    let result = <[u8; ML_DSA_65_SEED_BYTES]>::try_from(bytes.as_slice())
        .map(Zeroizing::new)
        .map_err(|_| format!("ML-DSA key must be {ML_DSA_65_SEED_BYTES} bytes"));
    bytes.zeroize();
    result
}

/// Crea una semilla ML-DSA nueva en `path` (nunca sobrescribe) y devuelve la
/// clave pública ML-DSA en hex.
pub fn generate_mldsa_seed(path: &Path) -> Result<String, String> {
    if path.exists() {
        return Err(format!(
            "{} already exists; refusing to overwrite an ML-DSA key",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let key = MlDsa65SecretKey::generate().map_err(|e| e.to_string())?;
    let mut secret_hex = hex::encode(key.export_seed().expose_secret());
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, secret_hex.as_bytes()));
    secret_hex.zeroize();
    written.map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(hex::encode(key.public_key().to_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOMAIN: &[u8] = b"zoka-test-hybrid";

    fn key(byte: u8) -> HybridSigningKey {
        HybridSigningKey::from_parts(SigningKey::from_bytes(&[byte; 32]), &[byte ^ 0x5a; 32])
            .unwrap()
    }

    #[test]
    fn a_signature_verifies_and_round_trips() {
        let k = key(1);
        let sig = k.sign(DOMAIN, b"altura 30").unwrap();
        let public = k.verifying_key();
        public.verify(DOMAIN, b"altura 30", &sig).unwrap();
        let bytes = sig.to_bytes();
        assert_eq!(bytes.len(), HYBRID_SIGNATURE_BYTES);
        let again = HybridSignature::from_bytes(&bytes).unwrap();
        public.verify(DOMAIN, b"altura 30", &again).unwrap();
        let (ed, ml) = sig.to_hex_parts();
        public
            .verify(
                DOMAIN,
                b"altura 30",
                &HybridSignature::from_hex_parts(&ed, &ml).unwrap(),
            )
            .unwrap();
        let restored = HybridVerifyingKey::from_hex(&public.to_hex()).unwrap();
        assert_eq!(restored, public);
        restored.verify(DOMAIN, b"altura 30", &sig).unwrap();
    }

    #[test]
    fn another_message_or_domain_fails() {
        let k = key(2);
        let sig = k.sign(DOMAIN, b"a").unwrap();
        assert!(k.verifying_key().verify(DOMAIN, b"b", &sig).is_err());
        assert!(k
            .verifying_key()
            .verify(b"zoka-other-domain", b"a", &sig)
            .is_err());
    }

    #[test]
    fn each_half_is_required() {
        let k = key(3);
        let other = key(4);
        let message = b"bloque";
        let good = k.sign(DOMAIN, message).unwrap().to_bytes();
        let foreign = other.sign(DOMAIN, message).unwrap().to_bytes();
        // Ed25519 de otra clave con ML-DSA correcta, y al revés.
        let mut ed_swapped = foreign[..64].to_vec();
        ed_swapped.extend_from_slice(&good[64..]);
        let sig = HybridSignature::from_bytes(&ed_swapped).unwrap();
        assert_eq!(
            k.verifying_key().verify(DOMAIN, message, &sig).unwrap_err(),
            HybridSignatureError::Ed25519
        );
        let mut ml_swapped = good[..64].to_vec();
        ml_swapped.extend_from_slice(&foreign[64..]);
        let sig = HybridSignature::from_bytes(&ml_swapped).unwrap();
        assert_eq!(
            k.verifying_key().verify(DOMAIN, message, &sig).unwrap_err(),
            HybridSignatureError::MlDsa
        );
        // Una clave pública que mezcla las dos mitades tampoco sirve.
        let mixed = HybridVerifyingKey::from_parts(
            *k.verifying_key().ed25519(),
            other.verifying_key().mldsa().clone(),
        );
        assert!(mixed
            .verify(
                DOMAIN,
                message,
                &HybridSignature::from_bytes(&good).unwrap()
            )
            .is_err());
    }

    #[test]
    fn encodings_are_strict() {
        let public = key(5).verifying_key().to_bytes();
        assert!(HybridVerifyingKey::from_bytes(&public[..public.len() - 1]).is_err());
        let mut wrong_version = public.clone();
        wrong_version[0] = 2;
        assert!(HybridVerifyingKey::from_bytes(&wrong_version).is_err());
        let sig = key(5).sign(DOMAIN, b"x").unwrap().to_bytes();
        assert!(HybridSignature::from_bytes(&sig[..sig.len() - 1]).is_err());
        assert!(HybridSignature::from_hex_parts("00", &hex::encode(&sig[64..])).is_err());
        assert_eq!(
            key(5).sign(b"", b"x").unwrap_err(),
            HybridSignatureError::Domain
        );
        assert_eq!(
            key(5).sign(&[1u8; 256], b"x").unwrap_err(),
            HybridSignatureError::Domain
        );
    }

    #[test]
    fn keys_files_are_created_once_and_read_back() {
        let dir = std::env::temp_dir().join(format!("zoka-hybrid-key-{}", std::process::id()));
        let path = dir.join("mldsa.key");
        let _ = std::fs::remove_file(&path);
        let public = generate_mldsa_seed(&path).unwrap();
        assert_eq!(public.len(), ML_DSA_65_PUBLIC_KEY_BYTES * 2);
        assert!(generate_mldsa_seed(&path).is_err(), "never overwrites");
        let seed = load_mldsa_seed(&path).unwrap();
        let k =
            HybridSigningKey::from_parts(SigningKey::from_bytes(&[6; 32]), seed.as_ref()).unwrap();
        assert_eq!(hex::encode(k.verifying_key().mldsa().to_bytes()), public);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn debug_output_hides_the_secret() {
        assert_eq!(format!("{:?}", key(7)), "HybridSigningKey([REDACTED])");
    }
}
