//! Canonical mainnet genesis definition (`zoka-genesis-v1`).
//!
//! Phase 3 of the mainnet roadmap requires a single, versioned genesis
//! document that every node consumes byte-identically. This module defines:
//!
//! - [`MainnetGenesisV1`]: the typed schema. Every field is consensus-
//!   relevant; `#[serde(deny_unknown_fields)]` blocks accidental drift.
//! - [`compute_digest`]: deterministic SHA-256 over the canonical encoding
//!   used to bind a `genesis.mainnet.json` to the chain state in
//!   `chain:genesis:digest`. Two nodes that load the same file get the
//!   same digest.
//! - [`validate_static`]: pure structural / consistency check. No DB, no
//!   network. Used by the `genesis-verify` CLI and by every loader path
//!   before touching storage.
//!
//! The mainnet binary publishes a single `docs/genesis.mainnet.v1.json`
//! signed by the core team. Operators run `zoka genesis-verify
//! docs/genesis.mainnet.v1.json` to confirm the digest matches the
//! announced value before they boot a node against it.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::economics;

/// Schema version embedded in the genesis file. The loader rejects
/// anything other than this value to force an explicit hard fork when
/// the schema evolves. Bumping this string is a consensus-breaking
/// change.
pub const GENESIS_SCHEMA_ID: &str = "zoka-genesis-v1";

/// Distinct roles a key can hold inside the validator set. Keeping
/// consensus and operator key material in separate fields stops the
/// hot-signing key (consensus) from leaking authority over operational
/// actions (operator) — a recurring foot-gun in PoS chains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum ValidatorRole {
    /// Active validator participating in PBFT for finalized blocks.
    Active,
    /// Standby validator that does not vote in epoch 0; promoted only by
    /// governance.
    Standby,
}

/// One row of the initial validator set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialValidator {
    /// Stable, human-readable identifier published by the core team
    /// (e.g. "validator-eu-1"). Used in dashboards and runbooks.
    pub label: String,
    /// Hex-encoded ed25519 public key used for PBFT consensus voting.
    /// Lives on the hot signing path.
    pub consensus_pubkey_hex: String,
    /// Hex-encoded ed25519 public key used for operator-only actions
    /// (key rotation, downtime announcements). MUST differ from
    /// `consensus_pubkey_hex`.
    pub operator_pubkey_hex: String,
    /// Voting power. Sum of all `Active` validators MUST be > 0.
    pub voting_power: u64,
    pub role: ValidatorRole,
}

/// Optional pre-mine line item published with the genesis. A `None` for
/// `pre_mine` means "no pre-mine, every ZKA is emitted by PoW". When
/// present, `total()` MUST be < `economics::MAX_SUPPLY_ZKA`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreMineEntry {
    pub recipient_address: String,
    pub amount_zka: u64,
    /// Free-form short rationale published with the genesis. Required;
    /// empty strings are rejected by `validate_static`.
    pub justification: String,
}

/// One entry in the regular initial-account section. Distinct from
/// `pre_mine` because pre-mine quantities count against `MAX_SUPPLY_ZKA`
/// while initial accounts represent on-chain bookkeeping (e.g. zero
/// balances published for transparency).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialAccount {
    pub address: String,
    pub balance_zka: u64,
}

/// Complete genesis document. Every byte feeds into the digest so any
/// edit produces a different digest — and therefore a different chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MainnetGenesisV1 {
    /// Schema discriminator. MUST equal [`GENESIS_SCHEMA_ID`].
    pub schema: String,
    /// Network id. Must match `economics::MAINNET_CHAIN_ID` for the
    /// canonical mainnet file; testnets carry their own chain id.
    pub chain_id: String,
    /// Algorithm id. Must equal `economics::pow_algorithm_for_chain(chain_id)`
    /// so we can never publish a genesis that contradicts the PoW backend
    /// the chain id implies.
    pub pow_algorithm: String,
    /// Network-wide unix timestamp at which the chain is considered
    /// "open". Informational; the loader records it but does not enforce
    /// wall-clock comparisons.
    pub genesis_timestamp: u64,
    /// Frozen economic parameters re-stated in the genesis itself so a
    /// node loading this file can detect a mismatch against its compiled
    /// `economics` constants.
    pub economics: GenesisEconomics,
    /// Validator set at chain birth. MAY be empty for pure-PoW chains
    /// that defer PBFT to a later phase.
    pub validators: Vec<InitialValidator>,
    /// Pre-mine line items that count against `MAX_SUPPLY_ZKA`. Pass
    /// an empty `Vec` (not `null`) when there is no pre-mine.
    pub pre_mine: Vec<PreMineEntry>,
    /// Bookkeeping accounts not counted against the supply cap.
    pub initial_accounts: Vec<InitialAccount>,
}

