//! Cryptographic primitives shared across the transaction pipeline.
//!
//! Provides:
//! - Pedersen commitments (`Commitment`)
//! - Nullifiers + nullifier set (`Nullifier`, `NullifierSet`)
//!
//! The canonical private transaction type lives in
//! `crate::private_wallet::PrivateTransaction`. A legacy
//! `transaction::PrivateTransaction` + `TransactionValidator` pair used
//! to live here but was removed: the types were deprecated, not
//! exercised by production code, and the only tests they had were
//! self-referential. Their role is now played by the ZK circuit
//! `private_tx` and the nullifier set used by `PrivateTransactionPool`.

pub mod commitment;
pub mod nullifier;

pub use commitment::Commitment;
pub use nullifier::Nullifier;

#[cfg(test)]
mod tests {
    // Status-enum smoke test used to live here alongside the
    // now-removed `TransactionIndexer`. The types exercised by that
    // test were dead code, so the test was removed with them.
}
