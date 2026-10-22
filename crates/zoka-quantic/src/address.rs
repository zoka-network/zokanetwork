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
//!
//! A short address (`docs/QUANTIC-DIRECCIONES-CORTAS.md`) is a digest of a full
//! one, about 58 characters instead of 1.734:
//!
//! ```text
//! digest   = BLAKE2b-256("ZokaQ_ShortAddr_", prefix ‖ full payload)
//! payload  = version 0x02 (1) ‖ digest (32)
//! address  = prefix ‖ Base58( payload ‖ checksum )
//! ```
//!
//! Paying to it needs the full address, fetched from the network: the digest
//! binds all of it, so whoever answers can withhold it but not swap the key.
//! No F4Jumble here: every character already depends on the whole full address.

use std::fmt;

use orchard::Address;
use subtle::ConstantTimeEq;
use zoka_pq::{XWingPublicKey, X_WING_PUBLIC_KEY_BYTES};

use crate::hash::{blake2b_256, ADDRESS_CHECKSUM, SHORT_ADDRESS};
use crate::network::Network;

pub const ADDRESS_VERSION: u8 = 0x01;
/// Version byte of a short address.
pub const SHORT_ADDRESS_VERSION: u8 = 0x02;
pub const SHORT_ADDRESS_DIGEST_BYTES: usize = 32;
pub const SHORT_ADDRESS_PAYLOAD_BYTES: usize = 1 + SHORT_ADDRESS_DIGEST_BYTES;
pub const ORCHARD_RAW_ADDRESS_BYTES: usize = 43;
pub const ADDRESS_PAYLOAD_BYTES: usize = 1 + ORCHARD_RAW_ADDRESS_BYTES + X_WING_PUBLIC_KEY_BYTES;
pub const ADDRESS_CHECKSUM_BYTES: usize = 4;
pub const ADDRESS_BYTES: usize = ADDRESS_PAYLOAD_BYTES + ADDRESS_CHECKSUM_BYTES;
pub const SHORT_ADDRESS_BYTES: usize = SHORT_ADDRESS_PAYLOAD_BYTES + ADDRESS_CHECKSUM_BYTES;
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
    /// A valid short address where a full one is needed: resolve it first.
    ShortAddress,
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
            AddressError::ShortAddress => {
                f.write_str("this is a short address: its full address has to be looked up first")
            }
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

/// The network of the prefix and the Base58 body as bytes, full or short.
fn decode_body(text: &str) -> Result<(Network, Vec<u8>), AddressError> {
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
    let bytes = bs58::decode(body)
        .into_vec()
        .map_err(|_| AddressError::InvalidBase58)?;
    Ok((network, bytes))
}

fn require_network<T>(found: Network, expected: Network, value: T) -> Result<T, AddressError> {
    if found != expected {
        return Err(AddressError::WrongNetwork { expected, found });
    }
    Ok(value)
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

    /// Decodes an address of any Quantic network. A short address is refused
    /// with [`AddressError::ShortAddress`], after checking that it is one.
    pub fn decode(text: &str) -> Result<Self, AddressError> {
        let (network, jumbled) = decode_body(text)?;
        if jumbled.len() == SHORT_ADDRESS_BYTES {
            return QuanticShortAddress::decode(text).and(Err(AddressError::ShortAddress));
        }
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
        require_network(address.network, expected, address)
    }

    /// Its short address.
    pub fn short(&self) -> QuanticShortAddress {
        QuanticShortAddress::of(self)
    }
}

/// A short address: the digest of a full [`QuanticAddress`]. To pay to it the
/// full address has to be fetched (from the network or an address book) and
/// checked with [`QuanticShortAddress::matches`].
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct QuanticShortAddress {
    network: Network,
    digest: [u8; SHORT_ADDRESS_DIGEST_BYTES],
}

impl QuanticShortAddress {
    pub fn of(full: &QuanticAddress) -> Self {
        let digest = blake2b_256(
            SHORT_ADDRESS,
            &[full.network.address_prefix().as_bytes(), &full.payload()],
        );
        Self {
            network: full.network,
            digest,
        }
    }

    pub fn from_digest(network: Network, digest: [u8; SHORT_ADDRESS_DIGEST_BYTES]) -> Self {
        Self { network, digest }
    }

    pub fn network(&self) -> Network {
        self.network
    }

    pub fn digest(&self) -> &[u8; SHORT_ADDRESS_DIGEST_BYTES] {
        &self.digest
    }

    /// Whether `full` is the address this one stands for (same network too).
    pub fn matches(&self, full: &QuanticAddress) -> bool {
        Self::of(full) == *self
    }

    fn payload(&self) -> [u8; SHORT_ADDRESS_PAYLOAD_BYTES] {
        let mut payload = [0u8; SHORT_ADDRESS_PAYLOAD_BYTES];
        payload[0] = SHORT_ADDRESS_VERSION;
        payload[1..].copy_from_slice(&self.digest);
        payload
    }

    pub fn encode(&self) -> String {
        let payload = self.payload();
        let mut bytes = Vec::with_capacity(SHORT_ADDRESS_BYTES);
        bytes.extend_from_slice(&payload);
        bytes.extend_from_slice(&checksum(self.network, &payload));
        format!(
            "{}{}",
            self.network.address_prefix(),
            bs58::encode(bytes).into_string()
        )
    }