/// Economic constants restated inside the genesis so loaders can detect
/// a mismatch between a published genesis and the running binary's
/// compiled-in constants. Drift here is treated as a hard fork.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenesisEconomics {
    pub max_supply_zka: u64,
    pub initial_block_reward_zka: u64,
    pub halving_interval_blocks: u64,
    pub target_block_time_secs: u64,
    pub coinbase_maturity_blocks: u64,
}

impl GenesisEconomics {
    pub fn from_constants() -> Self {
        Self {
            max_supply_zka: economics::MAX_SUPPLY_ZKA,
            initial_block_reward_zka: economics::INITIAL_BLOCK_REWARD_ZKA,
            halving_interval_blocks: economics::HALVING_INTERVAL_BLOCKS,
            target_block_time_secs: economics::TARGET_BLOCK_TIME_SECS,
            coinbase_maturity_blocks: economics::COINBASE_MATURITY_BLOCKS,
        }
    }
}

/// Errors raised by [`validate_static`]. Each variant maps to one
/// concrete invariant the genesis violates so operators see exactly
/// what to fix.
#[derive(Debug, Clone, Error)]
pub enum GenesisError {
    #[error("genesis schema is '{got}', expected '{expected}'")]
    SchemaMismatch { got: String, expected: &'static str },

    #[error("genesis economics drift: field '{field}' has value {got}, binary expects {expected}")]
    EconomicsDrift {
        field: &'static str,
        got: u64,
        expected: u64,
    },

    #[error(
        "pow_algorithm '{got}' does not match canonical for chain '{chain_id}' ('{expected}')"
    )]
    PowAlgorithmMismatch {
        chain_id: String,
        got: String,
        expected: &'static str,
    },

    #[error("validator '{label}' has identical consensus and operator pubkeys; consensus and operator authority MUST be on different keys")]
    ValidatorKeyReuse { label: String },

    #[error("validator '{label}' has invalid {field}: {reason}")]
    ValidatorMalformed {
        label: String,
        field: &'static str,
        reason: String,
    },

    #[error("validator label '{label}' appears more than once")]
    DuplicateValidatorLabel { label: String },

    #[error(
        "validator pubkey '{pubkey}' (role={role:?}) appears in more than one validator entry"
    )]
    DuplicateValidatorPubkey { pubkey: String, role: &'static str },

    #[error("validator set has zero total active voting power; at least one Active validator with voting_power > 0 is required when validators are listed")]
    NoActiveVotingPower,

    #[error("pre-mine address '{address}' has zero amount_zka; remove the entry or set a positive amount")]
    PreMineZeroAmount { address: String },

    #[error("pre-mine entry for '{address}' has empty justification; every pre-mine line MUST publish its rationale")]
    PreMineMissingJustification { address: String },

    #[error("pre-mine total {total} exceeds {cap} (MAX_SUPPLY_ZKA)")]
    PreMineExceedsCap { total: u64, cap: u64 },

    #[error("address '{address}' appears in pre_mine and initial_accounts; pick one section")]
    AddressInBothSections { address: String },

    #[error("duplicate address '{address}' in {section}")]
    DuplicateAddress {
        address: String,
        section: &'static str,
    },

    #[error("address is empty or whitespace")]
    EmptyAddress,

    #[error("hex field {field} for entry '{entry}' is malformed: {reason}")]
    MalformedHex {
        entry: String,
        field: &'static str,
        reason: String,
    },
}

