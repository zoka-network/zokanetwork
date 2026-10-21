//! Note encryption with X-Wing instead of Orchard's elliptic-curve key agreement.
//!
//! Orchard encrypts each note to `pk_d` with a Diffie-Hellman exchange on
//! Pallas. Whoever learns an address can compute its incoming viewing key with
//! a quantum computer and read every note sent to it (ZIP 2005). Here each
//! action carries instead:
//!
//! ```text
//! eseed      = BLAKE2b-512("ZokaQ_KemEncSeed", rseed ‖ rho ‖ cmx)
//! (kem, ss)  = X-Wing.Encaps(recipient X-Wing key, eseed)          1120-byte ciphertext
//! key        = BLAKE2b-256("ZokaQ_NoteEncKey", ss ‖ kem ‖ cmx)
//! enc        = ChaCha20-Poly1305(key, nonce 0, plaintext)          580 bytes
//! plaintext  = 0x03 ‖ diversifier (11) ‖ value (8, LE) ‖ rseed (32) ‖ memo (512)
//! ock        = BLAKE2b-256("ZokaQ_OutCiphKey", ovk ‖ cv_net ‖ cmx ‖ kem)
//! out        = ChaCha20-Poly1305(ock, nonce 0, Orchard address (43) ‖ ss (32))   91 bytes
//! ```
//!
//! The plaintext is Orchard's V3 note plaintext, unchanged. The recipient
//! accepts a note only if the decrypted fields rebuild the published note
//! commitment `cmx` and if re-encapsulating with the `eseed` of that note gives
//! the published ciphertext: the equivalent of Orchard's `epk = [esk] g_d`
//! check. Each key is used once, so the zero nonce is safe, as in Orchard.

use chacha20::cipher::{KeyIvInit, StreamCipher, StreamCipherSeek};
use chacha20::ChaCha20;
use chacha20poly1305::{AeadInOut, ChaCha20Poly1305, KeyInit, Nonce};
use orchard::keys::{Diversifier, OutgoingViewingKey, Scope};
use orchard::note::{ExtractedNoteCommitment, Nullifier, RandomSeed, Rho};
use orchard::value::{NoteValue, ValueCommitment};
use orchard::{Address, Note, NoteVersion};
use rand_core::{CryptoRng, Rng};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, Zeroizing};
use zoka_pq::{
    SharedSecret, XWingCiphertext, XWingPublicKey, XWingSecretKey, X_WING_CIPHERTEXT_BYTES,
    X_WING_ENCAPSULATION_RANDOMNESS_BYTES, X_WING_SEED_BYTES,
};

use crate::address::ORCHARD_RAW_ADDRESS_BYTES;
use crate::hash::{blake2b_256, blake2b_512, KEM_SEED, NOTE_KEY, OUT_KEY};
use crate::keys::QuanticFullViewingKey;

pub const MEMO_SIZE: usize = 512;
/// Lead byte of Ironwood (V3, quantum-recoverable) note plaintexts.
pub const NOTE_PLAINTEXT_LEAD_BYTE: u8 = 0x03;
pub const COMPACT_NOTE_SIZE: usize = 1 + 11 + 8 + 32;
pub const NOTE_PLAINTEXT_SIZE: usize = COMPACT_NOTE_SIZE + MEMO_SIZE;
pub const AEAD_TAG_SIZE: usize = 16;
pub const ENC_CIPHERTEXT_SIZE: usize = NOTE_PLAINTEXT_SIZE + AEAD_TAG_SIZE;
pub const OUT_PLAINTEXT_SIZE: usize = ORCHARD_RAW_ADDRESS_BYTES + 32;
pub const OUT_CIPHERTEXT_SIZE: usize = OUT_PLAINTEXT_SIZE + AEAD_TAG_SIZE;
pub const KEM_CIPHERTEXT_SIZE: usize = X_WING_CIPHERTEXT_BYTES;
/// Public outgoing viewing key: outputs sent with it can be opened by anyone.
/// Used for the premine, so that its amount and recipient can be checked.
pub const PUBLIC_OVK: [u8; 32] = [0u8; 32];

