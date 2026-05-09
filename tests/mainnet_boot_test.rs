use std::process::Command;

use zoka::genesis::{compute_digest, MainnetGenesisV1};
use zoka::storage::RocksDbStorage;

#[test]
fn zoka_fullnode_boots_from_canonical_mainnet_genesis() {
    let repo = env!("CARGO_MANIFEST_DIR");
    let zoka = env!("CARGO_BIN_EXE_zoka");
    let temp = tempfile::tempdir().expect("tempdir");
    let db = temp.path().join("mainnet-boot.db");
    let genesis_path = std::path::Path::new(repo).join("docs/genesis.mainnet.v1.json");

    let output = Command::new(zoka)
        .current_dir(repo)
        .env("USERPROFILE", temp.path())
        .env("HOME", temp.path())
        .args([
            "fullnode",
            "--password",
            "boot-test-pass",
            "--no-network-config",
            "--no-mine",
            "--seconds",
            "1",
            "--sync-warmup-secs",
            "1",
            "--listen",
            "/ip4/127.0.0.1/tcp/0",
            "--db",
            db.to_str().expect("db path utf8"),
            "--genesis",
            genesis_path.to_str().expect("genesis path utf8"),
        ])
        .output()
        .expect("run zoka fullnode");

    assert!(
        output.status.success(),
        "fullnode failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Fullnode session time limit reached"),
        "fullnode did not shut down cleanly:\n{stdout}"
    );

    let raw = std::fs::read_to_string(&genesis_path).expect("read genesis");
    let genesis: MainnetGenesisV1 = serde_json::from_str(&raw).expect("parse genesis");
    let expected_digest = compute_digest(&genesis).expect("digest");

    let storage = RocksDbStorage::new(&db).expect("open boot db");
    assert_eq!(storage.get_chain_height().expect("height"), 0);
    let stored_digest = storage
        .get_raw("chain:genesis:digest")
        .expect("read digest")
        .and_then(|raw| String::from_utf8(raw).ok())
        .expect("genesis digest persisted");
    assert_eq!(stored_digest, expected_digest);
}
