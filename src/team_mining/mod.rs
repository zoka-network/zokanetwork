//! Non-custodial team mining state and share validation.
//!
//! A team never owns a wallet. Every candidate names one member's normal
//! private address as the single coinbase recipient, preserving the existing
//! consensus shape. Members prove contributed work with lower-difficulty
//! shares; the coordinator validates each share against the exact candidate
//! before it affects the next payout turn.

use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

pub const TEAM_SCHEMA_VERSION: u32 = 1;
pub const TEAM_HEARTBEAT_SECONDS: u64 = 30;
pub const COORDINATOR_TTL_SECONDS: u64 = 90;
pub const SHARE_TARGET_SECONDS: u64 = 15;
pub const SHARES_PER_DIFFICULTY_ADJUSTMENT: u64 = 8;
pub const MAX_RECENT_SHARE_IDS: usize = 4096;
pub const TEAM_INVITE_PREFIX: &str = "ZTM1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMember {
    pub address: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub expelled: bool,
    #[serde(default)]
    pub total_work: u64,
    #[serde(default)]
    pub accepted_shares: u64,
    #[serde(default)]
    pub blocks_paid: u64,
    #[serde(default)]
    pub share_difficulty_bits: u32,
    #[serde(default)]
    pub last_share_at: Option<u64>,
    #[serde(default)]
    pub adjustment_started_at: Option<u64>,
    #[serde(default)]
    pub shares_since_adjustment: u64,
    #[serde(default)]
    pub estimated_hashrate_hps: u64,
}