pub type Memo = [u8; MEMO_SIZE];

/// What travels with each action in place of Orchard's note ciphertext.
#[derive(Clone, PartialEq, Eq)]
pub struct NoteEnvelope {
    kem_ciphertext: Box<[u8; KEM_CIPHERTEXT_SIZE]>,
    enc_ciphertext: Box<[u8; ENC_CIPHERTEXT_SIZE]>,
    out_ciphertext: [u8; OUT_CIPHERTEXT_SIZE],
}

impl NoteEnvelope {
    pub fn from_parts(
        kem_ciphertext: [u8; KEM_CIPHERTEXT_SIZE],
        enc_ciphertext: [u8; ENC_CIPHERTEXT_SIZE],
        out_ciphertext: [u8; OUT_CIPHERTEXT_SIZE],
    ) -> Self {
        Self {
            kem_ciphertext: Box::new(kem_ciphertext),
            enc_ciphertext: Box::new(enc_ciphertext),
            out_ciphertext,
        }
    }

    pub fn kem_ciphertext(&self) -> &[u8; KEM_CIPHERTEXT_SIZE] {
        &self.kem_ciphertext
    }

    pub fn enc_ciphertext(&self) -> &[u8; ENC_CIPHERTEXT_SIZE] {
        &self.enc_ciphertext
    }

    /// The part a light wallet needs to recognize its notes.
    pub fn enc_ciphertext_compact(&self) -> &[u8; COMPACT_NOTE_SIZE] {
        self.enc_ciphertext[..COMPACT_NOTE_SIZE]
            .try_into()
            .expect("prefix of the ciphertext")
    }

    pub fn out_ciphertext(&self) -> &[u8; OUT_CIPHERTEXT_SIZE] {
        &self.out_ciphertext
    }
}

impl std::fmt::Debug for NoteEnvelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NoteEnvelope(..)")
    }
}

/// A note the wallet found in an action.
#[derive(Clone, Debug)]
pub struct DecryptedNote {
    pub note: Note,
    /// External for payments received, internal for the wallet's own change.
    pub scope: Scope,
    pub memo: Memo,
}

fn kem_seed(
    rseed: &RandomSeed,
    rho: &Rho,
    cmx: &ExtractedNoteCommitment,
) -> Zeroizing<[u8; X_WING_ENCAPSULATION_RANDOMNESS_BYTES]> {
    Zeroizing::new(blake2b_512(
        KEM_SEED,
        &[rseed.as_bytes(), &rho.to_bytes(), &cmx.to_bytes()],
    ))
}

fn note_key(ss: &SharedSecret, kem: &[u8], cmx: &ExtractedNoteCommitment) -> Zeroizing<[u8; 32]> {
    Zeroizing::new(blake2b_256(
        NOTE_KEY,
        &[ss.expose_secret(), kem, &cmx.to_bytes()],
    ))
}

fn outgoing_key(
    ovk: &OutgoingViewingKey,
    cv_net: &ValueCommitment,
    cmx: &ExtractedNoteCommitment,
    kem: &[u8],
) -> Zeroizing<[u8; 32]> {
    Zeroizing::new(blake2b_256(
        OUT_KEY,
        &[ovk.as_ref(), &cv_net.to_bytes(), &cmx.to_bytes(), kem],
    ))
}

fn seal(key: &[u8; 32], plaintext: &mut [u8]) -> [u8; AEAD_TAG_SIZE] {
    ChaCha20Poly1305::new_from_slice(key)
        .expect("the key is 32 bytes")
        .encrypt_inout_detached(&Nonce::default(), &[], plaintext.into())
        .expect("the plaintext is far below the ChaCha20 limit")
        .into()
}

fn open(key: &[u8; 32], ciphertext: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    let (body, tag) = ciphertext.split_at(ciphertext.len() - AEAD_TAG_SIZE);
    let tag: [u8; AEAD_TAG_SIZE] = tag.try_into().ok()?;
    let mut plaintext = Zeroizing::new(body.to_vec());
    ChaCha20Poly1305::new_from_slice(key)
        .ok()?
        .decrypt_inout_detached(
            &Nonce::default(),
            &[],
            plaintext.as_mut_slice().into(),
            &tag.into(),
        )
        .ok()?;
    Some(plaintext)
}