/// Pure validator: structural and consistency checks. Returns Ok(()) if
/// the genesis is internally coherent and matches the binary's compiled
/// constants. Performs no I/O.
pub fn validate_static(genesis: &MainnetGenesisV1) -> Result<(), GenesisError> {
    if genesis.schema != GENESIS_SCHEMA_ID {
        return Err(GenesisError::SchemaMismatch {
            got: genesis.schema.clone(),
            expected: GENESIS_SCHEMA_ID,
        });
    }

    let canonical_algo = economics::pow_algorithm_for_chain(&genesis.chain_id);
    if genesis.pow_algorithm != canonical_algo {
        return Err(GenesisError::PowAlgorithmMismatch {
            chain_id: genesis.chain_id.clone(),
            got: genesis.pow_algorithm.clone(),
            expected: canonical_algo,
        });
    }

    let expected = GenesisEconomics::from_constants();
    if genesis.economics.max_supply_zka != expected.max_supply_zka {
        return Err(GenesisError::EconomicsDrift {
            field: "max_supply_zka",
            got: genesis.economics.max_supply_zka,
            expected: expected.max_supply_zka,
        });
    }
    if genesis.economics.initial_block_reward_zka != expected.initial_block_reward_zka {
        return Err(GenesisError::EconomicsDrift {
            field: "initial_block_reward_zka",
            got: genesis.economics.initial_block_reward_zka,
            expected: expected.initial_block_reward_zka,
        });
    }
    if genesis.economics.halving_interval_blocks != expected.halving_interval_blocks {
        return Err(GenesisError::EconomicsDrift {
            field: "halving_interval_blocks",
            got: genesis.economics.halving_interval_blocks,
            expected: expected.halving_interval_blocks,
        });
    }
    if genesis.economics.target_block_time_secs != expected.target_block_time_secs {
        return Err(GenesisError::EconomicsDrift {
            field: "target_block_time_secs",
            got: genesis.economics.target_block_time_secs,
            expected: expected.target_block_time_secs,
        });
    }
    if genesis.economics.coinbase_maturity_blocks != expected.coinbase_maturity_blocks {
        return Err(GenesisError::EconomicsDrift {
            field: "coinbase_maturity_blocks",
            got: genesis.economics.coinbase_maturity_blocks,
            expected: expected.coinbase_maturity_blocks,
        });
    }

    validate_validator_set(&genesis.validators)?;

    let mut pre_mine_addrs: HashSet<&str> = HashSet::new();
    let mut pre_mine_total: u64 = 0;
    for entry in &genesis.pre_mine {
        if entry.recipient_address.trim().is_empty() {
            return Err(GenesisError::EmptyAddress);
        }
        if entry.amount_zka == 0 {
            return Err(GenesisError::PreMineZeroAmount {
                address: entry.recipient_address.clone(),
            });
        }
        if entry.justification.trim().is_empty() {
            return Err(GenesisError::PreMineMissingJustification {
                address: entry.recipient_address.clone(),
            });
        }
        if !pre_mine_addrs.insert(entry.recipient_address.as_str()) {
            return Err(GenesisError::DuplicateAddress {
                address: entry.recipient_address.clone(),
                section: "pre_mine",
            });
        }
        pre_mine_total = pre_mine_total.checked_add(entry.amount_zka).ok_or(
            GenesisError::PreMineExceedsCap {
                total: u64::MAX,
                cap: economics::MAX_SUPPLY_ZKA,
            },
        )?;
    }
    if pre_mine_total > economics::MAX_SUPPLY_ZKA {
        return Err(GenesisError::PreMineExceedsCap {
            total: pre_mine_total,
            cap: economics::MAX_SUPPLY_ZKA,
        });
    }

    let mut initial_addrs: HashSet<&str> = HashSet::new();
    for account in &genesis.initial_accounts {
        if account.address.trim().is_empty() {
            return Err(GenesisError::EmptyAddress);
        }
        if !initial_addrs.insert(account.address.as_str()) {
            return Err(GenesisError::DuplicateAddress {
                address: account.address.clone(),
                section: "initial_accounts",
            });
        }
        if pre_mine_addrs.contains(account.address.as_str()) {
            return Err(GenesisError::AddressInBothSections {
                address: account.address.clone(),
            });
        }
    }

    Ok(())
}

