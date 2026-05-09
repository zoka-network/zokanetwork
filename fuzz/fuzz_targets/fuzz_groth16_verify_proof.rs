#![no_main]

use libfuzzer_sys::fuzz_target;
use zoka::zk::groth16_backend::{
    verify, BALANCE_CIRCUIT, MERKLE_CIRCUIT, NULLIFIER_CIRCUIT, PRIVATE_TX_CIRCUIT,
};

const MAX_PROOF_BYTES: usize = 8 * 1024;
const MAX_PUBLIC_INPUTS: usize = 16;
const MAX_PUBLIC_INPUT_BYTES: usize = 64;

fn take_byte(data: &[u8], index: &mut usize) -> Option<u8> {
    if *index >= data.len() {
        return None;
    }
    let byte = data[*index];
    *index += 1;
    Some(byte)
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }

    let circuits = [
        BALANCE_CIRCUIT,
        MERKLE_CIRCUIT,
        NULLIFIER_CIRCUIT,
        PRIVATE_TX_CIRCUIT,
        "unknown_circuit",
    ];

    let mut index = 0usize;
    let circuit_selector = take_byte(data, &mut index).unwrap_or(0) as usize;
    let circuit = circuits[circuit_selector % circuits.len()];

    let requested_proof_len = take_byte(data, &mut index).unwrap_or(0) as usize;
    let proof_len = requested_proof_len
        .min(MAX_PROOF_BYTES)
        .min(data.len().saturating_sub(index));
    let proof_bytes = &data[index..index + proof_len];
    index += proof_len;

    let requested_inputs = take_byte(data, &mut index).unwrap_or(0) as usize;
    let input_count = requested_inputs.min(MAX_PUBLIC_INPUTS);
    let mut public_inputs = Vec::with_capacity(input_count);

    for _ in 0..input_count {
        let requested_len = take_byte(data, &mut index).unwrap_or(0) as usize;
        let len = requested_len
            .min(MAX_PUBLIC_INPUT_BYTES)
            .min(data.len().saturating_sub(index));
        public_inputs.push(data[index..index + len].to_vec());
        index += len;
    }

    let _ = verify(circuit, proof_bytes, &public_inputs);
});
