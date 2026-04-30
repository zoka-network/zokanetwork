//! Consensus-grade PoW block validation.
//!
//! Every node — miner or verifier — funnels block acceptance through
//! `validate_pow_block`. The function is pure: same `(block, parent,
//! parent_supply, expected_difficulty)` always returns the same verdict, so
//! two honest nodes never disagree.
//!
//! Coinbase representation:
//! - Sender is the sentinel string [`COINBASE_SENDER`].
//! - Position is fixed at index 0 of `block.transactions`.
//! - Amount equals `economics::block_reward_for_height(height, parent_supply)`.
//! - If the subsidy is non-zero, the block must also commit a
//!   `private_coinbase` output. The subsidy remains public for emission
//!   auditability; the spendable reward is imported by the miner as a
//!   private note after maturity.
//! - Private fees are paid via encrypted `private_fee_claims`, not by
//!   increasing the visible coinbase amount.
//!
//! PoW commitment:
//! - The proof-of-work hash bytes are stored verbatim in
//!   `block.header.seal.0`. Validation recomputes the hash via
//!   [`crate::mining::pow::engine_for`] and rejects on mismatch.
//! - The preimage commits to `(chain_id, prev_hash, coinbase.receiver,
//!   height, timestamp, nonce)`. Tampering with the coinbase receiver
//!   invalidates the PoW.
//!
//! All consensus-relevant constants live in [`crate::economics`]. This
//! module never invents its own.

use thiserror::Error;

use std::collections::HashSet;

use crate::economics;
use crate::mining::pow;
use crate::types::{Address, Block, Signature, Transaction};

/// Reserved sender string used to mark coinbase transactions. Anything
/// other than this in `tx.sender` MUST NOT be a coinbase.
pub const COINBASE_SENDER: &str = "zoka:coinbase:v1";

/// Error returned by [`validate_pow_block`]. Each variant maps to one
/// concrete consensus rule so peers and observers can tell exactly why a
/// block was rejected.
#[derive(Debug, Clone, Error)]
pub enum BlockValidationError {
    #[error("missing coinbase: every PoW block must include a coinbase transaction at index 0")]
    MissingCoinbase,

    #[error("multiple coinbase transactions: expected exactly one at index 0, found another at index {0}")]
    MultipleCoinbases(usize),

    #[error("coinbase recipient is empty: every coinbase must credit a wallet address")]
    CoinbaseRecipientEmpty,

    #[error(
        "coinbase amount mismatch at height {height}: expected {expected} (subsidy), got {actual}"
    )]
    CoinbaseAmountMismatch {
        expected: u64,
        actual: u64,
        height: u64,
    },

    #[error(
        "coinbase emission overshoots remaining cap: requested {requested}, remaining {remaining}"
    )]
    CoinbaseExceedsCap { requested: u64, remaining: u64 },

    #[error("missing private coinbase output for non-zero block subsidy")]
    MissingPrivateCoinbase,

    #[error("private coinbase output exists even though this block has zero subsidy")]
    UnexpectedPrivateCoinbase,

    #[error("private coinbase recipient mismatch: coinbase receiver={coinbase_receiver}, private output recipient={private_recipient}")]
    PrivateCoinbaseRecipientMismatch {
        coinbase_receiver: String,
        private_recipient: String,
    },

    #[error(
        "private coinbase height mismatch: block={block_height}, private output={private_height}"
    )]
    PrivateCoinbaseHeightMismatch {
        block_height: u64,
        private_height: u64,
    },

    #[error("private coinbase unlock mismatch: expected {expected}, got {actual}")]
    PrivateCoinbaseUnlockMismatch { expected: u64, actual: u64 },

    #[error("private coinbase output has empty {field}")]
    MalformedPrivateCoinbase { field: &'static str },

    #[error("block height drift: parent {parent}, candidate {candidate}, expected {expected}")]
    HeightDrift {
        parent: u64,
        candidate: u64,
        expected: u64,
    },

    #[error(
        "block does not link to parent: candidate.prev_hash={candidate}, parent.hash={parent}"
    )]
    PrevHashMismatch { candidate: String, parent: String },

    #[error("block header hash drift: stored {stored} != recomputed {computed}")]
    HeaderHashMismatch { stored: String, computed: String },

    #[error("merkle root drift: stored {stored} != recomputed {computed}")]
    MerkleRootMismatch { stored: String, computed: String },

    #[error("expected PoW algorithm '{expected}' for chain '{chain_id}', refusing to validate as '{got}'")]
    WrongAlgorithm {
        expected: String,
        chain_id: String,
        got: String,
    },

    #[error("PoW hash does not meet difficulty: needed {needed} leading zero bits, got {actual}")]
    PowBelowDifficulty { needed: u32, actual: u32 },

    #[error("PoW seal does not match recomputed hash: header={header}, recomputed={computed}")]
    PowSealMismatch { header: String, computed: String },

    #[error("pow engine error: {0}")]
    PowEngine(String),

    #[error("duplicate private transaction hash in block: {0}")]
    DuplicatePrivateTxHash(String),

    #[error("private fee claim references tx not committed in this block: {0}")]
    FeeClaimForMissingPrivateTx(String),

    #[error("duplicate private fee claim for tx: {0}")]
    DuplicatePrivateFeeClaim(String),

    #[error("private fee claim for tx {tx_hash} has empty {field}")]
    MalformedPrivateFeeClaim {
        tx_hash: String,
        field: &'static str,
    },
}

