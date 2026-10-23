//! Dificultad de Zoka Quantic (etapa 7.2, desde el génesis).
//!
//! La de Zenith cuenta bits de ceros (potencias de 2), ajusta ±1 bit mirando
//! solo el último bloque y elige la cadena por la **suma de los bits**, no por
//! el trabajo: muchos bloques baratos pesan más que pocos caros. Quantic usa
//! lo estándar:
//!
//! - **Objetivo de 256 bits**: un bloque vale si su hash, como número big
//!   endian, no supera el objetivo. Se guarda en el formato compacto de 32
//!   bits de Bitcoin (`nBits`) donde el nodo guardaba los "bits".
//! - **ASERT** (el `aserti3-2d` de Bitcoin Cash): el objetivo de cada bloque
//!   sale del génesis y del adelanto o atraso acumulado de la cadena,
//!   `objetivo = objetivo_génesis · 2^((t_padre − t_génesis − 120 · (h_padre − h_génesis)) / τ)`,
//!   con aritmética entera exacta. Con τ = 1 hora: si la red mina el doble de
//!   rápido, en una hora la dificultad se duplica; si nadie mina, baja sola.
//! - **Trabajo** de un bloque = 2^256 / (objetivo + 1): la cadena que se elige
//!   es la de más cómputo esperado.

use num_bigint::BigUint;

use crate::economics;

/// Tiempo de bloque buscado, el mismo de Zenith.
pub const IDEAL_BLOCK_SECS: i64 = economics::TARGET_BLOCK_TIME_SECS as i64;
/// Vida media del ajuste: cuánto atraso o adelanto acumulado duplica o divide
/// a la mitad la dificultad.
pub const HALFLIFE_SECS: i64 = 3600;
/// Dificultad mínima de las redes públicas: 12 bits, el piso de Zenith.
pub const MIN_DIFFICULTY_BITS: u32 = 12;
/// Dificultad del génesis y de los primeros bloques de la testnet. La PC sola
/// mina Zenith a 16–18 bits; ASERT la lleva a 120 s en pocas horas.
pub const TESTNET_INITIAL_DIFFICULTY_BITS: u32 = 14;
/// La de la mainnet: un poco por encima, porque arranca con más mineros que la
/// PC sola. Si es poca, ASERT la sube en horas; si es mucha, la baja.
pub const MAINNET_INITIAL_DIFFICULTY_BITS: u32 = 16;

/// La dificultad del génesis y de sus primeros hijos en cada red pública.
pub fn initial_difficulty_bits(network: zoka_quantic::Network) -> u32 {
    match network {
        zoka_quantic::Network::Mainnet => MAINNET_INITIAL_DIFFICULTY_BITS,
        zoka_quantic::Network::Testnet => TESTNET_INITIAL_DIFFICULTY_BITS,
        zoka_quantic::Network::Regtest => economics::QUANTIC_REGTEST_DIFFICULTY_BITS,
    }
}

/// Los bits de ceros que pide `target`, redondeando para abajo: la cifra de
/// "dificultad" que muestran las wallets, como la de Zenith.
pub fn bits_of_target(target: &BigUint) -> u32 {
    256u32.saturating_sub(target.bits() as u32)
}

/// Hashes por segundo que hacen falta para un bloque cada 120 s con ese objetivo.
pub fn network_hashrate_for_target(target: &BigUint) -> u64 {
    u64::try_from(work_for_target(target) / IDEAL_BLOCK_SECS as u128).unwrap_or(u64::MAX)
}

/// El objetivo de `bits` bits de ceros: 2^(256 − bits) − 1.
pub fn target_for_bits(bits: u32) -> BigUint {
    (BigUint::from(1u32) << (256 - bits.min(255) as usize)) - 1u32
}

/// El objetivo más fácil que admite la red.
pub fn max_target() -> BigUint {
    target_for_bits(MIN_DIFFICULTY_BITS)
}