fn note_plaintext(note: &Note, memo: &Memo) -> Zeroizing<[u8; NOTE_PLAINTEXT_SIZE]> {
    let mut plaintext = Zeroizing::new([0u8; NOTE_PLAINTEXT_SIZE]);
    plaintext[0] = NOTE_PLAINTEXT_LEAD_BYTE;
    plaintext[1..12].copy_from_slice(note.recipient().diversifier().as_array());
    plaintext[12..20].copy_from_slice(&note.value().inner().to_le_bytes());
    plaintext[20..52].copy_from_slice(note.rseed().as_bytes());
    plaintext[52..].copy_from_slice(memo);
    plaintext
}

/// Reads the compact part of a plaintext: diversifier, value and seed.
fn parse_compact(compact: &[u8]) -> Option<(Diversifier, NoteValue, [u8; 32])> {
    if compact.len() < COMPACT_NOTE_SIZE || compact[0] != NOTE_PLAINTEXT_LEAD_BYTE {
        return None;
    }
    let diversifier = Diversifier::from_bytes(compact[1..12].try_into().ok()?);
    let value = NoteValue::from_raw(u64::from_le_bytes(compact[12..20].try_into().ok()?));
    let rseed: [u8; 32] = compact[20..52].try_into().ok()?;
    Some((diversifier, value, rseed))
}

fn rebuild_note(
    recipient: Address,
    value: NoteValue,
    nf: &Nullifier,
    rseed: [u8; 32],
    cmx: &ExtractedNoteCommitment,
) -> Option<Note> {
    let rho: Option<Rho> = Rho::from_bytes(&nf.to_bytes()).into();
    let rho = rho?;
    let rseed: Option<RandomSeed> = RandomSeed::from_bytes(rseed, &rho).into();
    let note: Option<Note> =
        Note::from_parts(recipient, value, rho, rseed?, NoteVersion::V3).into();
    let note = note?;
    let rebuilt = ExtractedNoteCommitment::from(note.commitment());
    bool::from(rebuilt.to_bytes().ct_eq(&cmx.to_bytes())).then_some(note)
}

/// Finds which of the wallet's two addresses (payments or change) the note is for.
fn rebuild_owned_note(
    fvk: &QuanticFullViewingKey,
    compact: &[u8],
    nf: &Nullifier,
    cmx: &ExtractedNoteCommitment,
) -> Option<(Note, Scope)> {
    let (diversifier, value, rseed) = parse_compact(compact)?;
    [Scope::External, Scope::Internal]
        .into_iter()
        .find_map(|scope| {
            let recipient = fvk.orchard().address(diversifier, scope);
            rebuild_note(recipient, value, nf, rseed, cmx).map(|note| (note, scope))
        })
}

/// The equivalent of Orchard's `epk` check: the ciphertext must be the one
/// this very note produces for the wallet's own X-Wing key.
fn ciphertext_matches_note(
    recipient_xwing: &XWingPublicKey,
    note: &Note,
    cmx: &ExtractedNoteCommitment,
    kem: &[u8; KEM_CIPHERTEXT_SIZE],
) -> bool {
    let (expected, _) =
        recipient_xwing.encapsulate_derand(&kem_seed(note.rseed(), &note.rho(), cmx));
    bool::from(expected.as_bytes().ct_eq(&kem[..]))
}

fn decapsulate(xwing: &XWingSecretKey, kem: &[u8; KEM_CIPHERTEXT_SIZE]) -> Option<SharedSecret> {
    let ciphertext = XWingCiphertext::from_bytes(kem).ok()?;
    // A non-contributory X25519 part is rejected: such an output is nobody's.
    xwing.decapsulate(&ciphertext).ok()
}

