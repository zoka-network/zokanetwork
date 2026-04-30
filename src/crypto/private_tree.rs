use bls12_381::Scalar;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

pub const PRIVATE_TREE_DEPTH: usize = 32;
pub const PRIVATE_TREE_SCALAR_WORDS: usize = 4;
const PRIVATE_TREE_HASH_ROUNDS: usize = 16;

pub fn round_constants() -> &'static [Scalar] {
    static CONSTANTS: OnceLock<Vec<Scalar>> = OnceLock::new();
    CONSTANTS.get_or_init(|| {
        (0..PRIVATE_TREE_HASH_ROUNDS)
            .map(|idx| {
                let mut first = Sha256::new();
                first.update(b"zoka-private-tree-round");
                first.update((idx as u64).to_le_bytes());
                let digest_a = first.finalize();

                let mut second = Sha256::new();
                second.update(b"zoka-private-tree-round-tail");
                second.update((idx as u64).to_le_bytes());
                let digest_b = second.finalize();

                let mut wide = [0u8; 64];
                wide[..32].copy_from_slice(&digest_a);
                wide[32..].copy_from_slice(&digest_b);
                Scalar::from_bytes_wide(&wide)
            })
            .collect()
    })
}

pub fn commitment_words(commitment: &str) -> [u64; PRIVATE_TREE_SCALAR_WORDS] {
    let bytes = match hex::decode(commitment) {
        Ok(decoded) if !decoded.is_empty() => decoded,
        _ => Sha256::digest(commitment.as_bytes()).to_vec(),
    };
    let mut words = [0u64; PRIVATE_TREE_SCALAR_WORDS];
    for (idx, word) in words.iter_mut().enumerate() {
        let start = idx * 8;
        let end = (start + 8).min(bytes.len());
        let mut chunk = [0u8; 8];
        if start < bytes.len() {
            chunk[..(end - start)].copy_from_slice(&bytes[start..end]);
        }
        *word = u64::from_le_bytes(chunk);
    }
    words
}

pub fn scalar_from_words(words: &[u64]) -> Option<Scalar> {
    if words.len() != PRIVATE_TREE_SCALAR_WORDS {
        return None;
    }

    let mut bytes = [0u8; 32];
    for (idx, word) in words.iter().enumerate() {
        let start = idx * 8;
        bytes[start..start + 8].copy_from_slice(&word.to_le_bytes());
    }

    if bytes == [0u8; 32] {
        return Some(Scalar::from(0u64));
    }

    Option::<Scalar>::from(Scalar::from_bytes(&bytes))
}

pub fn scalar_to_words(scalar: &Scalar) -> [u64; PRIVATE_TREE_SCALAR_WORDS] {
    let bytes = scalar.to_bytes();
    let mut words = [0u64; PRIVATE_TREE_SCALAR_WORDS];
    for (idx, word) in words.iter_mut().enumerate() {
        let start = idx * 8;
        let mut chunk = [0u8; 8];
        chunk.copy_from_slice(&bytes[start..start + 8]);
        *word = u64::from_le_bytes(chunk);
    }
    words
}

pub fn scalar_to_hex(scalar: &Scalar) -> String {
    hex::encode(scalar.to_bytes())
}

pub fn scalar_from_hex(hex_str: &str) -> Option<Scalar> {
    if hex_str == "0" {
        return Some(Scalar::from(0u64));
    }

    let bytes = hex::decode(hex_str).ok()?;
    if bytes.len() != 32 {
        return None;
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Option::<Scalar>::from(Scalar::from_bytes(&arr))
}

pub fn hash_pair(left: Scalar, right: Scalar) -> Scalar {
    let mut state = left + Scalar::from(0xA5A5_A5A5u64);
    let key = right + Scalar::from(0x5A5A_5A5Au64);
    for constant in round_constants() {
        let t = state + key + constant;
        state = (t * t) + key;
    }
    state + left
}

pub fn hash_commitment(commitment: &str) -> Scalar {
    let words = commitment_words(commitment);
    hash_commitment_words(&words)
}

pub fn hash_commitment_words(words: &[u64; PRIVATE_TREE_SCALAR_WORDS]) -> Scalar {
    let mut state = Scalar::from(0xC0DE_CAFEu64);
    for (idx, word) in words.iter().enumerate() {
        state = hash_pair(state + Scalar::from((idx as u64) + 1), Scalar::from(*word));
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_private_tree_scalar_word_roundtrip() {
        let scalar =
            hash_commitment("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff");
        let words = scalar_to_words(&scalar);
        let restored = scalar_from_words(&words).expect("scalar words should roundtrip");
        assert_eq!(scalar, restored);
    }

    #[test]
    fn test_private_tree_hash_commitment_is_deterministic() {
        let commitment = "abcd".repeat(16);
        assert_eq!(hash_commitment(&commitment), hash_commitment(&commitment));
    }
}
