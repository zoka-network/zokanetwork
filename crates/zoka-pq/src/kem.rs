use crate::{check_length, Error, SecretSeed, SharedSecret};
use ml_kem::{
    kem::Decapsulator, Decapsulate, Encapsulate, Generate, KeyExport, KeyInit, TryKeyInit,
};
use std::fmt;
use x_wing::TryDecapsulate;
use zeroize::{Zeroize, Zeroizing};

pub const ML_KEM_768_SEED_BYTES: usize = 64;
pub const ML_KEM_768_PUBLIC_KEY_BYTES: usize = 1184;
pub const ML_KEM_768_CIPHERTEXT_BYTES: usize = 1088;
pub const X_WING_SEED_BYTES: usize = 32;
pub const X_WING_PUBLIC_KEY_BYTES: usize = 1216;
pub const X_WING_CIPHERTEXT_BYTES: usize = 1120;

/// Experimental profile: the published X-Wing combiner, with explicit rejection
/// of non-contributory X25519 ciphertexts through RustCrypto's supported variant.
pub const X_WING_PROFILE: &str = "x-wing-draft06-rustcrypto-0.1.1-reject-noncontributory";

/// An expanded ML-KEM-768 private key. Upstream's zeroize feature clears secrets.
pub struct MlKem768SecretKey(pub(crate) ml_kem::DecapsulationKey768);

impl MlKem768SecretKey {
    pub fn generate() -> Result<Self, Error> {
        ml_kem::DecapsulationKey768::try_generate()
            .map(Self)
            .map_err(|_| Error::RandomnessUnavailable)
    }

    /// Restore the 64-byte FIPS 203 seed `d || z`, both uniformly random at creation.
    /// Never derive it by padding a password or a short identifier.
    pub fn from_seed(seed: &[u8]) -> Result<Self, Error> {
        check_length(seed, ML_KEM_768_SEED_BYTES)?;
        let mut encoded = ml_kem::Seed::default();
        encoded.copy_from_slice(seed);
        let key = Self(ml_kem::DecapsulationKey768::new(&encoded));
        encoded.zeroize();
        Ok(key)
    }

    /// Sensitive recovery material; its returned allocation is erased on drop.
    pub fn export_seed(&self) -> SecretSeed<ML_KEM_768_SEED_BYTES> {
        let mut encoded = self.0.to_bytes();
        let mut seed = Zeroizing::new([0u8; ML_KEM_768_SEED_BYTES]);
        seed.copy_from_slice(&encoded);
        encoded.zeroize();
        SecretSeed(seed)
    }

    pub fn public_key(&self) -> MlKem768PublicKey {
        MlKem768PublicKey(self.0.encapsulation_key().clone())
    }

    /// ML-KEM uses implicit rejection. A tampered ciphertext or wrong key returns
    /// a different pseudorandom secret, not a validity error. Authenticate payloads.
    pub fn decapsulate(&self, ciphertext: &MlKem768Ciphertext) -> SharedSecret {
        let mut shared = self.0.decapsulate(&ciphertext.0);
        SharedSecret::take_from(shared.as_mut())
    }
}

impl fmt::Debug for MlKem768SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MlKem768SecretKey([REDACTED])")
    }
}

#[derive(Clone)]
pub struct MlKem768PublicKey(pub(crate) ml_kem::EncapsulationKey768);

impl MlKem768PublicKey {
    /// Performs FIPS 203 encapsulation-key decoding and modulus validation.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        check_length(bytes, ML_KEM_768_PUBLIC_KEY_BYTES)?;
        ml_kem::EncapsulationKey768::new_from_slice(bytes)
            .map(Self)
            .map_err(|_| Error::InvalidPublicKey)
    }

    pub fn to_bytes(&self) -> [u8; ML_KEM_768_PUBLIC_KEY_BYTES] {
        self.0.to_bytes().into()
    }

    /// Uses OS randomness. Like RustCrypto's Encapsulate API, panics if the OS RNG fails.
    pub fn encapsulate(&self) -> (MlKem768Ciphertext, SharedSecret) {
        let (ciphertext, mut shared) = self.0.encapsulate();
        (
            MlKem768Ciphertext(ciphertext),
            SharedSecret::take_from(shared.as_mut()),
        )
    }
}