/// Encrypts `note` to `recipient_xwing`. With `ovk`, the sender (or anyone who
/// has that key) can recover the output later; without it, `out` is random.
pub fn encrypt_note<R: Rng + CryptoRng>(
    recipient_xwing: &XWingPublicKey,
    note: &Note,
    memo: &Memo,
    cv_net: &ValueCommitment,
    cmx: &ExtractedNoteCommitment,
    ovk: Option<&OutgoingViewingKey>,
    rng: &mut R,
) -> NoteEnvelope {
    debug_assert_eq!(note.version(), NoteVersion::V3);
    let (kem, ss) = recipient_xwing.encapsulate_derand(&kem_seed(note.rseed(), &note.rho(), cmx));
    let kem: [u8; KEM_CIPHERTEXT_SIZE] = kem.as_bytes().try_into().expect("X-Wing ciphertext size");

    let mut plaintext = note_plaintext(note, memo);
    let tag = seal(&note_key(&ss, &kem, cmx), plaintext.as_mut());
    let mut enc = [0u8; ENC_CIPHERTEXT_SIZE];
    enc[..NOTE_PLAINTEXT_SIZE].copy_from_slice(plaintext.as_ref());
    enc[NOTE_PLAINTEXT_SIZE..].copy_from_slice(&tag);

    let mut out = [0u8; OUT_CIPHERTEXT_SIZE];
    match ovk {
        Some(ovk) => {
            let mut out_plaintext = Zeroizing::new([0u8; OUT_PLAINTEXT_SIZE]);
            out_plaintext[..ORCHARD_RAW_ADDRESS_BYTES]
                .copy_from_slice(&note.recipient().to_raw_address_bytes());
            out_plaintext[ORCHARD_RAW_ADDRESS_BYTES..].copy_from_slice(ss.expose_secret());
            let tag = seal(
                &outgoing_key(ovk, cv_net, cmx, &kem),
                out_plaintext.as_mut(),
            );
            out[..OUT_PLAINTEXT_SIZE].copy_from_slice(out_plaintext.as_ref());
            out[OUT_PLAINTEXT_SIZE..].copy_from_slice(&tag);
        }
        None => rng.fill_bytes(&mut out),
    }
    NoteEnvelope::from_parts(kem, enc, out)
}

/// The envelope of a padding output: a real encapsulation to a throwaway
/// X-Wing key, so it looks exactly like any other.
pub fn encrypt_dummy_note<R: Rng + CryptoRng>(
    note: &Note,
    cv_net: &ValueCommitment,
    cmx: &ExtractedNoteCommitment,
    rng: &mut R,
) -> NoteEnvelope {
    let mut seed = [0u8; X_WING_SEED_BYTES];
    rng.fill_bytes(&mut seed);
    let throwaway = XWingSecretKey::from_seed(&seed).expect("exact seed size");
    seed.zeroize();
    encrypt_note(
        &throwaway.public_key(),
        note,
        &[0u8; MEMO_SIZE],
        cv_net,
        cmx,
        None,
        rng,
    )
}

/// Trial decryption of one action by the wallet that owns `fvk`.
pub fn try_note_decryption(
    fvk: &QuanticFullViewingKey,
    nf: &Nullifier,
    cmx: &ExtractedNoteCommitment,
    envelope: &NoteEnvelope,
) -> Option<DecryptedNote> {
    let ss = decapsulate(fvk.xwing_secret(), envelope.kem_ciphertext())?;
    let plaintext = open(
        &note_key(&ss, envelope.kem_ciphertext(), cmx),
        envelope.enc_ciphertext(),
    )?;
    let (note, scope) = rebuild_owned_note(fvk, &plaintext, nf, cmx)?;
    if !ciphertext_matches_note(fvk.xwing_public(), &note, cmx, envelope.kem_ciphertext()) {
        return None;
    }
    let memo: Memo = plaintext[COMPACT_NOTE_SIZE..].try_into().ok()?;
    Some(DecryptedNote { note, scope, memo })
}

