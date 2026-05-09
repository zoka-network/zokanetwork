//! Ed25519 Digital Signatures
//!
//! Provides secure digital signatures using Ed25519 (EdDSA)
//! for authenticating transactions and messages in ZOKA.

use ed25519_dalek::{Signature as DalekSignature, Signer, SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Ed25519 Public Key (32 bytes)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PublicKey([u8; 32]);

/// Ed25519 Secret Key (wrapped SigningKey)
/// Custom serialization for security
#[derive(Clone)]
pub struct SecretKey(SigningKey);

/// Ed25519 Digital Signature (64 bytes)
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Signature([u8; 64]);

/// Ed25519 KeyPair (public + secret key)
#[derive(Clone)]
pub struct KeyPair {
    pub secret: SecretKey,
    pub public: PublicKey,
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretKey([REDACTED])")
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", hex::encode(&self.0[..8]))
    }
}

impl fmt::Display for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Signature({})", hex::encode(&self.0[..8]))
    }
}

// Custom serialization for SecretKey
impl Serialize for SecretKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let bytes = self.0.to_bytes();
        serializer.serialize_bytes(&bytes)
    }
}

impl<'de> Deserialize<'de> for SecretKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes: Vec<u8> = Deserialize::deserialize(deserializer)?;
        if bytes.len() != 32 {
            return Err(serde::de::Error::custom("Invalid secret key length"));
        }

        let mut seed = [0u8; 32];
        seed.copy_from_slice(&bytes);
        let signing_key = SigningKey::from_bytes(&seed);

        Ok(SecretKey(signing_key))
    }
}

impl Serialize for KeyPair {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("KeyPair", 2)?;

        let secret_bytes = self.secret.0.to_bytes();
        state.serialize_field("secret", &secret_bytes)?;
        state.serialize_field("public", &self.public)?;

        state.end()
    }
}

impl<'de> Deserialize<'de> for KeyPair {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;

        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "lowercase")]
        enum Field {
            Secret,
            Public,
        }

        struct KeyPairVisitor;

        impl<'de> Visitor<'de> for KeyPairVisitor {
            type Value = KeyPair;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct KeyPair")
            }

            fn visit_map<V>(self, mut map: V) -> Result<KeyPair, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut secret = None;
                let mut public = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Secret => {
                            let secret_bytes: Vec<u8> = map.next_value()?;
                            if secret_bytes.len() != 32 {
                                return Err(de::Error::custom("Invalid secret key length"));
                            }
                            let mut seed = [0u8; 32];
                            seed.copy_from_slice(&secret_bytes);
                            secret = Some(SigningKey::from_bytes(&seed));
                        }
                        Field::Public => {
                            public = Some(map.next_value()?);
                        }
                    }
                }

                let secret = secret.ok_or_else(|| de::Error::missing_field("secret"))?;
                let public = public.ok_or_else(|| de::Error::missing_field("public"))?;

                Ok(KeyPair {
                    secret: SecretKey(secret),
                    public,
                })
            }
        }

        deserializer.deserialize_struct("KeyPair", &["secret", "public"], KeyPairVisitor)
    }
}

// Custom serialization for Signature
impl Serialize for Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes: Vec<u8> = Deserialize::deserialize(deserializer)?;
        if bytes.len() != 64 {
            return Err(serde::de::Error::custom("Invalid signature length"));
        }

        let mut sig = [0u8; 64];
        sig.copy_from_slice(&bytes);

        Ok(Signature(sig))
    }
}

/// Generate a new Ed25519 key pair
pub fn generate_keypair() -> KeyPair {
    let mut bytes = [0u8; 32];
    use rand::RngCore;
    OsRng.fill_bytes(&mut bytes);
    let signing_key = SigningKey::from_bytes(&bytes);
    let verifying_key = signing_key.verifying_key();

    KeyPair {
        secret: SecretKey(signing_key),
        public: PublicKey(verifying_key.to_bytes()),
    }
}

/// Sign a message with an Ed25519 secret key
pub fn sign(secret_key: &SecretKey, message: &[u8]) -> Signature {
    let signature = secret_key.0.sign(message);
    Signature(signature.to_bytes())
}

/// Verify an Ed25519 signature
pub fn verify(public_key: &PublicKey, message: &[u8], signature: &Signature) -> Result<(), String> {
    let verifying_key = VerifyingKey::from_bytes(&public_key.0)
        .map_err(|e| format!("Invalid public key: {}", e))?;

    let dalek_sig = DalekSignature::from_bytes(&signature.0);

    verifying_key
        .verify_strict(message, &dalek_sig)
        .map_err(|e| format!("Signature verification failed: {}", e))
}

impl PublicKey {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        PublicKey(bytes)
    }
}

impl Signature {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn to_bytes(&self) -> [u8; 64] {
        self.0
    }

    pub fn from_bytes(bytes: [u8; 64]) -> Self {
        Signature(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keypair_generation() {
        let kp = generate_keypair();
        assert_eq!(kp.public.0.len(), 32);
    }

    #[test]
    fn test_sign_and_verify() {
        let kp = generate_keypair();
        let message = b"Hello, ZOKA!";

        let signature = sign(&kp.secret, message);
        assert!(verify(&kp.public, message, &signature).is_ok());
    }

    #[test]
    fn test_invalid_signature() {
        let kp = generate_keypair();
        let message = b"Hello, ZOKA!";
        let wrong_message = b"Wrong message";

        let signature = sign(&kp.secret, message);
        assert!(verify(&kp.public, wrong_message, &signature).is_err());
    }

    #[test]
    fn test_signature_serialization() {
        let kp = generate_keypair();
        let message = b"Test";
        let sig = sign(&kp.secret, message);

        let json = serde_json::to_string(&sig).unwrap();
        let deserialized: Signature = serde_json::from_str(&json).unwrap();

        assert_eq!(sig, deserialized);
    }
}
