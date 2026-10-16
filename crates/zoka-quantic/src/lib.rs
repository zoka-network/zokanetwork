//! Zoka Quantic shielded protocol.
//!
//! The spend circuit is Ironwood (the corrected Orchard circuit over Halo 2,
//! no trusted setup) through the `orchard` crate, unmodified. Orchard's
//! elliptic-curve note encryption is replaced by X-Wing (X25519 + ML-KEM-768),
//! so that a future quantum computer that learns an address still cannot find
//! or read its notes. Nothing here is used by Zenith.
#![forbid(unsafe_code)]

pub mod address;
pub mod builder;
pub mod compact;
mod hash;
pub mod keys;
pub mod network;
pub mod note_encryption;
pub mod state;
pub mod tx;
pub mod verify;
pub mod wallet;

pub use address::{AddressError, QuanticAddress};
pub use keys::{KeyError, QuanticFullViewingKey, QuanticSpendingKey};
pub use network::Network;
