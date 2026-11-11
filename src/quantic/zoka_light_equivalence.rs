//! `zoka-light` (la verificación de las billeteras liviana y web) frente al
//! código del nodo: las mismas claves de la autoridad, los mismos checkpoints
//! aceptados y rechazados, y registros de direcciones que valen en los dos.

use crate::checkpoint_authority::{authority_keys_for_chain, AuthorityKeys, Cosigners, SignedCheckpoint};
use crate::economics::{QUANTIC_PQ_TESTNET_CHAIN_ID, QUANTIC_PQ_TESTNET_V2_CHAIN_ID};
use crate::hybrid_signature::HybridSigningKey;
use crate::quantic::stark::protocol::wallet::Account;
use crate::quantic::stark_directory;

const REAL: &str = include_str!("../../crates/zoka-light/tests/fixtures/checkpoint-testnet-v2-4384.json");

#[test]
fn the_light_crate_has_the_authority_the_node_compiles() {
    let node = authority_keys_for_chain(QUANTIC_PQ_TESTNET_CHAIN_ID).expect("testnet authority");
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
    let node_keys = AuthorityKeys::hybrid_key(*verifying.ed25519(), verifying.mldsa().clone());
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

/// La autoridad de k de n de la testnet v3 es la misma en los dos: las mismas
/// claves, en el mismo orden, y el mismo umbral.
#[test]
fn the_light_crate_has_the_cosigners_the_node_compiles() {
    let node = authority_keys_for_chain(QUANTIC_PQ_TESTNET_CHAIN_ID).unwrap();
    let cosigners = node.cosigners.as_ref().expect("the v3 testnet is k of n");
    let light = zoka_light::checkpoint::Authority::for_chain(QUANTIC_PQ_TESTNET_CHAIN_ID).unwrap();
    assert!(light.is_cosigned());
    assert_eq!(cosigners.threshold(), light.threshold());
    let node_keys: Vec<(String, String)> = cosigners
        .keys()
        .iter()
        .map(|key| (hex::encode(key.ed25519().as_bytes()), hex::encode(key.mldsa().to_bytes())))
        .collect();
    let light_keys: Vec<(String, String)> = light.keys().iter().map(|key| key.to_hex()).collect();
    assert_eq!(node_keys, light_keys);
    // La v2 sigue siendo de una clave en los dos.
    let v2 = zoka_light::checkpoint::Authority::for_chain(QUANTIC_PQ_TESTNET_V2_CHAIN_ID).unwrap();
    assert!(!v2.is_cosigned());
    assert!(authority_keys_for_chain(QUANTIC_PQ_TESTNET_V2_CHAIN_ID)
        .unwrap()
        .cosigners
        .is_none());
}

/// Dos de tres: lo que junta el nodo vale en `zoka-light`; una sola firma,
/// una ajena o una cambiada no valen en ninguno.
#[test]
fn a_cosigned_checkpoint_verifies_in_both_and_a_partial_in_neither() {
    let signing: Vec<HybridSigningKey> = (0u8..3)
        .map(|i| {
            let ed25519 = ed25519_dalek::SigningKey::from_bytes(&[30 + i; 32]);
            HybridSigningKey::from_parts(ed25519, &[40 + i; zoka_pq::ML_DSA_65_SEED_BYTES]).unwrap()
        })
        .collect();
    let public: Vec<_> = signing.iter().map(HybridSigningKey::verifying_key).collect();
    let node_keys = AuthorityKeys::cosigned(Cosigners::new(public.clone(), 2).unwrap());
    let light_keys = zoka_light::checkpoint::Authority::cosigned(
        public
            .iter()
            .map(|key| {
                zoka_light::checkpoint::AuthorityKey::from_hex(
                    &hex::encode(key.ed25519().as_bytes()),
                    &hex::encode(key.mldsa().to_bytes()),
                )
                .unwrap()
            })
            .collect(),
        2,
    )
    .unwrap();
    let hash = "ab".repeat(32);
    let one = SignedCheckpoint::cosign(&signing[0], 0, QUANTIC_PQ_TESTNET_CHAIN_ID, 4020, &hash);
    let light = |cp: &SignedCheckpoint| {
        zoka_light::checkpoint::SignedCheckpoint::from_json(&serde_json::to_string(cp).unwrap())
            .unwrap()
            .verify_with(&light_keys)
    };
    assert!(one.verify(&node_keys).is_err());
    assert_eq!(
        light(&one),
        Err(zoka_light::checkpoint::CheckpointError::Incomplete { have: 1, need: 2 })
    );
    let mut both = one.clone();
    assert!(both.merge(&SignedCheckpoint::cosign(
        &signing[2],
        2,
        QUANTIC_PQ_TESTNET_CHAIN_ID,
        4020,
        &hash
    )));
    both.verify(&node_keys).unwrap();
    light(&both).unwrap();
    for change in 0..3 {
        let mut changed = both.clone();
        match change {
            0 => changed.height += 1,
            1 => changed.cosignatures[1].signer = 1,
            _ => changed.cosignatures[0].ed25519 = changed.cosignatures[1].ed25519.clone(),
        }
        assert!(changed.verify(&node_keys).is_err(), "node accepted change {change}");
        assert!(light(&changed).is_err(), "zoka-light accepted change {change}");
    }
}
