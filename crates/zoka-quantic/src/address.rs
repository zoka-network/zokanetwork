//! Quantic addresses: an Orchard receiver plus an X-Wing key.
//!
//! ```text
//! payload  = version (1) ‖ Orchard raw address (43) ‖ X-Wing key (1216)
//! checksum = first 4 bytes of BLAKE2b-256("ZokaQ_AddrCheck_", prefix ‖ payload)
//! address  = prefix ‖ Base58( F4Jumble( payload ‖ checksum ) )
//! ```
//!
//! F4Jumble (ZIP 316) makes every character depend on every byte. Without it,
//! someone could keep the Orchard receiver of a real address, which gives the
//! first characters, and swap in their own X-Wing key: a sender who only
//! compares the beginning would pay a note that its owner can never find.
//! The checksum covers the network prefix, so an address of one network does
//! not decode on another.

use std::fmt;

use orchard::Address;
use subtle::ConstantTimeEq;
use zoka_pq::{XWingPublicKey, X_WING_PUBLIC_KEY_BYTES};

use crate::hash::{blake2b_256, ADDRESS_CHECKSUM};
use crate::network::Network;

pub const ADDRESS_VERSION: u8 = 0x01;
pub const ORCHARD_RAW_ADDRESS_BYTES: usize = 43;
pub const ADDRESS_PAYLOAD_BYTES: usize = 1 + ORCHARD_RAW_ADDRESS_BYTES + X_WING_PUBLIC_KEY_BYTES;
pub const ADDRESS_CHECKSUM_BYTES: usize = 4;
pub const ADDRESS_BYTES: usize = ADDRESS_PAYLOAD_BYTES + ADDRESS_CHECKSUM_BYTES;
/// Longer input is rejected before decoding it.
pub const MAX_ADDRESS_CHARS: usize = 1800;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressError {
    TooLong,
    UnknownPrefix,
    WrongNetwork { expected: Network, found: Network },
    InvalidBase58,
    InvalidLength,
    ChecksumMismatch,
    UnsupportedVersion,
    InvalidOrchardReceiver,
    InvalidXWingKey,
}

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AddressError::TooLong => f.write_str("address is too long"),
            AddressError::UnknownPrefix => f.write_str("not a Zoka Quantic address"),
            AddressError::WrongNetwork { expected, found } => {
                write!(f, "address is for {found}, expected {expected}")
            }
            AddressError::InvalidBase58 => f.write_str("address has characters outside Base58"),
            AddressError::InvalidLength => f.write_str("address has the wrong length"),
            AddressError::ChecksumMismatch => f.write_str("address checksum does not match"),
            AddressError::UnsupportedVersion => f.write_str("unsupported address version"),
            AddressError::InvalidOrchardReceiver => f.write_str("invalid Orchard receiver"),
            AddressError::InvalidXWingKey => f.write_str("invalid X-Wing key"),
        }
    }
}

impl std::error::Error for AddressError {}

#[derive(Clone)]
pub struct QuanticAddress {
    network: Network,
    orchard: Address,
    xwing: XWingPublicKey,
}

fn checksum(network: Network, payload: &[u8]) -> [u8; ADDRESS_CHECKSUM_BYTES] {
    let digest = blake2b_256(
        ADDRESS_CHECKSUM,
        &[network.address_prefix().as_bytes(), payload],
    );
    digest[..ADDRESS_CHECKSUM_BYTES]
        .try_into()
        .expect("prefix of a digest")
}

impl QuanticAddress {
    pub fn from_parts(network: Network, orchard: Address, xwing: XWingPublicKey) -> Self {
        Self {
            network,
            orchard,
            xwing,
        }
    }

    pub fn network(&self) -> Network {
        self.network
    }

    pub fn orchard(&self) -> &Address {
        &self.orchard
    }

    pub fn xwing(&self) -> &XWingPublicKey {
        &self.xwing
    }