fn validate_validator_set(validators: &[InitialValidator]) -> Result<(), GenesisError> {
    let mut labels: HashSet<&str> = HashSet::new();
    let mut seen_pubkeys: HashSet<String> = HashSet::new();
    let mut active_voting_power: u64 = 0;
    let any_validators = !validators.is_empty();

    for v in validators {
        if v.label.trim().is_empty() {
            return Err(GenesisError::ValidatorMalformed {
                label: v.label.clone(),
                field: "label",
                reason: "label is empty".to_string(),
            });
        }
        if v.consensus_pubkey_hex == v.operator_pubkey_hex {
            return Err(GenesisError::ValidatorKeyReuse {
                label: v.label.clone(),
            });
        }
        validate_pubkey_hex(&v.label, "consensus_pubkey_hex", &v.consensus_pubkey_hex)?;
        validate_pubkey_hex(&v.label, "operator_pubkey_hex", &v.operator_pubkey_hex)?;
        if !labels.insert(v.label.as_str()) {
            return Err(GenesisError::DuplicateValidatorLabel {
                label: v.label.clone(),
            });
        }
        if !seen_pubkeys.insert(v.consensus_pubkey_hex.clone()) {
            return Err(GenesisError::DuplicateValidatorPubkey {
                pubkey: v.consensus_pubkey_hex.clone(),
                role: "consensus",
            });
        }
        if !seen_pubkeys.insert(v.operator_pubkey_hex.clone()) {
            return Err(GenesisError::DuplicateValidatorPubkey {
                pubkey: v.operator_pubkey_hex.clone(),
                role: "operator",
            });
        }
        if matches!(v.role, ValidatorRole::Active) {
            active_voting_power = active_voting_power.saturating_add(v.voting_power);
        }
    }

    if any_validators && active_voting_power == 0 {
        return Err(GenesisError::NoActiveVotingPower);
    }
    Ok(())
}

fn validate_pubkey_hex(label: &str, field: &'static str, value: &str) -> Result<(), GenesisError> {
    if value.trim().is_empty() {
        return Err(GenesisError::ValidatorMalformed {
            label: label.to_string(),
            field,
            reason: "empty".to_string(),
        });
    }
    let decoded = hex::decode(value).map_err(|e| GenesisError::MalformedHex {
        entry: label.to_string(),
        field,
        reason: e.to_string(),
    })?;
    if decoded.len() != 32 {
        return Err(GenesisError::ValidatorMalformed {
            label: label.to_string(),
            field,
            reason: format!("ed25519 pubkey must be 32 bytes, got {}", decoded.len()),
        });
    }
    Ok(())
}

/// Compute the canonical SHA-256 digest of a genesis document. Uses the
/// pretty-printed JSON encoding so the same digest can be reproduced by
/// any tool that parses the published file.
pub fn compute_digest(genesis: &MainnetGenesisV1) -> Result<String, String> {
    let canonical = serde_json::to_vec_pretty(genesis)
        .map_err(|e| format!("failed to canonicalize genesis: {e}"))?;
    let mut hasher = Sha256::new();
    hasher.update(&canonical);
    Ok(hex::encode(hasher.finalize()))
}

/// Aggregate result of `genesis-verify`: digest + a short multi-line
/// human report describing the structural facts of the file.
#[derive(Debug, Clone)]
pub struct GenesisVerifyReport {
    pub digest_hex: String,
    pub chain_id: String,
    pub pow_algorithm: String,
    pub validator_count: usize,
    pub active_voting_power: u64,
    pub pre_mine_total_zka: u64,
    pub initial_account_count: usize,
}