    /// Decodes a short address of any Quantic network.
    pub fn decode(text: &str) -> Result<Self, AddressError> {
        let (network, bytes) = decode_body(text)?;
        if bytes.len() != SHORT_ADDRESS_BYTES {
            return Err(AddressError::InvalidLength);
        }
        let (payload, found) = bytes.split_at(SHORT_ADDRESS_PAYLOAD_BYTES);
        if !bool::from(checksum(network, payload).ct_eq(found)) {
            return Err(AddressError::ChecksumMismatch);
        }
        if payload[0] != SHORT_ADDRESS_VERSION {
            return Err(AddressError::UnsupportedVersion);
        }
        Ok(Self {
            network,
            digest: payload[1..].try_into().expect("exact slice"),
        })
    }

    pub fn decode_for(text: &str, expected: Network) -> Result<Self, AddressError> {
        let address = Self::decode(text)?;
        require_network(address.network, expected, address)
    }
}

impl fmt::Debug for QuanticShortAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "QuanticShortAddress({})", self.encode())
    }
}

impl fmt::Display for QuanticShortAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode())
    }
}

/// What someone can paste as a recipient: a full address, ready to pay, or a
/// short one that has to be resolved first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recipient {
    Full(QuanticAddress),
    Short(QuanticShortAddress),
}

impl Recipient {
    pub fn decode_for(text: &str, expected: Network) -> Result<Self, AddressError> {
        match QuanticAddress::decode_for(text, expected) {
            Ok(full) => Ok(Self::Full(full)),
            Err(AddressError::ShortAddress) => {
                QuanticShortAddress::decode_for(text, expected).map(Self::Short)
            }
            Err(error) => Err(error),
        }
    }

    pub fn short(&self) -> QuanticShortAddress {
        match self {
            Self::Full(full) => full.short(),
            Self::Short(short) => *short,
        }
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

    #[test]
    fn a_short_address_is_about_sixty_characters_and_round_trips() {
        for network in Network::ALL {
            let full = address(network, 3);
            let short = full.short();
            let text = short.encode();
            assert!(text.starts_with(network.address_prefix()));
            assert!((50..=64).contains(&text.len()), "{}", text.len());
            assert_eq!(QuanticShortAddress::decode(&text).unwrap(), short);
            assert_eq!(QuanticShortAddress::decode_for(&text, network).unwrap(), short);
            assert_eq!(short.network(), network);
        }
    }

    #[test]
    fn a_short_address_matches_only_its_full_address() {
        let full = address(Network::Testnet, 3);
        let short = full.short();
        assert!(short.matches(&full));
        assert!(!short.matches(&address(Network::Testnet, 4)));
        // The same keys on another network are another address.
        let other = QuanticAddress::from_parts(Network::Mainnet, *full.orchard(), full.xwing().clone());
        assert!(!short.matches(&other));
        assert_ne!(other.short().digest(), short.digest());
    }

    #[test]
    fn a_full_decoder_refuses_a_short_address_and_a_recipient_takes_both() {
        let full = address(Network::Testnet, 3);
        let short_text = full.short().encode();
        assert_eq!(
            QuanticAddress::decode(&short_text).unwrap_err(),
            AddressError::ShortAddress
        );
        assert_eq!(
            QuanticShortAddress::decode(&full.encode()).unwrap_err(),
            AddressError::InvalidLength
        );
        assert_eq!(
            Recipient::decode_for(&full.encode(), Network::Testnet).unwrap(),
            Recipient::Full(full.clone())
        );
        let recipient = Recipient::decode_for(&short_text, Network::Testnet).unwrap();
        assert_eq!(recipient, Recipient::Short(full.short()));
        assert_eq!(recipient.short(), full.short());
        assert_eq!(
            Recipient::decode_for(&short_text, Network::Mainnet).unwrap_err(),
            AddressError::WrongNetwork {
                expected: Network::Mainnet,
                found: Network::Testnet
            }
        );
    }

    #[test]
    fn a_changed_character_of_a_short_address_is_caught() {
        let text = address(Network::Testnet, 3).short().encode();
        let prefix = Network::Testnet.address_prefix().len();
        for position in [prefix, prefix + 20, text.len() - 1] {
            let mut chars: Vec<char> = text.chars().collect();
            chars[position] = if chars[position] == '2' { '3' } else { '2' };
            let changed: String = chars.into_iter().collect();
            assert!(
                matches!(
                    QuanticShortAddress::decode(&changed),
                    Err(AddressError::ChecksumMismatch | AddressError::InvalidLength)
                ),
                "{position}"
            );
        }
    }

    /// The format is fixed: a change here breaks every short address already
    /// shared. Same keys as `address(Testnet, 3)`.
    #[test]
    fn the_short_address_format_does_not_change() {
        assert_eq!(address(Network::Testnet, 3).short().encode(), SHORT_VECTOR_TESTNET_3);
    }

    const SHORT_VECTOR_TESTNET_3: &str = "zqtest14v4DLNV3HyMWaCir6w4Hv4kG3G98eLZBU2HPk5tffYnmqgQwcj";
}
