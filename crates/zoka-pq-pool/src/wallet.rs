//! Curve-free ownership tags and hybrid-encrypted, fixed-size note envelopes.
use crate::{
    crypto::Digest,
    model::{check_network, Note, SpendKey},
    Error,
};
use chacha20poly1305::{AeadInOut, ChaCha20Poly1305, KeyInit};
use rand::TryRng;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;
use zoka_pq::{XWingCiphertext, XWingPublicKey, XWingSecretKey, X_WING_CIPHERTEXT_BYTES};

pub const MEMO_BYTES: usize = 512;
/// Version byte, owner, rho, rcm, value and memo.
const PLAINTEXT_BYTES: usize = 1 + 64 + 64 + 64 + 8 + MEMO_BYTES;
pub const SEALED_NOTE_BYTES: usize = PLAINTEXT_BYTES + 16;

pub(crate) fn digest(domain: &[u8; 16], parts: &[&[u8]]) -> [u8; 32] {
    let mut state = blake2b_simd::Params::new()
        .hash_length(32)
        .personal(domain)
        .to_state();
    for part in parts {
        state.update(part);
    }
    state.finalize().as_bytes().try_into().unwrap()
}
fn fresh<const N: usize>() -> Result<[u8; N], Error> {
    let mut bytes = [0; N];
    rand::rngs::SysRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| Error::Encryption)?;
    Ok(bytes)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    pub network: u8,
    pub owner: Digest,
    pub xwing: Vec<u8>,
}
impl Address {
    pub fn validate(&self) -> Result<(), Error> {
        check_network(self.network)?;
        let _ = self.owner.fields()?;
        XWingPublicKey::from_bytes(&self.xwing).map_err(|_| Error::Encoding)?;
        Ok(())
    }
    fn prefix(&self) -> &'static str {
        address_prefix(self.network)
    }
    pub fn encode(&self) -> Result<String, Error> {
        self.validate()?;
        let mut payload = vec![2, self.network];
        for word in self.owner.0 {
            payload.extend(word.to_le_bytes());
        }
        payload.extend(&self.xwing);
        let check = digest(b"ZokaPQAddress2__", &[self.prefix().as_bytes(), &payload]);
        payload.extend(&check[..4]);
        Ok(format!(
            "{}{}",
            self.prefix(),
            bs58::encode(payload).into_string()
        ))
    }
    pub fn decode(text: &str, network: u8) -> Result<Self, Error> {
        check_network(network)?;
        let prefix = address_prefix(network);
        let body = text.strip_prefix(prefix).ok_or(Error::Network)?;
        if body.len() > 2100 {
            return Err(Error::Encoding);
        }
        let raw = bs58::decode(body).into_vec().map_err(|_| Error::Encoding)?;
        if raw.len() != 2 + 64 + zoka_pq::X_WING_PUBLIC_KEY_BYTES + 4 || raw[..2] != [2, network] {
            return Err(Error::Encoding);
        }
        let end = raw.len() - 4;
        let check = digest(b"ZokaPQAddress2__", &[prefix.as_bytes(), &raw[..end]]);
        if raw[end..] != check[..4] {
            return Err(Error::Encoding);
        }
        let owner = Digest(std::array::from_fn(|i| {
            u32::from_le_bytes(raw[2 + i * 4..6 + i * 4].try_into().unwrap())
        }));
        let result = Self {
            network,
            owner,
            xwing: raw[66..end].to_vec(),
        };
        result.validate()?;
        Ok(result)
    }
    pub fn short_address(&self) -> Result<ShortAddress, Error> {
        Ok(ShortAddress {
            network: self.network,
            digest: digest(b"ZokaPQShort2____", &[self.encode()?.as_bytes()]),
        })
    }
    pub fn short(&self) -> Result<String, Error> {
        Ok(self.short_address()?.encode())
    }
}

/// `zq2`, `zqtest2` or `zqregtest2`: what both address forms start with.
pub fn address_prefix(network: u8) -> &'static str {
    match network {
        1 => "zq2",
        2 => "zqtest2",
        _ => "zqregtest2",
    }
}

