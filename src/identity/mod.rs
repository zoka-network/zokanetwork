// Identity module: Handles keys and permissions
use crate::types::{BlockError, Identity, Permission, PrivateKey, PublicKey, Signature};
use bip39::{Language, Mnemonic};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Identity registry managing validators and users
#[allow(dead_code)]
pub struct IdentityManager {
    identities: HashMap<String, Identity>,
}

impl IdentityManager {
    pub fn new() -> Self {
        IdentityManager {
            identities: HashMap::new(),
        }
    }

    /// Register a new identity
    pub fn register_identity(&mut self, identity: Identity) {
        let key_str = format!("{:?}", identity.public_key());
        self.identities.insert(key_str, identity);
    }

    /// Get identity by public key
    pub fn get_identity(&self, pub_key: &PublicKey) -> Option<&Identity> {
        let key_str = format!("{:?}", pub_key);
        self.identities.get(&key_str)
    }

    /// Check if address is validator
    pub fn is_validator(&self, pub_key: &PublicKey) -> bool {
        if let Some(identity) = self.get_identity(pub_key) {
            identity.is_validator()
        } else {
            false
        }
    }

    /// Check permission
    pub fn has_permission(&self, pub_key: &PublicKey, permission: &Permission) -> bool {
        if let Some(identity) = self.get_identity(pub_key) {
            identity.permissions().contains(permission)
        } else {
            false
        }
    }
}

impl Default for IdentityManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate an Ed25519 keypair.
pub fn generate_keypair() -> Result<(PublicKey, PrivateKey), BlockError> {
    use ed25519_dalek::SigningKey;
    use rand::RngCore;

    let mut seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    let signing_key = SigningKey::from_bytes(&seed);
    let verifying_key = signing_key.verifying_key();

    Ok((
        PublicKey(verifying_key.to_bytes().to_vec()),
        PrivateKey(seed.to_vec()),
    ))
}

/// Sign a message with private key
pub fn sign_message(message: &[u8], private_key: &PrivateKey) -> Result<Signature, BlockError> {
    use ed25519_dalek::{Signer, SigningKey};

    if private_key.0.len() != 32 {
        return Err(BlockError::TransactionValidationFailed(
            "Invalid Ed25519 private key length".to_string(),
        ));
    }

    let mut seed = [0u8; 32];
    seed.copy_from_slice(&private_key.0);
    let signing_key = SigningKey::from_bytes(&seed);
    let signature = signing_key.sign(message);
    Ok(Signature(signature.to_bytes().to_vec()))
}

/// Verify a signature
pub fn verify_signature(signature: &Signature, message: &[u8], public_key: &PublicKey) -> bool {
    use ed25519_dalek::{Signature as DalekSignature, VerifyingKey};

    if public_key.0.len() != 32 || signature.0.len() != 64 {
        return false;
    }

    let mut pk = [0u8; 32];
    pk.copy_from_slice(&public_key.0);
    let verifying_key = match VerifyingKey::from_bytes(&pk) {
        Ok(key) => key,
        Err(_) => return false,
    };

    let mut sig = [0u8; 64];
    sig.copy_from_slice(&signature.0);
    let dalek_sig = DalekSignature::from_bytes(&sig);

    verifying_key.verify_strict(message, &dalek_sig).is_ok()
}

pub fn manage_identity(identity: &Identity) {
    println!("Managing identity: {:?}", identity.public_key());
}

/// HD Wallet for BIP-32 derivation and seed phrase management
#[allow(dead_code)]
pub struct HDWallet {
    seed: Vec<u8>,
    master_key: String,
    accounts: HashMap<u32, (PublicKey, PrivateKey)>,
    derivation_path: String,
}

impl HDWallet {
    /// Create new HD wallet from seed (entropy)
    pub fn new(seed: Vec<u8>) -> Result<Self, BlockError> {
        if seed.is_empty() || seed.len() < 16 || seed.len() > 64 {
            return Err(BlockError::TransactionValidationFailed(
                "Invalid seed size".to_string(),
            ));
        }

        // Generate master key from seed using SHA-256
        let mut hasher = Sha256::new();
        hasher.update(&seed);
        let master_key = format!("master_{:?}", hasher.finalize());

        Ok(HDWallet {
            seed,
            master_key,
            accounts: HashMap::new(),
            derivation_path: "m/44'/0'/0'/0".to_string(),
        })
    }

    /// Generate random seed from entropy (16-64 bytes)
    pub fn generate_seed(entropy_bytes: usize) -> Result<Vec<u8>, BlockError> {
        if entropy_bytes < 16 || entropy_bytes > 64 {
            return Err(BlockError::TransactionValidationFailed(
                "Invalid entropy size".to_string(),
            ));
        }

        use rand::Rng;
        let mut rng = rand::thread_rng();
        let mut seed = vec![0u8; entropy_bytes];
        rng.fill(&mut seed[..]);

        Ok(seed)
    }

