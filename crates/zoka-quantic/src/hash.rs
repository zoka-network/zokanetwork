//! BLAKE2b with a distinct 16-byte personalization for every use, as Orchard
//! does. Every derivation of this crate goes through these two functions.

use blake2b_simd::Params;

/// Wallet root (phrase entropy) -> 64-byte ZIP 32 seed.
pub(crate) const ROOT_SEED: &[u8; 16] = b"ZokaQuanticSeed_";
/// Orchard spending key -> X-Wing key seed.
pub(crate) const XWING_SEED: &[u8; 16] = b"ZokaQ_XWingSeed_";
/// Address checksum (prefix and payload).
pub(crate) const ADDRESS_CHECKSUM: &[u8; 16] = b"ZokaQ_AddrCheck_";
/// Note seed -> X-Wing encapsulation randomness.
pub(crate) const KEM_SEED: &[u8; 16] = b"ZokaQ_KemEncSeed";
/// X-Wing shared secret -> note encryption key.
pub(crate) const NOTE_KEY: &[u8; 16] = b"ZokaQ_NoteEncKey";
/// Outgoing viewing key -> outgoing cipher key.
pub(crate) const OUT_KEY: &[u8; 16] = b"ZokaQ_OutCiphKey";
/// Transaction id tree (see `tx`).
pub(crate) const TX_HEADER: &[u8; 16] = b"ZokaQ_TxHeader__";
pub(crate) const ACTIONS_COMPACT: &[u8; 16] = b"ZokaQ_ActCompact";
pub(crate) const ACTIONS_MEMOS: &[u8; 16] = b"ZokaQ_ActMemos__";
pub(crate) const ACTIONS_NONCOMPACT: &[u8; 16] = b"ZokaQ_ActNonComp";
pub(crate) const BUNDLE_EFFECTS: &[u8; 16] = b"ZokaQ_BundleEff_";
pub(crate) const TX_ID: &[u8; 16] = b"ZokaQ_TxId______";
pub(crate) const TX_AUTH: &[u8; 16] = b"ZokaQ_TxAuth____";
/// Inner nodes of the per-block transaction trees.
pub(crate) const MERKLE_NODE: &[u8; 16] = b"ZokaQ_MerkleNode";

pub(crate) fn blake2b_256(personal: &[u8; 16], parts: &[&[u8]]) -> [u8; 32] {
    let mut state = Params::new().hash_length(32).personal(personal).to_state();
    for part in parts {
        state.update(part);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(state.finalize().as_bytes());
    out
}

pub(crate) fn blake2b_512(personal: &[u8; 16], parts: &[&[u8]]) -> [u8; 64] {
    let mut state = Params::new().hash_length(64).personal(personal).to_state();
    for part in parts {
        state.update(part);
    }
    let mut out = [0u8; 64];
    out.copy_from_slice(state.finalize().as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_personalization_is_distinct() {
        let all = [
            ROOT_SEED,
            XWING_SEED,
            ADDRESS_CHECKSUM,
            KEM_SEED,
            NOTE_KEY,
            OUT_KEY,
            TX_HEADER,
            ACTIONS_COMPACT,
            ACTIONS_MEMOS,
            ACTIONS_NONCOMPACT,
            BUNDLE_EFFECTS,
            TX_ID,
            TX_AUTH,
            MERKLE_NODE,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn the_parts_are_hashed_as_one_message() {
        assert_eq!(
            blake2b_256(TX_ID, &[b"ab", b"c"]),
            blake2b_256(TX_ID, &[b"abc"])
        );
        assert_ne!(
            blake2b_256(TX_ID, &[b"abc"]),
            blake2b_256(TX_AUTH, &[b"abc"])
        );
    }
}
