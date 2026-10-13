use zoka::{api, economics, network::p2p::PROTOCOL_FREEZE_ID};

fn mainnet_manifest() -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs")
        .join("mainnet-network.v1.json");
    let raw = std::fs::read_to_string(path).expect("read mainnet network manifest");
    serde_json::from_str(&raw).expect("parse mainnet network manifest")
}

fn version_triplet(raw: &str) -> (u64, u64, u64) {
    let core = raw
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()
        .unwrap_or_default();
    let mut parts = core
        .split('.')
        .map(|part| part.parse::<u64>().expect("numeric version component"));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

#[test]
fn network_manifest_matches_the_compiled_policy() {
    let manifest = mainnet_manifest();
    let checkpoint = &manifest["recovery"]["checkpoint"];
    let gate = &manifest["recovery"]["release_gate"];

    assert_eq!(manifest["status"], "release-candidate");
    assert_eq!(manifest["chain_id"], economics::MAINNET_CHAIN_ID);
    // Cambiar el identificador desconecta en el acto a todo nodo que no
    // actualice (`PROTOCOL_FREEZE_ID`): este valor fijo obliga a hacerlo a
    // propósito y no de rebote con un cambio de versión.
    assert_eq!(manifest["protocol_freeze_id"], PROTOCOL_FREEZE_ID);
    assert_eq!(PROTOCOL_FREEZE_ID, "/zoka/1.9.0-mainnet-recovery");
    // El cliente mínimo que anuncia el manifiesto (ZSilent lo lee para pedir
    // que se actualice) es el que la API usa para marcar mineros viejos, y
    // nunca supera la versión de este binario.
    assert_eq!(
        manifest["minimum_client_version"],
        api::NETWORK_MIN_CLIENT_VERSION
    );
    assert!(
        version_triplet(api::NETWORK_MIN_CLIENT_VERSION)
            <= version_triplet(env!("CARGO_PKG_VERSION")),
        "minimum client {} is newer than this build {}",
        api::NETWORK_MIN_CLIENT_VERSION,
        env!("CARGO_PKG_VERSION")
    );

    assert_eq!(checkpoint["status"], "compiled-release-candidate");
    assert_eq!(
        checkpoint["height"],
        economics::MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT
    );
    assert_eq!(
        checkpoint["hash"],
        economics::MAINNET_RECOVERY_CHECKPOINT_10887_HASH
    );
    assert!(economics::MAINNET_EMERGENCY_CHECKPOINTS.contains(&(
        economics::MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT,
        economics::MAINNET_RECOVERY_CHECKPOINT_10887_HASH,
    )));

    assert_eq!(
        manifest["recovery"]["mismatch_action"],
        economics::MAINNET_RECOVERY_MISMATCH_ACTION
    );
    assert_eq!(
        manifest["recovery"]["recovery_action"],
        "validated-checkpoint-reorg"
    );
    assert_eq!(
        manifest["recovery"]["automatic_db_deletion"],
        economics::MAINNET_RECOVERY_AUTOMATIC_DB_DELETION
    );

    assert_eq!(
        gate["minimum_confirmations"],
        economics::MAINNET_RECOVERY_MIN_CONFIRMATIONS
    );
    assert_eq!(
        gate["minimum_head_height"],
        economics::MAINNET_RECOVERY_MIN_RELEASE_HEAD_HEIGHT
    );
    assert_eq!(
        economics::MAINNET_RECOVERY_MIN_RELEASE_HEAD_HEIGHT,
        economics::MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT
            + economics::MAINNET_RECOVERY_MIN_CONFIRMATIONS
    );
    assert_eq!(
        gate["minimum_independent_observations"],
        economics::MAINNET_RECOVERY_MIN_INDEPENDENT_OBSERVATIONS
    );
    assert_eq!(
        gate["require_release_artifact_sha256"],
        economics::MAINNET_RECOVERY_REQUIRE_RELEASE_ARTIFACT_SHA256
    );
    assert_eq!(manifest["recovery"]["signed_snapshot_recommended"], true);
}

#[test]
fn recovery_release_stays_blocked_until_external_evidence_is_recorded() {
    let manifest = mainnet_manifest();
    let gate = &manifest["recovery"]["release_gate"];

    assert_eq!(gate["status"], "blocked");
    assert_eq!(gate["allow_publish"], false);
    assert!(gate["observed_head_height"].is_null());
    assert!(gate["independent_observations"]
        .as_array()
        .is_some_and(Vec::is_empty));
    assert!(gate["release_artifact_sha256"].is_null());
}