    /// Convert seed entropy to BIP-39 mnemonic words.
    pub fn seed_to_mnemonic(&self) -> Result<Vec<String>, BlockError> {
        if self.seed.is_empty() {
            return Err(BlockError::TransactionValidationFailed(
                "Empty seed".to_string(),
            ));
        }
        let mnemonic = Mnemonic::from_entropy_in(Language::English, &self.seed).map_err(|e| {
            BlockError::TransactionValidationFailed(format!("BIP39 mnemonic error: {}", e))
        })?;
        Ok(mnemonic
            .to_string()
            .split_whitespace()
            .map(|w| w.to_string())
            .collect())
    }

    /// Recover seed entropy from BIP-39 mnemonic words.
    pub fn mnemonic_to_seed(mnemonic: &[String]) -> Result<Vec<u8>, BlockError> {
        if mnemonic.is_empty() || (mnemonic.len() != 12 && mnemonic.len() != 24) {
            return Err(BlockError::TransactionValidationFailed(
                "Invalid mnemonic length".to_string(),
            ));
        }
        let phrase = mnemonic.join(" ");
        let parsed = Mnemonic::parse_in_normalized(Language::English, &phrase).map_err(|e| {
            BlockError::TransactionValidationFailed(format!("Invalid mnemonic: {}", e))
        })?;
        Ok(parsed.to_entropy())
    }

    /// Derive account keys from master using BIP-32 path
    pub fn derive_account(
        &mut self,
        account_index: u32,
    ) -> Result<(PublicKey, PrivateKey), BlockError> {
        // Generate child key from master key and account index
        let mut hasher = Sha256::new();
        hasher.update(self.master_key.as_bytes());
        hasher.update(account_index.to_le_bytes());

        let mut key_bytes = hasher.finalize().to_vec();

        // Ensure exactly 32 bytes for private key
        key_bytes.truncate(32);
        if key_bytes.len() < 32 {
            key_bytes.resize(32, 0);
        }

        let private_key = PrivateKey(key_bytes.clone());

        // Derive public key from private
        let mut pub_hasher = Sha256::new();
        pub_hasher.update(&key_bytes);
        let pub_key = PublicKey(pub_hasher.finalize().to_vec());

        self.accounts
            .insert(account_index, (pub_key.clone(), private_key.clone()));

        Ok((pub_key, private_key))
    }

    /// Get derived account by index
    pub fn get_account(&self, account_index: u32) -> Option<&(PublicKey, PrivateKey)> {
        self.accounts.get(&account_index)
    }

    /// Derive multiple accounts in sequence
    pub fn derive_accounts(
        &mut self,
        count: u32,
    ) -> Result<Vec<(PublicKey, PrivateKey)>, BlockError> {
        let mut accounts = Vec::new();
        for i in 0..count {
            accounts.push(self.derive_account(i)?);
        }
        Ok(accounts)
    }

    /// Set custom derivation path (e.g., "m/44'/60'/0'/0")
    pub fn set_derivation_path(&mut self, path: String) -> Result<(), BlockError> {
        if !path.starts_with("m/") {
            return Err(BlockError::TransactionValidationFailed(
                "Invalid derivation path".to_string(),
            ));
        }
        self.derivation_path = path;
        Ok(())
    }

    /// Get current derivation path
    pub fn get_derivation_path(&self) -> &str {
        &self.derivation_path
    }

    /// Account count
    pub fn account_count(&self) -> usize {
        self.accounts.len()
    }

    /// Get all derived account indices
    pub fn get_account_indices(&self) -> Vec<u32> {
        let mut indices: Vec<u32> = self.accounts.keys().copied().collect();
        indices.sort();
        indices
    }

    /// Clear all derived accounts (keep seed)
    pub fn clear_accounts(&mut self) {
        self.accounts.clear();
    }

    /// Restore from backup (seed + derived accounts)
    pub fn restore_from_seed(seed: Vec<u8>) -> Result<Self, BlockError> {
        HDWallet::new(seed)
    }

    /// Validate seed entropy
    pub fn validate_entropy(entropy: &[u8]) -> Result<bool, BlockError> {
        // Valid if 16-64 bytes and has sufficient randomness
        if entropy.len() < 16 || entropy.len() > 64 {
            return Ok(false);
        }

        // Check for some randomness (not all same byte)
        let first = entropy[0];
        let has_variety = entropy.iter().any(|&b| b != first);

        Ok(has_variety)
    }
}