/// Formato compacto de Bitcoin: 1 byte de exponente (largo en bytes) y 3 de
/// mantisa. Redondea hacia abajo (un objetivo un poco más difícil).
pub fn to_compact(target: &BigUint) -> u32 {
    let bytes = target.to_bytes_be();
    let bytes: &[u8] = if target.bits() == 0 { &[] } else { &bytes };
    let mut size = bytes.len() as u32;
    let mut mantissa: u32 = if size <= 3 {
        let mut value = 0u32;
        for byte in bytes {
            value = (value << 8) | u32::from(*byte);
        }
        value << (8 * (3 - size))
    } else {
        (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2])
    };
    // El bit alto de la mantisa es de signo en Bitcoin: correr un byte.
    if mantissa & 0x0080_0000 != 0 {
        mantissa >>= 8;
        size += 1;
    }
    (size << 24) | mantissa
}

pub fn from_compact(compact: u32) -> BigUint {
    let size = compact >> 24;
    let mantissa = BigUint::from(compact & 0x007f_ffff);
    if size <= 3 {
        mantissa >> (8 * (3 - size) as usize)
    } else {
        mantissa << (8 * (size - 3) as usize)
    }
}

/// Si `hash` cumple el objetivo, leído como un número **little endian**, igual
/// que Monero: así el filtro de XMRig (los 8 bytes de arriba del hash contra
/// el objetivo del pool) coincide con la regla de la red.
pub fn hash_meets_target(hash: &[u8], target: &BigUint) -> bool {
    hash.len() == 32 && BigUint::from_bytes_le(hash) <= *target
}

/// Bits en cero de arriba de `hash` leído little endian (para mostrar cuánto
/// trabajo tiene un hash; la regla es [`hash_meets_target`]).
pub fn hash_zero_bits(hash: &[u8]) -> u32 {
    let mut bits = 0;
    for byte in hash.iter().rev() {
        if *byte == 0 {
            bits += 8;
        } else {
            return bits + byte.leading_zeros();
        }
    }
    bits
}

/// Trabajo esperado de un bloque con ese objetivo: 2^256 / (objetivo + 1),
/// saturado a `u128` (alcanza para objetivos de hasta 128 bits de ceros).
pub fn work_for_target(target: &BigUint) -> u128 {
    let work = (BigUint::from(1u32) << 256usize) / (target + 1u32);
    u128::try_from(&work).unwrap_or(u128::MAX)
}

