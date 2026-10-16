//! Keys of a Quantic wallet.
//!
//! Everything comes from the wallet root (the entropy of the recovery phrase):
//!
//! ```text
//! seed       = BLAKE2b-512("ZokaQuanticSeed_", root)                 64 bytes, ZIP 32 seed
//! sk         = ZIP 32 Orchard spending key at m/32'/coin_type'/account'
//! xwing_seed = BLAKE2b-256("ZokaQ_XWingSeed_", sk)                   X-Wing key seed
//! ```
//!
//! The Orchard keys (spend authorization, nullifier key, viewing keys,
//! diversified addresses) are exactly those of the `orchard` crate. The X-Wing
//! key replaces Orchard's elliptic-curve note encryption (see
//! `note_encryption`), so it acts as the incoming viewing key of the wallet:
//! whoever has it sees the payments, nobody else can tell them apart.

use std::fmt;

use orchard::keys::{FullViewingKey, OutgoingViewingKey, Scope, SpendAuthorizingKey, SpendingKey};
use zeroize::Zeroizing;
use zoka_pq::{XWingPublicKey, XWingSecretKey, X_WING_SEED_BYTES};

use crate::address::QuanticAddress;
use crate::hash::{blake2b_256, blake2b_512, ROOT_SEED, XWING_SEED};
use crate::network::Network;

/// A 12-word phrase (16 bytes) is the minimum; Quantic recommends 24 (32 bytes).
pub const MIN_ROOT_BYTES: usize = 16;
pub const MAX_ROOT_BYTES: usize = 64;
/// Network id, Orchard full viewing key and X-Wing seed.
pub const FULL_VIEWING_KEY_BYTES: usize = 1 + 96 + X_WING_SEED_BYTES;
/// Every address of a wallet uses diversifier index 0: the X-Wing key is the
/// same in all of them, so more diversified addresses would not be unlinkable.
pub const ADDRESS_INDEX: u32 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyError {
    RootLength {
        actual: usize,
    },
    InvalidAccount,
    /// ZIP 32 refused the seed (probability about 2^-252).
    Derivation,
    InvalidEncoding,
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyError::RootLength { actual } => write!(
                f,
                "wallet root must have {MIN_ROOT_BYTES} to {MAX_ROOT_BYTES} bytes, got {actual}"
            ),
            KeyError::InvalidAccount => f.write_str("invalid ZIP 32 account index"),
            KeyError::Derivation => f.write_str("ZIP 32 derivation failed for this seed"),
            KeyError::InvalidEncoding => f.write_str("invalid viewing key encoding"),
        }
    }
}

impl std::error::Error for KeyError {}

fn xwing_secret(seed: &[u8; X_WING_SEED_BYTES]) -> XWingSecretKey {
    XWingSecretKey::from_seed(seed).expect("the X-Wing seed has the exact length")
}

/// Spends, sees and receives. Its `Debug` output never shows key material.
pub struct QuanticSpendingKey {
    network: Network,
    orchard: SpendingKey,
    xwing_seed: Zeroizing<[u8; X_WING_SEED_BYTES]>,
}

impl QuanticSpendingKey {
    pub fn from_root(root: &[u8], network: Network, account: u32) -> Result<Self, KeyError> {
        if !(MIN_ROOT_BYTES..=MAX_ROOT_BYTES).contains(&root.len()) {
            return Err(KeyError::RootLength { actual: root.len() });
        }
        let seed = Zeroizing::new(blake2b_512(ROOT_SEED, &[root]));
        let account = zip32::AccountId::try_from(account).map_err(|_| KeyError::InvalidAccount)?;
        let orchard = SpendingKey::from_zip32_seed(seed.as_ref(), network.coin_type(), account)
            .map_err(|_| KeyError::Derivation)?;
        let xwing_seed = Zeroizing::new(blake2b_256(XWING_SEED, &[orchard.to_bytes()]));
        Ok(Self {
            network,
            orchard,
            xwing_seed,
        })
    }

    pub fn network(&self) -> Network {
        self.network
    }

    pub fn orchard(&self) -> &SpendingKey {
        &self.orchard
    }

