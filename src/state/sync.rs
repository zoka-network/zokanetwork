//! Initial block download (IBD) helpers.
//!
//! When a node boots behind the network — or skips a block in the gossip
//! stream — it needs to walk forward from its local tip to the network's
//! tip one block at a time. The plumbing lives in two places:
//!
//! - [`try_apply_synced_block_payload`]: pure, testable. Takes the bytes
//!   that arrived in a `ZokaMessage::BlockResponse`, parses them, checks
//!   the announced hash matches, and runs them through
//!   `state::chain::apply_block`. Used both for the active sync loop and
//!   for unsolicited block announcements over gossipsub.
//! - [`PeerChainView`]: a mutable observation of the highest block height
//!   any peer has revealed by publishing a block. Used to decide whether
//!   IBD is still needed.
//!
//! The async P2P round-trip (`request_block_with_retry`) lives in
//! `network::p2p` and is called from `main.rs`. This module deliberately
//! keeps the parser / state-mutating code in a non-async, easily-tested
//! function so the consensus rules can be exercised without a libp2p
//! swarm.

use serde::Deserialize;

use crate::network::message::BlockResponseMessage;
use crate::state::chain::{apply_block, read_chain_head, ChainError, ChainHead};
use crate::storage::RocksDbStorage;
use crate::types::Block;

/// Highest block height any peer has revealed since this node booted.
/// Treat 0 as "no peer has announced a block yet" — IBD is then a no-op.
#[derive(Debug, Default, Clone, Copy)]
pub struct PeerChainView {
    pub perceived_max_height: u64,
}

impl PeerChainView {
    pub fn observe(&mut self, height: u64) {
        if height > self.perceived_max_height {
            self.perceived_max_height = height;
        }
    }
}

/// Outcome of feeding a `BlockResponse` payload into the chain. Each
/// variant maps to one human-readable verdict for the dashboard / logs;
/// callers should not rely on the wording but on the variant.
#[derive(Debug, Clone)]
pub enum SyncOutcome {
    /// Block was applied; the chain head moved.
    Applied(ChainHead),
    /// Block was already on this chain; the message was a duplicate.
    AlreadyKnown { height: u64, hash: String },
    /// Block is at a height beyond `local_head + 1`. The local node has
    /// to fetch missing intermediate blocks before this one will fit.
    GapAhead { local_head: u64, candidate: u64 },
    /// Block is at a height already covered by the local chain. We don't
    /// reorg here; this is a sibling that loses fork choice for now.
    BehindOrSibling { local_head: u64, candidate: u64 },
    /// Decoding the payload failed. The peer sent garbage.
    BadPayload(String),
    /// Hash announced in the response does not match the header. The
    /// peer either tampered with the announcement or with the body.
    AnnouncedHashMismatch { announced: String, header: String },
    /// `apply_block` rejected the block. Includes the underlying reason
    /// (invalid PoW, wrong coinbase, etc.).
    Rejected(String),
}

/// Attempt to apply the body of a `BlockResponseMessage` to the chain.
/// Pure aside from reading and writing `storage`; no networking.
pub fn try_apply_synced_block_payload(
    storage: &RocksDbStorage,
    chain_id: &str,
    response: &BlockResponseMessage,
) -> SyncOutcome {
    let block: Block = match serde_json::from_slice(&response.encrypted_block) {
        Ok(b) => b,
        Err(err) => return SyncOutcome::BadPayload(err.to_string()),
    };

    if block.header.hash != response.block_hash {
        return SyncOutcome::AnnouncedHashMismatch {
            announced: response.block_hash.clone(),
            header: block.header.hash,
        };
    }

    let local_head = match read_chain_head(storage) {
        Ok(h) => h,
        Err(err) => return SyncOutcome::Rejected(format!("read chain head failed: {err}")),
    };
    let local_height = local_head.as_ref().map(|h| h.height).unwrap_or(0);

    let candidate_height = block.header.block_height;
    if storage.block_exists(&block.header.hash).unwrap_or(false) {
        return SyncOutcome::AlreadyKnown {
            height: candidate_height,
            hash: block.header.hash,
        };
    }
    if candidate_height > local_height.saturating_add(1) {
        return SyncOutcome::GapAhead {
            local_head: local_height,
            candidate: candidate_height,
        };
    }
    if candidate_height < local_height.saturating_add(1) {
        return SyncOutcome::BehindOrSibling {
            local_head: local_height,
            candidate: candidate_height,
        };
    }

    match apply_block(storage, chain_id, &block) {
        Ok(head) => SyncOutcome::Applied(head),
        Err(ChainError::BlockAlreadyKnown(hash)) => SyncOutcome::AlreadyKnown {
            height: candidate_height,
            hash,
        },
        Err(err) => SyncOutcome::Rejected(err.to_string()),
    }
}

