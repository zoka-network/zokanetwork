//! The Zoka Quantic networks. Nothing here exists on Zenith.

use std::fmt;

/// ZIP 32 coin type of Zoka Quantic mainnet. Zoka has no SLIP-44 entry; the
/// value only has to be fixed forever and differ from the testnet one (1).
pub const MAINNET_COIN_TYPE: u32 = 0x5a4b;
/// SLIP-44 convention for every test network.
pub const TESTNET_COIN_TYPE: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Network {
    Mainnet,
    /// Public test network of Quantic (Zenith is a different chain).
    Testnet,
    /// Local networks for development and tests.
    Regtest,
}

impl Network {
    pub const ALL: [Network; 3] = [Network::Mainnet, Network::Testnet, Network::Regtest];

    pub const fn chain_id(self) -> &'static str {
        match self {
            Network::Mainnet => "zoka-quantic-v1",
            Network::Testnet => "zoka-quantic-testnet-v1",
            Network::Regtest => "zoka-quantic-regtest-v1",
        }
    }

    /// Text in front of every address of this network.
    pub const fn address_prefix(self) -> &'static str {
        match self {
            Network::Mainnet => "zq1",
            Network::Testnet => "zqtest1",
            Network::Regtest => "zqregtest1",
        }
    }

    pub const fn coin_type(self) -> u32 {
        match self {
            Network::Mainnet => MAINNET_COIN_TYPE,
            Network::Testnet | Network::Regtest => TESTNET_COIN_TYPE,
        }
    }

    /// One byte inside every transaction, so that nothing signed for one
    /// network is valid on another.
    pub const fn id(self) -> u8 {
        match self {
            Network::Mainnet => 1,
            Network::Testnet => 2,
            Network::Regtest => 3,
        }
    }

    pub fn from_id(id: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|network| network.id() == id)
    }
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.chain_id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_prefixes_and_chain_ids_never_repeat() {
        for (i, a) in Network::ALL.iter().enumerate() {
            assert_eq!(Network::from_id(a.id()), Some(*a));
            for b in &Network::ALL[i + 1..] {
                assert_ne!(a.id(), b.id());
                assert_ne!(a.chain_id(), b.chain_id());
                // No prefix may start another one, or decoding would be ambiguous.
                assert!(!a.address_prefix().starts_with(b.address_prefix()));
                assert!(!b.address_prefix().starts_with(a.address_prefix()));
            }
        }
        assert_eq!(Network::from_id(0), None);
    }

    #[test]
    fn quantic_never_reuses_the_zenith_names() {
        for network in Network::ALL {
            assert!(!network.chain_id().contains("zenith"));
            assert!(!network.address_prefix().starts_with("zka"));
            assert!(!network.address_prefix().starts_with("zkq"));
        }
    }
}
