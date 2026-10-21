//! Prototipo del pool postcuántico (docs/QUANTIC-POOL-POSTCUANTICO.md): cuánto
//! pesa y cuánto tarda una prueba STARK **con conocimiento cero** del tamaño de
//! una transferencia, con lo que se usaría de verdad: KoalaBear (31 bits) y su
//! extensión de grado 4, Poseidon2 para todo (traza, árbol de compromisos de
//! FRI y transcript) y FRI con ocultamiento.
//!
//! Una transferencia de 1 entrada y 2 salidas necesita del orden de 40
//! permutaciones de Poseidon2: 32 del camino de Merkle, más el compromiso de la
//! nota, el nullifier, la clave y las dos salidas. Se prueban 64 y se mira cómo
//! crece con más. No es el circuito del pool —solo las permutaciones, sin las
//! restricciones que las unen—, así que da una cota de abajo.
//!
//! Uso: `zoka-pq-pool [permutaciones...]` (por omisión 64 128 1024).

use std::time::Instant;

use p3_challenger::DuplexChallenger;
use p3_commit::ExtensionMmcs;
use p3_dft::Radix2DitParallel;
use p3_field::coset::TwoAdicMultiplicativeCoset;
use p3_field::extension::BinomialExtensionField;
use p3_field::{Field, PrimeCharacteristicRing};
use p3_fri::{FriParameters, HidingFriPcs};
use p3_koala_bear::{
    GenericPoseidon2LinearLayersKoalaBear, KoalaBear, Poseidon2KoalaBear, KOALABEAR_POSEIDON2_HALF_FULL_ROUNDS,
    KOALABEAR_POSEIDON2_PARTIAL_ROUNDS_16, KOALABEAR_S_BOX_DEGREE,
};
use p3_merkle_tree::MerkleTreeHidingMmcs;
use p3_poseidon2_air::{Poseidon2Air, RoundConstants};
use p3_symmetric::{PaddingFreeSponge, TruncatedPermutation};
use p3_uni_stark::{prove, verify, AirLayout, OpeningShape, ProvenSecurity, StarkConfig, StarkSecurityParams};
use rand::rngs::StdRng;
use rand::SeedableRng;

type Val = KoalaBear;
type Challenge = BinomialExtensionField<Val, 4>;
type Perm = Poseidon2KoalaBear<16>;
type Hash = PaddingFreeSponge<Perm, 16, 8, 8>;
type Compress = TruncatedPermutation<Perm, 2, 8, 16>;
type ValMmcs = MerkleTreeHidingMmcs<
    <Val as Field>::Packing,
    <Val as Field>::Packing,
    Hash,
    Compress,
    StdRng,
    2,
    8,
    4,
>;
type ChallengeMmcs = ExtensionMmcs<Val, Challenge, ValMmcs>;
type Challenger = DuplexChallenger<Val, Perm, 16, 8>;
type Dft = Radix2DitParallel<Val>;
type Pcs = HidingFriPcs<Val, Dft, ValMmcs, ChallengeMmcs, StdRng>;
type Config = StarkConfig<Pcs, Challenge, Challenger>;

const WIDTH: usize = 16;
const SBOX_DEGREE: u64 = KOALABEAR_S_BOX_DEGREE;
const SBOX_REGISTERS: usize = 0;
const HALF_FULL_ROUNDS: usize = KOALABEAR_POSEIDON2_HALF_FULL_ROUNDS;
const PARTIAL_ROUNDS: usize = KOALABEAR_POSEIDON2_PARTIAL_ROUNDS_16;
type Air = Poseidon2Air<
    Val,
    GenericPoseidon2LinearLayersKoalaBear,
    WIDTH,
    SBOX_DEGREE,
    SBOX_REGISTERS,
    HALF_FULL_ROUNDS,
    PARTIAL_ROUNDS,
>;
/// Columnas aleatorias del ocultamiento (las del ejemplo de Plonky3).
const RANDOM_CODEWORDS: usize = 4;

struct Measure {
    proof_bytes: usize,
    prove_ms: f64,
    verify_ms: f64,
    proven_bits: usize,
}

fn fri(mmcs: ChallengeMmcs, num_queries: usize) -> FriParameters<ChallengeMmcs> {
    FriParameters {
        num_queries,
        ..FriParameters::new_benchmark_zk(mmcs)
    }
}

fn measure(permutations: usize, num_queries: usize) -> Result<Measure, String> {
    let mut rng = StdRng::seed_from_u64(2026);
    let perm = Perm::new_from_rng_128(&mut rng);
    let mmcs = ValMmcs::new(Hash::new(perm.clone()), Compress::new(perm.clone()), 0, StdRng::seed_from_u64(1));
    let air = Air::new(RoundConstants::from_rng(&mut rng));
    let fri_params = fri(ChallengeMmcs::new(mmcs.clone()), num_queries);
    let log_blowup = fri_params.log_blowup;
    let security = StarkSecurityParams::from_air::<Val, Challenge, _>(
        fri_params.security_regime(),
        &air,
        AirLayout::from_air::<Val>(&air),
        TwoAdicMultiplicativeCoset::new(Val::ONE, permutations.trailing_zeros() as usize).ok_or("trace domain")?,
        124,
        124,
        1,
        OpeningShape::hiding(RANDOM_CODEWORDS),
        fri_params.grinding_sites(),
    );
    let pcs = Pcs::new(Dft::default(), mmcs, fri_params, RANDOM_CODEWORDS, StdRng::seed_from_u64(2));
    let config = Config::new(pcs, Challenger::new(perm));
    let trace = air.generate_random_trace_rows(permutations, log_blowup);

    let started = Instant::now();
    let proof = prove(&config, &air, trace, &[]).map_err(|e| format!("{e:?}"))?;
    let prove_ms = started.elapsed().as_secs_f64() * 1000.0;
    let proof_bytes = postcard::to_allocvec(&proof).map_err(|e| e.to_string())?.len();
    let started = Instant::now();
    verify(&config, &air, &proof, &[]).map_err(|e| format!("{e:?}"))?;
    let verify_ms = started.elapsed().as_secs_f64() * 1000.0;
    let proven_bits = ProvenSecurity::compute_from_proof(proof.degree_bits, &security).security_bits();
    Ok(Measure { proof_bytes, prove_ms, verify_ms, proven_bits })
}

fn main() {
    let sizes: Vec<usize> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let sizes = if sizes.is_empty() { vec![64, 128, 1024] } else { sizes };
    println!("KoalaBear^4, Poseidon2, FRI con ocultamiento, blowup 4, 16 bits de grinding en las consultas");
    println!("{:>8} {:>9} {:>11} {:>10} {:>10} {:>14}", "perms", "consultas", "prueba KB", "probar ms", "verif ms", "seg. probada");
    for permutations in sizes {
        let permutations = permutations.next_power_of_two();
        for num_queries in [100, 56] {
            match measure(permutations, num_queries) {
                Ok(m) => println!(
                    "{:>8} {:>9} {:>11.1} {:>10.1} {:>10.1} {:>11} bits",
                    permutations,
                    num_queries,
                    m.proof_bytes as f64 / 1024.0,
                    m.prove_ms,
                    m.verify_ms,
                    m.proven_bits
                ),
                Err(error) => println!("{permutations:>8} {num_queries:>9} error: {error}"),
            }
        }
    }
}
