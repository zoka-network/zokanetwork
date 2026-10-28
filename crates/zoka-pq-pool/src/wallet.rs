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
        match self.network {
            1 => "zq2",
            2 => "zqtest2",
            _ => "zqregtest2",
        }
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
        let prefix = match network {
            1 => "zq2",
            2 => "zqtest2",
            _ => "zqregtest2",
        };
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
    pub fn short(&self) -> Result<String, Error> {
        let full = self.encode()?;
        let hash = digest(b"ZokaPQShort2____", &[full.as_bytes()]);
        let check = digest(b"ZokaPQShortSum2_", &[self.prefix().as_bytes(), &hash]);
        let mut data = hash.to_vec();
        data.extend(&check[..4]);
        Ok(format!(
            "{}{}",
            self.prefix(),
            bs58::encode(data).into_string()
        ))
    }
}

pub struct Account {
    pub network: u8,
    pub spend: SpendKey,
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
            view,
            outgoing,
        })
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
        envelope.validate()?;
        let ciphertext =
            XWingCiphertext::from_bytes(&envelope.kem).map_err(|_| Error::Encryption)?;
        let shared = self
            .view
            .decapsulate(&ciphertext)
            .map_err(|_| Error::Encryption)?;
        let aad = associated(self.network, commitment, &envelope.kem);
        let key = Zeroizing::new(digest(b"ZokaPQNoteKey2__", &[shared.expose_secret(), &aad]));
        let opened = open(&key, &envelope.nonce, &aad, &envelope.encrypted)?;
        let (note, memo) = decode_plaintext(&opened)?;
        if note.owner != self.spend.owner(self.network)
            || note.commitment(self.network)? != commitment
        {
            return Err(Error::Encryption);
        }
        Ok((note, memo))
    }
    pub fn recover_sent(
        &self,
        commitment: Digest,
        envelope: &Envelope,
    ) -> Result<(Note, [u8; MEMO_BYTES]), Error> {
        envelope.validate()?;
        let aad = associated(self.network, commitment, &envelope.kem);
        let key = Zeroizing::new(digest(b"ZokaPQOutNote2__", &[self.outgoing.as_ref(), &aad]));
        let opened = open(&key, &envelope.out_nonce, &aad, &envelope.outgoing)?;
        let result = decode_plaintext(&opened)?;
        if result.0.commitment(self.network)? != commitment {
            return Err(Error::Encryption);
        }
        Ok(result)
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
}