#[derive(Clone)]
pub struct MlKem768Ciphertext(pub(crate) ml_kem::Ciphertext<ml_kem::MlKem768>);

impl MlKem768Ciphertext {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        check_length(bytes, ML_KEM_768_CIPHERTEXT_BYTES)?;
        let mut encoded = ml_kem::Ciphertext::<ml_kem::MlKem768>::default();
        encoded.copy_from_slice(bytes);
        Ok(Self(encoded))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// X-Wing seed-backed key. RustCrypto re-expands it on every decapsulation;
/// callers must account for that cost when benchmarking scanning.
pub struct XWingSecretKey(pub(crate) x_wing::DecapsulationKeyRejectNonContrib);

impl XWingSecretKey {
    pub fn generate() -> Result<Self, Error> {
        x_wing::DecapsulationKeyRejectNonContrib::try_generate()
            .map(Self)
            .map_err(|_| Error::RandomnessUnavailable)
    }

    /// Restore a uniformly random 32-byte X-Wing seed.
    pub fn from_seed(seed: &[u8]) -> Result<Self, Error> {
        check_length(seed, X_WING_SEED_BYTES)?;
        let mut encoded = x_wing::Key::<x_wing::DecapsulationKeyRejectNonContrib>::default();
        encoded.copy_from_slice(seed);
        let key = Self(x_wing::DecapsulationKeyRejectNonContrib::new(&encoded));
        encoded.zeroize();
        Ok(key)
    }

    pub fn export_seed(&self) -> SecretSeed<X_WING_SEED_BYTES> {
        let mut encoded = self.0.to_bytes();
        let mut seed = Zeroizing::new([0u8; X_WING_SEED_BYTES]);
        seed.copy_from_slice(&encoded);
        encoded.zeroize();
        SecretSeed(seed)
    }

    pub fn public_key(&self) -> XWingPublicKey {
        XWingPublicKey(self.0.encapsulation_key().clone())
    }

    /// Rejects non-contributory X25519 results. The ML-KEM portion retains its
    /// implicit-rejection semantics and is not a ciphertext authentication check.
    pub fn decapsulate(&self, ciphertext: &XWingCiphertext) -> Result<SharedSecret, Error> {
        let mut shared = self
            .0
            .try_decapsulate(&ciphertext.0)
            .map_err(|_| Error::NonContributoryCiphertext)?;
        Ok(SharedSecret::take_from(shared.as_mut()))
    }
}

impl fmt::Debug for XWingSecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("XWingSecretKey([REDACTED])")
    }
}

#[derive(Clone)]
pub struct XWingPublicKey(pub(crate) x_wing::EncapsulationKey);

impl XWingPublicKey {
    /// Checks the complete length and canonical ML-KEM key encoding. The X25519
    /// component follows RFC 7748 decoding; parsing is not proof of possession.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        check_length(bytes, X_WING_PUBLIC_KEY_BYTES)?;
        x_wing::EncapsulationKey::new_from_slice(bytes)
            .map(Self)
            .map_err(|_| Error::InvalidPublicKey)
    }

    pub fn to_bytes(&self) -> [u8; X_WING_PUBLIC_KEY_BYTES] {
        self.0.to_bytes().into()
    }

    /// Uses OS randomness; panics if the OS RNG fails, as in RustCrypto's API.
    pub fn encapsulate(&self) -> (XWingCiphertext, SharedSecret) {
        let (ciphertext, mut shared) = self.0.encapsulate();
        (
            XWingCiphertext(ciphertext),
            SharedSecret::take_from(shared.as_mut()),
        )
    }
}

#[derive(Clone)]
pub struct XWingCiphertext(pub(crate) x_wing::Ciphertext);

impl XWingCiphertext {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        check_length(bytes, X_WING_CIPHERTEXT_BYTES)?;
        let mut encoded = x_wing::Ciphertext::default();
        encoded.copy_from_slice(bytes);
        Ok(Self(encoded))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}
