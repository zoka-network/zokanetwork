#![no_main]

use libfuzzer_sys::fuzz_target;
use zoka::zk::range_proofs::verify_output_range;

const MAX_PROOF_BYTES: usize = 16 * 1024;

fuzz_target!(|data: &[u8]| {
    let mut commitment_bytes = [0u8; 32];
    let commitment_len = data.len().min(32);
    commitment_bytes[..commitment_len].copy_from_slice(&data[..commitment_len]);
    let commitment_hex = hex::encode(commitment_bytes);

    let proof_start = commitment_len;
    let proof_end = (proof_start + MAX_PROOF_BYTES).min(data.len());
    let proof_bytes = &data[proof_start..proof_end];

    let _ = verify_output_range(&commitment_hex, proof_bytes);
});
