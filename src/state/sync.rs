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

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::Deserialize;

use crate::network::message::BlockResponseMessage;
use crate::private_wallet::{PrivateTransaction, PrivateWalletManager};
use crate::state::chain::{
    apply_block, apply_block_with_expected_difficulty, read_chain_head, ChainError, ChainHead,
};
use crate::storage::RocksDbStorage;
use crate::types::Block;

const MAX_PENDING_BLOCK_RESPONSES: usize = 256;
const MAX_PENDING_BLOCKS_PER_HEIGHT: usize = 4;
const MAX_PENDING_BLOCK_AHEAD: u64 = 512;
const MAX_BRANCH_BACKFILL_REQUEST_FAILURES: u8 = 4;

/// Highest block height any peer has revealed since this node booted.
/// Treat 0 as "no peer has announced a block yet" — IBD is then a no-op.
#[derive(Debug, Default, Clone)]
pub struct PeerChainView {
    pub perceived_max_height: u64,
    pub branch_backfill_height: Option<u64>,
    pub branch_backfill_ceiling: Option<u64>,
    pub branch_expected_hashes: BTreeMap<u64, String>,
    branch_backfill_request_failures: u8,
    pending_block_responses: BTreeMap<u64, BTreeMap<String, BlockResponseMessage>>,
}

impl PeerChainView {
    pub fn observe(&mut self, height: u64) {
        if height > self.perceived_max_height {
            self.perceived_max_height = height;
        }
    }

    pub fn record_sync_outcome(&mut self, outcome: &SyncOutcome) {
        match outcome {
            SyncOutcome::Applied(head) => {
                if self.matches_or_learns_branch_hash(head.height, &head.hash) {
                    self.advance_branch_backfill(head.height);
                }
            }
            SyncOutcome::AlreadyKnown { height, hash } => {
                if self.matches_or_learns_branch_hash(*height, hash) {
                    self.advance_branch_backfill(*height);
                }
            }
            SyncOutcome::BehindOrSibling {
                candidate,
                candidate_hash,
                ..
            } => {
                if self.matches_or_learns_branch_hash(*candidate, candidate_hash) {
                    self.advance_branch_backfill(*candidate)
                }
            }
            SyncOutcome::GapAhead { .. } => {}
            SyncOutcome::MissingParent {
                local_head,
                candidate,
                candidate_hash,
                parent_hash,
            } => {
                let continuing_expected_branch = self.branch_backfill_ceiling.is_some()
                    && self.matches_or_learns_branch_hash(*candidate, candidate_hash);
                if *candidate > *local_head || continuing_expected_branch {
                    self.backfill_before(*candidate, candidate_hash, parent_hash);
                }
            }
            SyncOutcome::BadPayload(_)
            | SyncOutcome::AnnouncedHashMismatch { .. }
            | SyncOutcome::Rejected(_) => {}
        }
    }

    pub fn remember_block_response(
        &mut self,
        outcome: &SyncOutcome,
        response: &BlockResponseMessage,
    ) {
        let (local_head, candidate, candidate_hash) = match outcome {
            SyncOutcome::GapAhead {
                local_head,
                candidate,
            } => (*local_head, *candidate, response.block_hash.as_str()),
            SyncOutcome::MissingParent {
                local_head,
                candidate,
                candidate_hash,
                ..
            } => (*local_head, *candidate, candidate_hash.as_str()),
            _ => return,
        };
        if response.block_height != candidate || response.block_hash != candidate_hash {
            return;
        }
        if candidate > local_head.saturating_add(MAX_PENDING_BLOCK_AHEAD) {
            return;
        }

        let responses_at_height = self.pending_block_responses.entry(candidate).or_default();
        if !responses_at_height.contains_key(candidate_hash)
            && responses_at_height.len() >= MAX_PENDING_BLOCKS_PER_HEIGHT
        {
            if let Some(oldest_hash) = responses_at_height.keys().next().cloned() {
                responses_at_height.remove(&oldest_hash);
            }
        }
        responses_at_height.insert(candidate_hash.to_string(), response.clone());
        self.trim_pending_block_responses();
    }

    pub fn take_pending_block_response(
        &mut self,
        request: &SyncRequest,
    ) -> Option<BlockResponseMessage> {
        let responses_at_height = self.pending_block_responses.get_mut(&request.height)?;
        let response = match request.expected_hash.as_deref() {
            Some(expected_hash) => responses_at_height.remove(expected_hash),
            None => responses_at_height
                .keys()
                .next()
                .cloned()
                .and_then(|hash| responses_at_height.remove(&hash)),
        };
        if responses_at_height.is_empty() {
            self.pending_block_responses.remove(&request.height);
        }
        response
    }

    fn pending_block_response_count(&self) -> usize {
        self.pending_block_responses
            .values()
            .map(BTreeMap::len)
            .sum()
    }

    pub fn abandon_branch_backfill(&mut self) -> bool {
        let was_active = self.branch_backfill_ceiling.is_some()
            || self.branch_backfill_height.is_some()
            || !self.branch_expected_hashes.is_empty();
        self.branch_backfill_height = None;
        self.branch_backfill_ceiling = None;
        self.branch_expected_hashes.clear();
        self.branch_backfill_request_failures = 0;
        was_active
    }