use BlockValidationError::*;

/// Returns true when `tx` is a coinbase by sentinel sender.
pub fn is_coinbase(tx: &Transaction) -> bool {
    tx.sender == COINBASE_SENDER
}

/// Constructs a coinbase transaction with the canonical sentinel sender,
/// the miner address as recipient, and the chain-driven subsidy as amount.
/// The signature field is intentionally a fixed sentinel; coinbase txs
/// are not signed (no sender keypair exists for them).
pub fn build_coinbase_tx(
    receiver: Address,
    amount: u64,
    height: u64,
    timestamp: u64,
) -> Transaction {
    Transaction::new(
        COINBASE_SENDER.to_string(),
        receiver,
        amount,
        height,
        timestamp,
        Signature(b"zoka:coinbase-v1:unsigned".to_vec()),
    )
}

/// Inputs the validator needs from outside the block itself.
///
/// `parent_total_supply_emitted` is the cumulative ZKA emission BEFORE
/// applying this block; the validator uses it to compute the expected
/// subsidy and the remaining cap. `expected_difficulty_bits` is the
/// difficulty target the consensus rules require for this height; the
/// retarget rule that produces it is owned by the caller (today
/// `retarget_difficulty` in `main.rs`, eventually a header-driven function).
#[derive(Debug, Clone)]
pub struct PowBlockContext<'a> {
    pub chain_id: &'a str,
    pub parent: &'a Block,
    pub parent_total_supply_emitted: u64,
    pub expected_difficulty_bits: u32,
}

/// Validates `candidate` against `ctx`. On success the block is safe to
/// extend the chain: PoW solved, coinbase honest, header self-consistent,
/// linked to the parent. On failure the variant names the rule that broke.
pub fn validate_pow_block(
    candidate: &Block,
    ctx: &PowBlockContext<'_>,
) -> Result<(), BlockValidationError> {
    validate_coinbase_shape(candidate)?;
    validate_private_fee_claims(candidate)?;
    validate_coinbase_amount(candidate, ctx)?;
    validate_chain_link(candidate, ctx)?;
    validate_private_coinbase(candidate)?;
    validate_header_self_consistency(candidate)?;
    validate_pow(candidate, ctx)?;
    Ok(())
}

fn validate_private_fee_claims(block: &Block) -> Result<(), BlockValidationError> {
    let mut committed_private_txs = HashSet::new();
    for tx_hash in &block.private_tx_hashes {
        if !committed_private_txs.insert(tx_hash.as_str()) {
            return Err(DuplicatePrivateTxHash(tx_hash.clone()));
        }
    }

    let mut claimed = HashSet::new();
    for claim in &block.private_fee_claims {
        if claim.tx_hash.trim().is_empty() {
            return Err(MalformedPrivateFeeClaim {
                tx_hash: claim.tx_hash.clone(),
                field: "tx_hash",
            });
        }
        if !committed_private_txs.contains(claim.tx_hash.as_str()) {
            return Err(FeeClaimForMissingPrivateTx(claim.tx_hash.clone()));
        }
        if !claimed.insert(claim.tx_hash.as_str()) {
            return Err(DuplicatePrivateFeeClaim(claim.tx_hash.clone()));
        }
        if claim.ephemeral_pubkey.is_empty() {
            return Err(MalformedPrivateFeeClaim {
                tx_hash: claim.tx_hash.clone(),
                field: "ephemeral_pubkey",
            });
        }
        if claim.ciphertext.is_empty() {
            return Err(MalformedPrivateFeeClaim {
                tx_hash: claim.tx_hash.clone(),
                field: "ciphertext",
            });
        }
    }

    Ok(())
}