    pub fn spend_authorizing_key(&self) -> SpendAuthorizingKey {
        SpendAuthorizingKey::from(&self.orchard)
    }

    pub fn full_viewing_key(&self) -> QuanticFullViewingKey {
        QuanticFullViewingKey::from_parts(
            self.network,
            FullViewingKey::from(&self.orchard),
            &self.xwing_seed,
        )
    }
}

impl fmt::Debug for QuanticSpendingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "QuanticSpendingKey({}, [REDACTED])", self.network)
    }
}

/// Sees every payment and every spend of the wallet, but cannot spend.
pub struct QuanticFullViewingKey {
    network: Network,
    orchard: FullViewingKey,
    xwing_seed: Zeroizing<[u8; X_WING_SEED_BYTES]>,
    xwing: XWingSecretKey,
    xwing_public: XWingPublicKey,
}

impl QuanticFullViewingKey {
    fn from_parts(
        network: Network,
        orchard: FullViewingKey,
        xwing_seed: &[u8; X_WING_SEED_BYTES],
    ) -> Self {
        let xwing = xwing_secret(xwing_seed);
        let xwing_public = xwing.public_key();
        Self {
            network,
            orchard,
            xwing_seed: Zeroizing::new(*xwing_seed),
            xwing,
            xwing_public,
        }
    }

    pub fn network(&self) -> Network {
        self.network
    }

    pub fn orchard(&self) -> &FullViewingKey {
        &self.orchard
    }

    pub fn xwing_public(&self) -> &XWingPublicKey {
        &self.xwing_public
    }

    pub(crate) fn xwing_secret(&self) -> &XWingSecretKey {
        &self.xwing
    }

    /// The address to give out.
    pub fn address(&self) -> QuanticAddress {
        self.address_for(Scope::External)
    }

    /// Where the change of this wallet goes. Never shown to anyone.
    pub fn change_address(&self) -> QuanticAddress {
        self.address_for(Scope::Internal)
    }

    pub fn address_for(&self, scope: Scope) -> QuanticAddress {
        QuanticAddress::from_parts(
            self.network,
            self.orchard.address_at(ADDRESS_INDEX, scope),
            self.xwing_public.clone(),
        )
    }

    pub fn ovk(&self, scope: Scope) -> OutgoingViewingKey {
        self.orchard.to_ovk(scope)
    }