/// The digest of a full address string. Paying to it needs the full address,
/// fetched from a node and checked with [`ShortAddress::matches`]: whoever
/// serves it can withhold it, not change its keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShortAddress {
    pub network: u8,
    pub digest: [u8; 32],
}
impl ShortAddress {
    pub fn encode(&self) -> String {
        let prefix = address_prefix(self.network);
        let check = digest(b"ZokaPQShortSum2_", &[prefix.as_bytes(), &self.digest]);
        let mut data = self.digest.to_vec();
        data.extend(&check[..4]);
        format!("{prefix}{}", bs58::encode(data).into_string())
    }
    pub fn decode(text: &str, network: u8) -> Result<Self, Error> {
        check_network(network)?;
        let prefix = address_prefix(network);
        let body = text.strip_prefix(prefix).ok_or(Error::Network)?;
        if body.len() > 64 {
            return Err(Error::Encoding);
        }
        let raw = bs58::decode(body).into_vec().map_err(|_| Error::Encoding)?;
        if raw.len() != 36 {
            return Err(Error::Encoding);
        }
        let check = digest(b"ZokaPQShortSum2_", &[prefix.as_bytes(), &raw[..32]]);
        if raw[32..] != check[..4] {
            return Err(Error::Encoding);
        }
        Ok(Self {
            network,
            digest: raw[..32].try_into().unwrap(),
        })
    }
    pub fn matches(&self, full: &Address) -> bool {
        full.short_address().is_ok_and(|short| short == *self)
    }
}

pub struct Account {
    pub network: u8,
    pub spend: SpendKey,
    view_seed: Zeroizing<[u8; 32]>,
    view: XWingSecretKey,
    outgoing: Zeroizing<[u8; 32]>,
}
impl Account {
    pub fn from_seed(seed: &[u8; 32], network: u8, index: u32) -> Result<Self, Error> {
        let spend = SpendKey::from_seed(seed, network, index)?;
        let view_seed = Zeroizing::new(digest(
            b"ZokaPQViewKey2__",
            &[seed, &[network], &index.to_le_bytes()],
        ));
        let view = XWingSecretKey::from_seed(view_seed.as_ref()).map_err(|_| Error::Encryption)?;
        let outgoing = Zeroizing::new(digest(
            b"ZokaPQOutKey2___",
            &[seed, &[network], &index.to_le_bytes()],
        ));
        Ok(Self {
            network,
            spend,
            view_seed,
            view,
            outgoing,
        })
    }
    /// What this account can show without spending: see [`ViewKey`].
    pub fn view_key(&self) -> Result<ViewKey, Error> {
        ViewKey::from_parts(
            self.network,
            self.spend.owner(self.network),
            *self.view_seed,
            *self.outgoing,
        )
    }
    pub fn address(&self) -> Address {
        Address {
            network: self.network,
            owner: self.spend.owner(self.network),
            xwing: self.view.public_key().to_bytes().to_vec(),
        }
    }
    pub fn receive(
        &self,
        commitment: Digest,
        envelope: &Envelope,
    ) -> Result<(Note, [u8; MEMO_BYTES]), Error> {
        open_received(
            &self.view,
            self.spend.owner(self.network),
            self.network,
            commitment,
            envelope,
        )
    }
    pub fn recover_sent(
        &self,
        commitment: Digest,
        envelope: &Envelope,
    ) -> Result<(Note, [u8; MEMO_BYTES]), Error> {
        open_sent(&self.outgoing, self.network, commitment, envelope)
    }
}