/// The light-wallet version: only the X-Wing ciphertext and the first 52 bytes
/// of `enc` (no tag, as in ZIP 307). The note commitment authenticates the result.
pub fn try_compact_note_decryption(
    fvk: &QuanticFullViewingKey,
    nf: &Nullifier,
    cmx: &ExtractedNoteCommitment,
    kem: &[u8; KEM_CIPHERTEXT_SIZE],
    enc_compact: &[u8; COMPACT_NOTE_SIZE],
) -> Option<(Note, Scope)> {
    let ss = decapsulate(fvk.xwing_secret(), kem)?;
    let key = note_key(&ss, kem, cmx);
    let mut compact = Zeroizing::new(*enc_compact);
    // Block 0 of the key stream keys Poly1305; the plaintext starts at block 1.
    let mut keystream = ChaCha20::new_from_slices(key.as_ref(), &[0u8; 12]).ok()?;
    keystream.seek(64u64);
    keystream.apply_keystream(compact.as_mut());
    let (note, scope) = rebuild_owned_note(fvk, compact.as_ref(), nf, cmx)?;
    ciphertext_matches_note(fvk.xwing_public(), &note, cmx, kem).then_some((note, scope))
}

/// Recovery by the sender, from `out`. Returns the note, its Orchard
/// recipient and the memo. It cannot check the X-Wing ciphertext (the sender
/// keeps only the Orchard part of the address); `cmx` still binds the note.
pub fn try_output_recovery_with_ovk(
    ovk: &OutgoingViewingKey,
    nf: &Nullifier,
    cmx: &ExtractedNoteCommitment,
    cv_net: &ValueCommitment,
    envelope: &NoteEnvelope,
) -> Option<(Note, Address, Memo)> {
    let kem = envelope.kem_ciphertext();
    let out = open(
        &outgoing_key(ovk, cv_net, cmx, kem),
        envelope.out_ciphertext(),
    )?;
    let raw: [u8; ORCHARD_RAW_ADDRESS_BYTES] = out[..ORCHARD_RAW_ADDRESS_BYTES].try_into().ok()?;
    let recipient: Option<Address> = Address::from_raw_address_bytes(&raw).into();
    let recipient = recipient?;
    let ss_bytes: [u8; 32] = out[ORCHARD_RAW_ADDRESS_BYTES..].try_into().ok()?;
    let key = Zeroizing::new(blake2b_256(
        NOTE_KEY,
        &[&ss_bytes, &kem[..], &cmx.to_bytes()],
    ));
    let plaintext = open(&key, envelope.enc_ciphertext())?;
    let (diversifier, value, rseed) = parse_compact(&plaintext)?;
    if diversifier.as_array() != recipient.diversifier().as_array() {
        return None;
    }
    let note = rebuild_note(recipient, value, nf, rseed, cmx)?;
    let memo: Memo = plaintext[COMPACT_NOTE_SIZE..].try_into().ok()?;
    Some((note, recipient, memo))
}

