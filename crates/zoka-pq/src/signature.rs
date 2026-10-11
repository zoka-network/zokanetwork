use crate::{check_length, Error, SecretSeed};
use ml_dsa::{Generate, KeyExport, KeyInit, Keypair, MlDsa65};
use std::fmt;
use zeroize::{Zeroize, Zeroizing};

pub const ML_DSA_65_SEED_BYTES: usize = 32;
pub const ML_DSA_65_PUBLIC_KEY_BYTES: usize = 1952;
pub const ML_DSA_65_SIGNATURE_BYTES: usize = 3309;

/// ML-DSA-65 key with zeroization enabled for its upstream expanded secret.
pub struct MlDsa65SecretKey(pub(crate) ml_dsa::SigningKey<MlDsa65>);

impl MlDsa65SecretKey {
    pub fn generate() -> Result<Self, Error> {
        ml_dsa::SigningKey::<MlDsa65>::try_generate()
            .map(Self)
            .map_err(|_| Error::RandomnessUnavailable)
    }

    /// Restore a uniformly random 32-byte FIPS 204 seed.
    pub fn from_seed(seed: &[u8]) -> Result<Self, Error> {
        check_length(seed, ML_DSA_65_SEED_BYTES)?;
        let mut encoded = ml_dsa::Seed::default();
        encoded.copy_from_slice(seed);
        let key = Self(ml_dsa::SigningKey::<MlDsa65>::from_seed(&encoded));
        encoded.zeroize();
        Ok(key)
    }

    pub fn export_seed(&self) -> SecretSeed<ML_DSA_65_SEED_BYTES> {
        let mut encoded = self.0.to_seed();
        let mut seed = Zeroizing::new([0u8; ML_DSA_65_SEED_BYTES]);
        seed.copy_from_slice(&encoded);
        encoded.zeroize();
        SecretSeed(seed)
    }

    pub fn public_key(&self) -> MlDsa65PublicKey {
        MlDsa65PublicKey(self.0.verifying_key())
    }

    /// FIPS 204 deterministic signing with an explicit context (0..=255 bytes).
    /// This is pure ML-DSA, not HashML-DSA or an implicit prehashed interface.
    pub fn sign(&self, message: &[u8], context: &[u8]) -> Result<MlDsa65Signature, Error> {
        if context.len() > 255 {
            return Err(Error::InvalidContext);
        }
        self.0
            .expanded_key()
            .sign_deterministic(message, context)
            .map(MlDsa65Signature)
            .map_err(|_| Error::InvalidSignature)
    }
}

impl fmt::Debug for MlDsa65SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MlDsa65SecretKey([REDACTED])")
    }
}

#[derive(Clone)]
pub struct MlDsa65PublicKey(pub(crate) ml_dsa::VerifyingKey<MlDsa65>);

impl MlDsa65PublicKey {
    /// All fixed-length ML-DSA public-key bit patterns have a defined decoding.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        check_length(bytes, ML_DSA_65_PUBLIC_KEY_BYTES)?;
        ml_dsa::VerifyingKey::<MlDsa65>::new_from_slice(bytes)
            .map(Self)
            .map_err(|_| Error::InvalidPublicKey)
    }

    pub fn to_bytes(&self) -> [u8; ML_DSA_65_PUBLIC_KEY_BYTES] {
        self.0.to_bytes().into()
    }

    pub fn verify(
        &self,
        message: &[u8],
        context: &[u8],
        signature: &MlDsa65Signature,
    ) -> Result<(), Error> {
        if context.len() > 255 {
            return Err(Error::InvalidContext);
        }
        if self.0.verify_with_context(message, context, &signature.0) {
            Ok(())
        } else {
            Err(Error::InvalidSignature)
        }
    }
}

#[derive(Clone)]
pub struct MlDsa65Signature(pub(crate) ml_dsa::Signature<MlDsa65>);

impl MlDsa65Signature {
    /// Validates length, encoded hints and coefficient bounds before verification.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        check_length(bytes, ML_DSA_65_SIGNATURE_BYTES)?;
        ml_dsa::Signature::<MlDsa65>::try_from(bytes)
            .map(Self)
            .map_err(|_| Error::InvalidSignature)
    }

    pub fn to_bytes(&self) -> [u8; ML_DSA_65_SIGNATURE_BYTES] {
        self.0.encode().into()
    }
}