fn open_received(
    view: &XWingSecretKey,
    owner: Digest,
    network: u8,
    commitment: Digest,
    envelope: &Envelope,
) -> Result<(Note, [u8; MEMO_BYTES]), Error> {
    envelope.validate()?;
    let ciphertext = XWingCiphertext::from_bytes(&envelope.kem).map_err(|_| Error::Encryption)?;
    let shared = view
        .decapsulate(&ciphertext)
        .map_err(|_| Error::Encryption)?;
    let aad = associated(network, commitment, &envelope.kem);
    let key = Zeroizing::new(digest(b"ZokaPQNoteKey2__", &[shared.expose_secret(), &aad]));
    let opened = open(&key, &envelope.nonce, &aad, &envelope.encrypted)?;
    let (note, memo) = decode_plaintext(&opened)?;
    if note.owner != owner || note.commitment(network)? != commitment {
        return Err(Error::Encryption);
    }
    Ok((note, memo))
}

fn open_sent(
    outgoing: &[u8; 32],
    network: u8,
    commitment: Digest,
    envelope: &Envelope,
) -> Result<(Note, [u8; MEMO_BYTES]), Error> {
    envelope.validate()?;
    let aad = associated(network, commitment, &envelope.kem);
    let key = Zeroizing::new(digest(b"ZokaPQOutNote2__", &[outgoing, &aad]));
    let opened = open(&key, &envelope.out_nonce, &aad, &envelope.outgoing)?;
    let result = decode_plaintext(&opened)?;
    if result.0.commitment(network)? != commitment {
        return Err(Error::Encryption);
    }
    Ok(result)
}

/// `zqview2`, `zqtestview2` or `zqregtestview2`: a view key, never confused
/// with an address (whose prefixes end in the `2` before the key data).
pub fn view_key_prefix(network: u8) -> &'static str {
    match network {
        1 => "zqview2",
        2 => "zqtestview2",
        _ => "zqregtestview2",
    }
}

const VIEW_KEY_BYTES: usize = 2 + 64 + 32 + 32;

