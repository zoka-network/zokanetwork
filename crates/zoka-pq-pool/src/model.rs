use crate::crypto::{self, Digest, F, WORDS};
use crate::Error;
use p3_field::{PrimeCharacteristicRing, PrimeField32};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub const DEPTH: usize = 32;
pub const MAX_INPUTS: usize = 8;
pub const MAX_OUTPUTS: usize = 8;

/// A spending secret has no curve-based public key. The owner tag is a hash.
/// Its debug representation deliberately contains no witness data.
pub struct SpendKey(Zeroizing<[u32; WORDS]>);
impl Clone for SpendKey {
    fn clone(&self) -> Self {
        Self(Zeroizing::new(*self.0))
    }
}
impl SpendKey {
    pub fn from_seed(seed: &[u8; 32], network: u8, account: u32) -> Result<Self, Error> {
        check_network(network)?;
        let mut state = blake2b_simd::Params::new()
            .hash_length(64)
            .personal(b"ZokaPQSpendKey2_")
            .to_state();
        state.update(seed);
        state.update(&[network]);
        state.update(&account.to_le_bytes());
        let digest = state.finalize();
        Ok(Self(Zeroizing::new(std::array::from_fn(|i| {
            u32::from_le_bytes(digest.as_bytes()[i * 4..i * 4 + 4].try_into().unwrap())
                % F::ORDER_U32
        }))))
    }
    pub(crate) fn fields(&self) -> [F; WORDS] {
        (*self.0).map(F::from_u32)
    }
    pub fn owner(&self, network: u8) -> Digest {
        crypto::hash(crypto::OWNER, network, &self.fields())
    }
}
impl std::fmt::Debug for SpendKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SpendKey([REDACTED])")
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Note {
    pub owner: Digest,
    pub rho: Digest,
    pub value: u64,
}
impl Note {
    pub fn commitment(&self, network: u8) -> Result<Digest, Error> {
        let mut fields = self.owner.fields()?.to_vec();
        fields.extend(self.rho.fields()?);
        fields.extend(crypto::value_fields(self.value));
        Ok(crypto::hash(crypto::NOTE, network, &fields))
    }
    pub fn nullifier(&self, key: &SpendKey, network: u8) -> Result<Digest, Error> {
        let mut fields = key.fields().to_vec();
        fields.extend(self.rho.fields()?);
        Ok(crypto::hash(crypto::NULLIFIER, network, &fields))
    }
}

#[derive(Clone)]
pub struct MerklePath {
    pub index: u32,
    pub siblings: [Digest; DEPTH],
}
impl MerklePath {
    pub fn root(&self, network: u8, leaf: Digest) -> Result<Digest, Error> {
        let mut current = leaf;
        for (level, sibling) in self.siblings.iter().enumerate() {
            current = if (self.index >> level) & 1 == 0 {
                crypto::parent(network, level, current, *sibling)?
            } else {
                crypto::parent(network, level, *sibling, current)?
            };
        }
        Ok(current)
    }
}

pub struct Input {
    pub key: SpendKey,
    pub note: Note,
    pub path: MerklePath,
}
pub struct Witness {
    pub inputs: Vec<Input>,
    pub outputs: Vec<Note>,
}

/// Public effects. `binding` must be the digest of the complete transaction
/// effects (including ciphertexts) calculated by the consuming protocol.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statement {
    pub version: u32,
    pub network: u8,
    pub anchor: Digest,
    pub nullifiers: Vec<Digest>,
    pub commitments: Vec<Digest>,
    pub fee: u64,
    pub minted: u64,
    pub binding: [u8; 32],
}

pub fn check_network(network: u8) -> Result<(), Error> {
    if !(1..=3).contains(&network) {
        return Err(Error::Network);
    }
    Ok(())
}

impl Statement {
    pub fn validate(&self) -> Result<(), Error> {
        check_network(self.network)?;
        if self.version != 2
            || self.nullifiers.len() > MAX_INPUTS
            || self.commitments.is_empty()
            || self.commitments.len() > MAX_OUTPUTS
            || (self.nullifiers.is_empty() && self.fee != 0)
            || (!self.nullifiers.is_empty() && self.minted != 0)
        {
            return Err(Error::Shape);
        }
        let _ = self.anchor.fields()?;
        for digest in self.nullifiers.iter().chain(&self.commitments) {
            let _ = digest.fields()?;
        }
        let unique: std::collections::BTreeSet<_> = self.nullifiers.iter().collect();
        if unique.len() != self.nullifiers.len() {
            return Err(Error::DuplicateNullifier);
        }
        Ok(())
    }

    pub fn from_witness(
        network: u8,
        anchor: Digest,
        fee: u64,
        minted: u64,
        binding: [u8; 32],
        witness: &Witness,
    ) -> Result<Self, Error> {
        let value = Self {
            version: 2,
            network,
            anchor,
            fee,
            minted,
            binding,
            nullifiers: witness
                .inputs
                .iter()
                .map(|i| i.note.nullifier(&i.key, network))
                .collect::<Result<_, _>>()?,
            commitments: witness
                .outputs
                .iter()
                .map(|n| n.commitment(network))
                .collect::<Result<_, _>>()?,
        };
        value.validate()?;
        Ok(value)
    }

    pub(crate) fn public_values(&self) -> Result<Vec<F>, Error> {
        self.validate()?;
        let mut values = vec![F::from_u32(self.version), F::from_u8(self.network)];
        values.extend(crypto::value_fields(self.fee));
        values.extend(crypto::value_fields(self.minted));
        values.extend(self.anchor.fields()?);
        for digest in self.nullifiers.iter().chain(&self.commitments) {
            values.extend(digest.fields()?);
        }
        values.extend(self.binding.map(F::from_u8));
        Ok(values)
    }
}