fn validate_coinbase_shape(block: &Block) -> Result<(), BlockValidationError> {
    let first = block.transactions.first().ok_or(MissingCoinbase)?;
    if !is_coinbase(first) {
        return Err(MissingCoinbase);
    }
    if first.receiver.trim().is_empty() {
        return Err(CoinbaseRecipientEmpty);
    }
    for (idx, tx) in block.transactions.iter().enumerate().skip(1) {
        if is_coinbase(tx) {
            return Err(MultipleCoinbases(idx));
        }
    }
    Ok(())
}

fn validate_coinbase_amount(
    block: &Block,
    ctx: &PowBlockContext<'_>,
) -> Result<(), BlockValidationError> {
    let coinbase = &block.transactions[0];
    let height = block.header.block_height;
    let expected_subsidy =
        economics::block_reward_for_height(height, ctx.parent_total_supply_emitted);
    if coinbase.amount != expected_subsidy {
        return Err(CoinbaseAmountMismatch {
            expected: expected_subsidy,
            actual: coinbase.amount,
            height,
        });
    }
    let remaining_cap = economics::MAX_SUPPLY_ZKA.saturating_sub(ctx.parent_total_supply_emitted);
    if coinbase.amount > remaining_cap {
        return Err(CoinbaseExceedsCap {
            requested: coinbase.amount,
            remaining: remaining_cap,
        });
    }
    Ok(())
}

fn validate_private_coinbase(block: &Block) -> Result<(), BlockValidationError> {
    let coinbase = &block.transactions[0];
    if coinbase.amount == 0 {
        if block.private_coinbase.is_some() {
            return Err(UnexpectedPrivateCoinbase);
        }
        return Ok(());
    }

    let private_coinbase = block
        .private_coinbase
        .as_ref()
        .ok_or(MissingPrivateCoinbase)?;
    if private_coinbase.block_height != block.header.block_height {
        return Err(PrivateCoinbaseHeightMismatch {
            block_height: block.header.block_height,
            private_height: private_coinbase.block_height,
        });
    }
    if private_coinbase.recipient_address != coinbase.receiver {
        return Err(PrivateCoinbaseRecipientMismatch {
            coinbase_receiver: coinbase.receiver.clone(),
            private_recipient: private_coinbase.recipient_address.clone(),
        });
    }
    let expected_unlock = block
        .header
        .block_height
        .saturating_add(economics::COINBASE_MATURITY_BLOCKS);
    if private_coinbase.unlock_height != expected_unlock {
        return Err(PrivateCoinbaseUnlockMismatch {
            expected: expected_unlock,
            actual: private_coinbase.unlock_height,
        });
    }

    let required = [
        (
            "recipient_address",
            private_coinbase.recipient_address.as_str(),
        ),
        (
            "recipient_commitment",
            private_coinbase.recipient_commitment.as_str(),
        ),
        ("output_id", private_coinbase.output_id.as_str()),
        ("asset_tag", private_coinbase.asset_tag.as_str()),
        (
            "asset_tag_commitment",
            private_coinbase.asset_tag_commitment.as_str(),
        ),
        ("stealth_address", private_coinbase.stealth_address.as_str()),
        ("commitment", private_coinbase.commitment.as_str()),
        (
            "amount_commitment",
            private_coinbase.amount_commitment.as_str(),
        ),
        (
            "one_time_public_key",
            private_coinbase.one_time_public_key.as_str(),
        ),
    ];
    for (field, value) in required {
        if value.trim().is_empty() {
            return Err(MalformedPrivateCoinbase { field });
        }
    }
    if private_coinbase.ephemeral_pubkey.is_empty() {
        return Err(MalformedPrivateCoinbase {
            field: "ephemeral_pubkey",
        });
    }
    if private_coinbase.range_proof.is_empty() {
        return Err(MalformedPrivateCoinbase {
            field: "range_proof",
        });
    }
    if private_coinbase.encrypted_amount.is_empty() {
        return Err(MalformedPrivateCoinbase {
            field: "encrypted_amount",
        });
    }
    if private_coinbase.commitment_blind.is_empty() {
        return Err(MalformedPrivateCoinbase {
            field: "commitment_blind",
        });
    }

    Ok(())
}