/// Convenience: returns the next block height the local chain needs in
/// order to advance, or `None` when it is already caught up to or ahead
/// of `peer_view.perceived_max_height`.
pub fn next_required_height(
    storage: &RocksDbStorage,
    peer_view: &PeerChainView,
) -> Result<Option<u64>, String> {
    let local_height = storage
        .get_chain_height()
        .map_err(|e| format!("read chain height failed: {e:?}"))?;
    if local_height >= peer_view.perceived_max_height {
        return Ok(None);
    }
    Ok(Some(local_height.saturating_add(1)))
}

/// Schema-tolerant deserializer for incoming peer block payloads. Used
/// by tests and the sync helper alike.
#[allow(dead_code)]
pub(crate) fn parse_block_payload(bytes: &[u8]) -> Result<Block, String> {
    let mut de = serde_json::Deserializer::from_slice(bytes);
    Block::deserialize(&mut de).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::economics;
    use crate::mining::block_validator::build_coinbase_tx;
    use crate::mining::pow;
    use crate::state::chain::{apply_block_with_expected_difficulty, genesis_parent_for};
    use crate::types::{Address, Block, BlockHeader, PrivateCoinbaseOutput, PublicKey, Signature};

    fn open_temp_storage() -> (RocksDbStorage, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let storage = RocksDbStorage::new(dir.path().to_str().unwrap()).expect("rocksdb open");
        (storage, dir)
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

    fn mine_test_block(
        chain_id: &str,
        parent_hash: &str,
        parent_height: u64,
        parent_supply: u64,
        miner: &Address,
        difficulty_bits: u32,
        timestamp: u64,
    ) -> Block {
        let height = parent_height.saturating_add(1);
        let subsidy = economics::block_reward_for_height(height, parent_supply);
        let coinbase = build_coinbase_tx(miner.clone(), subsidy, height, timestamp);
        let engine = pow::engine_for(chain_id, height).expect("testnet engine");
        let mut nonce = 0u64;
        let (computed, found_nonce) = loop {
            let preimage =
                pow::build_pow_input(chain_id, parent_hash, miner, height, timestamp, nonce);
            let hash = engine.calculate_hash(&preimage).expect("hash");
            if pow::hash_meets_difficulty(&hash, difficulty_bits) {
                break (hash, nonce);
            }
            nonce = nonce.wrapping_add(1);
        };
        let mut block = Block {
            header: BlockHeader {
                hash: String::new(),
                prev_hash: parent_hash.to_string(),
                merkle_root: String::new(),
                state_root: String::from("0"),
                timestamp,
                nonce: found_nonce,
                block_height: height,
                validator: PublicKey(Vec::new()),
                seal: Signature(computed),
            },
            transactions: vec![coinbase],
            private_tx_hashes: Vec::new(),
            private_fee_claims: Vec::new(),
            private_coinbase: Some(dummy_private_coinbase(height, miner)),
        };
        block.header.merkle_root = block.compute_merkle_root();
        block.header.hash = block.compute_hash();
        block
    }

    fn response_for(block: &Block) -> BlockResponseMessage {
        BlockResponseMessage {
            block_height: block.header.block_height,
            encrypted_block: serde_json::to_vec(block).expect("serialize block"),
            block_hash: block.header.hash.clone(),
            timestamp: block.header.timestamp,
        }
    }

    #[test]
    fn peer_chain_view_keeps_max_observed_height() {
        let mut view = PeerChainView::default();
        view.observe(5);
        view.observe(3);
        view.observe(12);
        view.observe(11);
        assert_eq!(view.perceived_max_height, 12);
    }

    #[test]
    fn try_apply_rejects_invalid_json_payload() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let response = BlockResponseMessage {
            block_height: 1,
            encrypted_block: b"not json".to_vec(),
            block_hash: "deadbeef".to_string(),
            timestamp: 0,
        };
        match try_apply_synced_block_payload(&storage, chain_id, &response) {
            SyncOutcome::BadPayload(_) => {}
            other => panic!("expected BadPayload, got: {other:?}"),
        }
    }

    #[test]
    fn try_apply_rejects_announced_hash_mismatch() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let block = mine_test_block(
            chain_id,
            &genesis_parent_for(chain_id).header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_000,
        );
        let mut response = response_for(&block);
        response.block_hash = "deadbeef".to_string();
        match try_apply_synced_block_payload(&storage, chain_id, &response) {
            SyncOutcome::AnnouncedHashMismatch { announced, header } => {
                assert_eq!(announced, "deadbeef");
                assert_eq!(header, block.header.hash);
            }
            other => panic!("expected AnnouncedHashMismatch, got: {other:?}"),
        }
    }

    #[test]
    fn try_apply_marks_gap_when_block_height_is_ahead_of_local() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        // Build a block at height 3 from a synthetic parent that the
        // chain doesn't have. The DB head is at 0; the block claims
        // height 3 → gap.
        let mut block = mine_test_block(
            chain_id,
            &genesis_parent_for(chain_id).header.hash,
            2,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_000,
        );
        block.header.block_height = 3;
        block.header.hash = block.compute_hash();
        let response = response_for(&block);
        match try_apply_synced_block_payload(&storage, chain_id, &response) {
            SyncOutcome::GapAhead {
                local_head,
                candidate,
            } => {
                assert_eq!(local_head, 0);
                assert_eq!(candidate, 3);
            }
            other => panic!("expected GapAhead, got: {other:?}"),
        }
    }

    #[test]
    fn try_apply_marks_already_known_when_block_is_a_duplicate() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let block = mine_test_block(
            chain_id,
            &genesis_parent_for(chain_id).header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_000,
        );
        apply_block_with_expected_difficulty(&storage, chain_id, &block, 4).expect("first apply");
        let response = response_for(&block);
        match try_apply_synced_block_payload(&storage, chain_id, &response) {
            SyncOutcome::AlreadyKnown { height, hash } => {
                assert_eq!(height, block.header.block_height);
                assert_eq!(hash, block.header.hash);
            }
            other => panic!("expected AlreadyKnown, got: {other:?}"),
        }
    }

    #[test]
    fn try_apply_marks_behind_when_block_height_is_below_local() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let block1 = mine_test_block(
            chain_id,
            &genesis_parent_for(chain_id).header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_000,
        );
        apply_block_with_expected_difficulty(&storage, chain_id, &block1, 4).expect("apply 1");
        let block2 = mine_test_block(
            chain_id,
            &block1.header.hash,
            block1.header.block_height,
            economics::block_reward_for_height(1, 0),
            &"zka1miner".to_string(),
            4,
            1_700_000_120,
        );
        apply_block_with_expected_difficulty(&storage, chain_id, &block2, 4).expect("apply 2");

        // Build a sibling block at height 1 with a DIFFERENT timestamp so
        // it has a different hash. This simulates a peer offering us a
        // forked block at an already-covered height.
        let sibling = mine_test_block(
            chain_id,
            &genesis_parent_for(chain_id).header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_777,
        );
        let response = response_for(&sibling);
        match try_apply_synced_block_payload(&storage, chain_id, &response) {
            SyncOutcome::BehindOrSibling {
                local_head,
                candidate,
            } => {
                assert_eq!(local_head, 2);
                assert_eq!(candidate, 1);
            }
            other => panic!("expected BehindOrSibling, got: {other:?}"),
        }
    }

    #[test]
    fn next_required_height_returns_none_when_caught_up() {
        let (storage, _dir) = open_temp_storage();
        let view = PeerChainView::default();
        let next = next_required_height(&storage, &view).expect("query");
        assert_eq!(next, None);
    }

    #[test]
    fn next_required_height_returns_local_plus_one_when_behind() {
        let (storage, _dir) = open_temp_storage();
        let mut view = PeerChainView::default();
        view.observe(5);
        let next = next_required_height(&storage, &view).expect("query");
        assert_eq!(next, Some(1));
    }
}