    pub fn has_active_branch_backfill(&self) -> bool {
        self.branch_backfill_ceiling.is_some()
            || self.branch_backfill_height.is_some()
            || !self.branch_expected_hashes.is_empty()
    }

    pub fn mining_pause_target(&self, local_height: u64) -> Option<u64> {
        if self.has_active_branch_backfill() {
            return self.branch_backfill_ceiling.or(self.branch_backfill_height);
        }
        if self.perceived_max_height > local_height {
            return Some(self.perceived_max_height);
        }
        None
    }

    pub fn record_branch_backfill_request_failure(&mut self) -> bool {
        if !self.has_active_branch_backfill() {
            return false;
        }
        self.branch_backfill_request_failures =
            self.branch_backfill_request_failures.saturating_add(1);
        if self.branch_backfill_request_failures >= MAX_BRANCH_BACKFILL_REQUEST_FAILURES {
            self.abandon_branch_backfill()
        } else {
            false
        }
    }

    fn matches_or_learns_branch_hash(&mut self, height: u64, hash: &str) -> bool {
        if self.branch_backfill_ceiling.is_none() {
            return true;
        }
        if let Some(expected) = self.branch_expected_hashes.get(&height) {
            return expected == hash;
        }
        if self.branch_backfill_height == Some(height) {
            self.branch_expected_hashes.insert(height, hash.to_string());
            return true;
        }
        false
    }

    fn backfill_before(&mut self, candidate_height: u64, candidate_hash: &str, parent_hash: &str) {
        if candidate_height <= 1 {
            return;
        }
        let ceiling = self
            .branch_backfill_ceiling
            .unwrap_or(candidate_height)
            .max(candidate_height);
        let next_height = candidate_height.saturating_sub(1);
        self.branch_backfill_ceiling = Some(ceiling);
        self.branch_backfill_height = Some(match self.branch_backfill_height {
            Some(current) => current.min(next_height),
            None => next_height,
        });
        self.branch_expected_hashes
            .insert(candidate_height, candidate_hash.to_string());
        self.branch_expected_hashes
            .insert(next_height, parent_hash.to_string());
        self.branch_backfill_request_failures = 0;
    }

    fn advance_branch_backfill(&mut self, applied_or_known_height: u64) {
        let Some(ceiling) = self.branch_backfill_ceiling else {
            return;
        };
        self.branch_backfill_request_failures = 0;
        if applied_or_known_height < ceiling {
            self.branch_backfill_height = Some(applied_or_known_height.saturating_add(1));
        } else if self.perceived_max_height > applied_or_known_height {
            let next_height = applied_or_known_height.saturating_add(1);
            self.branch_backfill_height = Some(next_height);
            self.branch_backfill_ceiling = Some(self.perceived_max_height);
            self.branch_expected_hashes
                .retain(|height, _| *height >= applied_or_known_height);
        } else {
            self.branch_backfill_height = None;
            self.branch_backfill_ceiling = None;
            self.branch_expected_hashes.clear();
            self.branch_backfill_request_failures = 0;
        }
    }

