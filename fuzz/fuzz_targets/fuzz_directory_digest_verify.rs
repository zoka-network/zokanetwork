#![no_main]

use libfuzzer_sys::fuzz_target;
use std::fs;
use std::path::Path;
use tempfile::TempDir;
use zoka::zk::setup_io::{compute_directory_digest, verify_directory_digest, write_digest};

const PARAM_FILES: [&str; 4] = [
    "balance.params",
    "merkle.params",
    "nullifier.params",
    "private_tx.params",
];
const MAX_FILE_BYTES: usize = 1024;

fn write_seeded_params(dir: &Path, data: &[u8]) {
    let mut offset = 0usize;
    for file in PARAM_FILES {
        let requested = data.get(offset).copied().unwrap_or(0) as usize;
        offset = offset.saturating_add(1);
        let take = requested.min(MAX_FILE_BYTES).min(data.len().saturating_sub(offset));
        let bytes = &data[offset..offset + take];
        offset = offset.saturating_add(take);
        let path = dir.join(file);
        let _ = fs::write(path, bytes);
    }
}

fuzz_target!(|data: &[u8]| {
    if let Ok(temp_dir) = TempDir::new() {
        write_seeded_params(temp_dir.path(), data);

        if let Ok(mut digest) = compute_directory_digest(temp_dir.path()) {
            if data.first().map(|b| b & 0x1 == 1).unwrap_or(false) {
                digest[0] ^= 0xA5;
            }
            let _ = write_digest(temp_dir.path(), &digest);

            if data.get(1).map(|b| b & 0x1 == 1).unwrap_or(false) {
                let _ = fs::write(temp_dir.path().join("balance.params"), b"tampered");
            }
        }

        let _ = verify_directory_digest(temp_dir.path());
    }
});