/// What a wallet can show without being able to spend: the notes it received
/// (its X-Wing key), the ones it sent, change included (its outgoing key), and
/// the owner digest its notes carry, which its address already shows. With
/// both, a scanner finds the wallet's transactions and their totals, so its
/// balance. Whoever holds it sees the wallet's whole history; nobody can spend
/// with it, because spending needs the spending key.
pub struct ViewKey {
    pub network: u8,
    pub owner: Digest,
    seed: Zeroizing<[u8; 32]>,
    view: XWingSecretKey,
    outgoing: Zeroizing<[u8; 32]>,
}
impl ViewKey {
    fn from_parts(
        network: u8,
        owner: Digest,
        seed: [u8; 32],
        outgoing: [u8; 32],
    ) -> Result<Self, Error> {
        check_network(network)?;
        let _ = owner.fields()?;
        let seed = Zeroizing::new(seed);
        let view = XWingSecretKey::from_seed(seed.as_ref()).map_err(|_| Error::Encryption)?;
        Ok(Self {
            network,
            owner,
            seed,
            view,
            outgoing: Zeroizing::new(outgoing),
        })
    }
    pub fn encode(&self) -> String {
        let prefix = view_key_prefix(self.network);
        let mut payload = Zeroizing::new(Vec::with_capacity(VIEW_KEY_BYTES + 4));
        payload.extend([1, self.network]);
        for word in self.owner.0 {
            payload.extend(word.to_le_bytes());
        }
        payload.extend(self.seed.as_ref());
        payload.extend(self.outgoing.as_ref());
        let check = digest(b"ZokaPQViewKeyC2_", &[prefix.as_bytes(), &payload]);
        payload.extend(&check[..4]);
        format!("{prefix}{}", bs58::encode(&payload[..]).into_string())
    }
    pub fn decode(text: &str, network: u8) -> Result<Self, Error> {
        check_network(network)?;
        let prefix = view_key_prefix(network);
        let body = text.trim().strip_prefix(prefix).ok_or(Error::Network)?;
        if body.len() > 260 {
            return Err(Error::Encoding);
        }
        let raw = Zeroizing::new(bs58::decode(body).into_vec().map_err(|_| Error::Encoding)?);
        if raw.len() != VIEW_KEY_BYTES + 4 || raw[..2] != [1, network] {
            return Err(Error::Encoding);
        }
        let check = digest(
            b"ZokaPQViewKeyC2_",
            &[prefix.as_bytes(), &raw[..VIEW_KEY_BYTES]],
        );
        if raw[VIEW_KEY_BYTES..] != check[..4] {
            return Err(Error::Encoding);
        }
        let owner = Digest(std::array::from_fn(|i| {
            u32::from_le_bytes(raw[2 + i * 4..6 + i * 4].try_into().unwrap())
        }));
        Self::from_parts(
            network,
            owner,
            raw[66..98].try_into().unwrap(),
            raw[98..130].try_into().unwrap(),
        )
    }
    /// The address whose payments this key sees.
    pub fn address(&self) -> Address {
        Address {
            network: self.network,
            owner: self.owner,
            xwing: self.view.public_key().to_bytes().to_vec(),
        }
    }
    pub fn receive(
        &self,
        commitment: Digest,
        envelope: &Envelope,
    ) -> Result<(Note, [u8; MEMO_BYTES]), Error> {
        open_received(&self.view, self.owner, self.network, commitment, envelope)
    }
    pub fn recover_sent(
        &self,
        commitment: Digest,
        envelope: &Envelope,
    ) -> Result<(Note, [u8; MEMO_BYTES]), Error> {
        open_sent(&self.outgoing, self.network, commitment, envelope)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub kem: Vec<u8>,
    pub nonce: [u8; 12],
    pub encrypted: Vec<u8>,
    pub out_nonce: [u8; 12],
    pub outgoing: Vec<u8>,
}
impl Envelope {
    pub fn validate(&self) -> Result<(), Error> {
        if self.kem.len() != X_WING_CIPHERTEXT_BYTES
            || self.encrypted.len() != SEALED_NOTE_BYTES
            || self.outgoing.len() != SEALED_NOTE_BYTES
        {
            return Err(Error::Encoding);
        }
        Ok(())
    }
}

fn associated(network: u8, commitment: Digest, kem: &[u8]) -> Vec<u8> {
    let mut aad = vec![2, network];
    for word in commitment.0 {
        aad.extend(word.to_le_bytes());
    }
    aad.extend(kem);
    aad
}
fn plaintext(note: &Note, memo: &[u8; MEMO_BYTES]) -> Zeroizing<Vec<u8>> {
    let mut bytes = Zeroizing::new(vec![3]);
    for word in note.owner.0.into_iter().chain(note.rho.0).chain(note.rcm.0) {
        bytes.extend(word.to_le_bytes());
    }
    bytes.extend(note.value.to_le_bytes());
    bytes.extend(memo);
    bytes
}
fn decode_plaintext(bytes: &[u8]) -> Result<(Note, [u8; MEMO_BYTES]), Error> {
    if bytes.len() != PLAINTEXT_BYTES || bytes[0] != 3 {
        return Err(Error::Encryption);
    }
    let read = |start| {
        Digest(std::array::from_fn(|i| {
            u32::from_le_bytes(bytes[start + i * 4..start + i * 4 + 4].try_into().unwrap())
        }))
    };
    let note = Note {
        owner: read(1),
        rho: read(65),
        rcm: read(129),
        value: u64::from_le_bytes(bytes[193..201].try_into().unwrap()),
    };
    Ok((note, bytes[201..].try_into().unwrap()))
}
fn seal(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], plain: &[u8]) -> Result<Vec<u8>, Error> {
    let mut body = plain.to_vec();
    let tag = ChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| Error::Encryption)?
        .encrypt_inout_detached(nonce.into(), aad, body.as_mut_slice().into())
        .map_err(|_| Error::Encryption)?;
    body.extend(tag);
    Ok(body)
}
fn open(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    encrypted: &[u8],
) -> Result<Zeroizing<Vec<u8>>, Error> {
    if encrypted.len() != SEALED_NOTE_BYTES {
        return Err(Error::Encryption);
    }
    let mut body = Zeroizing::new(encrypted[..PLAINTEXT_BYTES].to_vec());
    let tag: &[u8; 16] = encrypted[PLAINTEXT_BYTES..]
        .try_into()
        .map_err(|_| Error::Encryption)?;
    ChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| Error::Encryption)?
        .decrypt_inout_detached(nonce.into(), aad, body.as_mut_slice().into(), tag.into())
        .map_err(|_| Error::Encryption)?;
    Ok(body)
}