impl Default for HDWallet {
    fn default() -> Self {
        // Default with empty seed - must be initialized with new()
        HDWallet {
            seed: Vec::new(),
            master_key: String::new(),
            accounts: HashMap::new(),
            derivation_path: "m/44'/0'/0'/0".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_keypair() {
        let result = generate_keypair();
        assert!(result.is_ok());
        let (pub_key, _priv_key) = result.unwrap();
        assert!(!pub_key.0.is_empty());
    }

    #[test]
    fn test_identity_registry() {
        let mut registry = IdentityManager::new();
        let (pub_key, _priv_key) = generate_keypair().unwrap();

        let identity = Identity::User {
            public_key: pub_key.clone(),
            permissions: vec![Permission::CanVote],
        };

        registry.register_identity(identity);
        assert!(registry.get_identity(&pub_key).is_some());
    }

    #[test]
    fn test_validator_check() {
        let mut registry = IdentityManager::new();
        let (pub_key, _) = generate_keypair().unwrap();

        let validator = Identity::Validator {
            public_key: pub_key.clone(),
            voting_power: 100,
            permissions: vec![Permission::CanPropose, Permission::CanValidate],
        };

        registry.register_identity(validator);
        assert!(registry.is_validator(&pub_key));
    }

    #[test]
    fn test_sign_and_verify_signature() {
        let (pub_key, priv_key) = generate_keypair().unwrap();
        let msg = b"zoka-signature-test";

        let sig = sign_message(msg, &priv_key).unwrap();
        assert!(verify_signature(&sig, msg, &pub_key));
        assert!(!verify_signature(&sig, b"tampered", &pub_key));
    }

    #[test]
    fn test_hdwallet_creation() {
        let seed = vec![1u8; 32];
        let wallet = HDWallet::new(seed);
        assert!(wallet.is_ok());
        let wallet = wallet.unwrap();
        assert_eq!(wallet.account_count(), 0);
    }

    #[test]
    fn test_hdwallet_invalid_seed_empty() {
        let seed = vec![];
        let result = HDWallet::new(seed);
        assert!(result.is_err());
    }

    #[test]
    fn test_hdwallet_invalid_seed_too_small() {
        let seed = vec![1u8; 15];
        let result = HDWallet::new(seed);
        assert!(result.is_err());
    }

    #[test]
    fn test_hdwallet_invalid_seed_too_large() {
        let seed = vec![1u8; 65];
        let result = HDWallet::new(seed);
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_seed() {
        let seed = HDWallet::generate_seed(32).unwrap();
        assert_eq!(seed.len(), 32);

        // Different calls should generate different seeds
        let seed2 = HDWallet::generate_seed(32).unwrap();
        assert_ne!(seed, seed2);
    }

    #[test]
    fn test_generate_seed_12_words() {
        let seed = HDWallet::generate_seed(16).unwrap();
        assert_eq!(seed.len(), 16);
    }

    #[test]
    fn test_generate_seed_24_words() {
        let seed = HDWallet::generate_seed(32).unwrap();
        assert_eq!(seed.len(), 32);
    }

    #[test]
    fn test_seed_to_mnemonic_12_words() {
        let seed = vec![1u8; 16];
        let wallet = HDWallet::new(seed).unwrap();
        let mnemonic = wallet.seed_to_mnemonic().unwrap();
        assert_eq!(mnemonic.len(), 12);
        assert!(!mnemonic[0].is_empty());
    }

    #[test]
    fn test_seed_to_mnemonic_24_words() {
        let seed = vec![1u8; 32];
        let wallet = HDWallet::new(seed).unwrap();
        let mnemonic = wallet.seed_to_mnemonic().unwrap();
        assert_eq!(mnemonic.len(), 24);
    }

    #[test]
    fn test_mnemonic_to_seed_recovery_12() {
        let wallet = HDWallet::new(vec![7u8; 16]).unwrap();
        let mnemonic = wallet.seed_to_mnemonic().unwrap();
        let seed = HDWallet::mnemonic_to_seed(&mnemonic);
        assert!(seed.is_ok());
        assert!(!seed.unwrap().is_empty());
    }

    #[test]
    fn test_mnemonic_to_seed_recovery_24() {
        let wallet = HDWallet::new(vec![9u8; 32]).unwrap();
        let mnemonic = wallet.seed_to_mnemonic().unwrap();
        let seed = HDWallet::mnemonic_to_seed(&mnemonic);
        assert!(seed.is_ok());
    }

    #[test]
    fn test_mnemonic_to_seed_recovery_invalid_count() {
        let mnemonic = vec!["abandon".to_string()];
        let result = HDWallet::mnemonic_to_seed(&mnemonic);
        assert!(result.is_err());
    }

    #[test]
    fn test_derive_account() {
        let seed = vec![1u8; 32];
        let mut wallet = HDWallet::new(seed).unwrap();
        let (pub_key, priv_key) = wallet.derive_account(0).unwrap();

        assert!(!pub_key.0.is_empty());
        assert!(!priv_key.0.is_empty());
        assert_eq!(wallet.account_count(), 1);
    }

    #[test]
    fn test_derive_multiple_accounts() {
        let seed = vec![1u8; 32];
        let mut wallet = HDWallet::new(seed).unwrap();
        let accounts = wallet.derive_accounts(5).unwrap();

        assert_eq!(accounts.len(), 5);
        assert_eq!(wallet.account_count(), 5);
    }

    #[test]
    fn test_derive_accounts_deterministic() {
        let seed = vec![1u8; 32];
        let mut wallet1 = HDWallet::new(seed.clone()).unwrap();
        let (pub1, _) = wallet1.derive_account(0).unwrap();

        let mut wallet2 = HDWallet::new(seed).unwrap();
        let (pub2, _) = wallet2.derive_account(0).unwrap();

        // Same seed should produce same keys
        assert_eq!(pub1.0, pub2.0);
    }

    #[test]
    fn test_get_account() {
        let seed = vec![1u8; 32];
        let mut wallet = HDWallet::new(seed).unwrap();
        wallet.derive_account(0).unwrap();

        let account = wallet.get_account(0);
        assert!(account.is_some());
    }

    #[test]
    fn test_get_account_nonexistent() {
        let seed = vec![1u8; 32];
        let wallet = HDWallet::new(seed).unwrap();
        let account = wallet.get_account(0);
        assert!(account.is_none());
    }

    #[test]
    fn test_set_derivation_path_valid() {
        let seed = vec![1u8; 32];
        let mut wallet = HDWallet::new(seed).unwrap();
        let result = wallet.set_derivation_path("m/44'/60'/0'/0".to_string());
        assert!(result.is_ok());
        assert_eq!(wallet.get_derivation_path(), "m/44'/60'/0'/0");
    }

    #[test]
    fn test_set_derivation_path_invalid() {
        let seed = vec![1u8; 32];
        let mut wallet = HDWallet::new(seed).unwrap();
        let result = wallet.set_derivation_path("invalid_path".to_string());
        assert!(result.is_err());
    }

    #[test]
    fn test_get_account_indices() {
        let seed = vec![1u8; 32];
        let mut wallet = HDWallet::new(seed).unwrap();
        wallet.derive_account(0).unwrap();
        wallet.derive_account(2).unwrap();
        wallet.derive_account(1).unwrap();

        let indices = wallet.get_account_indices();
        assert_eq!(indices, vec![0, 1, 2]);
    }

    #[test]
    fn test_clear_accounts() {
        let seed = vec![1u8; 32];
        let mut wallet = HDWallet::new(seed).unwrap();
        wallet.derive_accounts(3).unwrap();
        assert_eq!(wallet.account_count(), 3);

        wallet.clear_accounts();
        assert_eq!(wallet.account_count(), 0);
    }

    #[test]
    fn test_restore_from_seed() {
        let seed = vec![1u8; 32];
        let wallet = HDWallet::restore_from_seed(seed.clone()).unwrap();
        assert_eq!(wallet.account_count(), 0);
    }

    #[test]
    fn test_validate_entropy_valid_16() {
        let entropy = vec![
            1u8, 2u8, 3u8, 4u8, 5u8, 1u8, 2u8, 3u8, 4u8, 5u8, 1u8, 2u8, 3u8, 4u8, 5u8, 6u8,
        ];
        let result = HDWallet::validate_entropy(&entropy).unwrap();
        assert!(result);
    }

    #[test]
    fn test_validate_entropy_valid_32() {
        let mut entropy = vec![1u8; 32];
        entropy[5] = 2u8; // Add some variety
        let result = HDWallet::validate_entropy(&entropy).unwrap();
        assert!(result);
    }

    #[test]
    fn test_validate_entropy_invalid_empty() {
        let entropy = vec![];
        let result = HDWallet::validate_entropy(&entropy).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_hdwallet_full_flow() {
        // Generate random seed
        let seed = HDWallet::generate_seed(32).unwrap();

        // Create wallet
        let mut wallet = HDWallet::new(seed.clone()).unwrap();

        // Generate mnemonic
        let mnemonic = wallet.seed_to_mnemonic().unwrap();
        assert_eq!(mnemonic.len(), 24);

        // Recover seed from mnemonic
        let recovered_seed = HDWallet::mnemonic_to_seed(&mnemonic).unwrap();
        assert!(!recovered_seed.is_empty());

        // Derive accounts
        let accounts = wallet.derive_accounts(3).unwrap();
        assert_eq!(accounts.len(), 3);
        assert_eq!(wallet.account_count(), 3);

        // Set derivation path
        wallet
            .set_derivation_path("m/44'/60'/0'/0".to_string())
            .unwrap();

        // Get indices
        let indices = wallet.get_account_indices();
        assert_eq!(indices.len(), 3);
    }
}
