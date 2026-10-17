//! Zoka Quantic shielded protocol.
//!
//! The spend circuit is Ironwood (the corrected Orchard circuit over Halo 2,
//! no trusted setup) through the `orchard` crate, unmodified. Orchard's
//! elliptic-curve note encryption is replaced by X-Wing (X25519 + ML-KEM-768),
//! so that a future quantum computer that learns an address still cannot find
//! or read its notes. Nothing here is used by Zenith.
#![forbid(unsafe_code)]

/// And the tree crate, for wallets that keep their own witnesses.
pub use incrementalmerkletree;
/// The exact `orchard` this crate builds on, for dependents that need its types.
pub use orchard;

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

/// Operating-system randomness with the `rand_core` 0.10 traits this crate
/// takes, for callers that use another `rand` (the node uses 0.8). Panics if
/// the OS cannot provide randomness, as `rand::rngs::OsRng` does.
pub fn os_rng() -> rand_core::UnwrapErr<rand::rngs::SysRng> {
    rand_core::UnwrapErr(rand::rngs::SysRng)
}