/// `rho` comes from the transaction (`model::transfer_rho`/`coinbase_rho`);
/// the hiding randomness `rcm` is fresh.
pub fn encrypt_note(
    to: &Address,
    value: u64,
    memo: &[u8; MEMO_BYTES],
    sender: Option<&Account>,
    rho: Digest,
) -> Result<(Note, Envelope), Error> {
    use p3_field::PrimeField32;
    to.validate()?;
    if sender.is_some_and(|s| s.network != to.network) {
        return Err(Error::Network);
    }
    let _ = rho.fields()?;
    let entropy = fresh::<64>()?;
    let rcm = Digest(std::array::from_fn(|i| {
        u32::from_le_bytes(entropy[i * 4..i * 4 + 4].try_into().unwrap())
            % crate::crypto::F::ORDER_U32
    }));
    let note = Note {
        owner: to.owner,
        rho,
        rcm,
        value,
    };
    let commitment = note.commitment(to.network)?;
    let (kem, shared) = XWingPublicKey::from_bytes(&to.xwing)
        .map_err(|_| Error::Encryption)?
        .encapsulate();
    let kem = kem.as_bytes().to_vec();
    let aad = associated(to.network, commitment, &kem);
    let key = Zeroizing::new(digest(b"ZokaPQNoteKey2__", &[shared.expose_secret(), &aad]));
    let nonce = fresh()?;
    let out_nonce = fresh()?;
    let plain = plaintext(&note, memo);
    let encrypted = seal(&key, &nonce, &aad, &plain)?;
    let out_key = Zeroizing::new(match sender {
        Some(s) => digest(b"ZokaPQOutNote2__", &[s.outgoing.as_ref(), &aad]),
        None => fresh()?,
    });
    let outgoing = seal(&out_key, &out_nonce, &aad, &plain)?;
    Ok((
        note,
        Envelope {
            kem,
            nonce,
            encrypted,
            out_nonce,
            outgoing,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recipient_sender_and_network_are_bound() {
        let alice = Account::from_seed(&[1; 32], 3, 0).unwrap();
        let bob = Account::from_seed(&[2; 32], 3, 0).unwrap();
        let address = bob.address();
        assert_eq!(
            Address::decode(&address.encode().unwrap(), 3).unwrap(),
            address
        );
        assert!(Address::decode(&address.encode().unwrap(), 2).is_err());
        assert!(address.short().unwrap().len() < 65);
        let rho = Digest([5; crate::crypto::WORDS]);
        let (note, envelope) =
            encrypt_note(&address, 456, &[9; MEMO_BYTES], Some(&alice), rho).unwrap();
        assert_eq!(note.rho, rho);
        let cm = note.commitment(3).unwrap();
        assert_eq!(bob.receive(cm, &envelope).unwrap().0.value, 456);
        assert_eq!(
            alice.recover_sent(cm, &envelope).unwrap().1,
            [9; MEMO_BYTES]
        );
        assert!(alice.receive(cm, &envelope).is_err());
        assert!(bob.recover_sent(cm, &envelope).is_err());
        for field in 0..4 {
            let mut broken = envelope.clone();
            match field {
                0 => broken.kem[10] ^= 1,
                1 => broken.encrypted[50] ^= 1,
                2 => broken.nonce[0] ^= 1,
                _ => broken.outgoing[0] ^= 1,
            }
            if field < 3 {
                assert!(bob.receive(cm, &broken).is_err())
            } else {
                assert!(alice.recover_sent(cm, &broken).is_err())
            }
        }
        let mut wrong = cm;
        wrong.0[0] += 1;
        assert!(bob.receive(wrong, &envelope).is_err());
    }

    #[test]
    fn a_short_address_names_one_full_address() {
        let bob = Account::from_seed(&[2; 32], 2, 0).unwrap().address();
        let carol = Account::from_seed(&[3; 32], 2, 0).unwrap().address();
        let short = bob.short_address().unwrap();
        let text = short.encode();
        assert_eq!(text, bob.short().unwrap());
        // Wallets and node directories depend on this exact form.
        assert_eq!(
            text,
            "zqtest22m1iDEYKpaemBFZMVj2k1SAzEV9BGL1uFXRZVABUrvAYuHQRbj"
        );
        assert_eq!(ShortAddress::decode(&text, 2).unwrap(), short);
        assert!(short.matches(&bob) && !short.matches(&carol));
        // Another network, a full address where a short one goes (and the
        // reverse) and a typo are all refused.
        assert!(ShortAddress::decode(&text, 3).is_err());
        assert!(ShortAddress::decode(&bob.encode().unwrap(), 2).is_err());
        assert!(Address::decode(&text, 2).is_err());
        let last = text.chars().last().unwrap();
        let typo = format!(
            "{}{}",
            &text[..text.len() - 1],
            if last == 'a' { 'b' } else { 'a' }
        );
        assert!(ShortAddress::decode(&typo, 2).is_err());
    }

    #[test]
    fn a_view_key_sees_what_its_wallet_sees_and_nothing_else() {
        let alice = Account::from_seed(&[1; 32], 2, 0).unwrap();
        let bob = Account::from_seed(&[2; 32], 2, 0).unwrap();
        let key = alice.view_key().unwrap();
        let text = key.encode();
        assert!(text.starts_with("zqtestview2"));
        let decoded = ViewKey::decode(&text, 2).unwrap();
        assert_eq!(decoded.encode(), text);
        assert_eq!(decoded.address(), alice.address());
        // Another network, an address where a key goes and a typo are refused.
        assert!(ViewKey::decode(&text, 3).is_err());
        assert!(ViewKey::decode(&alice.address().encode().unwrap(), 2).is_err());
        let last = text.chars().last().unwrap();
        let typo = format!(
            "{}{}",
            &text[..text.len() - 1],
            if last == 'a' { 'b' } else { 'a' }
        );
        assert!(ViewKey::decode(&typo, 2).is_err());

        let rho = Digest([7; crate::crypto::WORDS]);
        // A payment to alice, and one from alice to bob.
        let (paid, to_alice) =
            encrypt_note(&alice.address(), 300, &[1; MEMO_BYTES], Some(&bob), rho).unwrap();
        let (sent, to_bob) =
            encrypt_note(&bob.address(), 120, &[2; MEMO_BYTES], Some(&alice), rho).unwrap();
        let paid_cm = paid.commitment(2).unwrap();
        let sent_cm = sent.commitment(2).unwrap();
        assert_eq!(decoded.receive(paid_cm, &to_alice).unwrap().0.value, 300);
        assert_eq!(decoded.recover_sent(sent_cm, &to_bob).unwrap().0.value, 120);
        assert!(decoded.receive(sent_cm, &to_bob).is_err());
        assert!(decoded.recover_sent(paid_cm, &to_alice).is_err());
        // A coinbase has no sender: only the recipient's view key opens it.
        let (mined, coinbase) =
            encrypt_note(&alice.address(), 12, &[0; MEMO_BYTES], None, rho).unwrap();
        let mined_cm = mined.commitment(2).unwrap();
        assert_eq!(decoded.receive(mined_cm, &coinbase).unwrap().0.value, 12);
        assert!(decoded.recover_sent(mined_cm, &coinbase).is_err());
        assert!(bob
            .view_key()
            .unwrap()
            .receive(mined_cm, &coinbase)
            .is_err());
    }
}
