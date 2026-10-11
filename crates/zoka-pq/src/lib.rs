//! Experimental isolated primitives for Zoka Quantic research.
//!
//! This crate does not define addresses, transactions, consensus, wallet storage,
//! or a production protocol. The underlying implementations are not independently
//! audited. See the README for the exact algorithms and validation limits.
#![forbid(unsafe_code)]

mod kem;
mod signature;

pub use kem::{
    MlKem768Ciphertext, MlKem768PublicKey, MlKem768SecretKey, XWingCiphertext, XWingPublicKey,
    XWingSecretKey, ML_KEM_768_CIPHERTEXT_BYTES, ML_KEM_768_PUBLIC_KEY_BYTES,
    ML_KEM_768_SEED_BYTES, X_WING_CIPHERTEXT_BYTES, X_WING_PROFILE, X_WING_PUBLIC_KEY_BYTES,
    X_WING_SEED_BYTES,
};
pub use signature::{
    MlDsa65PublicKey, MlDsa65SecretKey, MlDsa65Signature, ML_DSA_65_PUBLIC_KEY_BYTES,
    ML_DSA_65_SEED_BYTES, ML_DSA_65_SIGNATURE_BYTES,
};

use std::fmt;
use zeroize::{Zeroize, Zeroizing};

/// Errors never include key, message, signature, or ciphertext contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidLength { expected: usize, actual: usize },
    InvalidPublicKey,
    InvalidSignature,
    InvalidContext,
    NonContributoryCiphertext,
    RandomnessUnavailable,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength { expected, actual } => {
                write!(
                    f,
                    "invalid length: expected {expected} bytes, received {actual}"
                )
            }
            Self::InvalidPublicKey => f.write_str("invalid public key encoding"),
            Self::InvalidSignature => f.write_str("invalid signature"),
            Self::InvalidContext => f.write_str("ML-DSA context exceeds 255 bytes"),
            Self::NonContributoryCiphertext => f.write_str("non-contributory X25519 ciphertext"),
            Self::RandomnessUnavailable => f.write_str("operating system randomness unavailable"),
        }
    }
}

impl std::error::Error for Error {}

fn check_length(bytes: &[u8], expected: usize) -> Result<(), Error> {
    if bytes.len() == expected {
        Ok(())
    } else {
        Err(Error::InvalidLength {
            expected,
            actual: bytes.len(),
        })
    }
}

/// A 32-byte KEM result, erased on drop and redacted in Debug.
///
/// This is not an authenticated message or an application encryption key. A
/// protocol still needs an appropriate context-bound KDF and authenticated cipher.
pub struct SharedSecret(Zeroizing<[u8; 32]>);

impl SharedSecret {
    fn take_from(bytes: &mut [u8]) -> Self {
        let mut secret = Zeroizing::new([0u8; 32]);
        secret.copy_from_slice(bytes);
        bytes.zeroize();
        Self(secret)
    }

    /// Explicit access for a downstream KDF. Callers must protect any copies.
    pub fn expose_secret(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for SharedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SharedSecret([REDACTED])")
    }
}

/// Sensitive recovery bytes. Debug is redacted; memory is erased on drop.
/// Unlike returning `Zeroizing` directly, formatting this wrapper cannot disclose
/// its contents accidentally. Explicitly exposed copies remain the caller's duty.
pub struct SecretSeed<const N: usize>(pub(crate) Zeroizing<[u8; N]>);

impl<const N: usize> SecretSeed<N> {
    pub fn expose_secret(&self) -> &[u8; N] {
        &self.0
    }
}

impl<const N: usize> fmt::Debug for SecretSeed<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretSeed([REDACTED])")
    }
}

#[cfg(test)]
mod tests;