    fn trim_pending_block_responses(&mut self) {
        while self.pending_block_response_count() > MAX_PENDING_BLOCK_RESPONSES {
            let Some(height) = self.pending_block_responses.keys().next_back().cloned() else {
                return;
            };
            let remove_height = {
                let Some(responses_at_height) = self.pending_block_responses.get_mut(&height)
                else {
                    return;
                };
                if let Some(hash) = responses_at_height.keys().next_back().cloned() {
                    responses_at_height.remove(&hash);
                }
                responses_at_height.is_empty()
            };
            if remove_height {
                self.pending_block_responses.remove(&height);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRequest {
    pub height: u64,
    pub expected_hash: Option<String>,
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
    /// Block is at a height already covered by the local chain and was
    /// not heavy enough to replace the current canonical head.
    BehindOrSibling {
        local_head: u64,
        candidate: u64,
        candidate_hash: String,
    },
    /// The block belongs to a side branch whose parent is not in local
    /// storage yet. IBD should walk backward until it finds a known
    /// ancestor, then replay forward.
    MissingParent {
        local_head: u64,
        candidate: u64,
        candidate_hash: String,
        parent_hash: String,
    },
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
    try_apply_synced_block_payload_inner(storage, chain_id, response, None)
}

/// Same as [`try_apply_synced_block_payload`], but drives validation at an
/// explicit difficulty for unsafe local rehearsals.
pub fn try_apply_synced_block_payload_with_expected_difficulty(
    storage: &RocksDbStorage,
    chain_id: &str,
    response: &BlockResponseMessage,
    expected_difficulty_bits: u32,
) -> SyncOutcome {
    try_apply_synced_block_payload_inner(
        storage,
        chain_id,
        response,
        Some(expected_difficulty_bits),
    )
}

fn try_apply_synced_block_payload_inner(
    storage: &RocksDbStorage,
    chain_id: &str,
    response: &BlockResponseMessage,
    expected_difficulty_bits: Option<u32>,
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
    let candidate_known = storage.block_exists(&block.header.hash).unwrap_or(false);
    if candidate_height > local_height.saturating_add(1) && !candidate_known {
        return SyncOutcome::GapAhead {
            local_head: local_height,
            candidate: candidate_height,
        };
    }
    let resolved_expected_difficulty_bits = expected_difficulty_bits.or(response.difficulty_bits);
    let applied = match resolved_expected_difficulty_bits {
        Some(bits) => apply_block_with_expected_difficulty(storage, chain_id, &block, bits),
        None => apply_block(storage, chain_id, &block),
    };
    match applied {
        Ok(head) => {
            persist_available_private_tx_bodies(storage, &block, &response.private_tx_bodies);
            SyncOutcome::Applied(head)
        }
        Err(ChainError::BlockAlreadyKnown(hash)) => {
            // Persist bodies even when block is already known — this lets a
            // node that synced the block header earlier (without bodies)
            // backfill bodies the next time the same block re-circulates.
            persist_available_private_tx_bodies(storage, &block, &response.private_tx_bodies);
            SyncOutcome::AlreadyKnown {
                height: candidate_height,
                hash,
            }
        }
        Err(ChainError::SideBranchNotHeavier { .. }) => SyncOutcome::BehindOrSibling {
            local_head: local_height,
            candidate: candidate_height,
            candidate_hash: block.header.hash,
        },
        Err(ChainError::DoesNotExtendHead { .. }) => SyncOutcome::MissingParent {
            local_head: local_height,
            candidate: candidate_height,
            candidate_hash: block.header.hash,
            parent_hash: block.header.prev_hash,
        },
        Err(err) => SyncOutcome::Rejected(err.to_string()),
    }
}

pub fn next_required_block(
    storage: &RocksDbStorage,
    peer_view: &PeerChainView,
) -> Result<Option<SyncRequest>, String> {
    if let Some(height) = peer_view.branch_backfill_height {
        if height > 0 {
            return Ok(Some(SyncRequest {
                height,
                expected_hash: peer_view.branch_expected_hashes.get(&height).cloned(),
            }));
        }
    }

    let local_height = storage
        .get_chain_height()
        .map_err(|e| format!("read chain height failed: {e:?}"))?;
    if local_height >= peer_view.perceived_max_height {
        return Ok(None);
    }
    Ok(Some(SyncRequest {
        height: local_height.saturating_add(1),
        expected_hash: None,
    }))
}

/// Convenience: returns the next block height the local chain needs in
/// order to advance, or `None` when it is already caught up to or ahead
/// of `peer_view.perceived_max_height`.
pub fn next_required_height(
    storage: &RocksDbStorage,
    peer_view: &PeerChainView,
) -> Result<Option<u64>, String> {
    Ok(next_required_block(storage, peer_view)?.map(|request| request.height))
}

/// Schema-tolerant deserializer for incoming peer block payloads. Used
/// by tests and the sync helper alike.
#[allow(dead_code)]
pub(crate) fn parse_block_payload(bytes: &[u8]) -> Result<Block, String> {
    let mut de = serde_json::Deserializer::from_slice(bytes);
    Block::deserialize(&mut de).map_err(|e| e.to_string())
}

fn strict_private_tx_spend_tags(tx: &PrivateTransaction) -> Result<Vec<String>, String> {
    let primary = tx.nullifier.trim();
    if primary.is_empty() || primary != tx.nullifier.as_str() {
        return Err("private transaction has a missing or non-canonical primary nullifier".into());
    }
    if tx.input_nullifiers.is_empty() {
        return Ok(vec![primary.to_string()]);
    }

    let mut tags = Vec::with_capacity(tx.input_nullifiers.len());
    for tag in &tx.input_nullifiers {
        let trimmed = tag.trim();
        if trimmed.is_empty() || trimmed != tag {
            return Err(
                "private transaction has a missing or non-canonical input nullifier".into(),
            );
        }
        tags.push(trimmed.to_string());
    }
    if tags.first().map(String::as_str) != Some(primary) {
        return Err("private transaction primary nullifier does not match its first input".into());
    }
    Ok(tags)
}

/// Strict private bundle validation for the future height-gated block format.
///
/// Current blocks commit hashes and nullifiers but only carry bodies as
/// best-effort attachments. The height-gated policy calls this before applying
/// any future block whose bodies are canonical data.
#[allow(dead_code)]
pub(crate) fn validate_private_tx_bundle_for_block(
    block: &Block,
    bodies: &[Vec<u8>],
) -> Result<(), String> {
    validate_private_tx_bundle_for_block_at(
        block,
        bodies,
        crate::economics::private_tx_v2_i1_activation_height(),
        crate::economics::private_tx_canonical_bundle_activation_height(),
    )
}

fn validate_private_tx_bundle_for_block_at(
    block: &Block,
    bodies: &[Vec<u8>],
    v2_i1_activation_height: Option<u64>,
    canonical_bundle_activation_height: Option<u64>,
) -> Result<(), String> {
    let mut committed_hashes = HashSet::new();
    for hash in &block.private_tx_hashes {
        let trimmed = hash.trim();
        if trimmed.is_empty() || trimmed != hash {
            return Err("block has a missing or non-canonical private transaction hash".into());
        }
        if !committed_hashes.insert(trimmed.to_string()) {
            return Err(format!("block repeats private transaction hash {trimmed}"));
        }
    }
    if bodies.len() != committed_hashes.len() {
        return Err(format!(
            "private transaction body count mismatch: committed {}, received {}",
            committed_hashes.len(),
            bodies.len()
        ));
    }

    let mut committed_nullifiers = BTreeSet::new();
    for nullifier in &block.private_nullifiers {
        let trimmed = nullifier.trim();
        if trimmed.is_empty() || trimmed != nullifier {
            return Err("block has a missing or non-canonical private nullifier".into());
        }
        if !committed_nullifiers.insert(trimmed.to_string()) {
            return Err(format!("block repeats private nullifier {trimmed}"));
        }
    }

    let mut represented_hashes = HashSet::new();
    let mut derived_nullifiers = BTreeSet::new();
    let mut validator = PrivateWalletManager::new();
    for (expected_hash, body_bytes) in block.private_tx_hashes.iter().zip(bodies) {
        let tx: PrivateTransaction = serde_json::from_slice(body_bytes)
            .map_err(|err| format!("private transaction body deserialize failed: {err}"))?;
        let hash = tx.tx_hash.trim();
        if hash.is_empty() || hash != tx.tx_hash {
            return Err("private transaction body has a missing or non-canonical hash".into());
        }
        if hash != expected_hash {
            return Err(format!(
                "private transaction body hash {hash} does not match committed position {expected_hash}"
            ));
        }
        if !represented_hashes.insert(hash.to_string()) {
            return Err(format!(
                "private transaction hash {hash} has more than one body"
            ));
        }

        validator
            .validate_inbound_private_transaction_at(
                &tx,
                block.header.block_height,
                v2_i1_activation_height,
                canonical_bundle_activation_height,
            )
            .map_err(|err| format!("private transaction {hash} failed validation: {err}"))?;
        for nullifier in strict_private_tx_spend_tags(&tx)? {
            if !derived_nullifiers.insert(nullifier.clone()) {
                return Err(format!(
                    "private transaction bundle repeats nullifier {nullifier}"
                ));
            }
        }
    }

    if represented_hashes != committed_hashes {
        return Err("not every committed private transaction hash has exactly one body".into());
    }
    if derived_nullifiers != committed_nullifiers {
        return Err(format!(
            "private nullifier set mismatch: derived {}, committed {}",
            derived_nullifiers.len(),
            committed_nullifiers.len()
        ));
    }
    Ok(())
}

pub(crate) fn validate_private_tx_bundle_policy_for_block(block: &Block) -> Result<(), String> {
    validate_private_tx_bundle_policy_for_block_at(
        block,
        crate::economics::private_tx_canonical_bundle_activation_height(),
    )
}

fn validate_private_tx_bundle_policy_for_block_at(
    block: &Block,
    activation_height: Option<u64>,
) -> Result<(), String> {
    if crate::economics::private_tx_canonical_bundle_required_at(
        block.header.block_height,
        activation_height,
    ) {
        validate_private_tx_bundle_for_block(block, &block.private_tx_bodies)
    } else if block.private_tx_bodies.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "canonical private transaction bodies are not active at height {}",
            block.header.block_height
        ))
    }
}

/// Persist private transaction bodies that arrived attached to a block
/// payload. Each body's `tx_hash` is cross-checked against the block's
/// `private_tx_hashes` list — bodies that don't correspond to a committed
/// hash are silently dropped (a malicious peer cannot inject arbitrary
/// bodies). Already-stored bodies are not overwritten by this path.
///
/// Failures are logged but never abort the caller: persisting a body is
/// best-effort, the block apply already succeeded.
fn persist_attached_private_tx_bodies(storage: &RocksDbStorage, block: &Block, bodies: &[Vec<u8>]) {
    if bodies.is_empty() || block.private_tx_hashes.is_empty() {
        return;
    }
    let committed: std::collections::HashSet<&str> =
        block.private_tx_hashes.iter().map(|s| s.as_str()).collect();
    for body_bytes in bodies {
        let tx: crate::private_wallet::PrivateTransaction = match serde_json::from_slice(body_bytes)
        {
            Ok(tx) => tx,
            Err(err) => {
                tracing::debug!("ignoring attached private tx body: deserialize failed: {err}");
                continue;
            }
        };
        let hash = tx.tx_hash.trim();
        if hash.is_empty() || !committed.contains(hash) {
            tracing::debug!(
                "ignoring attached private tx body: hash {} not committed in block {}",
                hash,
                block.header.hash
            );
            continue;
        }
        // Skip if we already have a body for this hash (avoid clobbering
        // a copy we trust with a peer-supplied one).
        if matches!(storage.get_private_transaction_bytes(hash), Ok(Some(_))) {
            continue;
        }
        if let Err(err) = storage.save_private_transaction_bytes(hash, body_bytes) {
            tracing::warn!("failed to persist attached private tx body {hash}: {err}");
        }
    }
}

fn persist_available_private_tx_bodies(
    storage: &RocksDbStorage,
    block: &Block,
    attached_bodies: &[Vec<u8>],
) {
    persist_available_private_tx_bodies_at(
        storage,
        block,
        attached_bodies,
        crate::economics::private_tx_canonical_bundle_activation_height(),
    );
}

fn persist_available_private_tx_bodies_at(
    storage: &RocksDbStorage,
    block: &Block,
    attached_bodies: &[Vec<u8>],
    activation_height: Option<u64>,
) {
    if !block.private_tx_bodies.is_empty() {
        match validate_private_tx_bundle_policy_for_block_at(block, activation_height) {
            Ok(()) => persist_attached_private_tx_bodies(storage, block, &block.private_tx_bodies),
            Err(err) => tracing::warn!(
                "refusing to persist invalid canonical private tx bodies for block {}: {err}",
                block.header.hash
            ),
        }
    }
    persist_attached_private_tx_bodies(storage, block, attached_bodies);
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
            private_tx_bodies: Vec::new(),
            private_nullifiers: Vec::new(),
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
            difficulty_bits: None,
            private_tx_bodies: Vec::new(),
            timestamp: block.header.timestamp,
        }
    }

    fn block_with_private_bundle(
        private_tx_hashes: Vec<String>,
        private_nullifiers: Vec<String>,
    ) -> Block {
        let chain_id = "zoka-test-net";
        let mut block = mine_test_block(
            chain_id,
            &genesis_parent_for(chain_id).header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_000,
        );
        block.private_tx_hashes = private_tx_hashes;
        block.private_nullifiers = private_nullifiers;
        block.header.merkle_root = block.compute_merkle_root();
        block.header.hash = block.compute_hash();
        block
    }

    #[test]
    fn strict_private_bundle_rejects_missing_body() {
        let block = block_with_private_bundle(
            vec!["committed-private-tx".to_string()],
            vec!["committed-nullifier".to_string()],
        );

        let err = validate_private_tx_bundle_for_block(&block, &[])
            .expect_err("strict bundle must require one body per committed hash");

        assert!(err.contains("body count mismatch"));
    }

    #[test]
    fn strict_private_bundle_rejects_duplicate_committed_hash() {
        let block = block_with_private_bundle(
            vec![
                "committed-private-tx".to_string(),
                "committed-private-tx".to_string(),
            ],
            Vec::new(),
        );

        let err = validate_private_tx_bundle_for_block(&block, &[])
            .expect_err("strict bundle must reject duplicate committed hashes");

        assert!(err.contains("repeats private transaction hash"));
    }

    #[test]
    fn strict_private_bundle_rejects_duplicate_committed_nullifier() {
        let block = block_with_private_bundle(
            Vec::new(),
            vec![
                "committed-nullifier".to_string(),
                "committed-nullifier".to_string(),
            ],
        );

        let err = validate_private_tx_bundle_for_block(&block, &[])
            .expect_err("strict bundle must reject duplicate committed nullifiers");

        assert!(err.contains("repeats private nullifier"));
    }

    #[test]
    fn strict_private_bundle_rejects_dormant_v2_i1_profile() {
        let block = block_with_private_bundle(
            vec!["private-v2-disabled".to_string()],
            vec!["nullifier-v2-disabled".to_string()],
        );
        let body = serde_json::to_vec(&serde_json::json!({
            "tx_hash": "private-v2-disabled",
            "commitment": "commitment-v2-disabled",
            "private_proof_version": 2,
            "private_proof_profile": "private_tx_v2_i1",
            "nullifier": "nullifier-v2-disabled",
            "zk_proof": null,
            "timestamp": 1,
            "status": "Pending"
        }))
        .expect("serialize private tx fixture");

        let err = validate_private_tx_bundle_for_block(&block, &[body])
            .expect_err("strict bundle must reject profiles before activation");

        assert!(err.contains("not active at height"));
    }

    #[test]
    fn strict_private_bundle_uses_block_height_for_configured_v2_i1_profile() {
        let block = block_with_private_bundle(
            vec!["private-v2-configured".to_string()],
            vec!["nullifier-v2-configured".to_string()],
        );
        let body = serde_json::to_vec(&serde_json::json!({
            "tx_hash": "private-v2-configured",
            "commitment": "commitment-v2-configured",
            "private_proof_version": 2,
            "private_proof_profile": "private_tx_v2_i1",
            "nullifier": "nullifier-v2-configured",
            "zk_proof": null,
            "timestamp": 1,
            "status": "Pending"
        }))
        .expect("serialize private tx fixture");

        let err = validate_private_tx_bundle_for_block_at(&block, &[body], Some(0), Some(0))
            .expect_err("fixture remains invalid after profile activation");

        assert!(err.contains("hash does not match body"));
        assert!(!err.contains("not active"));
    }

    #[test]
    fn strict_private_bundle_rejects_body_order_mismatch() {
        let block = block_with_private_bundle(
            vec!["private-tx-a".to_string(), "private-tx-b".to_string()],
            vec!["nullifier-a".to_string(), "nullifier-b".to_string()],
        );
        let body_a = serde_json::to_vec(&serde_json::json!({
            "tx_hash": "private-tx-a",
            "commitment": "commitment-a",
            "nullifier": "nullifier-a",
            "zk_proof": null,
            "timestamp": 1,
            "status": "Pending"
        }))
        .expect("serialize private tx fixture");
        let body_b = serde_json::to_vec(&serde_json::json!({
            "tx_hash": "private-tx-b",
            "commitment": "commitment-b",
            "nullifier": "nullifier-b",
            "zk_proof": null,
            "timestamp": 1,
            "status": "Pending"
        }))
        .expect("serialize private tx fixture");

        let err = validate_private_tx_bundle_for_block(&block, &[body_b, body_a])
            .expect_err("strict bundle must preserve hash/body order");

        assert!(err.contains("does not match committed position"));
    }

    #[test]
    fn canonical_private_bundle_policy_rejects_bodies_before_activation() {
        let mut block = block_with_private_bundle(Vec::new(), Vec::new());
        block.private_tx_bodies = vec![b"unexpected-body".to_vec()];

        let err = validate_private_tx_bundle_policy_for_block_at(&block, None)
            .expect_err("dormant policy must reject canonical bodies");

        assert!(err.contains("not active"));
    }

    #[test]
    fn strict_private_bundle_accepts_valid_tx_and_rejects_nullifier_mismatch() {
        let mut manager = PrivateWalletManager::new();
        manager
            .create_private_wallet("alice")
            .expect("alice wallet");
        let bob = manager.create_private_wallet("bob").expect("bob wallet");
        manager
            .deposit_to_private_wallet("alice", 30)
            .expect("fund alice");
        let tx_hash = manager
            .send_private_transaction("alice", bob, 10)
            .expect("build private tx");
        let tx = manager
            .get_private_transaction(&tx_hash)
            .expect("private tx");
        let nullifiers = strict_private_tx_spend_tags(&tx).expect("derived nullifiers");
        let body = serde_json::to_vec(&tx).expect("serialize private tx");
        let block = block_with_private_bundle(vec![tx_hash], nullifiers);

        validate_private_tx_bundle_for_block(&block, std::slice::from_ref(&body))
            .expect("valid strict private bundle");

        let mut canonical = block.clone();
        canonical.private_tx_bodies = vec![body.clone()];
        validate_private_tx_bundle_policy_for_block_at(
            &canonical,
            Some(canonical.header.block_height),
        )
        .expect("activated policy accepts valid canonical private bundle");
        let (storage, _dir) = open_temp_storage();
        persist_available_private_tx_bodies_at(
            &storage,
            &canonical,
            &[],
            Some(canonical.header.block_height),
        );
        assert_eq!(
            storage
                .get_private_transaction_bytes(&canonical.private_tx_hashes[0])
                .expect("read persisted canonical body"),
            Some(body.clone())
        );

        let mut mismatched = block.clone();
        mismatched.private_nullifiers = vec!["different-nullifier".to_string()];
        let err = validate_private_tx_bundle_for_block(&mismatched, &[body])
            .expect_err("strict bundle must derive the committed nullifier set");

        assert!(err.contains("private nullifier set mismatch"));

        let mut tampered_tx = tx;
        tampered_tx.outputs[0].encrypted_amount[0] ^= 1;
        let tampered_body = serde_json::to_vec(&tampered_tx).expect("serialize tampered tx");
        let err = validate_private_tx_bundle_for_block(&block, &[tampered_body])
            .expect_err("strict bundle must reject body drift");

        assert!(err.contains("hash does not match body"));
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
            difficulty_bits: None,
            private_tx_bodies: Vec::new(),
            timestamp: 0,
        };
        match try_apply_synced_block_payload_with_expected_difficulty(
            &storage, chain_id, &response, 4,
        ) {
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
        match try_apply_synced_block_payload_with_expected_difficulty(
            &storage, chain_id, &response, 4,
        ) {
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
    fn try_apply_known_body_without_parent_requests_branch_backfill() {
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

        let orphan = mine_test_block(
            chain_id,
            "missing-parent-hash",
            1,
            economics::block_reward_for_height(1, 0),
            &"zka1miner".to_string(),
            4,
            1_700_000_120,
        );
        storage
            .save_block_body(&orphan)
            .expect("cache orphan body by hash");

        let response = response_for(&orphan);
        match try_apply_synced_block_payload_with_expected_difficulty(
            &storage, chain_id, &response, 4,
        ) {
            SyncOutcome::MissingParent {
                local_head,
                candidate,
                candidate_hash,
                parent_hash,
            } => {
                assert_eq!(local_head, 1);
                assert_eq!(candidate, 2);
                assert_eq!(candidate_hash, orphan.header.hash);
                assert_eq!(parent_hash, "missing-parent-hash");
            }
            other => panic!("expected MissingParent, got: {other:?}"),
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
        match try_apply_synced_block_payload_with_expected_difficulty(
            &storage, chain_id, &response, 4,
        ) {
            SyncOutcome::BehindOrSibling {
                local_head,
                candidate,
                candidate_hash,
            } => {
                assert_eq!(local_head, 2);
                assert_eq!(candidate, 1);
                assert_eq!(candidate_hash, sibling.header.hash);
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

    #[test]
    fn next_required_height_prefers_branch_backfill() {
        let (storage, _dir) = open_temp_storage();
        let mut view = PeerChainView::default();
        view.observe(20);
        view.branch_backfill_height = Some(7);
        view.branch_backfill_ceiling = Some(12);
        view.branch_expected_hashes
            .insert(7, "branch-h7".to_string());

        let next = next_required_height(&storage, &view).expect("query");
        assert_eq!(next, Some(7));
        let request = next_required_block(&storage, &view)
            .expect("request")
            .expect("some request");
        assert_eq!(request.height, 7);
        assert_eq!(request.expected_hash.as_deref(), Some("branch-h7"));
    }

    #[test]
    fn peer_chain_view_backfills_then_replays_to_ceiling() {
        let mut view = PeerChainView::default();
        view.record_sync_outcome(&SyncOutcome::MissingParent {
            local_head: 11,
            candidate: 12,
            candidate_hash: "h12".to_string(),
            parent_hash: "h11".to_string(),
        });
        assert_eq!(view.branch_backfill_height, Some(11));
        assert_eq!(view.branch_backfill_ceiling, Some(12));
        assert_eq!(
            view.branch_expected_hashes.get(&11).map(String::as_str),
            Some("h11")
        );

        view.record_sync_outcome(&SyncOutcome::MissingParent {
            local_head: 11,
            candidate: 11,
            candidate_hash: "h11".to_string(),
            parent_hash: "h10".to_string(),
        });
        assert_eq!(view.branch_backfill_height, Some(10));
        assert_eq!(view.branch_backfill_ceiling, Some(12));
        assert_eq!(
            view.branch_expected_hashes.get(&10).map(String::as_str),
            Some("h10")
        );

        view.record_sync_outcome(&SyncOutcome::MissingParent {
            local_head: 11,
            candidate: 12,
            candidate_hash: "h12".to_string(),
            parent_hash: "h11".to_string(),
        });
        assert_eq!(view.branch_backfill_height, Some(10));
        assert_eq!(view.branch_backfill_ceiling, Some(12));

        view.record_sync_outcome(&SyncOutcome::BehindOrSibling {
            local_head: 11,
            candidate: 10,
            candidate_hash: "h10".to_string(),
        });
        assert_eq!(view.branch_backfill_height, Some(11));

        view.record_sync_outcome(&SyncOutcome::BehindOrSibling {
            local_head: 11,
            candidate: 11,
            candidate_hash: "h11".to_string(),
        });
        assert_eq!(view.branch_backfill_height, Some(12));

        view.record_sync_outcome(&SyncOutcome::AlreadyKnown {
            height: 12,
            hash: "h12".to_string(),
        });
        assert_eq!(view.branch_backfill_height, None);
        assert_eq!(view.branch_backfill_ceiling, None);
        assert!(view.branch_expected_hashes.is_empty());
    }

    #[test]
    fn peer_chain_view_does_not_backfill_simple_gap_ahead() {
        let mut view = PeerChainView::default();
        view.record_sync_outcome(&SyncOutcome::GapAhead {
            local_head: 0,
            candidate: 5,
        });

        assert_eq!(view.branch_backfill_height, None);
        assert_eq!(view.branch_backfill_ceiling, None);
    }

    #[test]
    fn peer_chain_view_caches_gap_ahead_response_for_later_ibd() {
        let mut view = PeerChainView::default();
        let response = BlockResponseMessage {
            block_height: 5,
            encrypted_block: b"cached-block".to_vec(),
            block_hash: "h5".to_string(),
            difficulty_bits: None,
            private_tx_bodies: Vec::new(),
            timestamp: 1_700_000_000,
        };
        let outcome = SyncOutcome::GapAhead {
            local_head: 2,
            candidate: 5,
        };

        view.remember_block_response(&outcome, &response);
        assert_eq!(view.pending_block_response_count(), 1);

        let replayed = view
            .take_pending_block_response(&SyncRequest {
                height: 5,
                expected_hash: Some("h5".to_string()),
            })
            .expect("cached response");
        assert_eq!(replayed.block_hash, "h5");
        assert_eq!(replayed.encrypted_block, b"cached-block".to_vec());
        assert_eq!(view.pending_block_response_count(), 0);
    }

    #[test]
    fn peer_chain_view_does_not_cache_unbounded_future_responses() {
        let mut view = PeerChainView::default();
        let future_height = MAX_PENDING_BLOCK_AHEAD.saturating_add(10);
        let response = BlockResponseMessage {
            block_height: future_height,
            encrypted_block: b"far-future-block".to_vec(),
            block_hash: "too-far".to_string(),
            difficulty_bits: None,
            private_tx_bodies: Vec::new(),
            timestamp: 1_700_000_000,
        };
        let outcome = SyncOutcome::GapAhead {
            local_head: 0,
            candidate: future_height,
        };

        view.remember_block_response(&outcome, &response);
        assert_eq!(view.pending_block_response_count(), 0);
    }

    #[test]
    fn peer_chain_view_ignores_stale_missing_parent_without_active_backfill() {
        let mut view = PeerChainView::default();
        view.record_sync_outcome(&SyncOutcome::MissingParent {
            local_head: 5,
            candidate: 4,
            candidate_hash: "old-side-4".to_string(),
            parent_hash: "old-side-3".to_string(),
        });

        assert_eq!(view.branch_backfill_height, None);
        assert_eq!(view.branch_backfill_ceiling, None);
        assert!(view.branch_expected_hashes.is_empty());
    }

    #[test]
    fn peer_chain_view_ignores_unrelated_known_blocks_while_backfilling() {
        let mut view = PeerChainView::default();
        view.record_sync_outcome(&SyncOutcome::MissingParent {
            local_head: 16,
            candidate: 17,
            candidate_hash: "winner-17".to_string(),
            parent_hash: "winner-16".to_string(),
        });

        view.record_sync_outcome(&SyncOutcome::AlreadyKnown {
            height: 17,
            hash: "local-17".to_string(),
        });

        assert_eq!(view.branch_backfill_height, Some(16));
        assert_eq!(view.branch_backfill_ceiling, Some(17));
    }

    #[test]
    fn peer_chain_view_can_abandon_unavailable_branch_backfill() {
        let mut view = PeerChainView::default();
        view.record_sync_outcome(&SyncOutcome::MissingParent {
            local_head: 6,
            candidate: 7,
            candidate_hash: "gone-7".to_string(),
            parent_hash: "gone-6".to_string(),
        });

        assert!(view.abandon_branch_backfill());
        assert_eq!(view.branch_backfill_height, None);
        assert_eq!(view.branch_backfill_ceiling, None);
        assert!(view.branch_expected_hashes.is_empty());
        assert!(!view.abandon_branch_backfill());
    }

    #[test]
    fn peer_chain_view_keeps_branch_backfill_across_transient_request_misses() {
        let mut view = PeerChainView::default();
        view.record_sync_outcome(&SyncOutcome::MissingParent {
            local_head: 6,
            candidate: 7,
            candidate_hash: "winner-7".to_string(),
            parent_hash: "winner-6".to_string(),
        });

        for _ in 1..MAX_BRANCH_BACKFILL_REQUEST_FAILURES {
            assert!(!view.record_branch_backfill_request_failure());
            assert_eq!(view.branch_backfill_height, Some(6));
            assert_eq!(view.branch_backfill_ceiling, Some(7));
        }

        assert!(view.record_branch_backfill_request_failure());
        assert_eq!(view.branch_backfill_height, None);
        assert_eq!(view.branch_backfill_ceiling, None);
        assert!(view.branch_expected_hashes.is_empty());
    }

    #[test]
    fn peer_chain_view_pauses_mining_when_behind_or_backfilling() {
        let mut view = PeerChainView::default();
        assert_eq!(view.mining_pause_target(3), None);

        view.observe(8);
        assert_eq!(view.mining_pause_target(3), Some(8));
        assert_eq!(view.mining_pause_target(8), None);

        view.record_sync_outcome(&SyncOutcome::MissingParent {
            local_head: 8,
            candidate: 9,
            candidate_hash: "winner-9".to_string(),
            parent_hash: "winner-8".to_string(),
        });
        assert_eq!(view.mining_pause_target(9), Some(9));
    }

    #[test]
    fn peer_chain_view_continues_forward_side_branch_until_peer_tip() {
        let mut view = PeerChainView::default();
        view.observe(6);
        view.record_sync_outcome(&SyncOutcome::MissingParent {
            local_head: 1,
            candidate: 2,
            candidate_hash: "peer-2".to_string(),
            parent_hash: "peer-1".to_string(),
        });
        assert_eq!(view.branch_backfill_height, Some(1));
        assert_eq!(view.branch_backfill_ceiling, Some(2));

        view.record_sync_outcome(&SyncOutcome::BehindOrSibling {
            local_head: 1,
            candidate: 1,
            candidate_hash: "peer-1".to_string(),
        });
        assert_eq!(view.branch_backfill_height, Some(2));

        view.record_sync_outcome(&SyncOutcome::BehindOrSibling {
            local_head: 1,
            candidate: 2,
            candidate_hash: "peer-2".to_string(),
        });
        assert_eq!(view.branch_backfill_height, Some(3));
        assert_eq!(view.branch_backfill_ceiling, Some(6));
        assert!(!view.branch_expected_hashes.contains_key(&3));

        view.record_sync_outcome(&SyncOutcome::BehindOrSibling {
            local_head: 1,
            candidate: 3,
            candidate_hash: "peer-3".to_string(),
        });
        assert_eq!(view.branch_backfill_height, Some(4));
        assert_eq!(
            view.branch_expected_hashes.get(&3).map(String::as_str),
            Some("peer-3")
        );
    }
}