impl GenesisVerifyReport {
    pub fn from_genesis(genesis: &MainnetGenesisV1) -> Result<Self, GenesisError> {
        validate_static(genesis)?;
        let digest_hex = compute_digest(genesis).map_err(|e| GenesisError::ValidatorMalformed {
            label: "<digest>".to_string(),
            field: "json",
            reason: e,
        })?;
        let active_voting_power = genesis
            .validators
            .iter()
            .filter(|v| matches!(v.role, ValidatorRole::Active))
            .map(|v| v.voting_power)
            .fold(0u64, u64::saturating_add);
        let pre_mine_total = genesis
            .pre_mine
            .iter()
            .map(|p| p.amount_zka)
            .fold(0u64, u64::saturating_add);
        Ok(Self {
            digest_hex,
            chain_id: genesis.chain_id.clone(),
            pow_algorithm: genesis.pow_algorithm.clone(),
            validator_count: genesis.validators.len(),
            active_voting_power,
            pre_mine_total_zka: pre_mine_total,
            initial_account_count: genesis.initial_accounts.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_pubkey(byte: u8) -> String {
        hex::encode([byte; 32])
    }

    fn baseline() -> MainnetGenesisV1 {
        MainnetGenesisV1 {
            schema: GENESIS_SCHEMA_ID.to_string(),
            chain_id: economics::MAINNET_CHAIN_ID.to_string(),
            pow_algorithm: economics::POW_ALGORITHM_MAINNET.to_string(),
            genesis_timestamp: 1_700_000_000,
            economics: GenesisEconomics::from_constants(),
            validators: vec![
                InitialValidator {
                    label: "validator-1".to_string(),
                    consensus_pubkey_hex: dummy_pubkey(0x01),
                    operator_pubkey_hex: dummy_pubkey(0x02),
                    voting_power: 100,
                    role: ValidatorRole::Active,
                },
                InitialValidator {
                    label: "validator-2".to_string(),
                    consensus_pubkey_hex: dummy_pubkey(0x03),
                    operator_pubkey_hex: dummy_pubkey(0x04),
                    voting_power: 50,
                    role: ValidatorRole::Standby,
                },
            ],
            pre_mine: vec![],
            initial_accounts: vec![],
        }
    }

    #[test]
    fn baseline_validates_and_digests() {
        let g = baseline();
        validate_static(&g).expect("baseline must validate");
        let digest = compute_digest(&g).expect("digest");
        assert_eq!(digest.len(), 64); // 32 bytes hex
    }

    #[test]
    fn digest_is_deterministic_for_identical_input() {
        let a = baseline();
        let b = baseline();
        assert_eq!(compute_digest(&a).unwrap(), compute_digest(&b).unwrap());
    }

    #[test]
    fn digest_changes_when_any_field_changes() {
        let mut a = baseline();
        let original = compute_digest(&a).unwrap();
        a.genesis_timestamp += 1;
        let mutated = compute_digest(&a).unwrap();
        assert_ne!(original, mutated);
    }

    #[test]
    fn schema_mismatch_is_rejected() {
        let mut g = baseline();
        g.schema = "zoka-genesis-v0".to_string();
        let err = validate_static(&g).unwrap_err();
        assert!(matches!(err, GenesisError::SchemaMismatch { .. }));
    }

    #[test]
    fn pow_algorithm_must_match_chain_id() {
        let mut g = baseline();
        g.pow_algorithm = "zoka-cpu-fallback-v1".to_string();
        let err = validate_static(&g).unwrap_err();
        assert!(matches!(err, GenesisError::PowAlgorithmMismatch { .. }));
    }

    #[test]
    fn economics_drift_is_rejected_field_by_field() {
        let mut g = baseline();
        g.economics.max_supply_zka = 99;
        let err = validate_static(&g).unwrap_err();
        match err {
            GenesisError::EconomicsDrift { field, .. } => assert_eq!(field, "max_supply_zka"),
            other => panic!("expected EconomicsDrift, got: {other:?}"),
        }
    }

    #[test]
    fn validator_with_same_consensus_and_operator_key_is_rejected() {
        let mut g = baseline();
        g.validators[0].operator_pubkey_hex = g.validators[0].consensus_pubkey_hex.clone();
        let err = validate_static(&g).unwrap_err();
        assert!(matches!(err, GenesisError::ValidatorKeyReuse { .. }));
    }

    #[test]
    fn duplicate_validator_label_is_rejected() {
        let mut g = baseline();
        g.validators[1].label = g.validators[0].label.clone();
        let err = validate_static(&g).unwrap_err();
        assert!(matches!(err, GenesisError::DuplicateValidatorLabel { .. }));
    }

    #[test]
    fn duplicate_consensus_pubkey_across_validators_is_rejected() {
        let mut g = baseline();
        g.validators[1].consensus_pubkey_hex = g.validators[0].consensus_pubkey_hex.clone();
        let err = validate_static(&g).unwrap_err();
        assert!(matches!(err, GenesisError::DuplicateValidatorPubkey { .. }));
    }

    #[test]
    fn pubkey_with_wrong_length_is_rejected() {
        let mut g = baseline();
        g.validators[0].consensus_pubkey_hex = "deadbeef".to_string();
        let err = validate_static(&g).unwrap_err();
        assert!(matches!(err, GenesisError::ValidatorMalformed { .. }));
    }

    #[test]
    fn validator_set_with_zero_active_voting_power_is_rejected() {
        let mut g = baseline();
        for v in g.validators.iter_mut() {
            v.role = ValidatorRole::Standby;
        }
        let err = validate_static(&g).unwrap_err();
        assert!(matches!(err, GenesisError::NoActiveVotingPower));
    }

    #[test]
    fn empty_validator_set_is_allowed() {
        let mut g = baseline();
        g.validators.clear();
        validate_static(&g).expect("empty validator set is valid for pure-PoW chains");
    }

    #[test]
    fn pre_mine_zero_amount_is_rejected() {
        let mut g = baseline();
        g.pre_mine.push(PreMineEntry {
            recipient_address: "zka1someaddr".to_string(),
            amount_zka: 0,
            justification: "x".to_string(),
        });
        let err = validate_static(&g).unwrap_err();
        assert!(matches!(err, GenesisError::PreMineZeroAmount { .. }));
    }

    #[test]
    fn pre_mine_missing_justification_is_rejected() {
        let mut g = baseline();
        g.pre_mine.push(PreMineEntry {
            recipient_address: "zka1someaddr".to_string(),
            amount_zka: 100,
            justification: String::new(),
        });
        let err = validate_static(&g).unwrap_err();
        assert!(matches!(
            err,
            GenesisError::PreMineMissingJustification { .. }
        ));
    }

    #[test]
    fn pre_mine_total_above_cap_is_rejected() {
        let mut g = baseline();
        g.pre_mine.push(PreMineEntry {
            recipient_address: "zka1someaddr".to_string(),
            amount_zka: economics::MAX_SUPPLY_ZKA + 1,
            justification: "test".to_string(),
        });
        let err = validate_static(&g).unwrap_err();
        assert!(matches!(err, GenesisError::PreMineExceedsCap { .. }));
    }

    #[test]
    fn pre_mine_duplicate_address_is_rejected() {
        let mut g = baseline();
        g.pre_mine.push(PreMineEntry {
            recipient_address: "zka1addr".to_string(),
            amount_zka: 100,
            justification: "test".to_string(),
        });
        g.pre_mine.push(PreMineEntry {
            recipient_address: "zka1addr".to_string(),
            amount_zka: 200,
            justification: "test 2".to_string(),
        });
        let err = validate_static(&g).unwrap_err();
        assert!(matches!(err, GenesisError::DuplicateAddress { .. }));
    }

    #[test]
    fn address_in_both_pre_mine_and_initial_accounts_is_rejected() {
        let mut g = baseline();
        g.pre_mine.push(PreMineEntry {
            recipient_address: "zka1shared".to_string(),
            amount_zka: 100,
            justification: "test".to_string(),
        });
        g.initial_accounts.push(InitialAccount {
            address: "zka1shared".to_string(),
            balance_zka: 0,
        });
        let err = validate_static(&g).unwrap_err();
        assert!(matches!(err, GenesisError::AddressInBothSections { .. }));
    }

    #[test]
    fn unknown_field_in_json_is_rejected_by_serde() {
        let json = r#"{
            "schema": "zoka-genesis-v1",
            "chain_id": "zoka-mainnet-v1",
            "pow_algorithm": "zoka-randomx-v1",
            "genesis_timestamp": 0,
            "economics": {
                "max_supply_zka": 23000000,
                "initial_block_reward_zka": 12,
                "halving_interval_blocks": 1051200,
                "target_block_time_secs": 120,
                "coinbase_maturity_blocks": 60
            },
            "validators": [],
            "pre_mine": [],
            "initial_accounts": [],
            "rogue_extra_field": 42
        }"#;
        let result: Result<MainnetGenesisV1, _> = serde_json::from_str(json);
        assert!(result.is_err(), "deny_unknown_fields should reject extras");
    }

    #[test]
    fn json_roundtrip_preserves_digest() {
        let g = baseline();
        let bytes = serde_json::to_vec_pretty(&g).expect("serialize");
        let parsed: MainnetGenesisV1 = serde_json::from_slice(&bytes).expect("parse");
        assert_eq!(g, parsed);
        assert_eq!(
            compute_digest(&g).unwrap(),
            compute_digest(&parsed).unwrap()
        );
    }

    #[test]
    fn verify_report_summarizes_genesis_facts() {
        let g = baseline();
        let report = GenesisVerifyReport::from_genesis(&g).expect("verify");
        assert_eq!(report.chain_id, economics::MAINNET_CHAIN_ID);
        assert_eq!(report.pow_algorithm, economics::POW_ALGORITHM_MAINNET);
        assert_eq!(report.validator_count, 2);
        assert_eq!(report.active_voting_power, 100);
        assert_eq!(report.pre_mine_total_zka, 0);
        assert_eq!(report.initial_account_count, 0);
        assert_eq!(report.digest_hex.len(), 64);
    }

    #[test]
    fn verify_report_fails_when_genesis_is_malformed() {
        let mut g = baseline();
        g.schema = "rogue".to_string();
        let err = GenesisVerifyReport::from_genesis(&g).unwrap_err();
        assert!(matches!(err, GenesisError::SchemaMismatch { .. }));
    }

    /// Regression gate for the canonical published mainnet genesis file.
    ///
    /// This test parses `docs/genesis.mainnet.v1.json` exactly as a node
    /// loader would, runs `validate_static`, computes the canonical
    /// digest, and asserts it matches the digest that
    /// `docs/genesis-dry-run-evidence.md` claims operators will see when
    /// they run `zoka genesis-verify`. Any silent drift between the
    /// committed file and the documented digest fails this test.
    ///
    /// If you are intentionally rotating the mainnet genesis (a hard
    /// fork or pre-launch revision), update both the JSON file AND the
    /// digest published in `docs/genesis-dry-run-evidence.md` in the
    /// same commit so this test stays the canary.
    #[test]
    fn committed_mainnet_genesis_matches_published_digest() {
        let raw = include_str!("../docs/genesis.mainnet.v1.json");
        let evidence = include_str!("../docs/genesis-dry-run-evidence.md");

        let genesis: MainnetGenesisV1 =
            serde_json::from_str(raw).expect("docs/genesis.mainnet.v1.json must parse");
        validate_static(&genesis).expect("docs/genesis.mainnet.v1.json must validate");

        let actual_digest = compute_digest(&genesis).expect("digest");

        let documented_digest = evidence
            .lines()
            .find_map(|line| {
                let trimmed = line.trim();
                trimmed.find("\"digest_sha256_hex\"").map(|idx| {
                    let after = &trimmed[idx..];
                    let after_colon = after
                        .split_once(':')
                        .map(|(_, rhs)| rhs.trim_start())
                        .unwrap_or("");
                    let mut chars = after_colon.chars();
                    if chars.next() != Some('"') {
                        return String::new();
                    }
                    chars
                        .by_ref()
                        .take_while(|c| *c != '"')
                        .collect::<String>()
                })
            })
            .filter(|s| !s.is_empty())
            .expect(
                "docs/genesis-dry-run-evidence.md must contain a \"digest_sha256_hex\": \"<hex>\" line",
            );

        assert_eq!(
            actual_digest, documented_digest,
            "committed genesis digest drifted from the value documented in docs/genesis-dry-run-evidence.md.\n  computed: {actual_digest}\n  doc:      {documented_digest}\nUpdate both the JSON file and the evidence doc in the same commit."
        );
    }

    /// Sanity gate: the published mainnet genesis MUST target the
    /// canonical mainnet chain id and the canonical mainnet PoW
    /// algorithm. A misconfigured `chain_id` or `pow_algorithm` is a
    /// release-blocking bug regardless of the digest match.
    #[test]
    fn committed_mainnet_genesis_targets_mainnet_chain() {
        let raw = include_str!("../docs/genesis.mainnet.v1.json");
        let genesis: MainnetGenesisV1 =
            serde_json::from_str(raw).expect("docs/genesis.mainnet.v1.json must parse");
        assert_eq!(genesis.chain_id, economics::MAINNET_CHAIN_ID);
        assert_eq!(genesis.pow_algorithm, economics::POW_ALGORITHM_MAINNET);
        assert_eq!(genesis.schema, GENESIS_SCHEMA_ID);
    }
}