    fn payload(&self) -> [u8; ADDRESS_PAYLOAD_BYTES] {
        let mut payload = [0u8; ADDRESS_PAYLOAD_BYTES];
        payload[0] = ADDRESS_VERSION;
        payload[1..1 + ORCHARD_RAW_ADDRESS_BYTES]
            .copy_from_slice(&self.orchard.to_raw_address_bytes());
        payload[1 + ORCHARD_RAW_ADDRESS_BYTES..].copy_from_slice(&self.xwing.to_bytes());
        payload
    }

    pub fn encode(&self) -> String {
        let payload = self.payload();
        let mut bytes = Vec::with_capacity(ADDRESS_BYTES);
        bytes.extend_from_slice(&payload);
        bytes.extend_from_slice(&checksum(self.network, &payload));
        let jumbled =
            f4jumble::f4jumble(&bytes).expect("the address length is a valid F4Jumble length");
        format!(
            "{}{}",
            self.network.address_prefix(),
            bs58::encode(jumbled).into_string()
        )
    }

    /// Decodes an address of any Quantic network.
    pub fn decode(text: &str) -> Result<Self, AddressError> {
        if text.len() > MAX_ADDRESS_CHARS {
            return Err(AddressError::TooLong);
        }
        let (network, body) = Network::ALL
            .into_iter()
            .find_map(|network| {
                text.strip_prefix(network.address_prefix())
                    .map(|body| (network, body))
            })
            .ok_or(AddressError::UnknownPrefix)?;
        let jumbled = bs58::decode(body)
            .into_vec()
            .map_err(|_| AddressError::InvalidBase58)?;
        if jumbled.len() != ADDRESS_BYTES {
            return Err(AddressError::InvalidLength);
        }
        let bytes = f4jumble::f4jumble_inv(&jumbled).map_err(|_| AddressError::InvalidLength)?;
        let (payload, found) = bytes.split_at(ADDRESS_PAYLOAD_BYTES);
        if !bool::from(checksum(network, payload).ct_eq(found)) {
            return Err(AddressError::ChecksumMismatch);
        }
        if payload[0] != ADDRESS_VERSION {
            return Err(AddressError::UnsupportedVersion);
        }
        let raw: [u8; ORCHARD_RAW_ADDRESS_BYTES] = payload[1..1 + ORCHARD_RAW_ADDRESS_BYTES]
            .try_into()
            .expect("exact slice");
        let orchard: Option<Address> = Address::from_raw_address_bytes(&raw).into();
        let orchard = orchard.ok_or(AddressError::InvalidOrchardReceiver)?;
        let xwing = XWingPublicKey::from_bytes(&payload[1 + ORCHARD_RAW_ADDRESS_BYTES..])
            .map_err(|_| AddressError::InvalidXWingKey)?;
        Ok(Self {
            network,
            orchard,
            xwing,
        })
    }

    /// Decodes and requires the given network.
    pub fn decode_for(text: &str, expected: Network) -> Result<Self, AddressError> {
        let address = Self::decode(text)?;
        if address.network != expected {
            return Err(AddressError::WrongNetwork {
                expected,
                found: address.network,
            });
        }
        Ok(address)
    }
}

impl PartialEq for QuanticAddress {
    fn eq(&self, other: &Self) -> bool {
        self.network == other.network && self.payload()[..] == other.payload()[..]
    }
}

impl Eq for QuanticAddress {}

impl fmt::Debug for QuanticAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let encoded = self.encode();
        write!(f, "QuanticAddress({}…)", &encoded[..encoded.len().min(24)])
    }
}

