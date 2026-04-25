use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::types::{Block, BlockError};

const DEFAULT_VIEW_CHANGE_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_CHECKPOINT_INTERVAL: u64 = 100; // checkpoint every 100 blocks

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PbftCheckpoint {
    /// Sequence number where checkpoint was created
    pub sequence: u64,
    /// State root hash at this checkpoint
    pub state_root: String,
    /// Certifying replicas (at least 2f+1)
    pub validators: Vec<String>,
    /// Timestamp in milliseconds
    pub timestamp_ms: u64,
}

/// Persistent consensus state for recovery
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConsensusState {
    pub node_id: String,
    pub current_view: u64,
    pub last_stable_checkpoint: u64,
    pub last_progress_tick_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PbftMessageType {
    PrePrepare,
    Prepare,
    Commit,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PbftMessage {
    pub msg_type: PbftMessageType,
    pub view: u64,
    pub sequence: u64,
    pub block_hash: String,
    pub sender: String,
    pub timestamp_ms: u64,
}

impl PbftMessage {
    pub fn new(
        msg_type: PbftMessageType,
        view: u64,
        sequence: u64,
        block_hash: String,
        sender: String,
    ) -> Self {
        Self {
            msg_type,
            view,
            sequence,
            block_hash,
            sender,
            timestamp_ms: 0,
        }
    }

    pub fn with_timestamp(
        msg_type: PbftMessageType,
        view: u64,
        sequence: u64,
        block_hash: String,
        sender: String,
        timestamp_ms: u64,
    ) -> Self {
        Self {
            msg_type,
            view,
            sequence,
            block_hash,
            sender,
            timestamp_ms,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PbftViewChange {
    pub new_view: u64,
    pub sender: String,
    pub timestamp_ms: u64,
}

impl PbftViewChange {
    pub fn new(new_view: u64, sender: String, timestamp_ms: u64) -> Self {
        Self {
            new_view,
            sender,
            timestamp_ms,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PbftNewView {
    pub new_view: u64,
    pub sender: String,
    pub supporting_validators: Vec<String>,
    pub timestamp_ms: u64,
}

impl PbftNewView {
    pub fn new(
        new_view: u64,
        sender: String,
        supporting_validators: Vec<String>,
        timestamp_ms: u64,
    ) -> Self {
        Self {
            new_view,
            sender,
            supporting_validators,
            timestamp_ms,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PbftEvent {
    Prepared {
        view: u64,
        sequence: u64,
        block_hash: String,
    },
    Committed {
        view: u64,
        sequence: u64,
        block_hash: String,
    },
    ViewChanged {
        new_view: u64,
        primary: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ConsensusKey {
    view: u64,
    sequence: u64,
    block_hash: String,
}

impl ConsensusKey {
    fn new(view: u64, sequence: u64, block_hash: String) -> Self {
        Self {
            view,
            sequence,
            block_hash,
        }
    }
}

/// Minimal PBFT engine for deterministic testing and integration.
///
/// Design notes:
/// - Enforces primary-only pre-prepare by view.
/// - Tracks prepare/commit quorums with unique sender votes.
/// - Adds timeout-driven view changes and recovery path with new-view messages.
/// - Uses static membership and deterministic primary rotation.
/// - Supports checkpoints every N blocks for optimization.
/// - Implements garbage collection to clean old state.
pub struct PbftConsensus {
    node_id: String,
    validators: Vec<String>,
    validator_set: HashSet<String>,
    f: usize,
    current_view: u64,
    view_change_timeout_ms: u64,
    last_progress_tick_ms: u64,
    pre_prepared: HashMap<(u64, u64), String>,
    prepares: HashMap<ConsensusKey, HashSet<String>>,
    commits: HashMap<ConsensusKey, HashSet<String>>,
    committed: HashSet<ConsensusKey>,
    view_change_votes: HashMap<u64, HashSet<String>>,
    local_view_change_sent: HashSet<u64>,
    /// Checkpoints: map from sequence to state root
    checkpoints: HashMap<u64, PbftCheckpoint>,
    /// Last stable checkpoint sequence
    last_stable_checkpoint: u64,
    /// Checkpoint interval (blocks between checkpoints)
    checkpoint_interval: u64,
}

impl PbftConsensus {
    pub fn new(node_id: String, validators: Vec<String>) -> Result<Self, BlockError> {
        Self::with_timeout(node_id, validators, DEFAULT_VIEW_CHANGE_TIMEOUT_MS)
    }

    pub fn with_timeout(
        node_id: String,
        validators: Vec<String>,
        view_change_timeout_ms: u64,
    ) -> Result<Self, BlockError> {
        if validators.len() < 4 {
            return Err(BlockError::TransactionValidationFailed(
                "PBFT requires at least 4 validators".to_string(),
            ));
        }
        if !validators.iter().any(|id| id == &node_id) {
            return Err(BlockError::UnauthorizedValidator);
        }

        let validator_set: HashSet<String> = validators.iter().cloned().collect();
        let f = (validators.len().saturating_sub(1)) / 3;

        Ok(Self {
            node_id,
            validators,
            validator_set,
            f,
            current_view: 0,
            view_change_timeout_ms,
            last_progress_tick_ms: 0,
            pre_prepared: HashMap::new(),
            prepares: HashMap::new(),
            commits: HashMap::new(),
            committed: HashSet::new(),
            view_change_votes: HashMap::new(),
            local_view_change_sent: HashSet::new(),
            checkpoints: HashMap::new(),
            last_stable_checkpoint: 0,
            checkpoint_interval: DEFAULT_CHECKPOINT_INTERVAL,
        })
    }

    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    pub fn current_view(&self) -> u64 {
        self.current_view
    }

    pub fn timeout_ms(&self) -> u64 {
        self.view_change_timeout_ms
    }

    pub fn fault_tolerance(&self) -> usize {
        self.f
    }

    pub fn prepare_quorum(&self) -> usize {
        2 * self.f
    }

    pub fn commit_quorum(&self) -> usize {
        2 * self.f + 1
    }

    pub fn validators(&self) -> &[String] {
        &self.validators
    }

    pub fn primary_for_view(&self, view: u64) -> Option<&str> {
        self.validators
            .get((view as usize) % self.validators.len())
            .map(String::as_str)
    }

    pub fn is_primary(&self, view: u64) -> bool {
        self.primary_for_view(view) == Some(self.node_id())
    }

    pub fn make_preprepare(
        &self,
        view: u64,
        sequence: u64,
        block: &Block,
    ) -> Result<PbftMessage, BlockError> {
        if view != self.current_view {
            return Err(BlockError::TransactionValidationFailed(
                "Cannot create pre-prepare for non-current view".to_string(),
            ));
        }
        if !self.is_primary(view) {
            return Err(BlockError::UnauthorizedValidator);
        }
        Ok(PbftMessage::new(
            PbftMessageType::PrePrepare,
            view,
            sequence,
            block.header.hash.clone(),
            self.node_id.clone(),
        ))
    }

    pub fn make_prepare(&self, view: u64, sequence: u64, block_hash: String) -> PbftMessage {
        PbftMessage::new(
            PbftMessageType::Prepare,
            view,
            sequence,
            block_hash,
            self.node_id.clone(),
        )
    }

    pub fn make_commit(&self, view: u64, sequence: u64, block_hash: String) -> PbftMessage {
        PbftMessage::new(
            PbftMessageType::Commit,
            view,
            sequence,
            block_hash,
            self.node_id.clone(),
        )
    }

    pub fn on_timeout(&mut self, now_ms: u64) -> Option<PbftViewChange> {
        if now_ms.saturating_sub(self.last_progress_tick_ms) < self.view_change_timeout_ms {
            return None;
        }

        let new_view = self.current_view + 1;
        if !self.local_view_change_sent.insert(new_view) {
            return None;
        }

        self.view_change_votes
            .entry(new_view)
            .or_default()
            .insert(self.node_id.clone());

        Some(PbftViewChange::new(new_view, self.node_id.clone(), now_ms))
    }

    pub fn process_view_change(
        &mut self,
        view_change: PbftViewChange,
    ) -> Result<Option<PbftEvent>, BlockError> {
        if !self.validator_set.contains(&view_change.sender) {
            return Err(BlockError::UnauthorizedValidator);
        }
        if view_change.new_view <= self.current_view {
            return Ok(None);
        }

        self.view_change_votes
            .entry(view_change.new_view)
            .or_default()
            .insert(view_change.sender);

        Ok(self.try_enter_view_from_votes(view_change.new_view, view_change.timestamp_ms))
    }

    pub fn make_new_view(
        &self,
        new_view: u64,
        supporting_validators: Vec<String>,
        timestamp_ms: u64,
    ) -> Result<PbftNewView, BlockError> {
        if self.primary_for_view(new_view) != Some(self.node_id()) {
            return Err(BlockError::UnauthorizedValidator);
        }
        Ok(PbftNewView::new(
            new_view,
            self.node_id.clone(),
            supporting_validators,
            timestamp_ms,
        ))
    }

    pub fn process_new_view(
        &mut self,
        new_view: PbftNewView,
    ) -> Result<Option<PbftEvent>, BlockError> {
        if !self.validator_set.contains(&new_view.sender) {
            return Err(BlockError::UnauthorizedValidator);
        }
        if self.primary_for_view(new_view.new_view) != Some(new_view.sender.as_str()) {
            return Err(BlockError::UnauthorizedValidator);
        }
        if new_view.new_view < self.current_view {
            return Ok(None);
        }

        let unique_supporters: HashSet<&String> = new_view
            .supporting_validators
            .iter()
            .filter(|validator| self.validator_set.contains(*validator))
            .collect();

        if unique_supporters.len() < self.commit_quorum() {
            return Err(BlockError::TransactionValidationFailed(
                "Insufficient view-change certificates for new-view".to_string(),
            ));
        }

        self.enter_view(new_view.new_view, new_view.timestamp_ms);
        let primary = self
            .primary_for_view(new_view.new_view)
            .unwrap_or_default()
            .to_string();
        Ok(Some(PbftEvent::ViewChanged {
            new_view: new_view.new_view,
            primary,
        }))
    }

    pub fn process_message(&mut self, msg: PbftMessage) -> Result<Option<PbftEvent>, BlockError> {
        self.validate_sender_and_hash(&msg)?;

        match msg.msg_type {
            PbftMessageType::PrePrepare => self.process_preprepare(msg),
            PbftMessageType::Prepare => self.process_prepare(msg),
            PbftMessageType::Commit => self.process_commit(msg),
        }
    }

    pub fn is_prepared(&self, view: u64, sequence: u64, block_hash: &str) -> bool {
        let key = ConsensusKey::new(view, sequence, block_hash.to_string());
        self.pre_prepared
            .get(&(view, sequence))
            .is_some_and(|hash| hash == block_hash)
            && self
                .prepares
                .get(&key)
                .is_some_and(|votes| votes.len() >= self.prepare_quorum())
    }

    pub fn is_committed(&self, view: u64, sequence: u64, block_hash: &str) -> bool {
        let key = ConsensusKey::new(view, sequence, block_hash.to_string());
        self.committed.contains(&key)
    }

    fn validate_sender_and_hash(&self, msg: &PbftMessage) -> Result<(), BlockError> {
        if msg.block_hash.is_empty() {
            return Err(BlockError::InvalidBlockHash);
        }
        if !self.validator_set.contains(&msg.sender) {
            return Err(BlockError::UnauthorizedValidator);
        }
        if msg.view != self.current_view {
            return Err(BlockError::TransactionValidationFailed(format!(
                "Message for stale/future view: current={}, message={}",
                self.current_view, msg.view
            )));
        }
        Ok(())
    }

    fn process_preprepare(&mut self, msg: PbftMessage) -> Result<Option<PbftEvent>, BlockError> {
        let expected_primary = self
            .primary_for_view(msg.view)
            .ok_or(BlockError::UnauthorizedValidator)?;
        if msg.sender != expected_primary {
            return Err(BlockError::UnauthorizedValidator);
        }

        let slot = (msg.view, msg.sequence);
        if let Some(existing_hash) = self.pre_prepared.get(&slot) {
            if existing_hash != &msg.block_hash {
                return Err(BlockError::TransactionValidationFailed(
                    "Conflicting pre-prepare detected".to_string(),
                ));
            }
            self.note_progress(msg.timestamp_ms);
            return Ok(None);
        }

        self.pre_prepared.insert(slot, msg.block_hash);
        self.note_progress(msg.timestamp_ms);
        Ok(None)
    }

    fn process_prepare(&mut self, msg: PbftMessage) -> Result<Option<PbftEvent>, BlockError> {
        if self
            .pre_prepared
            .get(&(msg.view, msg.sequence))
            .is_none_or(|hash| hash != &msg.block_hash)
        {
            return Err(BlockError::TransactionValidationFailed(
                "Prepare without matching pre-prepare".to_string(),
            ));
        }

        let key = ConsensusKey::new(msg.view, msg.sequence, msg.block_hash.clone());
        let vote_count = {
            let votes = self.prepares.entry(key.clone()).or_default();
            votes.insert(msg.sender);
            votes.len()
        };

        self.note_progress(msg.timestamp_ms);

        if vote_count >= self.prepare_quorum() {
            return Ok(Some(PbftEvent::Prepared {
                view: key.view,
                sequence: key.sequence,
                block_hash: key.block_hash,
            }));
        }

        Ok(None)
    }

    fn process_commit(&mut self, msg: PbftMessage) -> Result<Option<PbftEvent>, BlockError> {
        let key = ConsensusKey::new(msg.view, msg.sequence, msg.block_hash.clone());

        if !self.is_prepared(key.view, key.sequence, &key.block_hash) {
            return Err(BlockError::TransactionValidationFailed(
                "Commit received before prepared state".to_string(),
            ));
        }

        let vote_count = {
            let votes = self.commits.entry(key.clone()).or_default();
            votes.insert(msg.sender);
            votes.len()
        };

        self.note_progress(msg.timestamp_ms);

        if vote_count >= self.commit_quorum() && self.committed.insert(key.clone()) {
            return Ok(Some(PbftEvent::Committed {
                view: key.view,
                sequence: key.sequence,
                block_hash: key.block_hash,
            }));
        }

        Ok(None)
    }

    fn try_enter_view_from_votes(&mut self, new_view: u64, now_ms: u64) -> Option<PbftEvent> {
        let reached_quorum = self
            .view_change_votes
            .get(&new_view)
            .is_some_and(|votes| votes.len() >= self.commit_quorum());

        if reached_quorum && new_view > self.current_view {
            self.enter_view(new_view, now_ms);
            let primary = self
                .primary_for_view(new_view)
                .unwrap_or_default()
                .to_string();
            return Some(PbftEvent::ViewChanged { new_view, primary });
        }

        None
    }

    fn enter_view(&mut self, new_view: u64, now_ms: u64) {
        self.current_view = new_view;
        self.pre_prepared.clear();
        self.prepares.clear();
        self.commits.clear();
        self.committed.clear();
        self.view_change_votes.retain(|view, _| *view >= new_view);
        self.local_view_change_sent.retain(|view| *view > new_view);
        self.last_progress_tick_ms = now_ms;
    }

    fn note_progress(&mut self, tick_ms: u64) {
        if tick_ms > self.last_progress_tick_ms {
            self.last_progress_tick_ms = tick_ms;
        } else {
            self.last_progress_tick_ms = self.last_progress_tick_ms.saturating_add(1);
        }
    }

    /// Create a checkpoint at the specified sequence with state root
    pub fn create_checkpoint(
        &mut self,
        sequence: u64,
        state_root: String,
        validators: Vec<String>,
        timestamp_ms: u64,
    ) -> Result<(), BlockError> {
        if sequence <= self.last_stable_checkpoint {
            return Err(BlockError::TransactionValidationFailed(
                "checkpoint sequence must be newer than last stable".to_string(),
            ));
        }

        let checkpoint = PbftCheckpoint {
            sequence,
            state_root,
            validators,
            timestamp_ms,
        };

        self.checkpoints.insert(sequence, checkpoint);
        self.last_stable_checkpoint = sequence;
        self.garbage_collect(sequence);

        Ok(())
    }

    /// Get the last stable checkpoint
    pub fn get_last_checkpoint(&self) -> Option<&PbftCheckpoint> {
        self.checkpoints.get(&self.last_stable_checkpoint)
    }

    /// Perform garbage collection to clean state older than checkpoint
    pub fn garbage_collect(&mut self, stable_sequence: u64) {
        // Clean old pre-prepared messages
        self.pre_prepared
            .retain(|(_view, seq), _| *seq > stable_sequence);

        // Clean old prepares
        self.prepares
            .retain(|key, _| key.sequence > stable_sequence);

        // Clean old commits
        self.commits.retain(|key, _| key.sequence > stable_sequence);

        // Clean old committed entries
        self.committed.retain(|key| key.sequence > stable_sequence);

        // Clean old checkpoints (keep last 2 for safety)
        if self.checkpoints.len() > 2 {
            let mut to_remove = Vec::new();
            for seq in self.checkpoints.keys() {
                if *seq < stable_sequence.saturating_sub(self.checkpoint_interval) {
                    to_remove.push(*seq);
                }
            }
            for seq in to_remove {
                self.checkpoints.remove(&seq);
            }
        }
    }

    /// Set custom checkpoint interval
    pub fn set_checkpoint_interval(&mut self, interval: u64) {
        self.checkpoint_interval = interval;
    }

    /// Get current checkpoint interval
    pub fn checkpoint_interval(&self) -> u64 {
        self.checkpoint_interval
    }

    /// Export consensus state for persistence
    pub fn export_state(&self) -> ConsensusState {
        ConsensusState {
            node_id: self.node_id.clone(),
            current_view: self.current_view,
            last_stable_checkpoint: self.last_stable_checkpoint,
            last_progress_tick_ms: self.last_progress_tick_ms,
        }
    }

    /// Restore consensus state from persistence
    pub fn import_state(&mut self, state: ConsensusState) -> Result<(), BlockError> {
        if state.node_id != self.node_id {
            return Err(BlockError::TransactionValidationFailed(
                "state node_id mismatch".to_string(),
            ));
        }
        self.current_view = state.current_view;
        self.last_stable_checkpoint = state.last_stable_checkpoint;
        self.last_progress_tick_ms = state.last_progress_tick_ms;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Block, PublicKey, Signature};

    fn validators() -> Vec<String> {
        vec![
            "n0".to_string(),
            "n1".to_string(),
            "n2".to_string(),
            "n3".to_string(),
        ]
    }

    fn make_nodes() -> Vec<PbftConsensus> {
        let validators = validators();
        validators
            .iter()
            .map(|id| PbftConsensus::new(id.clone(), validators.clone()).expect("valid node"))
            .collect()
    }

    fn make_nodes_with_timeout(timeout_ms: u64) -> Vec<PbftConsensus> {
        let validators = validators();
        validators
            .iter()
            .map(|id| {
                PbftConsensus::with_timeout(id.clone(), validators.clone(), timeout_ms)
                    .expect("valid node")
            })
            .collect()
    }

    fn sample_block() -> Block {
        Block::new(
            "0".to_string(),
            1,
            0,
            1,
            vec![],
            PublicKey(vec![1]),
            Signature(vec![]),
        )
    }

    fn broadcast(nodes: &mut [PbftConsensus], msg: &PbftMessage) {
        for node in nodes.iter_mut() {
            node.process_message(msg.clone()).expect("message accepted");
        }
    }

    fn broadcast_to_indices(nodes: &mut [PbftConsensus], msg: &PbftMessage, indices: &[usize]) {
        for idx in indices {
            nodes[*idx]
                .process_message(msg.clone())
                .expect("message accepted");
        }
    }

    fn broadcast_view_change(nodes: &mut [PbftConsensus], msg: &PbftViewChange) {
        for node in nodes.iter_mut() {
            node.process_view_change(msg.clone())
                .expect("view-change accepted");
        }
    }

    #[test]
    fn test_pbft_quorum_math_for_4_nodes() {
        let engine = PbftConsensus::new("n0".to_string(), validators()).expect("engine");
        assert_eq!(engine.fault_tolerance(), 1);
        assert_eq!(engine.prepare_quorum(), 2);
        assert_eq!(engine.commit_quorum(), 3);
        assert_eq!(engine.current_view(), 0);
        assert!(engine.is_primary(0));
        assert!(!engine.is_primary(1));
    }

    #[test]
    fn test_pbft_reaches_commit_with_deterministic_broadcast() {
        let mut nodes = make_nodes();
        let block = sample_block();
        let view = 0;
        let sequence = 1;

        let preprepare = nodes[0]
            .make_preprepare(view, sequence, &block)
            .expect("primary should create pre-prepare");
        broadcast(&mut nodes, &preprepare);

        let prepares: Vec<PbftMessage> = nodes
            .iter()
            .map(|node| node.make_prepare(view, sequence, block.header.hash.clone()))
            .collect();
        for msg in &prepares {
            broadcast(&mut nodes, msg);
        }

        for node in &nodes {
            assert!(node.is_prepared(view, sequence, &block.header.hash));
        }

        let commits: Vec<PbftMessage> = nodes
            .iter()
            .map(|node| node.make_commit(view, sequence, block.header.hash.clone()))
            .collect();
        for msg in &commits {
            broadcast(&mut nodes, msg);
        }

        for node in &nodes {
            assert!(node.is_committed(view, sequence, &block.header.hash));
        }
    }

    #[test]
    fn test_pbft_timeout_triggers_view_change_message() {
        let mut node =
            PbftConsensus::with_timeout("n0".to_string(), validators(), 100).expect("engine");
        assert!(node.on_timeout(50).is_none());

        let vc = node.on_timeout(150).expect("timeout should trigger");
        assert_eq!(vc.new_view, 1);
        assert_eq!(vc.sender, "n0");
    }

    #[test]
    fn test_pbft_view_change_reaches_quorum() {
        let mut nodes = make_nodes_with_timeout(100);
        let vcs: Vec<PbftViewChange> = nodes
            .iter_mut()
            .filter_map(|node| node.on_timeout(200))
            .collect();
        assert_eq!(vcs.len(), 4);

        for vc in &vcs {
            broadcast_view_change(&mut nodes, vc);
        }

        for node in &nodes {
            assert_eq!(node.current_view(), 1);
        }
    }

    #[test]
    fn test_pbft_partition_then_recovery_via_view_change_and_new_view() {
        let mut nodes = make_nodes_with_timeout(100);
        let block = sample_block();
        let view = 0;
        let sequence = 1;

        // Partition: only n0 and n1 receive pre-prepare and prepare.
        let preprepare = nodes[0]
            .make_preprepare(view, sequence, &block)
            .expect("primary should create pre-prepare");
        broadcast_to_indices(&mut nodes, &preprepare, &[0, 1]);

        let prepare_n0 = nodes[0].make_prepare(view, sequence, block.header.hash.clone());
        let prepare_n1 = nodes[1].make_prepare(view, sequence, block.header.hash.clone());
        broadcast_to_indices(&mut nodes, &prepare_n0, &[0, 1]);
        broadcast_to_indices(&mut nodes, &prepare_n1, &[0, 1]);

        assert!(!nodes[0].is_committed(view, sequence, &block.header.hash));
        assert!(!nodes[1].is_committed(view, sequence, &block.header.hash));

        // Timeout on all nodes triggers view-change to view 1.
        let view_changes: Vec<PbftViewChange> = nodes
            .iter_mut()
            .filter_map(|node| node.on_timeout(200))
            .collect();
        assert_eq!(view_changes.len(), 4);

        for vc in &view_changes {
            broadcast_view_change(&mut nodes, vc);
        }
        for node in &nodes {
            assert_eq!(node.current_view(), 1);
        }

        // New primary (n1 for view 1) announces new-view with certificates.
        let new_view_msg = nodes[1]
            .make_new_view(1, validators(), 210)
            .expect("n1 should be primary for view 1");
        for node in nodes.iter_mut() {
            let event = node
                .process_new_view(new_view_msg.clone())
                .expect("new-view accepted");
            assert!(matches!(
                event,
                Some(PbftEvent::ViewChanged { new_view: 1, .. })
            ));
        }

        // Recovery: consensus proceeds in view 1.
        let recovered_block = sample_block();
        let recovered_sequence = 2;
        let preprepare_v1 = nodes[1]
            .make_preprepare(1, recovered_sequence, &recovered_block)
            .expect("new primary should create pre-prepare");
        broadcast(&mut nodes, &preprepare_v1);

        let prepares: Vec<PbftMessage> = nodes
            .iter()
            .map(|node| {
                node.make_prepare(1, recovered_sequence, recovered_block.header.hash.clone())
            })
            .collect();
        for msg in &prepares {
            broadcast(&mut nodes, msg);
        }

        let commits: Vec<PbftMessage> = nodes
            .iter()
            .map(|node| {
                node.make_commit(1, recovered_sequence, recovered_block.header.hash.clone())
            })
            .collect();
        for msg in &commits {
            broadcast(&mut nodes, msg);
        }

        for node in &nodes {
            assert!(node.is_committed(1, recovered_sequence, &recovered_block.header.hash));
        }
    }

    #[test]
    fn test_pbft_rejects_non_primary_preprepare() {
        let mut node = PbftConsensus::new("n1".to_string(), validators()).expect("engine");
        let msg = PbftMessage::new(
            PbftMessageType::PrePrepare,
            0,
            1,
            "block_hash".to_string(),
            "n1".to_string(), // Non-primary for view 0
        );

        assert!(node.process_message(msg).is_err());
    }

    #[test]
    fn test_pbft_rejects_prepare_without_preprepare() {
        let mut node = PbftConsensus::new("n1".to_string(), validators()).expect("engine");
        let msg = PbftMessage::new(
            PbftMessageType::Prepare,
            0,
            1,
            "block_hash".to_string(),
            "n1".to_string(),
        );

        assert!(node.process_message(msg).is_err());
    }

    #[test]
    fn test_pbft_rejects_conflicting_preprepare() {
        let mut node = PbftConsensus::new("n2".to_string(), validators()).expect("engine");
        let first = PbftMessage::new(
            PbftMessageType::PrePrepare,
            0,
            1,
            "hash_a".to_string(),
            "n0".to_string(),
        );
        let conflict = PbftMessage::new(
            PbftMessageType::PrePrepare,
            0,
            1,
            "hash_b".to_string(),
            "n0".to_string(),
        );

        node.process_message(first).expect("first accepted");
        assert!(node.process_message(conflict).is_err());
    }

    #[test]
    fn test_pbft_checkpoint_creation() {
        let mut node = PbftConsensus::new("n0".to_string(), validators()).expect("engine");

        let root = "state_root_1".to_string();
        let result = node.create_checkpoint(100, root.clone(), validators(), 1000);
        assert!(result.is_ok());

        let checkpoint = node.get_last_checkpoint();
        assert!(checkpoint.is_some());
        assert_eq!(checkpoint.unwrap().sequence, 100);
        assert_eq!(checkpoint.unwrap().state_root, root);
        assert_eq!(node.last_stable_checkpoint, 100);
    }

    #[test]
    fn test_pbft_checkpoint_rejects_old_sequence() {
        let mut node = PbftConsensus::new("n0".to_string(), validators()).expect("engine");

        node.create_checkpoint(100, "root_1".to_string(), validators(), 1000)
            .expect("first checkpoint");
        let result = node.create_checkpoint(50, "root_2".to_string(), validators(), 1100);
        assert!(result.is_err());
    }

    #[test]
    fn test_pbft_garbage_collection_cleans_old_state() {
        let mut node = PbftConsensus::new("n0".to_string(), validators()).expect("engine");
        let block = sample_block();

        // Add some committed entries
        let key = ConsensusKey::new(0, 50, block.header.hash.clone());
        node.committed.insert(key);

        let old_key = ConsensusKey::new(0, 10, "old_block".to_string());
        node.committed.insert(old_key);

        // Create checkpoint at sequence 100 and run GC
        node.create_checkpoint(100, "root".to_string(), validators(), 1000)
            .expect("checkpoint created");

        // old_key (seq 10) should be removed, key (seq 50) should remain
        assert!(node
            .committed
            .iter()
            .all(|k| k.sequence > 100 || k.sequence == 50));
    }

    #[test]
    fn test_pbft_export_import_state() {
        let mut node1 = PbftConsensus::new("n0".to_string(), validators()).expect("node1");
        node1.current_view = 3;
        node1.last_stable_checkpoint = 50;

        let exported = node1.export_state();
        assert_eq!(exported.current_view, 3);
        assert_eq!(exported.last_stable_checkpoint, 50);

        let mut node2 = PbftConsensus::new("n0".to_string(), validators()).expect("node2");
        node2.import_state(exported).expect("import success");
        assert_eq!(node2.current_view, 3);
        assert_eq!(node2.last_stable_checkpoint, 50);
    }

    #[test]
    fn test_pbft_checkpoint_interval_configuration() {
        let mut node = PbftConsensus::new("n0".to_string(), validators()).expect("engine");
        assert_eq!(node.checkpoint_interval(), DEFAULT_CHECKPOINT_INTERVAL);

        node.set_checkpoint_interval(200);
        assert_eq!(node.checkpoint_interval(), 200);
    }
}