fn validate_chain_link(
    block: &Block,
    ctx: &PowBlockContext<'_>,
) -> Result<(), BlockValidationError> {
    let expected_height = ctx.parent.header.block_height.saturating_add(1);
    if block.header.block_height != expected_height {
        return Err(HeightDrift {
            parent: ctx.parent.header.block_height,
            candidate: block.header.block_height,
            expected: expected_height,
        });
    }
    if block.header.prev_hash != ctx.parent.header.hash {
        return Err(PrevHashMismatch {
            candidate: block.header.prev_hash.clone(),
            parent: ctx.parent.header.hash.clone(),
        });
    }
    Ok(())
}

fn validate_header_self_consistency(block: &Block) -> Result<(), BlockValidationError> {
    let computed_merkle = block.compute_merkle_root();
    if computed_merkle != block.header.merkle_root {
        return Err(MerkleRootMismatch {
            stored: block.header.merkle_root.clone(),
            computed: computed_merkle,
        });
    }
    let computed_hash = block.compute_hash();
    if computed_hash != block.header.hash {
        return Err(HeaderHashMismatch {
            stored: block.header.hash.clone(),
            computed: computed_hash,
        });
    }
    Ok(())
}

fn validate_pow(block: &Block, ctx: &PowBlockContext<'_>) -> Result<(), BlockValidationError> {
    let canonical_algorithm = economics::pow_algorithm_for_chain(ctx.chain_id);
    let engine = pow::engine_for(ctx.chain_id, block.header.block_height)
        .map_err(|e| PowEngine(e.to_string()))?;
    if engine.algorithm_id() != canonical_algorithm {
        return Err(WrongAlgorithm {
            expected: canonical_algorithm.to_string(),
            chain_id: ctx.chain_id.to_string(),
            got: engine.algorithm_id().to_string(),
        });
    }
    let coinbase_receiver = &block.transactions[0].receiver;
    let preimage = pow::build_pow_input(
        ctx.chain_id,
        &block.header.prev_hash,
        coinbase_receiver,
        block.header.block_height,
        block.header.timestamp,
        block.header.nonce,
    );
    let computed = engine
        .calculate_hash(&preimage)
        .map_err(|e| PowEngine(e.to_string()))?;
    if !pow::hash_meets_difficulty(&computed, ctx.expected_difficulty_bits) {
        return Err(PowBelowDifficulty {
            needed: ctx.expected_difficulty_bits,
            actual: pow::leading_zero_bits(&computed),
        });
    }
    if block.header.seal.0 != computed {
        return Err(PowSealMismatch {
            header: hex::encode(&block.header.seal.0),
            computed: hex::encode(&computed),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BlockHeader, PrivateCoinbaseOutput, PrivateFeeClaim, PublicKey};

    fn dummy_validator() -> PublicKey {
        PublicKey(vec![0u8; 32])
    }

    fn dummy_private_coinbase(block_height: u64, recipient_address: &str) -> PrivateCoinbaseOutput {
        PrivateCoinbaseOutput {
            block_height,
            recipient_address: recipient_address.to_string(),
            unlock_height: block_height.saturating_add(economics::COINBASE_MATURITY_BLOCKS),
            output_id: format!("coinbase-output-{block_height}"),
            asset_tag: "ZOKA".to_string(),
            asset_tag_commitment: "asset-commitment".to_string(),
            recipient_commitment: "zpriv:0000000000000000000000000000000000000000000000000000000000000000:1111111111111111111111111111111111111111111111111111111111111111".to_string(),
            stealth_address: "stlth_test".to_string(),
            ephemeral_pubkey: vec![1; 32],
            commitment: "coinbase-commitment".to_string(),
            amount_commitment: "coinbase-amount-commitment".to_string(),
            range_proof: vec![2; 32],
            one_time_public_key: "coinbase-one-time-key".to_string(),
            encrypted_amount: vec![3; 8],
            commitment_blind: vec![4; 32],
        }
    }

    /// Builds a parent block consistent with a fresh chain.
    fn build_genesis_parent(chain_id: &str) -> Block {
        let prev_hash = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(b"zoka-mainnet-genesis-pow");
            hasher.update([0x1f]);
            hasher.update(chain_id.as_bytes());
            hex::encode(hasher.finalize())
        };
        Block {
            header: BlockHeader {
                hash: prev_hash,
                prev_hash: String::new(),
                merkle_root: String::from("0"),
                state_root: String::from("0"),
                timestamp: 0,
                nonce: 0,
                block_height: 0,
                validator: dummy_validator(),
                seal: Signature(vec![]),
            },
            transactions: vec![],
            private_tx_hashes: vec![],
            private_fee_claims: vec![],
            private_coinbase: None,
        }
    }

    /// Builds a candidate block for `height = parent.height + 1` mining
    /// PoW until the hash clears `difficulty_bits`. Used by tests so the
    /// happy-path block actually solves PoW deterministically.
    fn mine_block(
        chain_id: &str,
        parent: &Block,
        miner_address: &str,
        subsidy: u64,
        difficulty_bits: u32,
        timestamp: u64,
    ) -> Block {
        let height = parent.header.block_height.saturating_add(1);
        let coinbase = build_coinbase_tx(miner_address.to_string(), subsidy, height, timestamp);
        let engine = pow::engine_for(chain_id, height).expect("testnet engine builds in tests");
        let mut nonce = 0u64;
        let (computed, found_nonce) = loop {
            let preimage = pow::build_pow_input(
                chain_id,
                &parent.header.hash,
                miner_address,
                height,
                timestamp,
                nonce,
            );
            let hash = engine.calculate_hash(&preimage).expect("hash");
            if pow::hash_meets_difficulty(&hash, difficulty_bits) {
                break (hash, nonce);
            }
            nonce = nonce.wrapping_add(1);
        };
        let mut block = Block {
            header: BlockHeader {
                hash: String::new(),
                prev_hash: parent.header.hash.clone(),
                merkle_root: String::new(),
                state_root: String::from("0"),
                timestamp,
                nonce: found_nonce,
                block_height: height,
                validator: dummy_validator(),
                seal: Signature(computed),
            },
            transactions: vec![coinbase],
            private_tx_hashes: vec![],
            private_fee_claims: vec![],
            private_coinbase: Some(dummy_private_coinbase(height, miner_address)),
        };
        block.header.merkle_root = block.compute_merkle_root();
        block.header.hash = block.compute_hash();
        block
    }

    fn ctx_for<'a>(
        chain_id: &'a str,
        parent: &'a Block,
        difficulty_bits: u32,
    ) -> PowBlockContext<'a> {
        PowBlockContext {
            chain_id,
            parent,
            parent_total_supply_emitted: 0,
            expected_difficulty_bits: difficulty_bits,
        }
    }

    #[test]
    fn happy_path_block_validates() {
        let chain_id = "zoka-test-net";
        let parent = build_genesis_parent(chain_id);
        let block = mine_block(chain_id, &parent, "zka1miner", 12, 4, 1_700_000_000);
        validate_pow_block(&block, &ctx_for(chain_id, &parent, 4)).expect("block must validate");
    }

    #[test]
    fn missing_coinbase_is_rejected() {
        let chain_id = "zoka-test-net";
        let parent = build_genesis_parent(chain_id);
        let mut block = mine_block(chain_id, &parent, "zka1miner", 12, 4, 1_700_000_000);
        block.transactions.clear();
        block.header.merkle_root = block.compute_merkle_root();
        block.header.hash = block.compute_hash();
        let err = validate_pow_block(&block, &ctx_for(chain_id, &parent, 4)).unwrap_err();
        assert!(matches!(err, MissingCoinbase));
    }

    #[test]
    fn non_coinbase_at_index_zero_is_rejected() {
        let chain_id = "zoka-test-net";
        let parent = build_genesis_parent(chain_id);
        let mut block = mine_block(chain_id, &parent, "zka1miner", 12, 4, 1_700_000_000);
        block.transactions[0].sender = "zka1somebodyelse".to_string();
        block.transactions[0].hash = block.transactions[0].compute_hash();
        block.header.merkle_root = block.compute_merkle_root();
        block.header.hash = block.compute_hash();
        let err = validate_pow_block(&block, &ctx_for(chain_id, &parent, 4)).unwrap_err();
        assert!(matches!(err, MissingCoinbase));
    }

    #[test]
    fn extra_coinbase_is_rejected() {
        let chain_id = "zoka-test-net";
        let parent = build_genesis_parent(chain_id);
        let mut block = mine_block(chain_id, &parent, "zka1miner", 12, 4, 1_700_000_000);
        let phantom = build_coinbase_tx("zka1ghost".to_string(), 0, 1, 1_700_000_000);
        block.transactions.push(phantom);
        block.header.merkle_root = block.compute_merkle_root();
        block.header.hash = block.compute_hash();
        // The block has the wrong merkle root for the original PoW preimage,
        // but we're testing the multiple-coinbase rule fires first. Move
        // the coinbase shape check earlier than PoW recomputation, which it
        // is — so this should error MultipleCoinbases.
        let err = validate_pow_block(&block, &ctx_for(chain_id, &parent, 4)).unwrap_err();
        assert!(matches!(err, MultipleCoinbases(_)));
    }

    #[test]
    fn private_fee_claim_must_reference_private_tx_in_same_block() {
        let chain_id = "zoka-test-net";
        let parent = build_genesis_parent(chain_id);
        let block = mine_block(chain_id, &parent, "zka1miner", 12, 4, 1_700_000_000)
            .with_private_fee_claims(vec![PrivateFeeClaim {
                tx_hash: "missing-private-tx".to_string(),
                ephemeral_pubkey: vec![1, 2, 3],
                ciphertext: vec![4, 5, 6],
            }]);
        let err = validate_pow_block(&block, &ctx_for(chain_id, &parent, 4)).unwrap_err();
        assert!(matches!(err, FeeClaimForMissingPrivateTx(_)));
    }

    #[test]
    fn duplicate_private_fee_claim_is_rejected() {
        let chain_id = "zoka-test-net";
        let parent = build_genesis_parent(chain_id);
        let claim = PrivateFeeClaim {
            tx_hash: "private-tx-1".to_string(),
            ephemeral_pubkey: vec![1, 2, 3],
            ciphertext: vec![4, 5, 6],
        };
        let block = mine_block(chain_id, &parent, "zka1miner", 12, 4, 1_700_000_000)
            .with_private_tx_hashes(vec!["private-tx-1".to_string()])
            .with_private_fee_claims(vec![claim.clone(), claim]);
        let err = validate_pow_block(&block, &ctx_for(chain_id, &parent, 4)).unwrap_err();
        assert!(matches!(err, DuplicatePrivateFeeClaim(_)));
    }

    #[test]
    fn valid_private_fee_claim_is_committed_without_changing_coinbase() {
        let chain_id = "zoka-test-net";
        let parent = build_genesis_parent(chain_id);
        let block = mine_block(chain_id, &parent, "zka1miner", 12, 4, 1_700_000_000)
            .with_private_tx_hashes(vec!["private-tx-1".to_string()])
            .with_private_fee_claims(vec![PrivateFeeClaim {
                tx_hash: "private-tx-1".to_string(),
                ephemeral_pubkey: vec![1, 2, 3],
                ciphertext: vec![4, 5, 6],
            }]);

        assert_eq!(block.transactions[0].amount, 12);
        validate_pow_block(&block, &ctx_for(chain_id, &parent, 4))
            .expect("block with encrypted fee claim must validate");
    }

    #[test]
    fn non_zero_subsidy_requires_private_coinbase_output() {
        let chain_id = "zoka-test-net";
        let parent = build_genesis_parent(chain_id);
        let mut block = mine_block(chain_id, &parent, "zka1miner", 12, 4, 1_700_000_000);
        block.private_coinbase = None;
        block.header.merkle_root = block.compute_merkle_root();
        block.header.hash = block.compute_hash();

        let err = validate_pow_block(&block, &ctx_for(chain_id, &parent, 4)).unwrap_err();
        assert!(matches!(err, MissingPrivateCoinbase));
    }

    #[test]
    fn private_coinbase_maturity_is_consensus_checked() {
        let chain_id = "zoka-test-net";
        let parent = build_genesis_parent(chain_id);
        let mut block = mine_block(chain_id, &parent, "zka1miner", 12, 4, 1_700_000_000);
        block.private_coinbase.as_mut().unwrap().unlock_height = 1;
        block.header.merkle_root = block.compute_merkle_root();
        block.header.hash = block.compute_hash();

        let err = validate_pow_block(&block, &ctx_for(chain_id, &parent, 4)).unwrap_err();
        assert!(matches!(err, PrivateCoinbaseUnlockMismatch { .. }));
    }

    #[test]
    fn coinbase_amount_mismatch_is_rejected() {
        let chain_id = "zoka-test-net";
        let parent = build_genesis_parent(chain_id);
        // Subsidy at height 1 with 0 emitted is 12. Build with 11 instead.
        let block = mine_block(chain_id, &parent, "zka1miner", 11, 4, 1_700_000_000);
        let err = validate_pow_block(&block, &ctx_for(chain_id, &parent, 4)).unwrap_err();
        match err {
            CoinbaseAmountMismatch {
                expected, actual, ..
            } => {
                assert_eq!(expected, 12);
                assert_eq!(actual, 11);
            }
            other => panic!("expected CoinbaseAmountMismatch, got: {other:?}"),
        }
    }

    #[test]
    fn coinbase_exceeds_cap_is_rejected() {
        let chain_id = "zoka-test-net";
        let parent = build_genesis_parent(chain_id);
        let block = mine_block(chain_id, &parent, "zka1miner", 12, 4, 1_700_000_000);
        // Tell the validator the cap is already fully emitted; the
        // coinbase amount of 12 must clash with remaining=0.
        let mut ctx = ctx_for(chain_id, &parent, 4);
        ctx.parent_total_supply_emitted = economics::MAX_SUPPLY_ZKA;
        // Now the expected subsidy is 0 (cap reached) — this should fire
        // CoinbaseAmountMismatch first because amount is 12 vs expected 0.
        let err = validate_pow_block(&block, &ctx).unwrap_err();
        match err {
            CoinbaseAmountMismatch {
                expected, actual, ..
            } => {
                assert_eq!(expected, 0);
                assert_eq!(actual, 12);
            }
            other => panic!("expected CoinbaseAmountMismatch, got: {other:?}"),
        }
    }

    #[test]
    fn empty_coinbase_recipient_is_rejected() {
        let chain_id = "zoka-test-net";
        let parent = build_genesis_parent(chain_id);
        let mut block = mine_block(chain_id, &parent, "zka1miner", 12, 4, 1_700_000_000);
        block.transactions[0].receiver = String::new();
        block.transactions[0].hash = block.transactions[0].compute_hash();
        block.header.merkle_root = block.compute_merkle_root();
        block.header.hash = block.compute_hash();
        let err = validate_pow_block(&block, &ctx_for(chain_id, &parent, 4)).unwrap_err();
        assert!(matches!(err, CoinbaseRecipientEmpty));
    }

    #[test]
    fn wrong_height_is_rejected() {
        let chain_id = "zoka-test-net";
        let parent = build_genesis_parent(chain_id);
        let mut block = mine_block(chain_id, &parent, "zka1miner", 12, 4, 1_700_000_000);
        block.header.block_height = 5; // skip ahead
        block.header.hash = block.compute_hash();
        let err = validate_pow_block(&block, &ctx_for(chain_id, &parent, 4)).unwrap_err();
        match err {
            HeightDrift {
                parent,
                candidate,
                expected,
            } => {
                assert_eq!(parent, 0);
                assert_eq!(candidate, 5);
                assert_eq!(expected, 1);
            }
            other => panic!("expected HeightDrift, got: {other:?}"),
        }
    }

    #[test]
    fn detached_block_is_rejected() {
        let chain_id = "zoka-test-net";
        let parent = build_genesis_parent(chain_id);
        let mut block = mine_block(chain_id, &parent, "zka1miner", 12, 4, 1_700_000_000);
        block.header.prev_hash = "deadbeef".to_string();
        block.header.hash = block.compute_hash();
        let err = validate_pow_block(&block, &ctx_for(chain_id, &parent, 4)).unwrap_err();
        assert!(matches!(err, PrevHashMismatch { .. }));
    }

    #[test]
    fn tampered_header_hash_is_rejected() {
        let chain_id = "zoka-test-net";
        let parent = build_genesis_parent(chain_id);
        let mut block = mine_block(chain_id, &parent, "zka1miner", 12, 4, 1_700_000_000);
        block.header.hash = "00deadbeef".to_string();
        let err = validate_pow_block(&block, &ctx_for(chain_id, &parent, 4)).unwrap_err();
        assert!(matches!(err, HeaderHashMismatch { .. }));
    }

    #[test]
    fn tampered_merkle_root_is_rejected() {
        let chain_id = "zoka-test-net";
        let parent = build_genesis_parent(chain_id);
        let mut block = mine_block(chain_id, &parent, "zka1miner", 12, 4, 1_700_000_000);
        block.header.merkle_root = "ff".to_string();
        block.header.hash = block.compute_hash();
        let err = validate_pow_block(&block, &ctx_for(chain_id, &parent, 4)).unwrap_err();
        assert!(matches!(err, MerkleRootMismatch { .. }));
    }

    #[test]
    fn pow_below_difficulty_is_rejected() {
        let chain_id = "zoka-test-net";
        let parent = build_genesis_parent(chain_id);
        // Mine for 4 bits but require 24 — same block can't satisfy.
        let block = mine_block(chain_id, &parent, "zka1miner", 12, 4, 1_700_000_000);
        let err = validate_pow_block(&block, &ctx_for(chain_id, &parent, 24)).unwrap_err();
        assert!(matches!(err, PowBelowDifficulty { .. }));
    }

    #[test]
    fn tampered_pow_seal_is_rejected() {
        let chain_id = "zoka-test-net";
        let parent = build_genesis_parent(chain_id);
        let mut block = mine_block(chain_id, &parent, "zka1miner", 12, 4, 1_700_000_000);
        // Flip a byte of the seal so it stops matching the PoW preimage.
        if let Some(byte) = block.header.seal.0.first_mut() {
            *byte ^= 0xff;
        }
        // The seal change does not feed into compute_hash, so the header
        // hash still matches; the failure should come from PoW recompute.
        let err = validate_pow_block(&block, &ctx_for(chain_id, &parent, 4)).unwrap_err();
        // Tampering the seal yields either PowBelowDifficulty (when the
        // recomputed hash from the preimage no longer matches the seal,
        // but still meets the bar — unlikely) or PowSealMismatch. Either
        // is a valid rejection; both indicate the PoW didn't survive.
        assert!(
            matches!(err, PowSealMismatch { .. }) || matches!(err, PowBelowDifficulty { .. }),
            "expected a PoW failure, got: {err:?}"
        );
    }

    #[test]
    fn block_for_mainnet_chain_without_randomx_feature_is_rejected_cleanly() {
        // On a default build, mainnet engine isn't compiled in.
        // validate_pow_block must surface that as a PowEngine error
        // pointing at the missing backend, not as an obscure crash.
        let chain_id = economics::MAINNET_CHAIN_ID;
        let parent = build_genesis_parent(chain_id);
        // We can't mine a real mainnet block without the feature, so we
        // construct a syntactically well-formed block and verify the
        // backend complaint is what surfaces.
        let coinbase = build_coinbase_tx("zka1miner".to_string(), 12, 1, 1_700_000_000);
        let mut block = Block {
            header: BlockHeader {
                hash: String::new(),
                prev_hash: parent.header.hash.clone(),
                merkle_root: String::new(),
                state_root: String::from("0"),
                timestamp: 1_700_000_000,
                nonce: 0,
                block_height: 1,
                validator: dummy_validator(),
                seal: Signature(vec![]),
            },
            transactions: vec![coinbase],
            private_tx_hashes: vec![],
            private_fee_claims: vec![],
            private_coinbase: Some(dummy_private_coinbase(1, "zka1miner")),
        };
        block.header.merkle_root = block.compute_merkle_root();
        block.header.hash = block.compute_hash();

        let result = validate_pow_block(&block, &ctx_for(chain_id, &parent, 4));
        #[cfg(not(feature = "randomx"))]
        {
            match result {
                Err(PowEngine(msg)) => {
                    assert!(
                        msg.to_lowercase().contains("randomx") || msg.to_lowercase().contains("not compiled"),
                        "PowEngine error must mention the missing backend; got: {msg}"
                    );
                }
                other => panic!(
                    "expected PowEngine error for mainnet without --features randomx, got: {other:?}"
                ),
            }
        }
        #[cfg(feature = "randomx")]
        {
            // With the feature, the block won't validate (it has empty
            // seal and unsolved PoW) but must NOT fail with PowEngine.
            assert!(matches!(
                result,
                Err(PowSealMismatch { .. }) | Err(PowBelowDifficulty { .. })
            ));
        }
    }

    #[test]
    fn coinbase_sender_constant_is_immutable() {
        // Changing this string is a hard fork. Locked here so any change
        // to the constant must be intentional and reviewed.
        assert_eq!(COINBASE_SENDER, "zoka:coinbase:v1");
    }
}