/// El objetivo de un bloque cuyo padre está `height_delta` bloques después del
/// génesis y tiene `time_delta` segundos más que él. `aserti3-2d` de Bitcoin
/// Cash, con el génesis como ancla.
pub fn asert_target(anchor_target: &BigUint, time_delta: i64, height_delta: i64) -> BigUint {
    // Exponente en punto fijo 16.16.
    let exponent: i128 =
        ((i128::from(time_delta) - i128::from(IDEAL_BLOCK_SECS) * i128::from(height_delta)) * 65_536)
            / i128::from(HALFLIFE_SECS);
    let shifts = exponent >> 16;
    let fraction = (exponent - (shifts << 16)) as u128;
    debug_assert!(fraction < 65_536);
    // 2^(fraction/65536) · 65536, con la aproximación cúbica de la especificación.
    let factor = ((195_766_423_245_049u128 * fraction
        + 971_821_376u128 * fraction * fraction
        + 5_127u128 * fraction * fraction * fraction
        + (1u128 << 47))
        >> 48)
        + 65_536;
    let mut target = anchor_target * BigUint::from(factor);
    if shifts < 0 {
        target >>= (-shifts) as usize;
    } else {
        target <<= shifts as usize;
    }
    target >>= 16usize;
    let max = max_target();
    if target.bits() == 0 {
        BigUint::from(1u32)
    } else if target > max {
        max
    } else {
        target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_targets_round_trip_like_bitcoin() {
        // El objetivo de dificultad 1 de Bitcoin.
        let bitcoin_one = from_compact(0x1d00_ffff);
        assert_eq!(bitcoin_one, BigUint::from(0xffffu32) << 208usize);
        assert_eq!(to_compact(&bitcoin_one), 0x1d00_ffff);
        for bits in [8, 12, 14, 20, 32, 64] {
            let target = target_for_bits(bits);
            let compact = to_compact(&target);
            let back = from_compact(compact);
            assert!(back <= target, "rounds down at {bits} bits");
            assert!(back > target_for_bits(bits + 1), "stays within one bit at {bits}");
            assert_eq!(to_compact(&back), compact);
        }
    }

    // Como Monero: el hash es un número little endian, así que los ceros que
    // cuentan son los del final (lo que mira XMRig: los bytes 24..32).
    #[test]
    fn a_hash_meets_a_target_as_a_little_endian_number() {
        let target = target_for_bits(12);
        let mut hash = [0xffu8; 32];
        hash[31] = 0x00;
        hash[30] = 0x0f; // 12 bits de ceros arriba
        assert!(hash_meets_target(&hash, &target));
        assert_eq!(hash_zero_bits(&hash), 12);
        hash[30] = 0x10; // 11 bits de ceros
        assert!(!hash_meets_target(&hash, &target));
        assert_eq!(hash_zero_bits(&hash), 11);
        // Los ceros al principio no cuentan.
        let mut leading = [0u8; 32];
        leading[31] = 0xff;
        assert!(!hash_meets_target(&leading, &target));
        assert!(!hash_meets_target(&[0u8; 31], &target));
    }

    #[test]
    fn the_displayed_bits_survive_the_compact_form() {
        for bits in [12, 14, 16, 20] {
            let compact = to_compact(&target_for_bits(bits));
            assert_eq!(bits_of_target(&from_compact(compact)), bits);
        }
        // 14 bits: 16.384 hashes por bloque, 136 H/s para uno cada 120 s.
        assert_eq!(network_hashrate_for_target(&target_for_bits(14)), 136);
    }

    #[test]
    fn work_doubles_with_each_bit() {
        let w12 = work_for_target(&target_for_bits(12));
        let w13 = work_for_target(&target_for_bits(13));
        assert_eq!(w12, 4096);
        assert_eq!(w13, 2 * w12);
        // Lo que estaba mal en Zenith: diez bloques de 12 bits no pesan más que
        // seis de 17.
        assert!(10 * w12 < 6 * work_for_target(&target_for_bits(17)));
    }

    #[test]
    fn asert_keeps_the_target_on_schedule_and_moves_it_otherwise() {
        let anchor = target_for_bits(16);
        // En horario: el mismo objetivo.
        assert_eq!(asert_target(&anchor, 120 * 100, 100), anchor);
        // Una vida media de adelanto: el doble de difícil (la mitad del objetivo).
        let ahead = asert_target(&anchor, 120 * 100 - HALFLIFE_SECS, 100);
        let half = &anchor >> 1usize;
        assert!(ahead <= &half + 1u32 && ahead >= &half - (&half >> 16usize));
        // Una vida media de atraso: la mitad de difícil.
        let behind = asert_target(&anchor, 120 * 100 + HALFLIFE_SECS, 100);
        let double = &anchor << 1usize;
        assert!(behind <= &double + 2u32 && behind >= &double - (&double >> 16usize));
        // Muy atrasada (nadie mina hace días): el piso de 12 bits, no más fácil.
        assert_eq!(asert_target(&anchor, 86_400 * 3, 1), max_target());
        // Monótono: más adelanto, objetivo menor.
        let mut previous = asert_target(&anchor, 0, 1);
        for seconds in (0..7200).step_by(97) {
            let next = asert_target(&anchor, -seconds, 1);
            assert!(next <= previous);
            previous = next;
        }
    }
}