impl fmt::Display for QuanticAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::QuanticSpendingKey;

    fn address(network: Network, seed: u8) -> QuanticAddress {
        QuanticSpendingKey::from_root(&[seed; 32], network, 0)
            .unwrap()
            .full_viewing_key()
            .address()
    }

    #[test]
    fn every_network_round_trips_with_its_prefix() {
        for network in Network::ALL {
            let original = address(network, 3);
            let text = original.encode();
            assert!(text.starts_with(network.address_prefix()));
            assert!(text.len() <= MAX_ADDRESS_CHARS, "{}", text.len());
            assert_eq!(QuanticAddress::decode(&text).unwrap(), original);
            assert_eq!(
                QuanticAddress::decode_for(&text, network).unwrap(),
                original
            );
        }
    }

    #[test]
    fn the_length_is_about_seventeen_hundred_characters() {
        let text = address(Network::Mainnet, 3).encode();
        assert!((1700..=1740).contains(&text.len()), "{}", text.len());
    }

    #[test]
    fn an_address_of_another_network_is_refused() {
        let text = address(Network::Testnet, 3).encode();
        assert_eq!(
            QuanticAddress::decode_for(&text, Network::Mainnet).unwrap_err(),
            AddressError::WrongNetwork {
                expected: Network::Mainnet,
                found: Network::Testnet
            }
        );
        // Same body under another prefix: the checksum covers the prefix.
        let swapped = text.replacen("zqtest1", "zq1", 1);
        assert_eq!(
            QuanticAddress::decode(&swapped).unwrap_err(),
            AddressError::ChecksumMismatch
        );
    }

    #[test]
    fn any_changed_character_is_caught() {
        let text = address(Network::Mainnet, 3).encode();
        let prefix = Network::Mainnet.address_prefix().len();
        for position in [
            prefix,
            prefix + 1,
            text.len() / 2,
            text.len() - 2,
            text.len() - 1,
        ] {
            let mut chars: Vec<char> = text.chars().collect();
            chars[position] = if chars[position] == 'x' { 'y' } else { 'x' };
            let altered: String = chars.into_iter().collect();
            assert!(
                QuanticAddress::decode(&altered).is_err(),
                "position {position}"
            );
        }
    }

    #[test]
    fn swapping_the_xwing_key_changes_the_beginning_of_the_address() {
        let a = address(Network::Mainnet, 3);
        let b = address(Network::Mainnet, 4);
        let forged = QuanticAddress::from_parts(Network::Mainnet, *a.orchard(), b.xwing().clone());
        let prefix = Network::Mainnet.address_prefix().len();
        // F4Jumble: the same Orchard receiver does not give the same first characters.
        assert_ne!(&a.encode()[..prefix + 8], &forged.encode()[..prefix + 8]);
    }

    #[test]
    fn malformed_input_gets_a_precise_error() {
        assert_eq!(
            QuanticAddress::decode("zka1abc").unwrap_err(),
            AddressError::UnknownPrefix
        );
        assert_eq!(
            QuanticAddress::decode("zq1abc0").unwrap_err(),
            AddressError::InvalidBase58
        );
        assert_eq!(
            QuanticAddress::decode("zq1abc").unwrap_err(),
            AddressError::InvalidLength
        );
        let long = format!("zq1{}", "1".repeat(MAX_ADDRESS_CHARS));
        assert_eq!(
            QuanticAddress::decode(&long).unwrap_err(),
            AddressError::TooLong
        );
        let text = address(Network::Mainnet, 3).encode();
        assert_eq!(
            QuanticAddress::decode(&format!(" {text}")).unwrap_err(),
            AddressError::UnknownPrefix
        );
        assert!(QuanticAddress::decode(&format!("{text} ")).is_err());
    }

    #[test]
    fn a_wrong_version_is_reported_after_a_valid_checksum() {
        let original = address(Network::Mainnet, 3);
        let mut payload = original.payload();
        payload[0] = 0x02;
        let mut bytes = payload.to_vec();
        bytes.extend_from_slice(&checksum(Network::Mainnet, &payload));
        let text = format!(
            "zq1{}",
            bs58::encode(f4jumble::f4jumble(&bytes).unwrap()).into_string()
        );
        assert_eq!(
            QuanticAddress::decode(&text).unwrap_err(),
            AddressError::UnsupportedVersion
        );
    }
}