    /// Secret: whoever holds these bytes sees the whole history of the wallet.
    pub fn to_bytes(&self) -> Zeroizing<[u8; FULL_VIEWING_KEY_BYTES]> {
        let mut out = Zeroizing::new([0u8; FULL_VIEWING_KEY_BYTES]);
        out[0] = self.network.id();
        out[1..97].copy_from_slice(&self.orchard.to_bytes());
        out[97..].copy_from_slice(self.xwing_seed.as_ref());
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, KeyError> {
        if bytes.len() != FULL_VIEWING_KEY_BYTES {
            return Err(KeyError::InvalidEncoding);
        }
        let network = Network::from_id(bytes[0]).ok_or(KeyError::InvalidEncoding)?;
        let fvk_bytes: [u8; 96] = bytes[1..97].try_into().expect("exact slice");
        let orchard = FullViewingKey::from_bytes(&fvk_bytes).ok_or(KeyError::InvalidEncoding)?;
        let seed =
            Zeroizing::new(<[u8; X_WING_SEED_BYTES]>::try_from(&bytes[97..]).expect("exact slice"));
        Ok(Self::from_parts(network, orchard, &seed))
    }
}

impl Clone for QuanticFullViewingKey {
    fn clone(&self) -> Self {
        Self::from_parts(self.network, self.orchard.clone(), &self.xwing_seed)
    }
}

impl fmt::Debug for QuanticFullViewingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "QuanticFullViewingKey({}, [REDACTED])", self.network)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_root_gives_the_same_wallet_and_another_root_another() {
        let a = QuanticSpendingKey::from_root(&[7; 32], Network::Testnet, 0).unwrap();
        let b = QuanticSpendingKey::from_root(&[7; 32], Network::Testnet, 0).unwrap();
        let c = QuanticSpendingKey::from_root(&[8; 32], Network::Testnet, 0).unwrap();
        assert_eq!(a.orchard().to_bytes(), b.orchard().to_bytes());
        assert_eq!(
            a.full_viewing_key().address(),
            b.full_viewing_key().address()
        );
        assert_ne!(
            a.full_viewing_key().address(),
            c.full_viewing_key().address()
        );
    }

    #[test]
    fn mainnet_and_testnet_derive_different_keys_from_one_phrase() {
        let main = QuanticSpendingKey::from_root(&[7; 32], Network::Mainnet, 0).unwrap();
        let test = QuanticSpendingKey::from_root(&[7; 32], Network::Testnet, 0).unwrap();
        assert_ne!(main.orchard().to_bytes(), test.orchard().to_bytes());
        // Testnet and regtest share the coin type, as in Zcash.
        let reg = QuanticSpendingKey::from_root(&[7; 32], Network::Regtest, 0).unwrap();
        assert_eq!(test.orchard().to_bytes(), reg.orchard().to_bytes());
    }

    #[test]
    fn accounts_are_separate_wallets() {
        let a = QuanticSpendingKey::from_root(&[7; 32], Network::Testnet, 0).unwrap();
        let b = QuanticSpendingKey::from_root(&[7; 32], Network::Testnet, 1).unwrap();
        assert_ne!(a.orchard().to_bytes(), b.orchard().to_bytes());
        assert_ne!(
            a.full_viewing_key().xwing_public().to_bytes(),
            b.full_viewing_key().xwing_public().to_bytes()
        );
    }

    #[test]
    fn the_root_length_is_checked() {
        for len in [0, 15, 65] {
            assert_eq!(
                QuanticSpendingKey::from_root(&vec![1; len], Network::Testnet, 0).unwrap_err(),
                KeyError::RootLength { actual: len }
            );
        }
        assert!(QuanticSpendingKey::from_root(&[1; 16], Network::Testnet, 0).is_ok());
        assert!(QuanticSpendingKey::from_root(&[1; 64], Network::Testnet, 0).is_ok());
    }

    #[test]
    fn the_viewing_key_round_trips_and_rejects_garbage() {
        let sk = QuanticSpendingKey::from_root(&[9; 32], Network::Testnet, 0).unwrap();
        let fvk = sk.full_viewing_key();
        let restored = QuanticFullViewingKey::from_bytes(fvk.to_bytes().as_ref()).unwrap();
        assert_eq!(restored.address(), fvk.address());
        assert_eq!(restored.change_address(), fvk.change_address());
        assert_eq!(restored.network(), Network::Testnet);
        assert!(QuanticFullViewingKey::from_bytes(&fvk.to_bytes()[..100]).is_err());
        let mut bad_network = *fvk.to_bytes();
        bad_network[0] = 0;
        assert!(QuanticFullViewingKey::from_bytes(&bad_network).is_err());
    }

    #[test]
    fn change_goes_to_another_orchard_address_with_the_same_xwing_key() {
        let fvk = QuanticSpendingKey::from_root(&[9; 32], Network::Testnet, 0)
            .unwrap()
            .full_viewing_key();
        let pay = fvk.address();
        let change = fvk.change_address();
        assert_ne!(
            pay.orchard().to_raw_address_bytes(),
            change.orchard().to_raw_address_bytes()
        );
        assert_eq!(pay.xwing().to_bytes(), change.xwing().to_bytes());
        assert_eq!(
            fvk.orchard().scope_for_address(pay.orchard()),
            Some(Scope::External)
        );
        assert_eq!(
            fvk.orchard().scope_for_address(change.orchard()),
            Some(Scope::Internal)
        );
    }

    #[test]
    fn debug_output_hides_the_keys() {
        let sk = QuanticSpendingKey::from_root(&[9; 32], Network::Testnet, 0).unwrap();
        assert_eq!(
            format!("{sk:?}"),
            "QuanticSpendingKey(zoka-quantic-testnet-v1, [REDACTED])"
        );
        assert!(format!("{:?}", sk.full_viewing_key()).ends_with("[REDACTED])"));
    }
}