/// Checks a publicly recoverable output (sent with [`PUBLIC_OVK`]) against an
/// announced recipient and value, including its X-Wing ciphertext.
pub fn verify_public_output(
    recipient: &crate::address::QuanticAddress,
    value: NoteValue,
    nf: &Nullifier,
    cmx: &ExtractedNoteCommitment,
    cv_net: &ValueCommitment,
    envelope: &NoteEnvelope,
) -> bool {
    let ovk = OutgoingViewingKey::from(PUBLIC_OVK);
    match try_output_recovery_with_ovk(&ovk, nf, cmx, cv_net, envelope) {
        Some((note, orchard, _)) => {
            orchard.to_raw_address_bytes() == recipient.orchard().to_raw_address_bytes()
                && note.value() == value
                && ciphertext_matches_note(recipient.xwing(), &note, cmx, envelope.kem_ciphertext())
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::QuanticSpendingKey;
    use crate::network::Network;
    use orchard::value::ValueCommitTrapdoor;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    struct Sample {
        note: Note,
        nf: Nullifier,
        cmx: ExtractedNoteCommitment,
        cv_net: ValueCommitment,
    }

    fn wallet(seed: u8) -> QuanticFullViewingKey {
        QuanticSpendingKey::from_root(&[seed; 32], Network::Regtest, 0)
            .unwrap()
            .full_viewing_key()
    }

    fn sample(recipient: Address, value: u64, rng: &mut StdRng) -> Sample {
        loop {
            let mut nf_bytes = [0u8; 32];
            rng.fill_bytes(&mut nf_bytes);
            nf_bytes[31] &= 0x3f;
            let mut rseed = [0u8; 32];
            rng.fill_bytes(&mut rseed);
            let nf: Option<Nullifier> = Nullifier::from_bytes(&nf_bytes).into();
            let Some(nf) = nf else { continue };
            let rho: Rho = Option::from(Rho::from_bytes(&nf_bytes)).unwrap();
            let Some(rseed) = Option::<RandomSeed>::from(RandomSeed::from_bytes(rseed, &rho))
            else {
                continue;
            };
            let Some(note) = Option::<Note>::from(Note::from_parts(
                recipient,
                NoteValue::from_raw(value),
                rho,
                rseed,
                NoteVersion::V3,
            )) else {
                continue;
            };
            let cmx = ExtractedNoteCommitment::from(note.commitment());
            let rcv: ValueCommitTrapdoor =
                Option::from(ValueCommitTrapdoor::from_bytes([3; 32])).unwrap();
            let cv_net =
                ValueCommitment::derive(NoteValue::from_raw(0) - NoteValue::from_raw(value), rcv);
            return Sample {
                note,
                nf,
                cmx,
                cv_net,
            };
        }
    }

    fn memo(text: &[u8]) -> Memo {
        let mut memo = [0u8; MEMO_SIZE];
        memo[..text.len()].copy_from_slice(text);
        memo
    }

    fn envelope_for(
        to: &QuanticFullViewingKey,
        s: &Sample,
        memo: &Memo,
        ovk: Option<&OutgoingViewingKey>,
        rng: &mut StdRng,
    ) -> NoteEnvelope {
        encrypt_note(
            to.xwing_public(),
            &s.note,
            memo,
            &s.cv_net,
            &s.cmx,
            ovk,
            rng,
        )
    }

    #[test]
    fn the_recipient_finds_the_note_and_nobody_else_does() {
        let mut rng = StdRng::seed_from_u64(1);
        let bob = wallet(2);
        let carol = wallet(3);
        let s = sample(*bob.address().orchard(), 30_000_000, &mut rng);
        let envelope = envelope_for(&bob, &s, &memo(b"hola"), None, &mut rng);

        let found = try_note_decryption(&bob, &s.nf, &s.cmx, &envelope).expect("note of bob");
        assert_eq!(found.note.value().inner(), 30_000_000);
        assert_eq!(found.scope, Scope::External);
        assert_eq!(&found.memo[..4], b"hola");
        assert_eq!(
            ExtractedNoteCommitment::from(found.note.commitment()).to_bytes(),
            s.cmx.to_bytes()
        );
        assert!(try_note_decryption(&carol, &s.nf, &s.cmx, &envelope).is_none());
    }

    /// Atacante cuántico simulado (docs/QUANTIC-INVENTARIO-CRIPTOGRAFICO.md):
    /// con Shor, de una dirección se saca la `ivk` de Orchard. Acá el atacante
    /// tiene más todavía —toda la clave de visión de Orchard de Bob— pero no la
    /// semilla de X-Wing, que no sale de ninguna clave pública de curva.
    #[test]
    fn a_quantum_attacker_with_every_orchard_key_still_cannot_read_the_note() {
        let mut rng = StdRng::seed_from_u64(9);
        let bob = wallet(2);
        let s = sample(*bob.address().orchard(), 30_000_000, &mut rng);
        let envelope = envelope_for(&bob, &s, &memo(b"secreto"), None, &mut rng);

        let mut stolen = bob.to_bytes();
        rng.fill_bytes(&mut stolen[97..]);
        let attacker = QuanticFullViewingKey::from_bytes(stolen.as_ref()).unwrap();
        assert_eq!(attacker.orchard().to_bytes(), bob.orchard().to_bytes());

        assert!(try_note_decryption(&attacker, &s.nf, &s.cmx, &envelope).is_none());
        assert!(try_compact_note_decryption(
            &attacker,
            &s.nf,
            &s.cmx,
            envelope.kem_ciphertext(),
            envelope.enc_ciphertext_compact(),
        )
        .is_none());
        assert!(try_note_decryption(&bob, &s.nf, &s.cmx, &envelope).is_some());
    }

    #[test]
    fn change_is_recognized_as_internal() {
        let mut rng = StdRng::seed_from_u64(2);
        let alice = wallet(1);
        let s = sample(*alice.change_address().orchard(), 7, &mut rng);
        let envelope = envelope_for(&alice, &s, &[0; MEMO_SIZE], None, &mut rng);
        let found = try_note_decryption(&alice, &s.nf, &s.cmx, &envelope).unwrap();
        assert_eq!(found.scope, Scope::Internal);
    }

    #[test]
    fn the_compact_part_alone_finds_the_same_note() {
        let mut rng = StdRng::seed_from_u64(3);
        let bob = wallet(2);
        let s = sample(*bob.address().orchard(), 5, &mut rng);
        let envelope = envelope_for(&bob, &s, &memo(b"x"), None, &mut rng);
        let (note, scope) = try_compact_note_decryption(
            &bob,
            &s.nf,
            &s.cmx,
            envelope.kem_ciphertext(),
            envelope.enc_ciphertext_compact(),
        )
        .unwrap();
        assert_eq!(note.value().inner(), 5);
        assert_eq!(scope, Scope::External);
        assert!(try_compact_note_decryption(
            &wallet(3),
            &s.nf,
            &s.cmx,
            envelope.kem_ciphertext(),
            envelope.enc_ciphertext_compact()
        )
        .is_none());
    }

    #[test]
    fn a_changed_byte_anywhere_hides_the_note() {
        let mut rng = StdRng::seed_from_u64(4);
        let bob = wallet(2);
        let s = sample(*bob.address().orchard(), 9, &mut rng);
        let envelope = envelope_for(&bob, &s, &[0; MEMO_SIZE], None, &mut rng);
        // ML-KEM part, X25519 part, note body and tag.
        for offset in [0, 1100, 1119] {
            let mut kem = *envelope.kem_ciphertext();
            kem[offset] ^= 1;
            let altered = NoteEnvelope::from_parts(
                kem,
                *envelope.enc_ciphertext(),
                *envelope.out_ciphertext(),
            );
            assert!(
                try_note_decryption(&bob, &s.nf, &s.cmx, &altered).is_none(),
                "kem {offset}"
            );
        }
        for offset in [0, 30, ENC_CIPHERTEXT_SIZE - 1] {
            let mut enc = *envelope.enc_ciphertext();
            enc[offset] ^= 1;
            let altered = NoteEnvelope::from_parts(
                *envelope.kem_ciphertext(),
                enc,
                *envelope.out_ciphertext(),
            );
            assert!(
                try_note_decryption(&bob, &s.nf, &s.cmx, &altered).is_none(),
                "enc {offset}"
            );
        }
        // The envelope of one action does not open in another one.
        let other = sample(*bob.address().orchard(), 9, &mut rng);
        assert!(try_note_decryption(&bob, &other.nf, &other.cmx, &envelope).is_none());
    }

    #[test]
    fn a_ciphertext_that_is_not_the_one_of_the_note_is_refused() {
        // A sender who encapsulates with randomness of their own: the note
        // decrypts, but the wallet rejects it, as Orchard rejects a wrong epk.
        let mut rng = StdRng::seed_from_u64(5);
        let bob = wallet(2);
        let s = sample(*bob.address().orchard(), 11, &mut rng);
        let (kem, ss) = bob.xwing_public().encapsulate_derand(&[42; 64]);
        let kem: [u8; KEM_CIPHERTEXT_SIZE] = kem.as_bytes().try_into().unwrap();
        let mut plaintext = note_plaintext(&s.note, &[0; MEMO_SIZE]);
        let tag = seal(&note_key(&ss, &kem, &s.cmx), plaintext.as_mut());
        let mut enc = [0u8; ENC_CIPHERTEXT_SIZE];
        enc[..NOTE_PLAINTEXT_SIZE].copy_from_slice(plaintext.as_ref());
        enc[NOTE_PLAINTEXT_SIZE..].copy_from_slice(&tag);
        let envelope = NoteEnvelope::from_parts(kem, enc, [0; OUT_CIPHERTEXT_SIZE]);
        assert!(try_note_decryption(&bob, &s.nf, &s.cmx, &envelope).is_none());
    }

    #[test]
    fn the_sender_recovers_what_it_sent_only_with_its_ovk() {
        let mut rng = StdRng::seed_from_u64(6);
        let alice = wallet(1);
        let bob = wallet(2);
        let ovk = alice.ovk(Scope::External);
        let s = sample(*bob.address().orchard(), 1_234, &mut rng);
        let envelope = envelope_for(&bob, &s, &memo(b"factura 7"), Some(&ovk), &mut rng);
        let (note, to, memo) =
            try_output_recovery_with_ovk(&ovk, &s.nf, &s.cmx, &s.cv_net, &envelope).unwrap();
        assert_eq!(note.value().inner(), 1_234);
        assert_eq!(
            to.to_raw_address_bytes(),
            bob.address().orchard().to_raw_address_bytes()
        );
        assert_eq!(&memo[..9], b"factura 7");
        let bob_ovk = bob.ovk(Scope::External);
        assert!(
            try_output_recovery_with_ovk(&bob_ovk, &s.nf, &s.cmx, &s.cv_net, &envelope).is_none()
        );
        // Without an ovk, `out` is noise.
        let hidden = envelope_for(&bob, &s, &[0; MEMO_SIZE], None, &mut rng);
        assert!(try_output_recovery_with_ovk(&ovk, &s.nf, &s.cmx, &s.cv_net, &hidden).is_none());
    }

    #[test]
    fn a_public_output_can_be_checked_by_anyone_against_its_announcement() {
        let mut rng = StdRng::seed_from_u64(7);
        let team = wallet(9);
        let s = sample(*team.address().orchard(), 5_000_000, &mut rng);
        let public = OutgoingViewingKey::from(PUBLIC_OVK);
        let envelope = envelope_for(&team, &s, &[0; MEMO_SIZE], Some(&public), &mut rng);
        let value = NoteValue::from_raw(5_000_000);
        let check = |to: &crate::address::QuanticAddress, value: NoteValue| {
            verify_public_output(to, value, &s.nf, &s.cmx, &s.cv_net, &envelope)
        };
        assert!(check(&team.address(), value));
        assert!(!check(&team.address(), NoteValue::from_raw(4_999_999)));
        assert!(!check(&wallet(8).address(), value));
        // Same Orchard receiver with another X-Wing key: refused.
        let forged = crate::address::QuanticAddress::from_parts(
            Network::Regtest,
            *team.address().orchard(),
            wallet(8).xwing_public().clone(),
        );
        assert!(!check(&forged, value));
        // And the team still receives it normally.
        assert!(try_note_decryption(&team, &s.nf, &s.cmx, &envelope).is_some());
    }

    #[test]
    fn a_padding_output_opens_for_nobody() {
        let mut rng = StdRng::seed_from_u64(8);
        let bob = wallet(2);
        let s = sample(*bob.address().orchard(), 0, &mut rng);
        let envelope = encrypt_dummy_note(&s.note, &s.cv_net, &s.cmx, &mut rng);
        assert!(try_note_decryption(&bob, &s.nf, &s.cmx, &envelope).is_none());
    }

    #[test]
    fn sizes_match_orchard_and_x_wing() {
        assert_eq!(
            NOTE_PLAINTEXT_SIZE,
            orchard::note_encryption::NOTE_PLAINTEXT_SIZE
        );
        assert_eq!(
            ENC_CIPHERTEXT_SIZE,
            orchard::note_encryption::ENC_CIPHERTEXT_SIZE
        );
        assert_eq!(
            COMPACT_NOTE_SIZE,
            orchard::note_encryption::COMPACT_NOTE_SIZE
        );
        assert_eq!(KEM_CIPHERTEXT_SIZE, 1120);
        assert_eq!(OUT_CIPHERTEXT_SIZE, 91);
    }
}
