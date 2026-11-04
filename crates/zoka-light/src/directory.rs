//! El registro de una dirección corta: la dirección completa y una
//! estampilla de trabajo (Argon2id), como lo arma y lo verifica el nodo.

use serde::{Deserialize, Serialize};
use zoka_pq_pool::wallet::{address_prefix, Address, ShortAddress};

/// Bits en cero que pide la estampilla: unos 256 intentos en promedio.
pub const STAMP_BITS: u32 = 8;
/// Memoria de cada intento de Argon2id, en KiB.
pub const STAMP_MEMORY_KIB: u32 = 8 * 1024;
const STAMP_SALT: &[u8] = b"ZokaQ_AddrStampV2";

/// El JSON que acepta `POST /address` y que viaja por `SUBMIT_ADDRESS`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddressRecord {
    pub address: String,
    pub stamp: u64,
}

fn work_hash(input: &[u8]) -> [u8; 32] {
    let params = argon2::Params::new(STAMP_MEMORY_KIB, 1, 1, Some(32)).expect("fixed Argon2 parameters");
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = [0u8; 32];
    argon
        .hash_password_into(input, STAMP_SALT, &mut out)
        .expect("Argon2 with fixed parameters and salt");
    out
}

fn leading_zero_bits(bytes: &[u8]) -> u32 {
    let mut bits = 0;
    for byte in bytes {
        if *byte == 0 {
            bits += 8;
        } else {
            return bits + byte.leading_zeros();
        }
    }
    bits
}

fn stamp_hash(short: &ShortAddress, stamp: u64) -> [u8; 32] {
    let mut input = Vec::with_capacity(56);
    input.extend_from_slice(address_prefix(short.network).as_bytes());
    input.extend_from_slice(&short.digest);
    input.extend_from_slice(&stamp.to_le_bytes());
    work_hash(&input)
}

pub fn stamp_is_valid(short: &ShortAddress, stamp: u64) -> bool {
    leading_zero_bits(&stamp_hash(short, stamp)) >= STAMP_BITS
}

/// Busca una estampilla desde `start` (un número al azar del que llama: así
/// este crate no necesita una fuente de azar propia).
pub fn find_stamp(short: &ShortAddress, start: u64) -> u64 {
    (0u64..)
        .map(|i| start.wrapping_add(i))
        .find(|stamp| stamp_is_valid(short, *stamp))
        .expect("an unbounded search ends")
}

impl AddressRecord {
    /// El registro de `full`, con una estampilla buscada desde `start`.
    pub fn new(full: &Address, start: u64) -> Result<Self, String> {
        let short = full.short_address().map_err(|e| e.to_string())?;
        Ok(Self {
            address: full.encode().map_err(|e| e.to_string())?,
            stamp: find_stamp(&short, start),
        })
    }

    /// La dirección completa y su corta, si el registro es de `network` y su
    /// estampilla vale.
    pub fn verify(&self, network: u8) -> Result<(Address, ShortAddress), String> {
        let full = Address::decode(&self.address, network).map_err(|e| e.to_string())?;
        let short = full.short_address().map_err(|e| e.to_string())?;
        if !stamp_is_valid(&short, self.stamp) {
            return Err("the address record has no valid work stamp".into());
        }
        Ok((full, short))
    }
}
