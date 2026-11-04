//! `zoka-light` (la verificación de las billeteras liviana y web) frente al
//! código del nodo: las mismas claves de la autoridad, los mismos checkpoints
//! aceptados y rechazados, y registros de direcciones que valen en los dos.

use crate::checkpoint_authority::{authority_keys_for_chain, AuthorityKeys, SignedCheckpoint};
use crate::economics::QUANTIC_PQ_TESTNET_CHAIN_ID;
use crate::hybrid_signature::HybridSigningKey;
use crate::quantic::stark::protocol::wallet::Account;
use crate::quantic::stark_directory;

const REAL: &str = include_str!("../../crates/zoka-light/tests/fixtures/checkpoint-testnet-v2-4384.json");

#[test]
fn the_light_crate_has_the_authority_the_node_compiles() {
    let node = authority_keys_for_chain(QUANTIC_PQ_TESTNET_CHAIN_ID).expect("testnet v2 authority");
    let hybrid = node.hybrid().expect("Quantic requires the hybrid key");
    assert_eq!(QUANTIC_PQ_TESTNET_CHAIN_ID, zoka_light::checkpoint::QUANTIC_PQ_TESTNET_CHAIN_ID);
    assert_eq!(
        hex::encode(hybrid.ed25519().as_bytes()),
        zoka_light::checkpoint::QUANTIC_TESTNET_AUTHORITY
    );
    assert_eq!(
        hex::encode(hybrid.mldsa().to_bytes()),
        zoka_light::checkpoint::QUANTIC_TESTNET_AUTHORITY_MLDSA
    );
}

#[test]
fn the_real_checkpoint_verifies_in_both() {
    let node: SignedCheckpoint = serde_json::from_str(REAL).unwrap();
    node.verify_for_declared_chain().unwrap();
    zoka_light::checkpoint::SignedCheckpoint::from_json(REAL)
        .unwrap()
        .verify_for_declared_chain()
        .unwrap();
}

#[test]
fn what_the_node_signs_both_accept_and_a_change_both_refuse() {
    let ed25519 = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let key = HybridSigningKey::from_parts(ed25519, &[9; zoka_pq::ML_DSA_65_SEED_BYTES]).unwrap();
    let verifying = key.verifying_key();
    let node_keys = AuthorityKeys {
        ed25519: *verifying.ed25519(),
        mldsa: Some(verifying.mldsa().clone()),
    };
    let light_key = zoka_light::checkpoint::AuthorityKey::from_hex(
        &hex::encode(verifying.ed25519().as_bytes()),
        &hex::encode(verifying.mldsa().to_bytes()),
    )
    .unwrap();

    let signed = SignedCheckpoint::sign_hybrid(&key, QUANTIC_PQ_TESTNET_CHAIN_ID, 4022, &"ab".repeat(32));
    let light = zoka_light::checkpoint::SignedCheckpoint::from_json(&serde_json::to_string(&signed).unwrap()).unwrap();
    signed.verify(&node_keys).unwrap();
    light.verify(&light_key).unwrap();

    for change in 0..3 {
        let mut node = signed.clone();
        let mut lite = light.clone();
        match change {
            0 => {
                node.height += 1;
                lite.height += 1;
            }
            1 => {
                node.block_hash = "cd".repeat(32);
                lite.block_hash = "cd".repeat(32);
            }
            _ => {
                node.chain_id = "zoka-quantic-testnet-v1".into();
                lite.chain_id = "zoka-quantic-testnet-v1".into();
            }
        }
        assert!(node.verify(&node_keys).is_err(), "node accepted change {change}");
        assert!(lite.verify(&light_key).is_err(), "zoka-light accepted change {change}");
    }
}

#[test]
fn address_records_built_by_either_verify_in_the_other() {
    let account = Account::from_seed(&[5; 32], 2, 0).unwrap();
    let full = account.address();

    let light = zoka_light::directory::AddressRecord::new(&full, 12_345).unwrap();
    let as_node: stark_directory::AddressRecord =
        serde_json::from_str(&serde_json::to_string(&light).unwrap()).unwrap();
    as_node.verify(2).unwrap();

    let node = stark_directory::AddressRecord::new(&full).unwrap();
    let as_light: zoka_light::directory::AddressRecord =
        serde_json::from_str(&serde_json::to_string(&node).unwrap()).unwrap();
    as_light.verify(2).unwrap();

    // Una estampilla que no vale para zoka-light tampoco vale para el nodo.
    let (_, short) = light.verify(2).unwrap();
    let bad = (0u64..)
        .find(|stamp| !zoka_light::directory::stamp_is_valid(&short, *stamp))
        .unwrap();
    assert!(!stark_directory::stamp_is_valid(&short, bad));
}
