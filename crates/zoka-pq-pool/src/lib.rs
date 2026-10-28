//! Private, hash-based transfer proofs. The public statement is verified along
//! with the proof; no host-side witness check substitutes for AIR constraints.
#![forbid(unsafe_code)]
pub mod crypto;
pub mod ledger;
pub mod model;
pub mod transaction;
mod transfer;
pub mod wallet;
pub use transfer::{prove_transfer, verify_transfer, ProofEnvelope};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported network")]
    Network,
    #[error("invalid transaction shape")]
    Shape,
    #[error("noncanonical encoding")]
    Encoding,
    #[error("duplicate nullifier")]
    DuplicateNullifier,
    #[error("invalid witness: {0}")]
    Witness(&'static str),
    #[error("proof error: {0}")]
    Proof(String),
    #[error("note encryption or authentication failed")]
    Encryption,
    #[error("ledger rule: {0}")]
    Ledger(&'static str),
}
