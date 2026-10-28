//! Fixed, versioned algebraic hash. Constants are the upstream canonical
//! KoalaBear width-32 Poseidon2 constants, shared by hashing and the AIR.
use p3_field::{PrimeCharacteristicRing, PrimeField32};
use p3_koala_bear::*;
use p3_poseidon2_air::{Poseidon2Air, RoundConstants};
use p3_symmetric::Permutation;
use serde::{Deserialize, Serialize};

pub type F = KoalaBear;
pub const WORDS: usize = 16;
pub const WIDTH: usize = 32;
pub const RATE: usize = 16;
pub const ROUNDS: usize = KOALABEAR_POSEIDON2_PARTIAL_ROUNDS_32;
pub type Perm = Poseidon2KoalaBear<WIDTH>;
pub type HashAir = Poseidon2Air<F, GenericPoseidon2LinearLayersKoalaBear, WIDTH, 3, 0, 4, ROUNDS>;
pub const OWNER: u32 = 1;
pub const NOTE: u32 = 2;
pub const NULLIFIER: u32 = 3;
pub const TREE: u32 = 4;
/// `H(owner ‖ rcm)`: hides the owner inside a note commitment.
pub const HIDE: u32 = 5;
/// The `rho` of a transfer output, from the transaction's first nullifier.
pub const RHO: u32 = 6;
/// The `rho` of a coinbase output, from its block height.
pub const COINBASE_RHO: u32 = 7;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Digest(pub [u32; WORDS]);

impl std::fmt::Debug for Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Digest").field(&self.0).finish()
    }
}

impl Digest {
    pub const ZERO: Self = Self([0; WORDS]);
    pub fn fields(&self) -> Result<[F; WORDS], crate::Error> {
        if self.0.iter().any(|&x| x >= F::ORDER_U32) {
            return Err(crate::Error::Encoding);
        }
        Ok(self.0.map(F::from_u32))
    }
    pub fn from_fields(values: [F; WORDS]) -> Self {
        Self(values.map(|x| x.as_canonical_u32()))
    }
}

pub fn constants() -> RoundConstants<F, WIDTH, 4, ROUNDS> {
    RoundConstants::new(
        KOALABEAR_POSEIDON2_RC_32_EXTERNAL_INITIAL,
        KOALABEAR_POSEIDON2_RC_32_INTERNAL,
        KOALABEAR_POSEIDON2_RC_32_EXTERNAL_FINAL,
    )
}

/// The leading domain and network and final 1 padding are part of the hash.
/// Digest words are canonical field elements, never reduced from wire input.
pub fn hash(domain: u32, network: u8, message: &[F]) -> Digest {
    let mut words = vec![F::from_u32(domain), F::from_u8(network)];
    words.extend_from_slice(message);
    words.push(F::ONE);
    words.resize(words.len().div_ceil(RATE) * RATE, F::ZERO);
    let permutation = default_koalabear_poseidon2_32();
    let mut state = [F::ZERO; WIDTH];
    for chunk in words.chunks_exact(RATE) {
        for i in 0..RATE {
            state[i] += chunk[i];
        }
        permutation.permute_mut(&mut state);
    }
    Digest::from_fields(state[..WORDS].try_into().unwrap())
}

pub fn value_fields(value: u64) -> [F; 4] {
    std::array::from_fn(|i| F::from_u16((value >> (i * 16)) as u16))
}

pub fn parent(
    network: u8,
    level: usize,
    left: Digest,
    right: Digest,
) -> Result<Digest, crate::Error> {
    let mut data = vec![F::from_usize(level)];
    data.extend(left.fields()?);
    data.extend(right.fields()?);
    Ok(hash(TREE, network, &data))
}