impl TeamMember {
    fn new(address: String, share_difficulty_bits: u32) -> Self {
        Self {
            address,
            active: true,
            expelled: false,
            total_work: 0,
            accepted_shares: 0,
            blocks_paid: 0,
            share_difficulty_bits,
            last_share_at: None,
            adjustment_started_at: None,
            shares_since_adjustment: 0,
            estimated_hashrate_hps: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MiningTemplate {
    pub id: String,
    pub team_id: String,
    pub epoch: u64,
    pub chain_id: String,
    pub prev_hash: String,
    pub height: u64,
    pub timestamp: u64,
    pub recipient_address: String,
    pub network_difficulty_bits: u32,
}

impl MiningTemplate {
    pub fn new(
        team_id: String,
        epoch: u64,
        chain_id: String,
        prev_hash: String,
        height: u64,
        timestamp: u64,
        recipient_address: String,
        network_difficulty_bits: u32,
    ) -> Result<Self, TeamError> {
        validate_address(&recipient_address)?;
        if team_id.trim().is_empty() || chain_id.trim().is_empty() || prev_hash.trim().is_empty() {
            return Err(TeamError::InvalidTemplate);
        }
        if height == 0 || network_difficulty_bits == 0 {
            return Err(TeamError::InvalidTemplate);
        }
        let id = template_id(
            &team_id,
            epoch,
            &chain_id,
            &prev_hash,
            height,
            timestamp,
            &recipient_address,
            network_difficulty_bits,
        );
        Ok(Self {
            id,
            team_id,
            epoch,
            chain_id,
            prev_hash,
            height,
            timestamp,
            recipient_address,
            network_difficulty_bits,
        })
    }

    pub fn validate_id(&self) -> bool {
        self.id
            == template_id(
                &self.team_id,
                self.epoch,
                &self.chain_id,
                &self.prev_hash,
                self.height,
                self.timestamp,
                &self.recipient_address,
                self.network_difficulty_bits,
            )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmittedShare {
    pub team_id: String,
    pub template_id: String,
    pub miner_address: String,
    pub nonce: u64,
    pub pow_hash_hex: String,
    pub difficulty_bits: u32,
    pub timestamp: u64,
}

impl SubmittedShare {
    pub fn replay_id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"zoka-team-share-v1");
        hasher.update([0x1f]);
        hasher.update(self.team_id.as_bytes());
        hasher.update([0x1f]);
        hasher.update(self.template_id.as_bytes());
        hasher.update([0x1f]);
        hasher.update(self.miner_address.as_bytes());
        hasher.update([0x1f]);
        hasher.update(self.nonce.to_le_bytes());
        hex::encode(hasher.finalize())
    }

    pub fn validate_shape(&self) -> Result<(), TeamError> {
        if self.team_id.trim().is_empty()
            || self.template_id.len() != 64
            || !self
                .template_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.difficulty_bits == 0
        {
            return Err(TeamError::InvalidShareHash);
        }
        validate_address(&self.miner_address)?;
        if self.pow_hash_hex.len() != 64
            || !self
                .pow_hash_hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(TeamError::InvalidShareHash);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedShare {
    pub work: u64,
    pub network_solution: bool,
    pub actual_difficulty_bits: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamState {
    pub schema_version: u32,
    /// Monotonic coordinator-owned state revision. Heartbeats may refresh
    /// local liveness without changing it; every authoritative mutation
    /// increments it so delayed signed gossip cannot roll accounting back.
    #[serde(default)]
    pub revision: u64,
    pub team_id: String,
    pub invite_code: String,
    pub coordinator: String,
    pub backup_coordinator: Option<String>,
    pub epoch: u64,
    pub members: BTreeMap<String, TeamMember>,
    pub total_work: u64,
    pub confirmed_blocks: u64,
    pub active_template: Option<MiningTemplate>,
    pub last_coordinator_seen: u64,
    #[serde(default)]
    pub recent_share_ids: Vec<String>,
    #[serde(default)]
    pub last_confirmed_recipient: Option<String>,
    #[serde(default)]
    pub last_hijacked_recipient: Option<String>,
}

impl TeamState {
    pub fn create(coordinator: String, now: u64) -> Result<Self, TeamError> {
        validate_address(&coordinator)?;
        let team_id = new_team_id();
        let invite_code = invite_code_for(&team_id);
        let mut members = BTreeMap::new();
        // The network target is not known until the coordinator creates the
        // first template. Zero is an internal "unassigned" sentinel; set_template
        // replaces it with the same adaptive starting target used for joiners.
        members.insert(coordinator.clone(), TeamMember::new(coordinator.clone(), 0));
        Ok(Self {
            schema_version: TEAM_SCHEMA_VERSION,
            revision: 0,
            team_id,
            invite_code,
            coordinator,
            backup_coordinator: None,
            epoch: 0,
            members,
            total_work: 0,
            confirmed_blocks: 0,
            active_template: None,
            last_coordinator_seen: now,
            recent_share_ids: Vec::new(),
            last_confirmed_recipient: None,
            last_hijacked_recipient: None,
        })
    }

    pub fn validate(&self) -> Result<(), TeamError> {
        if self.schema_version != TEAM_SCHEMA_VERSION
            || self.team_id.len() != 32
            || !self.team_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || parse_invite_code(&self.invite_code).as_deref() != Some(self.team_id.as_str())
        {
            return Err(TeamError::InvalidTeam);
        }
        validate_address(&self.coordinator)?;
        let coordinator = self
            .members
            .get(&self.coordinator)
            .ok_or(TeamError::InvalidTeam)?;
        if !coordinator.active || coordinator.expelled {
            return Err(TeamError::InvalidTeam);
        }
        if let Some(backup) = &self.backup_coordinator {
            if backup == &self.coordinator {
                return Err(TeamError::InvalidTeam);
            }
            let member = self.members.get(backup).ok_or(TeamError::InvalidTeam)?;
            if !member.active || member.expelled {
                return Err(TeamError::InvalidTeam);
            }
        }
        for (key, member) in &self.members {
            validate_address(key)?;
            if key != &member.address {
                return Err(TeamError::InvalidTeam);
            }
        }
        if let Some(template) = &self.active_template {
            if template.team_id != self.team_id
                || template.epoch != self.epoch
                || !template.validate_id()
                || !self
                    .members
                    .get(&template.recipient_address)
                    .is_some_and(|member| member.active && !member.expelled)
            {
                return Err(TeamError::InvalidTemplate);
            }
        }
        Ok(())
    }

    pub fn add_member(
        &mut self,
        address: String,
        network_difficulty_bits: u32,
    ) -> Result<bool, TeamError> {
        validate_address(&address)?;
        if self
            .members
            .get(&address)
            .is_some_and(|member| member.expelled)
        {
            return Err(TeamError::MemberExpelled);
        }
        let initial_difficulty = initial_share_difficulty(network_difficulty_bits);
        let inserted = match self.members.get_mut(&address) {
            Some(member) => {
                member.active = true;
                false
            }
            None => {
                self.members.insert(
                    address.clone(),
                    TeamMember::new(address.clone(), initial_difficulty),
                );
                true
            }
        };
        if self.backup_coordinator.is_none() && address != self.coordinator {
            self.backup_coordinator = Some(address);
        }
        Ok(inserted)
    }

    pub fn leave_member(&mut self, address: &str) -> Result<(), TeamError> {
        if address == self.coordinator {
            return Err(TeamError::CoordinatorCannotLeave);
        }
        let member = self
            .members
            .get_mut(address)
            .ok_or(TeamError::UnknownMember)?;
        member.active = false;
        if self.backup_coordinator.as_deref() == Some(address) {
            self.backup_coordinator = self.next_backup();
        }
        Ok(())
    }

    pub fn expel_member(&mut self, address: &str) -> Result<(), TeamError> {
        if address == self.coordinator {
            return Err(TeamError::CoordinatorCannotLeave);
        }
        let member = self
            .members
            .get_mut(address)
            .ok_or(TeamError::UnknownMember)?;
        member.active = false;
        member.expelled = true;
        if self.backup_coordinator.as_deref() == Some(address) {
            self.backup_coordinator = self.next_backup();
        }
        Ok(())
    }

    pub fn active_members(&self) -> impl Iterator<Item = &TeamMember> {
        self.members
            .values()
            .filter(|member| member.active && !member.expelled)
    }

    pub fn next_recipient(&self) -> Option<String> {
        let active = self.active_members().collect::<Vec<_>>();
        if active.is_empty() {
            return None;
        }
        let (active_work, projected_blocks) = active_turn_totals(&active);
        if active_work == 0 {
            return active
                .iter()
                .find(|member| member.address == self.coordinator)
                .or_else(|| active.first())
                .map(|member| member.address.clone());
        }
        active
            .into_iter()
            .max_by(|left, right| {
                compare_turn_debt(left, right, active_work, projected_blocks)
                    .then_with(|| right.address.cmp(&left.address))
            })
            .map(|member| member.address.clone())
    }

    pub fn turn_order(&self) -> Vec<String> {
        let mut members = self.active_members().collect::<Vec<_>>();
        let (active_work, projected_blocks) = active_turn_totals(&members);
        members.sort_by(|left, right| {
            compare_turn_debt(right, left, active_work, projected_blocks)
                .then_with(|| left.address.cmp(&right.address))
        });
        members
            .into_iter()
            .map(|member| member.address.clone())
            .collect()
    }

    pub fn set_template(
        &mut self,
        chain_id: String,
        prev_hash: String,
        height: u64,
        timestamp: u64,
        network_difficulty_bits: u32,
    ) -> Result<MiningTemplate, TeamError> {
        let recipient = self.next_recipient().ok_or(TeamError::NoActiveMembers)?;
        for member in self.members.values_mut() {
            if member.share_difficulty_bits == 0 {
                member.share_difficulty_bits = initial_share_difficulty(network_difficulty_bits);
            }
            member.share_difficulty_bits = member
                .share_difficulty_bits
                .min(network_difficulty_bits.max(1));
        }
        let template = MiningTemplate::new(
            self.team_id.clone(),
            self.epoch,
            chain_id,
            prev_hash,
            height,
            timestamp,
            recipient,
            network_difficulty_bits,
        )?;
        self.active_template = Some(template.clone());
        Ok(template)
    }

    pub fn validate_share(&self, share: &SubmittedShare) -> Result<ValidatedShare, TeamError> {
        share.validate_shape()?;
        if share.team_id != self.team_id {
            return Err(TeamError::WrongTeam);
        }
        let template = self
            .active_template
            .as_ref()
            .ok_or(TeamError::NoActiveTemplate)?;
        if share.template_id != template.id || !template.validate_id() {
            return Err(TeamError::StaleTemplate);
        }
        let member = self
            .members
            .get(&share.miner_address)
            .ok_or(TeamError::UnknownMember)?;
        if !member.active || member.expelled {
            return Err(TeamError::UnknownMember);
        }
        if share.difficulty_bits != member.share_difficulty_bits {
            return Err(TeamError::WrongShareDifficulty);
        }
        if self
            .recent_share_ids
            .iter()
            .any(|id| id == &share.replay_id())
        {
            return Err(TeamError::ReplayedShare);
        }
        if share.pow_hash_hex.len() != 64
            || !share
                .pow_hash_hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(TeamError::InvalidShareHash);
        }
        let input = crate::mining::pow::build_pow_input(
            &template.chain_id,
            &template.prev_hash,
            &template.recipient_address,
            template.height,
            template.timestamp,
            share.nonce,
        );
        let engine = crate::mining::pow::engine_for(&template.chain_id, template.height)
            .map_err(|error| TeamError::Pow(error.to_string()))?;
        let hash = engine
            .calculate_hash(&input)
            .map_err(|error| TeamError::Pow(error.to_string()))?;
        if hex::encode(&hash) != share.pow_hash_hex.to_ascii_lowercase() {
            return Err(TeamError::InvalidShareHash);
        }
        let actual_difficulty_bits = crate::mining::pow::leading_zero_bits(&hash);
        if actual_difficulty_bits < member.share_difficulty_bits {
            return Err(TeamError::ShareBelowTarget);
        }
        Ok(ValidatedShare {
            work: share_work(member.share_difficulty_bits),
            network_solution: actual_difficulty_bits >= template.network_difficulty_bits,
            actual_difficulty_bits,
        })
    }

    pub fn record_validated_share(
        &mut self,
        share: &SubmittedShare,
        validated: &ValidatedShare,
    ) -> Result<(), TeamError> {
        let replay_id = share.replay_id();
        if self.recent_share_ids.iter().any(|id| id == &replay_id) {
            return Err(TeamError::ReplayedShare);
        }
        let network_difficulty = self
            .active_template
            .as_ref()
            .map(|template| template.network_difficulty_bits)
            .ok_or(TeamError::NoActiveTemplate)?;
        let member = self
            .members
            .get_mut(&share.miner_address)
            .ok_or(TeamError::UnknownMember)?;
        let previous_share_at = member.last_share_at;
        member.total_work = member.total_work.saturating_add(validated.work);
        member.accepted_shares = member.accepted_shares.saturating_add(1);
        member.shares_since_adjustment = member.shares_since_adjustment.saturating_add(1);
        member.last_share_at = Some(share.timestamp);
        if member.adjustment_started_at.is_none() {
            member.adjustment_started_at = previous_share_at.or(Some(share.timestamp));
        }
        if let Some(previous) = previous_share_at {
            let elapsed = share.timestamp.saturating_sub(previous).max(1);
            member.estimated_hashrate_hps =
                share_work(member.share_difficulty_bits).saturating_div(elapsed);
        }
        adjust_member_difficulty(member, share.timestamp, network_difficulty);
        self.total_work = self.total_work.saturating_add(validated.work);
        self.recent_share_ids.push(replay_id);
        if self.recent_share_ids.len() > MAX_RECENT_SHARE_IDS {
            let overflow = self.recent_share_ids.len() - MAX_RECENT_SHARE_IDS;
            self.recent_share_ids.drain(..overflow);
        }
        Ok(())
    }

    pub fn confirm_assigned_block(&mut self, recipient: &str) -> Result<(), TeamError> {
        let expected = self
            .active_template
            .as_ref()
            .map(|template| template.recipient_address.clone())
            .or_else(|| self.next_recipient())
            .ok_or(TeamError::NoActiveMembers)?;
        if recipient != expected {
            self.last_hijacked_recipient = Some(recipient.to_string());
            self.confirmed_blocks = self.confirmed_blocks.saturating_add(1);
            if let Some(member) = self.members.get_mut(recipient) {
                member.blocks_paid = member.blocks_paid.saturating_add(1);
            }
            self.active_template = None;
            return Err(TeamError::CoinbaseHijacked {
                expected,
                actual: recipient.to_string(),
            });
        }
        let member = self
            .members
            .get_mut(recipient)
            .ok_or(TeamError::UnknownMember)?;
        member.blocks_paid = member.blocks_paid.saturating_add(1);
        self.confirmed_blocks = self.confirmed_blocks.saturating_add(1);
        self.last_confirmed_recipient = Some(recipient.to_string());
        self.last_hijacked_recipient = None;
        self.active_template = None;
        Ok(())
    }

    pub fn note_coordinator_seen(&mut self, coordinator: &str, epoch: u64, now: u64) {
        if coordinator == self.coordinator && epoch == self.epoch {
            self.last_coordinator_seen = now;
        }
    }

    pub fn backup_should_take_over(&self, local_address: &str, now: u64) -> bool {
        self.backup_coordinator.as_deref() == Some(local_address)
            && now.saturating_sub(self.last_coordinator_seen) > COORDINATOR_TTL_SECONDS
    }

    pub fn promote_backup(&mut self, local_address: &str, now: u64) -> Result<(), TeamError> {
        if !self.backup_should_take_over(local_address, now) {
            return Err(TeamError::FailoverNotReady);
        }
        let previous_coordinator = self.coordinator.clone();
        self.coordinator = local_address.to_string();
        if let Some(previous) = self.members.get_mut(&previous_coordinator) {
            previous.active = false;
        }
        if let Some(promoted) = self.members.get_mut(local_address) {
            promoted.active = true;
            promoted.expelled = false;
        }
        self.epoch = self.epoch.saturating_add(1);
        self.backup_coordinator = self.next_backup();
        self.last_coordinator_seen = now;
        self.active_template = None;
        Ok(())
    }

    /// Traspasa la coordinacion al backup de forma voluntaria.
    ///
    /// Es lo mismo que `promote_backup` pero sin la guarda de TTL: esa guarda
    /// existe para el failover, donde hay que esperar a estar seguros de que el
    /// coordinador desaparecio. Aca el coordinador se esta yendo a proposito, y
    /// hacer esperar al equipo el TTL completo solo lo dejaria sin plantillas
    /// varios minutos.
    ///
    /// Devuelve la direccion del nuevo coordinador.
    pub fn hand_over_coordination(&mut self, now: u64) -> Result<String, TeamError> {
        let successor = self
            .backup_coordinator
            .clone()
            .or_else(|| self.next_backup())
            .ok_or(TeamError::NoActiveMembers)?;
        let previous = self.coordinator.clone();
        self.coordinator = successor.clone();
        if let Some(member) = self.members.get_mut(&previous) {
            member.active = false;
        }
        if let Some(member) = self.members.get_mut(&successor) {
            member.active = true;
            member.expelled = false;
        }
        self.epoch = self.epoch.saturating_add(1);
        self.backup_coordinator = self.next_backup();
        self.last_coordinator_seen = now;
        self.active_template = None;
        Ok(successor)
    }

    fn next_backup(&self) -> Option<String> {
        self.members
            .values()
            .filter(|member| {
                member.active && !member.expelled && member.address != self.coordinator
            })
            .map(|member| member.address.clone())
            .min()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamAdvertise {
    pub team_id: String,
    pub invite_code: String,
    pub coordinator: String,
    pub backup_coordinator: Option<String>,
    pub member_count: usize,
    pub epoch: u64,
    pub timestamp: u64,
}

impl TeamAdvertise {
    pub fn from_state(state: &TeamState, timestamp: u64) -> Self {
        Self {
            team_id: state.team_id.clone(),
            invite_code: state.invite_code.clone(),
            coordinator: state.coordinator.clone(),
            backup_coordinator: state.backup_coordinator.clone(),
            member_count: state.active_members().count(),
            epoch: state.epoch,
            timestamp,
        }
    }

    pub fn validate(&self) -> Result<(), TeamError> {
        validate_address(&self.coordinator)?;
        if parse_invite_code(&self.invite_code).as_deref() != Some(self.team_id.as_str())
            || self.member_count == 0
        {
            return Err(TeamError::InvalidTeam);
        }
        if let Some(backup) = &self.backup_coordinator {
            validate_address(backup)?;
            if backup == &self.coordinator {
                return Err(TeamError::InvalidTeam);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MembershipAction {
    Join,
    Leave,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMembership {
    pub team_id: String,
    pub member_address: String,
    pub action: MembershipAction,
    pub timestamp: u64,
}

impl TeamMembership {
    pub fn validate(&self) -> Result<(), TeamError> {
        if self.team_id.trim().is_empty() {
            return Err(TeamError::WrongTeam);
        }
        validate_address(&self.member_address)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamTurnUpdate {
    pub state: TeamState,
    pub timestamp: u64,
}

impl TeamTurnUpdate {
    pub fn validate(&self) -> Result<(), TeamError> {
        self.state.validate()
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TeamError {
    #[error("invalid team state")]
    InvalidTeam,
    #[error("invalid team mining template")]
    InvalidTemplate,
    #[error("invalid team member address")]
    InvalidAddress,
    #[error("message belongs to another team")]
    WrongTeam,
    #[error("team invite has not been observed from an online coordinator")]
    TeamNotDiscovered,
    #[error("unknown or inactive team member")]
    UnknownMember,
    #[error("member was expelled")]
    MemberExpelled,
    #[error("the coordinator must hand over instead of leaving")]
    CoordinatorCannotLeave,
    #[error("only the current coordinator can perform this operation")]
    NotCoordinator,
    #[error("message source does not match the member's signed node identity")]
    PeerBindingMismatch,
    #[error("turn update is not from the current coordinator or its timed-out backup")]
    InvalidCoordinatorUpdate,
    #[error("team has no active members")]
    NoActiveMembers,
    #[error("team has no active mining template")]
    NoActiveTemplate,
    #[error("share targets an old mining template")]
    StaleTemplate,
    #[error("share used a stale difficulty target")]
    WrongShareDifficulty,
    #[error("share hash is malformed or does not match its nonce")]
    InvalidShareHash,
    #[error("share does not meet its assigned target")]
    ShareBelowTarget,
    #[error("share was already credited")]
    ReplayedShare,
    #[error("proof-of-work backend failed: {0}")]
    Pow(String),
    #[error("backup coordinator timeout has not elapsed")]
    FailoverNotReady,
    #[error("coinbase recipient was hijacked: expected {expected}, got {actual}")]
    CoinbaseHijacked { expected: String, actual: String },
    #[error("team state persistence failed: {0}")]
    Persistence(String),
}

#[derive(Clone, Debug)]
pub struct TeamStore {
    path: PathBuf,
    state: Option<TeamState>,
}

#[derive(Clone, Debug)]
struct ObservedTeam {
    advertisement: TeamAdvertise,
    source_peer: String,
    seen_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkSolution {
    pub share: SubmittedShare,
    pub template: MiningTemplate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamRuntimeStatus {
    pub enabled: bool,
    pub hashing: bool,
    pub threads: u64,
    pub join_status: String,
    pub local_address: String,
    pub team: Option<TeamState>,
    pub combined_hashrate_hps: u64,
    pub local_accepted_shares: u64,
    pub team_accepted_shares: u64,
    pub estimated_turn: Option<usize>,
}

/// Async bridge shared by HTTP handlers and the fullnode network/mining loop.
///
/// Durable accounting remains in [`TeamStore`]. Peer bindings and discovered
/// invite advertisements are intentionally process-local: gossipsub signs the
/// node identity, and those bindings have to be relearned after a restart.
pub struct TeamMiningRuntime {
    pub store: Arc<Mutex<TeamStore>>,
    pub local_address: String,
    observed: Mutex<HashMap<String, ObservedTeam>>,
    member_peers: Mutex<HashMap<String, String>>,
    outbound_messages: Mutex<Vec<crate::network::message::ZokaMessage>>,
    pending_network_solutions: Mutex<Vec<NetworkSolution>>,
    enabled: Mutex<bool>,
    hashing: Mutex<bool>,
    threads: Mutex<u64>,
    pending_join_team: Mutex<Option<String>>,
    join_status: Mutex<String>,
    last_broadcast_at: Mutex<u64>,
}

impl TeamMiningRuntime {
    pub fn new(store: TeamStore, local_address: String) -> Result<Self, TeamError> {
        validate_address(&local_address)?;
        if let Some(state) = store.state() {
            state.validate()?;
            if !state.members.contains_key(&local_address) {
                return Err(TeamError::UnknownMember);
            }
        }
        Ok(Self {
            store: Arc::new(Mutex::new(store)),
            local_address,
            observed: Mutex::new(HashMap::new()),
            member_peers: Mutex::new(HashMap::new()),
            outbound_messages: Mutex::new(Vec::new()),
            pending_network_solutions: Mutex::new(Vec::new()),
            enabled: Mutex::new(false),
            hashing: Mutex::new(false),
            threads: Mutex::new(1),
            pending_join_team: Mutex::new(None),
            join_status: Mutex::new("not_joined".to_string()),
            last_broadcast_at: Mutex::new(0),
        })
    }

    pub async fn queue_message(&self, message: crate::network::message::ZokaMessage) {
        self.outbound_messages.lock().await.push(message);
    }

    pub async fn drain_outbound_messages(&self) -> Vec<crate::network::message::ZokaMessage> {
        std::mem::take(&mut *self.outbound_messages.lock().await)
    }

    pub async fn create_team(&self, now: u64) -> Result<TeamState, TeamError> {
        let state = TeamState::create(self.local_address.clone(), now)?;
        self.store.lock().await.replace(state.clone())?;
        *self.pending_join_team.lock().await = None;
        *self.join_status.lock().await = "joined".to_string();
        self.publish_authoritative_state(&state, now).await;
        Ok(state)
    }

    pub async fn set_enabled(&self, enabled: bool, threads: u64) {
        *self.enabled.lock().await = enabled;
        *self.threads.lock().await = threads.max(1);
        if !enabled {
            *self.hashing.lock().await = false;
        }
    }

    pub async fn set_hashing(&self, hashing: bool) {
        *self.hashing.lock().await = hashing;
    }

    pub async fn observe_advertisement(
        &self,
        advertisement: TeamAdvertise,
        source_peer: String,
        now: u64,
    ) -> Result<(), TeamError> {
        advertisement.validate()?;
        self.observed.lock().await.insert(
            advertisement.invite_code.to_ascii_uppercase(),
            ObservedTeam {
                advertisement: advertisement.clone(),
                source_peer: source_peer.clone(),
                seen_at: now,
            },
        );
        self.member_peers
            .lock()
            .await
            .insert(advertisement.coordinator.clone(), source_peer);
        let current = self.store.lock().await.state().cloned();
        if current.as_ref().is_some_and(|state| {
            state.team_id == advertisement.team_id
                && state.coordinator == advertisement.coordinator
                && state.epoch == advertisement.epoch
        }) {
            self.store.lock().await.mutate_local(|state| {
                state.note_coordinator_seen(&advertisement.coordinator, advertisement.epoch, now);
                Ok(())
            })?;
        }
        Ok(())
    }

    pub async fn request_join(&self, invite_code: &str, now: u64) -> Result<(), TeamError> {
        let normalized = invite_code.trim().to_ascii_uppercase();
        let team_id = parse_invite_code(&normalized).ok_or(TeamError::InvalidTeam)?;
        let observed = self
            .observed
            .lock()
            .await
            .get(&normalized)
            .filter(|team| now.saturating_sub(team.seen_at) <= COORDINATOR_TTL_SECONDS)
            .cloned()
            .ok_or(TeamError::TeamNotDiscovered)?;
        if observed.advertisement.team_id != team_id {
            return Err(TeamError::InvalidTeam);
        }
        self.member_peers
            .lock()
            .await
            .insert(observed.advertisement.coordinator, observed.source_peer);
        *self.pending_join_team.lock().await = Some(team_id.clone());
        *self.join_status.lock().await = "join_requested".to_string();
        self.queue_message(crate::network::message::ZokaMessage::TeamJoin(
            TeamMembership {
                team_id,
                member_address: self.local_address.clone(),
                action: MembershipAction::Join,
                timestamp: now,
            },
        ))
        .await;
        Ok(())
    }

    pub async fn request_leave(&self, now: u64) -> Result<(), TeamError> {
        let state = self
            .store
            .lock()
            .await
            .state()
            .cloned()
            .ok_or(TeamError::InvalidTeam)?;
        if state.coordinator == self.local_address {
            // El coordinador ya no queda atrapado en su propio equipo: si hay
            // sucesor, traspasa y se va como un miembro mas. Si no hay nadie a
            // quien traspasar, el unico camino es disolver, y eso lo pide la
            // interfaz explicitamente -- disolver por accidente al tocar
            // "salir" seria destruir el equipo de todos.
            let successor = {
                let mut guard = self.store.lock().await;
                let mut updated = state.clone();
                let successor = updated.hand_over_coordination(now)?;
                updated.revision = updated.revision.saturating_add(1);
                guard.replace(updated.clone())?;
                self.publish_authoritative_state(&updated, now).await;
                successor
            };
            *self.join_status.lock().await = "left".to_string();
            let _ = successor;
            return Ok(());
        }
        self.queue_message(crate::network::message::ZokaMessage::TeamJoin(
            TeamMembership {
                team_id: state.team_id,
                member_address: self.local_address.clone(),
                action: MembershipAction::Leave,
                timestamp: now,
            },
        ))
        .await;
        *self.join_status.lock().await = "leave_requested".to_string();
        Ok(())
    }

    /// Disuelve el equipo del que este nodo es coordinador.
    ///
    /// Solo borra el estado local. No se manda un mensaje de disolucion porque
    /// `MembershipAction` solo tiene Join y Leave: agregar una variante nueva
    /// romperia la deserializacion en los nodos que todavia no la conozcan. Los
    /// miembros detectan la ausencia del coordinador por el TTL de failover y
    /// promueven al backup o quedan sin plantilla, que es el mismo camino que
    /// si el coordinador se apagara.
    pub async fn disband(&self) -> Result<(), TeamError> {
        let state = self
            .store
            .lock()
            .await
            .state()
            .cloned()
            .ok_or(TeamError::InvalidTeam)?;
        if state.coordinator != self.local_address {
            return Err(TeamError::CoordinatorCannotLeave);
        }
        self.store.lock().await.clear()?;
        *self.join_status.lock().await = "not_joined".to_string();
        Ok(())
    }

    pub async fn ingest_membership(
        &self,
        membership: TeamMembership,
        source_peer: String,
        now: u64,
    ) -> Result<(), TeamError> {
        membership.validate()?;
        let mut store = self.store.lock().await;
        let state = store.state().cloned().ok_or(TeamError::InvalidTeam)?;
        if state.team_id != membership.team_id || state.coordinator != self.local_address {
            return Err(TeamError::NotCoordinator);
        }
        if let Some(bound) = self
            .member_peers
            .lock()
            .await
            .get(&membership.member_address)
            .cloned()
        {
            if bound != source_peer {
                return Err(TeamError::PeerBindingMismatch);
            }
        }
        let network_difficulty = state
            .active_template
            .as_ref()
            .map(|template| template.network_difficulty_bits)
            .unwrap_or(1);
        match membership.action {
            MembershipAction::Join => {
                store.mutate(|state| {
                    state
                        .add_member(membership.member_address.clone(), network_difficulty)
                        .map(|_| ())
                })?;
                self.member_peers
                    .lock()
                    .await
                    .insert(membership.member_address, source_peer);
            }
            MembershipAction::Leave => {
                store.mutate(|state| state.leave_member(&membership.member_address))?;
                self.member_peers
                    .lock()
                    .await
                    .remove(&membership.member_address);
            }
        }
        let state = store.state().cloned().ok_or(TeamError::InvalidTeam)?;
        drop(store);
        self.publish_authoritative_state(&state, now).await;
        Ok(())
    }

    pub async fn ingest_turn_update(
        &self,
        update: TeamTurnUpdate,
        source_peer: String,
        now: u64,
    ) -> Result<(), TeamError> {
        update.validate()?;
        let incoming = update.state;
        let current = self.store.lock().await.state().cloned();
        let allowed = match current.as_ref() {
            None => {
                self.pending_join_team.lock().await.as_deref() == Some(incoming.team_id.as_str())
                    && incoming
                        .members
                        .get(&self.local_address)
                        .is_some_and(|member| member.active && !member.expelled)
            }
            Some(current) if current.team_id != incoming.team_id => false,
            Some(current) if incoming.epoch == current.epoch => {
                incoming.coordinator == current.coordinator
            }
            Some(current) if incoming.epoch == current.epoch.saturating_add(1) => {
                current.backup_coordinator.as_deref() == Some(incoming.coordinator.as_str())
            }
            Some(_) => false,
        };
        if !allowed {
            return Err(TeamError::InvalidCoordinatorUpdate);
        }
        if let Some(current) = current.as_ref() {
            let revision_regressed = incoming.epoch == current.epoch
                && incoming.revision < current.revision;
            let failover_did_not_advance = incoming.epoch > current.epoch
                && incoming.revision <= current.revision;
            if revision_regressed || failover_did_not_advance {
                return Err(TeamError::InvalidCoordinatorUpdate);
            }
        }
        let expected_peer = self
            .member_peers
            .lock()
            .await
            .get(&incoming.coordinator)
            .cloned();
        if expected_peer
            .as_deref()
            .is_some_and(|peer| peer != source_peer)
        {
            return Err(TeamError::PeerBindingMismatch);
        }
        self.member_peers
            .lock()
            .await
            .insert(incoming.coordinator.clone(), source_peer);
        if current.as_ref().is_some_and(|current| {
            incoming.epoch == current.epoch && incoming.revision == current.revision
        }) {
            self.store.lock().await.mutate_local(|state| {
                state.note_coordinator_seen(&incoming.coordinator, incoming.epoch, now);
                Ok(())
            })?;
            *self.pending_join_team.lock().await = None;
            *self.join_status.lock().await = "joined".to_string();
            return Ok(());
        }
        let mut incoming = incoming;
        incoming.note_coordinator_seen(&incoming.coordinator.clone(), incoming.epoch, now);
        *self.pending_join_team.lock().await = None;
        let local_is_active = incoming
            .members
            .get(&self.local_address)
            .is_some_and(|member| member.active && !member.expelled);
        if local_is_active {
            self.store.lock().await.replace(incoming)?;
            *self.join_status.lock().await = "joined".to_string();
        } else {
            self.store.lock().await.clear()?;
            self.member_peers.lock().await.clear();
            *self.enabled.lock().await = false;
            *self.hashing.lock().await = false;
            *self.join_status.lock().await = "not_joined".to_string();
        }
        Ok(())
    }

    pub async fn submit_local_share(
        &self,
        share: SubmittedShare,
        now: u64,
    ) -> Result<ValidatedShare, TeamError> {
        let is_coordinator = self
            .store
            .lock()
            .await
            .state()
            .is_some_and(|state| state.coordinator == self.local_address);
        if is_coordinator {
            self.accept_share(share, None, now).await
        } else {
            let validation = self
                .store
                .lock()
                .await
                .state()
                .ok_or(TeamError::InvalidTeam)?
                .validate_share(&share)?;
            self.queue_message(crate::network::message::ZokaMessage::ShareSubmit(share))
                .await;
            Ok(validation)
        }
    }

    pub async fn ingest_share(
        &self,
        share: SubmittedShare,
        source_peer: String,
        now: u64,
    ) -> Result<ValidatedShare, TeamError> {
        self.accept_share(share, Some(source_peer), now).await
    }

    async fn accept_share(
        &self,
        share: SubmittedShare,
        source_peer: Option<String>,
        now: u64,
    ) -> Result<ValidatedShare, TeamError> {
        let mut store = self.store.lock().await;
        let state = store.state().cloned().ok_or(TeamError::InvalidTeam)?;
        if state.coordinator != self.local_address {
            return Err(TeamError::NotCoordinator);
        }
        if let Some(source_peer) = source_peer {
            let bound = self
                .member_peers
                .lock()
                .await
                .get(&share.miner_address)
                .cloned()
                .ok_or(TeamError::PeerBindingMismatch)?;
            if bound != source_peer {
                return Err(TeamError::PeerBindingMismatch);
            }
        } else if share.miner_address != self.local_address {
            return Err(TeamError::PeerBindingMismatch);
        }
        let validated = state.validate_share(&share)?;
        let template = state
            .active_template
            .clone()
            .ok_or(TeamError::NoActiveTemplate)?;
        store.mutate(|state| state.record_validated_share(&share, &validated))?;
        let updated = store.state().cloned().ok_or(TeamError::InvalidTeam)?;
        drop(store);
        if validated.network_solution {
            self.pending_network_solutions
                .lock()
                .await
                .push(NetworkSolution { share, template });
        }
        self.publish_turn_update(&updated, now).await;
        Ok(validated)
    }

    pub async fn take_network_solution(&self) -> Option<NetworkSolution> {
        let mut solutions = self.pending_network_solutions.lock().await;
        if solutions.is_empty() {
            None
        } else {
            Some(solutions.remove(0))
        }
    }

    pub async fn set_template(
        &self,
        chain_id: String,
        prev_hash: String,
        height: u64,
        timestamp: u64,
        network_difficulty_bits: u32,
    ) -> Result<MiningTemplate, TeamError> {
        let mut store = self.store.lock().await;
        if store
            .state()
            .is_none_or(|state| state.coordinator != self.local_address)
        {
            return Err(TeamError::NotCoordinator);
        }
        let template = store.mutate(|state| {
            state.set_template(
                chain_id,
                prev_hash,
                height,
                timestamp,
                network_difficulty_bits,
            )
        })?;
        let updated = store.state().cloned().ok_or(TeamError::InvalidTeam)?;
        drop(store);
        self.publish_turn_update(&updated, timestamp).await;
        Ok(template)
    }

    pub async fn record_confirmed_block(
        &self,
        height: u64,
        prev_hash: &str,
        recipient: &str,
        source_peer: Option<String>,
        now: u64,
    ) -> Result<(), TeamError> {
        let mut store = self.store.lock().await;
        let state = store.state().cloned().ok_or(TeamError::InvalidTeam)?;
        if state.coordinator != self.local_address {
            return Err(TeamError::NotCoordinator);
        }
        let template = state
            .active_template
            .as_ref()
            .ok_or(TeamError::NoActiveTemplate)?;
        if template.height != height || template.prev_hash != prev_hash {
            return Err(TeamError::StaleTemplate);
        }
        let mut hijacked = false;
        store.mutate(|state| match state.confirm_assigned_block(recipient) {
            Ok(()) => Ok(()),
            Err(TeamError::CoinbaseHijacked { .. }) => {
                hijacked = true;
                Ok(())
            }
            Err(error) => Err(error),
        })?;
        if hijacked {
            if let Some(peer) = source_peer {
                let finder = self
                    .member_peers
                    .lock()
                    .await
                    .iter()
                    .find_map(|(address, bound)| (bound == &peer).then_some(address.clone()));
                if let Some(finder) = finder {
                    let _ = store.mutate(|state| state.expel_member(&finder));
                }
            }
        }
        let updated = store.state().cloned().ok_or(TeamError::InvalidTeam)?;
        drop(store);
        self.publish_authoritative_state(&updated, now).await;
        Ok(())
    }

    pub async fn tick(&self, now: u64) -> Result<(), TeamError> {
        let state = self.store.lock().await.state().cloned();
        let Some(state) = state else {
            return Ok(());
        };
        if state.backup_should_take_over(&self.local_address, now) {
            self.store
                .lock()
                .await
                .mutate(|state| state.promote_backup(&self.local_address, now))?;
            let promoted = self
                .store
                .lock()
                .await
                .state()
                .cloned()
                .ok_or(TeamError::InvalidTeam)?;
            self.publish_authoritative_state(&promoted, now).await;
            return Ok(());
        }
        if state.coordinator == self.local_address {
            let mut last = self.last_broadcast_at.lock().await;
            if now.saturating_sub(*last) >= TEAM_HEARTBEAT_SECONDS {
                *last = now;
                drop(last);
                self.publish_authoritative_state(&state, now).await;
            }
        }
        Ok(())
    }

    pub async fn status(&self) -> TeamRuntimeStatus {
        let team = self.store.lock().await.state().cloned();
        let (combined_hashrate_hps, local_accepted_shares, team_accepted_shares, estimated_turn) =
            if let Some(state) = &team {
                let combined = state
                    .active_members()
                    .map(|member| member.estimated_hashrate_hps)
                    .fold(0u64, u64::saturating_add);
                let local = state
                    .members
                    .get(&self.local_address)
                    .map(|member| member.accepted_shares)
                    .unwrap_or(0);
                let total = state
                    .members
                    .values()
                    .map(|member| member.accepted_shares)
                    .fold(0u64, u64::saturating_add);
                let turn = state
                    .turn_order()
                    .iter()
                    .position(|address| address == &self.local_address)
                    .map(|position| position + 1);
                (combined, local, total, turn)
            } else {
                (0, 0, 0, None)
            };
        TeamRuntimeStatus {
            enabled: *self.enabled.lock().await,
            hashing: *self.hashing.lock().await,
            threads: *self.threads.lock().await,
            join_status: self.join_status.lock().await.clone(),
            local_address: self.local_address.clone(),
            team,
            combined_hashrate_hps,
            local_accepted_shares,
            team_accepted_shares,
            estimated_turn,
        }
    }

    async fn publish_turn_update(&self, state: &TeamState, now: u64) {
        self.queue_message(crate::network::message::ZokaMessage::TurnUpdate(
            TeamTurnUpdate {
                state: state.clone(),
                timestamp: now,
            },
        ))
        .await;
    }

    async fn publish_authoritative_state(&self, state: &TeamState, now: u64) {
        self.queue_message(crate::network::message::ZokaMessage::TeamAdvertise(
            TeamAdvertise::from_state(state, now),
        ))
        .await;
        self.publish_turn_update(state, now).await;
    }
}

impl TeamStore {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, TeamError> {
        let path = path.into();
        let state = if path.exists() {
            let bytes =
                fs::read(&path).map_err(|error| TeamError::Persistence(error.to_string()))?;
            let state: TeamState = serde_json::from_slice(&bytes)
                .map_err(|error| TeamError::Persistence(error.to_string()))?;
            state.validate()?;
            Some(state)
        } else {
            None
        };
        Ok(Self { path, state })
    }

    pub fn memory() -> Self {
        Self {
            path: PathBuf::new(),
            state: None,
        }
    }

    pub fn state(&self) -> Option<&TeamState> {
        self.state.as_ref()
    }

    pub fn replace(&mut self, state: TeamState) -> Result<(), TeamError> {
        state.validate()?;
        let previous = self.state.replace(state);
        if let Err(error) = self.persist() {
            self.state = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn mutate<T>(
        &mut self,
        mutate: impl FnOnce(&mut TeamState) -> Result<T, TeamError>,
    ) -> Result<T, TeamError> {
        let previous = self.state.clone();
        let state = self.state.as_mut().ok_or(TeamError::InvalidTeam)?;
        let result = match mutate(state) {
            Ok(result) => result,
            Err(error) => {
                self.state = previous;
                return Err(error);
            }
        };
        state.revision = state.revision.saturating_add(1);
        if let Err(error) = state.validate() {
            self.state = previous;
            return Err(error);
        }
        if let Err(error) = self.persist() {
            self.state = previous;
            return Err(error);
        }
        Ok(result)
    }

    fn mutate_local<T>(
        &mut self,
        mutate: impl FnOnce(&mut TeamState) -> Result<T, TeamError>,
    ) -> Result<T, TeamError> {
        let previous = self.state.clone();
        let state = self.state.as_mut().ok_or(TeamError::InvalidTeam)?;
        let result = match mutate(state) {
            Ok(result) => result,
            Err(error) => {
                self.state = previous;
                return Err(error);
            }
        };
        if let Err(error) = state.validate() {
            self.state = previous;
            return Err(error);
        }
        if let Err(error) = self.persist() {
            self.state = previous;
            return Err(error);
        }
        Ok(result)
    }

    pub fn clear(&mut self) -> Result<(), TeamError> {
        let previous = self.state.take();
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        if self.path.exists() {
            if let Err(error) = fs::remove_file(&self.path) {
                self.state = previous;
                return Err(TeamError::Persistence(error.to_string()));
            }
        }
        Ok(())
    }

    fn persist(&self) -> Result<(), TeamError> {
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        let Some(state) = &self.state else {
            return Ok(());
        };
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| TeamError::Persistence(error.to_string()))?;
        }
        let temp = temporary_path(&self.path);
        let bytes = serde_json::to_vec_pretty(state)
            .map_err(|error| TeamError::Persistence(error.to_string()))?;
        fs::write(&temp, bytes).map_err(|error| TeamError::Persistence(error.to_string()))?;
        if let Err(error) = fs::rename(&temp, &self.path) {
            let _ = fs::remove_file(&temp);
            return Err(TeamError::Persistence(error.to_string()));
        }
        Ok(())
    }
}

pub fn initial_share_difficulty(network_difficulty_bits: u32) -> u32 {
    network_difficulty_bits.saturating_sub(8).max(1)
}

pub fn share_work(difficulty_bits: u32) -> u64 {
    1u64.checked_shl(difficulty_bits.min(62))
        .unwrap_or(u64::MAX)
}

pub fn new_team_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn invite_code_for(team_id: &str) -> String {
    let normalized = team_id.to_ascii_lowercase();
    let mut hasher = Sha256::new();
    hasher.update(b"zoka-team-invite-v1");
    hasher.update([0x1f]);
    hasher.update(normalized.as_bytes());
    let checksum = hex::encode(&hasher.finalize()[..4]).to_ascii_uppercase();
    format!(
        "{}-{}-{}",
        TEAM_INVITE_PREFIX,
        normalized.to_ascii_uppercase(),
        checksum
    )
}

pub fn parse_invite_code(code: &str) -> Option<String> {
    let parts = code.trim().split('-').collect::<Vec<_>>();
    if parts.len() != 3 || parts[0].to_ascii_uppercase() != TEAM_INVITE_PREFIX {
        return None;
    }
    let team_id = parts[1].to_ascii_lowercase();
    if team_id.len() != 32 || !team_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    (invite_code_for(&team_id) == code.trim().to_ascii_uppercase()).then_some(team_id)
}

fn validate_address(address: &str) -> Result<(), TeamError> {
    crate::private_wallet::canonical_payment_address(address)
        .map(|_| ())
        .map_err(|_| TeamError::InvalidAddress)
}

fn template_id(
    team_id: &str,
    epoch: u64,
    chain_id: &str,
    prev_hash: &str,
    height: u64,
    timestamp: u64,
    recipient_address: &str,
    network_difficulty_bits: u32,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"zoka-team-template-v1");
    for field in [team_id, chain_id, prev_hash, recipient_address] {
        hasher.update([0x1f]);
        hasher.update(field.as_bytes());
    }
    hasher.update(epoch.to_le_bytes());
    hasher.update(height.to_le_bytes());
    hasher.update(timestamp.to_le_bytes());
    hasher.update(network_difficulty_bits.to_le_bytes());
    hex::encode(hasher.finalize())
}

/// Denominadores del reparto por turnos, calculados SOLO sobre los miembros
/// activos.
///
/// `state.total_work` y `state.confirmed_blocks` son historicos: nunca bajan
/// cuando alguien deja el equipo. Usarlos como denominador mientras el conjunto
/// de candidatos ya excluye a quien se fue mezcla dos poblaciones distintas, y
/// el efecto es que el trabajo del que se fue diluye para siempre el derecho de
/// los que quedan. En la practica eso sesga el reparto hacia quien menos aporta:
/// con un equipo 60/30/10 donde se va el mayor aportante, los que quedan
/// recibian 70.5/29.5 en vez del 75/25 que les corresponde.
///
/// Se calculan aca en vez de descontar al salir a proposito: no muta el estado
/// persistido, no cambia el esquema y `TeamState::validate` no compara estos
/// totales, asi que los nodos que todavia no tengan este arreglo siguen
/// aceptando el estado que publica el coordinador.
fn active_turn_totals(active: &[&TeamMember]) -> (u64, u64) {
    let work = active
        .iter()
        .fold(0u64, |acc, member| acc.saturating_add(member.total_work));
    let paid = active
        .iter()
        .fold(0u64, |acc, member| acc.saturating_add(member.blocks_paid));
    (work, paid.saturating_add(1))
}

fn compare_turn_debt(
    left: &TeamMember,
    right: &TeamMember,
    total_work: u64,
    projected_blocks: u64,
) -> Ordering {
    let left_score = (left.total_work as u128)
        .saturating_mul(projected_blocks as u128)
        .saturating_add((right.blocks_paid as u128).saturating_mul(total_work as u128));
    let right_score = (right.total_work as u128)
        .saturating_mul(projected_blocks as u128)
        .saturating_add((left.blocks_paid as u128).saturating_mul(total_work as u128));
    left_score.cmp(&right_score)
}

fn adjust_member_difficulty(member: &mut TeamMember, now: u64, network_difficulty_bits: u32) {
    if member.shares_since_adjustment < SHARES_PER_DIFFICULTY_ADJUSTMENT {
        return;
    }
    let Some(started) = member.adjustment_started_at else {
        return;
    };
    let elapsed = now.saturating_sub(started).max(1);
    let expected = SHARE_TARGET_SECONDS.saturating_mul(member.shares_since_adjustment);
    if elapsed < expected / 2 {
        member.share_difficulty_bits = member.share_difficulty_bits.saturating_add(1);
    } else if elapsed > expected.saturating_mul(2) {
        member.share_difficulty_bits = member.share_difficulty_bits.saturating_sub(1).max(1);
    }
    member.share_difficulty_bits = member
        .share_difficulty_bits
        .clamp(1, network_difficulty_bits.max(1));
    member.adjustment_started_at = Some(now);
    member.shares_since_adjustment = 0;
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut temp = path.as_os_str().to_os_string();
    temp.push(".tmp");
    PathBuf::from(temp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::private_wallet::PrivateWalletManager;
    use tempfile::tempdir;

    fn addresses() -> (String, String, String) {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        manager.create_private_wallet("bob").unwrap();
        manager.create_private_wallet("carol").unwrap();
        let names = ["alice", "bob", "carol"];
        let mut result = Vec::new();
        for name in names {
            let wallet = manager.get_private_wallet(name).unwrap();
            result.push(wallet.commitment_key.clone());
        }
        (result[0].clone(), result[1].clone(), result[2].clone())
    }

    fn valid_share(
        state: &TeamState,
        member: &str,
        timestamp: u64,
    ) -> (SubmittedShare, ValidatedShare) {
        let template = state.active_template.as_ref().unwrap();
        let difficulty = state.members[member].share_difficulty_bits;
        let engine = crate::mining::pow::engine_for(&template.chain_id, template.height).unwrap();
        for nonce in 0..u64::MAX {
            let input = crate::mining::pow::build_pow_input(
                &template.chain_id,
                &template.prev_hash,
                &template.recipient_address,
                template.height,
                template.timestamp,
                nonce,
            );
            let hash = engine.calculate_hash(&input).unwrap();
            if crate::mining::pow::hash_meets_difficulty(&hash, difficulty) {
                let share = SubmittedShare {
                    team_id: state.team_id.clone(),
                    template_id: template.id.clone(),
                    miner_address: member.to_string(),
                    nonce,
                    pow_hash_hex: hex::encode(hash),
                    difficulty_bits: difficulty,
                    timestamp,
                };
                let validated = state.validate_share(&share).unwrap();
                return (share, validated);
            }
        }
        unreachable!()
    }

    #[test]
    fn invite_code_round_trips_and_detects_typos() {
        let id = "00112233445566778899aabbccddeeff";
        let code = invite_code_for(id);
        assert_eq!(parse_invite_code(&code).as_deref(), Some(id));
        assert!(parse_invite_code(&format!("{code}0")).is_none());
    }

    #[test]
    fn first_joiner_becomes_backup_without_a_team_size_cap() {
        let (alice, bob, carol) = addresses();
        let mut state = TeamState::create(alice.clone(), 100).unwrap();
        state.add_member(bob.clone(), 16).unwrap();
        state.add_member(carol.clone(), 16).unwrap();
        assert_eq!(state.backup_coordinator.as_deref(), Some(bob.as_str()));
        assert_eq!(state.active_members().count(), 3);
    }

    #[test]
    fn shares_are_recomputed_and_replays_are_rejected() {
        let (alice, bob, _) = addresses();
        let mut state = TeamState::create(alice.clone(), 100).unwrap();
        state.add_member(bob.clone(), 6).unwrap();
        state
            .set_template("zoka-dev-team".to_string(), "parent".to_string(), 1, 101, 6)
            .unwrap();
        let (share, validated) = valid_share(&state, &bob, 110);
        state.record_validated_share(&share, &validated).unwrap();
        assert!(state.members[&bob].total_work > 0);
        assert_eq!(state.validate_share(&share), Err(TeamError::ReplayedShare));

        let mut forged = share.clone();
        forged.nonce = forged.nonce.saturating_add(1);
        assert_eq!(
            state.validate_share(&forged),
            Err(TeamError::InvalidShareHash)
        );
    }

    #[test]
    fn coordinator_starts_near_network_target_and_difficulty_adapts() {
        let (alice, _, _) = addresses();
        let mut state = TeamState::create(alice.clone(), 100).unwrap();
        assert_eq!(state.members[&alice].share_difficulty_bits, 0);
        state
            .set_template(
                "zoka-dev-team".to_string(),
                "parent".to_string(),
                1,
                101,
                20,
            )
            .unwrap();
        assert_eq!(
            state.members[&alice].share_difficulty_bits,
            initial_share_difficulty(20)
        );

        let member = state.members.get_mut(&alice).unwrap();
        member.shares_since_adjustment = SHARES_PER_DIFFICULTY_ADJUSTMENT;
        member.adjustment_started_at = Some(100);
        adjust_member_difficulty(member, 120, 20);
        assert_eq!(member.share_difficulty_bits, 13);

        member.shares_since_adjustment = SHARES_PER_DIFFICULTY_ADJUSTMENT;
        member.adjustment_started_at = Some(200);
        adjust_member_difficulty(member, 500, 20);
        assert_eq!(member.share_difficulty_bits, 12);
    }

    #[test]
    fn payout_turn_converges_by_contributed_work() {
        let (alice, bob, _) = addresses();
        let mut state = TeamState::create(alice.clone(), 100).unwrap();
        state.add_member(bob.clone(), 8).unwrap();
        state.members.get_mut(&alice).unwrap().total_work = 100;
        state.members.get_mut(&bob).unwrap().total_work = 300;
        state.total_work = 400;

        let mut paid = BTreeMap::<String, u64>::new();
        for _ in 0..4 {
            let recipient = state.next_recipient().unwrap();
            state.confirm_assigned_block(&recipient).unwrap();
            *paid.entry(recipient).or_default() += 1;
        }
        assert_eq!(paid.get(&bob), Some(&3));
        assert_eq!(paid.get(&alice), Some(&1));
    }

    #[test]
    fn backup_promotes_only_after_coordinator_ttl() {
        let (alice, bob, _) = addresses();
        let mut state = TeamState::create(alice, 100).unwrap();
        state.add_member(bob.clone(), 8).unwrap();
        assert_eq!(
            state.promote_backup(&bob, 190),
            Err(TeamError::FailoverNotReady)
        );
        state.promote_backup(&bob, 191).unwrap();
        assert_eq!(state.coordinator, bob);
        assert_eq!(state.epoch, 1);
    }

    #[test]
    fn turn_rotation_stays_proportional_after_a_member_leaves() {
        // Regresion: `total_work` y `confirmed_blocks` son historicos y nunca
        // bajan cuando alguien deja el equipo. Si se usan como denominador
        // mientras los candidatos ya excluyen al que se fue, su trabajo diluye
        // para siempre el derecho de los que quedan y el reparto se sesga hacia
        // quien menos aporta.
        let (alice, bob, carol) = addresses();
        let mut state = TeamState::create(alice.clone(), 100).unwrap();
        state.add_member(bob.clone(), 8).unwrap();
        state.add_member(carol.clone(), 8).unwrap();

        // alice aporto mucho y ya cobro lo suyo; bob y carol quedan en una
        // proporcion 3:1 entre ellos.
        state.members.get_mut(&alice).unwrap().total_work = 6_000;
        state.members.get_mut(&alice).unwrap().blocks_paid = 60;
        state.members.get_mut(&bob).unwrap().total_work = 3_000;
        state.members.get_mut(&bob).unwrap().blocks_paid = 30;
        state.members.get_mut(&carol).unwrap().total_work = 1_000;
        state.members.get_mut(&carol).unwrap().blocks_paid = 10;
        state.total_work = 10_000;
        state.confirmed_blocks = 100;

        // alice se va: quedan bob (75% del trabajo activo) y carol (25%).
        // Se marca inactiva directamente porque es la coordinadora y
        // leave_member la rechaza -- ese hueco se resuelve aparte.
        state.members.get_mut(&alice).unwrap().active = false;

        let mut bob_turns = 0u32;
        let mut carol_turns = 0u32;
        for _ in 0..400 {
            let winner = state.next_recipient().expect("hay miembros activos");
            if winner == bob {
                bob_turns += 1;
            } else if winner == carol {
                carol_turns += 1;
            } else {
                panic!("un miembro inactivo recibio un turno: {winner}");
            }
            // el ganador cobra y ambos siguen aportando en su proporcion
            let member = state.members.get_mut(&winner).unwrap();
            member.blocks_paid = member.blocks_paid.saturating_add(1);
            state.confirmed_blocks = state.confirmed_blocks.saturating_add(1);
            state.members.get_mut(&bob).unwrap().total_work += 30;
            state.members.get_mut(&carol).unwrap().total_work += 10;
            state.total_work += 40;
        }

        let total = bob_turns + carol_turns;
        let bob_share = 100 * bob_turns / total;
        // Con el denominador historico esto daba ~70%; lo correcto es ~75%.
        assert!(
            (73..=77).contains(&bob_share),
            "bob aporta el 75% del trabajo activo pero recibio {bob_share}% de los turnos"
        );
    }

    #[test]
    fn coordinator_hands_over_instead_of_being_trapped() {
        // El coordinador no podia salir nunca (CoordinatorCannotLeave), asi que
        // quedaba atrapado en su propio equipo.
        let (alice, bob, carol) = addresses();
        let mut state = TeamState::create(alice.clone(), 100).unwrap();
        state.add_member(bob.clone(), 8).unwrap();
        state.add_member(carol.clone(), 8).unwrap();
        assert_eq!(state.coordinator, alice);

        let successor = state.hand_over_coordination(200).unwrap();

        assert_ne!(successor, alice, "el sucesor no puede ser quien se va");
        assert_eq!(state.coordinator, successor);
        assert!(
            !state.members[&alice].active,
            "quien traspasa queda inactivo"
        );
        assert!(state.members[&successor].active);
        assert_eq!(state.epoch, 1, "el traspaso abre una epoca nueva");
        assert!(
            state.active_template.is_none(),
            "la plantilla del coordinador anterior no sigue vigente"
        );
        assert_ne!(state.backup_coordinator.as_deref(), Some(successor.as_str()));
        // el estado sigue siendo valido para los pares que lo reciban
        state.validate().unwrap();
    }

    #[test]
    fn handover_without_anyone_to_promote_is_rejected() {
        // Un coordinador solo no puede traspasar: ahi el unico camino es
        // disolver, y eso lo pide la interfaz explicitamente.
        let (alice, _, _) = addresses();
        let mut state = TeamState::create(alice, 100).unwrap();
        assert_eq!(
            state.hand_over_coordination(200),
            Err(TeamError::NoActiveMembers)
        );
    }

    #[test]
    fn persistent_store_round_trips_or_rolls_back() {
        let (alice, bob, _) = addresses();
        let dir = tempdir().unwrap();
        let path = dir.path().join("team.json");
        let state = TeamState::create(alice, 100).unwrap();
        let mut store = TeamStore::load(&path).unwrap();
        store.replace(state).unwrap();
        store
            .mutate(|state| state.add_member(bob.clone(), 12).map(|_| ()))
            .unwrap();

        let loaded = TeamStore::load(&path).unwrap();
        assert!(loaded.state().unwrap().members.contains_key(&bob));
    }

    #[test]
    fn unexpected_coinbase_is_flagged_and_charged_to_its_actual_recipient() {
        let (alice, bob, carol) = addresses();
        let mut state = TeamState::create(alice, 100).unwrap();
        state.add_member(bob.clone(), 8).unwrap();
        state.add_member(carol.clone(), 8).unwrap();
        state.members.get_mut(&bob).unwrap().total_work = 10;
        state.total_work = 10;
        assert!(matches!(
            state.confirm_assigned_block(&carol),
            Err(TeamError::CoinbaseHijacked { .. })
        ));
        assert_eq!(state.confirmed_blocks, 1);
        assert_eq!(state.members[&carol].blocks_paid, 1);
        assert_eq!(
            state.last_hijacked_recipient.as_deref(),
            Some(carol.as_str())
        );
    }

    #[tokio::test]
    async fn invite_join_and_full_state_transfer_bind_both_node_identities() {
        let (alice, bob, _) = addresses();
        let coordinator = TeamMiningRuntime::new(TeamStore::memory(), alice.clone()).unwrap();
        let member = TeamMiningRuntime::new(TeamStore::memory(), bob.clone()).unwrap();
        let created = coordinator.create_team(100).await.unwrap();
        let messages = coordinator.drain_outbound_messages().await;
        let advertisement = messages
            .into_iter()
            .find_map(|message| match message {
                crate::network::message::ZokaMessage::TeamAdvertise(advertisement) => {
                    Some(advertisement)
                }
                _ => None,
            })
            .unwrap();
        member
            .observe_advertisement(advertisement, "peer-alice".to_string(), 101)
            .await
            .unwrap();
        member
            .request_join(&created.invite_code, 102)
            .await
            .unwrap();
        let join = member
            .drain_outbound_messages()
            .await
            .into_iter()
            .find_map(|message| match message {
                crate::network::message::ZokaMessage::TeamJoin(join) => Some(join),
                _ => None,
            })
            .unwrap();
        coordinator
            .ingest_membership(join, "peer-bob".to_string(), 103)
            .await
            .unwrap();
        let update = coordinator
            .drain_outbound_messages()
            .await
            .into_iter()
            .find_map(|message| match message {
                crate::network::message::ZokaMessage::TurnUpdate(update) => Some(update),
                _ => None,
            })
            .unwrap();
        let stale_update = update.clone();
        member
            .ingest_turn_update(update, "peer-alice".to_string(), 104)
            .await
            .unwrap();

        let status = member.status().await;
        assert_eq!(status.join_status, "joined");
        assert_eq!(
            status.team.as_ref().unwrap().backup_coordinator.as_deref(),
            Some(bob.as_str())
        );

        coordinator
            .set_template("zoka-dev-team".to_string(), "parent".to_string(), 1, 105, 6)
            .await
            .unwrap();
        let fresh_update = coordinator
            .drain_outbound_messages()
            .await
            .into_iter()
            .find_map(|message| match message {
                crate::network::message::ZokaMessage::TurnUpdate(update) => Some(update),
                _ => None,
            })
            .unwrap();
        member
            .ingest_turn_update(fresh_update, "peer-alice".to_string(), 106)
            .await
            .unwrap();
        assert_eq!(
            member
                .ingest_turn_update(stale_update, "peer-alice".to_string(), 107)
                .await,
            Err(TeamError::InvalidCoordinatorUpdate)
        );
    }

    #[tokio::test]
    async fn coordinator_rejects_a_share_from_the_wrong_signed_peer() {
        let (alice, bob, _) = addresses();
        let coordinator = TeamMiningRuntime::new(TeamStore::memory(), alice.clone()).unwrap();
        coordinator.create_team(100).await.unwrap();
        let team_id = coordinator
            .store
            .lock()
            .await
            .state()
            .unwrap()
            .team_id
            .clone();
        coordinator
            .ingest_membership(
                TeamMembership {
                    team_id,
                    member_address: bob.clone(),
                    action: MembershipAction::Join,
                    timestamp: 101,
                },
                "peer-bob".to_string(),
                101,
            )
            .await
            .unwrap();
        coordinator
            .set_template("zoka-dev-team".to_string(), "parent".to_string(), 1, 102, 5)
            .await
            .unwrap();
        let state = coordinator.store.lock().await.state().unwrap().clone();
        let (share, _) = valid_share(&state, &bob, 103);

        assert_eq!(
            coordinator
                .ingest_share(share, "peer-impostor".to_string(), 103)
                .await,
            Err(TeamError::PeerBindingMismatch)
        );
    }

    #[tokio::test]
    async fn coordinator_expels_source_bound_member_after_coinbase_hijack() {
        let (alice, bob, _) = addresses();
        let coordinator = TeamMiningRuntime::new(TeamStore::memory(), alice.clone()).unwrap();
        coordinator.create_team(100).await.unwrap();
        let team_id = coordinator
            .store
            .lock()
            .await
            .state()
            .unwrap()
            .team_id
            .clone();
        coordinator
            .ingest_membership(
                TeamMembership {
                    team_id,
                    member_address: bob.clone(),
                    action: MembershipAction::Join,
                    timestamp: 101,
                },
                "peer-bob".to_string(),
                101,
            )
            .await
            .unwrap();
        coordinator
            .set_template(
                "zoka-dev-team".to_string(),
                "parent".to_string(),
                1,
                102,
                5,
            )
            .await
            .unwrap();

        coordinator
            .record_confirmed_block(
                1,
                "parent",
                &bob,
                Some("peer-bob".to_string()),
                103,
            )
            .await
            .unwrap();

        let status = coordinator.status().await;
        let team = status.team.unwrap();
        assert!(team.members[&bob].expelled);
        assert!(!team.members[&bob].active);
        assert_eq!(team.last_hijacked_recipient.as_deref(), Some(bob.as_str()));
    }

    #[tokio::test]
    async fn accepted_leave_clears_member_state_and_stops_hashing() {
        let (alice, bob, _) = addresses();
        let coordinator = TeamMiningRuntime::new(TeamStore::memory(), alice.clone()).unwrap();
        let member = TeamMiningRuntime::new(TeamStore::memory(), bob.clone()).unwrap();
        let created = coordinator.create_team(100).await.unwrap();
        let advertisement = coordinator
            .drain_outbound_messages()
            .await
            .into_iter()
            .find_map(|message| match message {
                crate::network::message::ZokaMessage::TeamAdvertise(advertisement) => {
                    Some(advertisement)
                }
                _ => None,
            })
            .unwrap();
        member
            .observe_advertisement(advertisement, "peer-alice".to_string(), 101)
            .await
            .unwrap();
        member.request_join(&created.invite_code, 102).await.unwrap();
        let join = member
            .drain_outbound_messages()
            .await
            .into_iter()
            .find_map(|message| match message {
                crate::network::message::ZokaMessage::TeamJoin(join) => Some(join),
                _ => None,
            })
            .unwrap();
        coordinator
            .ingest_membership(join, "peer-bob".to_string(), 103)
            .await
            .unwrap();
        let joined = coordinator
            .drain_outbound_messages()
            .await
            .into_iter()
            .find_map(|message| match message {
                crate::network::message::ZokaMessage::TurnUpdate(update) => Some(update),
                _ => None,
            })
            .unwrap();
        member
            .ingest_turn_update(joined, "peer-alice".to_string(), 104)
            .await
            .unwrap();
        member.set_enabled(true, 2).await;

        member.request_leave(105).await.unwrap();
        let leave = member
            .drain_outbound_messages()
            .await
            .into_iter()
            .find_map(|message| match message {
                crate::network::message::ZokaMessage::TeamJoin(leave) => Some(leave),
                _ => None,
            })
            .unwrap();
        coordinator
            .ingest_membership(leave, "peer-bob".to_string(), 106)
            .await
            .unwrap();
        let left = coordinator
            .drain_outbound_messages()
            .await
            .into_iter()
            .find_map(|message| match message {
                crate::network::message::ZokaMessage::TurnUpdate(update) => Some(update),
                _ => None,
            })
            .unwrap();
        member
            .ingest_turn_update(left, "peer-alice".to_string(), 107)
            .await
            .unwrap();

        let status = member.status().await;
        assert_eq!(status.join_status, "not_joined");
        assert!(!status.enabled);
        assert!(!status.hashing);
        assert!(status.team.is_none());
    }
}
