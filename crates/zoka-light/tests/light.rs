//! zoka-light por sí solo: un checkpoint real de la testnet v2 y un registro de
//! dirección corta. La comparación con el código del nodo está en el nodo
//! (src/quantic/zoka_light_equivalence.rs).

use zoka_light::checkpoint::{CheckpointError, SignedCheckpoint};
use zoka_light::directory::{stamp_is_valid, AddressRecord};
use zoka_pq_pool::wallet::Account;

const REAL: &str = include_str!("fixtures/checkpoint-testnet-v2-4384.json");

#[test]
fn the_real_checkpoint_of_block_4384_verifies() {
    let checkpoint = SignedCheckpoint::from_json(REAL).unwrap();
    assert_eq!(checkpoint.chain_id, "zoka-quantic-testnet-v2");
    assert_eq!(checkpoint.height, 4384);
    checkpoint.verify_for_declared_chain().unwrap();
}

#[test]
fn a_changed_checkpoint_is_refused() {
    let mut checkpoint = SignedCheckpoint::from_json(REAL).unwrap();
    checkpoint.height += 1;
    assert_eq!(checkpoint.verify_for_declared_chain(), Err(CheckpointError::BadSignature));

    let mut checkpoint = SignedCheckpoint::from_json(REAL).unwrap();
    checkpoint.block_hash = "0".repeat(64);
    assert_eq!(checkpoint.verify_for_declared_chain(), Err(CheckpointError::BadSignature));

    // Solo Ed25519 (versión 1): Quantic no la acepta.
    let mut checkpoint = SignedCheckpoint::from_json(REAL).unwrap();
    checkpoint.version = 1;
    checkpoint.mldsa_signature = None;
    assert_eq!(checkpoint.verify_for_declared_chain(), Err(CheckpointError::Version(1)));

    // Otra cadena: sin autoridad conocida.
    let mut checkpoint = SignedCheckpoint::from_json(REAL).unwrap();
    checkpoint.chain_id = "zoka-quantic-mainnet".into();
    assert!(matches!(checkpoint.verify_for_declared_chain(), Err(CheckpointError::NoAuthority(_))));
}

#[test]
fn an_address_record_carries_a_valid_stamp() {
    let account = Account::from_seed(&[5; 32], 2, 0).unwrap();
    let record = AddressRecord::new(&account.address(), 77).unwrap();
    let (full, short) = record.verify(2).unwrap();
    assert_eq!(full.encode().unwrap(), account.address().encode().unwrap());
    assert!(stamp_is_valid(&short, record.stamp));
    // De otra red no.
    assert!(record.verify(3).is_err());
}
