//! The soundness margin of every transfer shape, term by term: point 5 of
//! the review of 10/11/2026, without changing the protocol.
//!
//! What this prints is the estimator of Plonky3 (`p3-security`): classical
//! round-by-round soundness, proven (unique- and list-decoding regimes) and
//! conjectured, under an explicit hypothesis of 128-bit collision resistance
//! for the commitment hash. It is not a quantum analysis. `docs/MARGEN-DE-
//! SOLIDEZ.md` says what the numbers do and do not mean for an attacker with
//! a quantum computer.
//!
//! ```text
//! cargo test --release --lib transfer::margin -- --nocapture
//! cargo test --release --lib transfer::margin -- --ignored --nocapture   # costs
//! ```
use super::*;
use p3_field::coset::TwoAdicMultiplicativeCoset;
use p3_security::{
    fri::FriRegime,
    shape::{InstanceShape, StarkAirParams},
    stark::{conjectured_security_report, proven_security_report},
    RegimeReport,
};
use p3_uni_stark::{
    AirLayout, ConjecturedSecurity, OpeningShape, ProvenSecurity, StarkSecurityParams,
};

/// The hash hypothesis the project uses (see `parameter_soundness_estimates…`).
const COLLISION_BITS: usize = 128;
/// `log2(p^8)` for KoalaBear's degree-8 extension, floored.
const CHALLENGE_BITS: usize = 247;

/// A FRI configuration: the current one and the candidates to compare.
#[derive(Clone, Copy)]
struct Candidate {
    name: &'static str,
    log_blowup: usize,
    queries: usize,
    query_pow: usize,
    /// The shortest trace a proof may have. Hiding FRI needs
    /// `2 · (8 · 2 + queries)` mask values (`check_hiding_budget` of
    /// `p3-fri`): 512 rows allow at most 240 queries, so more queries mean a
    /// taller trace for every payment, which is a format change.
    rows: usize,
}
const CURRENT: Candidate = Candidate {
    name: "current: blowup 8, 160 queries",
    log_blowup: LOG_BLOWUP,
    queries: QUERIES,
    query_pow: 0,
    rows: 512,
};
const CANDIDATES: [Candidate; 6] = [
    CURRENT,
    Candidate {
        name: "blowup 8, 160 queries, 16-bit grinding",
        log_blowup: 3,
        queries: 160,
        query_pow: 16,
        rows: 512,
    },
    Candidate {
        name: "blowup 8, 256 queries, 1024-row traces",
        log_blowup: 3,
        queries: 256,
        query_pow: 0,
        rows: 1024,
    },
    Candidate {
        name: "blowup 8, 320 queries, 1024-row traces",
        log_blowup: 3,
        queries: 320,
        query_pow: 0,
        rows: 1024,
    },
    Candidate {
        name: "blowup 16, 128 queries",
        log_blowup: 4,
        queries: 128,
        query_pow: 0,
        rows: 512,
    },
    Candidate {
        name: "blowup 16, 160 queries, 16-bit grinding",
        log_blowup: 4,
        queries: 160,
        query_pow: 16,
        rows: 512,
    },
];

/// The circuit of a shape, at least as tall as `candidate` needs.
fn circuit(n: usize, m: usize, candidate: Candidate) -> TransferAir {
    let mut air = TransferAir::new(n, m);
    air.program.rows = air.program.rows.max(candidate.rows);
    air
}

#[test]
fn more_than_240_queries_do_not_fit_a_512_row_trace() {
    let budget = |queries: usize| 2 * (8 * 2 + queries);
    assert!(budget(QUERIES) <= 512, "the current configuration fits");
    assert!(budget(240) <= 512 && budget(241) > 512);
    for candidate in CANDIDATES {
        assert!(
            budget(candidate.queries) <= candidate.rows,
            "{}",
            candidate.name
        );
    }
}

fn fri(candidate: Candidate) -> FriParameters<()> {
    FriParameters {
        log_blowup: candidate.log_blowup,
        log_final_poly_len: 0,
        max_log_arity: 1,
        num_queries: candidate.queries,
        batch_proof_of_work_bits: 0,
        commit_proof_of_work_bits: 0,
        query_proof_of_work_bits: candidate.query_pow,
        mmcs: (),
    }
}

/// What `p3-uni-stark` derives for a shape, and the trace height it commits
/// (hiding doubles it).
fn params(n: usize, m: usize, candidate: Candidate) -> (StarkSecurityParams, usize) {
    let air = circuit(n, m, candidate);
    let fri = fri(candidate);
    let params = StarkSecurityParams::from_air::<F, Challenge, _>(
        fri.security_regime(),
        &air,
        AirLayout::from_air::<F>(&air),
        TwoAdicMultiplicativeCoset::new(F::ONE, air.program.rows.ilog2() as usize).unwrap(),
        CHALLENGE_BITS,
        COLLISION_BITS,
        2,
        OpeningShape::hiding(RANDOM_CODEWORDS),
        fri.grinding_sites(),
    );
    (params, air.program.rows.ilog2() as usize + 1)
}

/// The same reports `p3-uni-stark` composes, kept whole so every term shows.
struct Margin {
    unique: RegimeReport,
    list: Option<RegimeReport>,
    conjectured: RegimeReport,
}
fn margin(n: usize, m: usize, candidate: Candidate) -> Margin {
    let (params, degree_bits) = params(n, m, candidate);
    let regime = FriRegime {
        log_blowup: params.fri_log_blowup,
        num_queries: params.fri_num_queries,
        log_final_poly_len: params.fri_log_final_poly_len,
        max_log_arity: params.fri_max_log_arity,
        commit_pow_bits: params.fri_commit_proof_of_work_bits,
        query_pow_bits: params.fri_query_proof_of_work_bits,
    };
    let air = StarkAirParams {
        num_constraints: params.num_constraints,
        max_constraint_degree: params.air_max_constraint_degree,
        num_quotient_chunks: params.num_quotient_chunks,
        max_combo: params.max_combo,
    };
    let shape = InstanceShape {
        log_trace_length: degree_bits,
        modulus_bits: params.num_modulus_bits,
        collision_resistance: params.collision_resistance,
        num_batched_functions: params.num_batched_functions,
    };
    let proven = proven_security_report(&regime, &air, &shape, &[], &params.grinding);
    let conjectured = conjectured_security_report(&regime, &air, &shape, &[], &params.grinding);
    // Rebuilt from public fields: it has to be what Plonky3 itself reports.
    let reference = ProvenSecurity::compute_from_proof(degree_bits, &params);
    assert_eq!(
        proven.udr.security_bits() as usize,
        reference.unique_decoding_bits
    );
    assert_eq!(
        proven
            .ldr
            .as_ref()
            .map_or(0, |r| r.security_bits() as usize),
        reference.list_decoding_bits
    );
    assert_eq!(
        conjectured.security_bits() as usize,
        ConjecturedSecurity::compute_from_params(&params, degree_bits).security_bits
    );
    Margin {
        unique: proven.udr,
        list: proven.ldr,
        conjectured,
    }
}

fn terms(report: &RegimeReport) -> String {
    report
        .terms()
        .iter()
        .map(|term| format!("{} {:.1}", term.label, term.bits.bits()))
        .collect::<Vec<_>>()
        .join(", ")
}
fn best(margin: &Margin) -> f64 {
    let list = margin
        .list
        .as_ref()
        .map_or(0.0, RegimeReport::security_bits);
    margin.unique.security_bits().max(list)
}

#[test]
fn the_margin_of_every_shape_term_by_term() {
    let shapes: Vec<(usize, usize)> = (1..=MAX_INPUTS)
        .flat_map(|n| (1..=MAX_OUTPUTS).map(move |m| (n, m)))
        .collect();
    for candidate in CANDIDATES {
        let margins: Vec<_> = shapes
            .iter()
            .map(|&(n, m)| ((n, m), margin(n, m, candidate)))
            .collect();
        let (weakest, low) = margins
            .iter()
            .min_by(|a, b| best(&a.1).partial_cmp(&best(&b.1)).unwrap())
            .unwrap();
        eprintln!("\n## {}", candidate.name);
        eprintln!(
            "weakest shape {}x{}: proven {:.1} bits, conjectured {:.1}",
            weakest.0,
            weakest.1,
            best(low),
            low.conjectured.security_bits()
        );
        for &(shape, ref margin) in &margins {
            if [(1, 2), (2, 2), (8, 8)].contains(&shape) || shape == *weakest {
                eprintln!(
                    "{}x{} unique decoding: {}",
                    shape.0,
                    shape.1,
                    terms(&margin.unique)
                );
                if let Some(list) = &margin.list {
                    eprintln!("{}x{} list decoding: {}", shape.0, shape.1, terms(list));
                }
                eprintln!(
                    "{}x{} conjectured: {}",
                    shape.0,
                    shape.1,
                    terms(&margin.conjectured)
                );
            }
        }
        if candidate.name == CURRENT.name {
            // What `parameter_soundness_estimates…` already requires.
            assert!(best(low) >= 100.0, "{} bits", best(low));
        }
    }
}

/// The FRI parameters of `candidate`, with the hiding of the real prover.
fn config_for(candidate: Candidate) -> Config {
    let perm = default_koalabear_poseidon2_32();
    let rng = || StdRng::try_from_rng(&mut rand::rngs::SysRng).unwrap();
    let mmcs = ValMmcs::new(
        Hash::new(perm.clone()),
        Compress::new(perm.clone()),
        0,
        rng(),
    );
    let fri = FriParameters {
        log_blowup: candidate.log_blowup,
        log_final_poly_len: 0,
        max_log_arity: 1,
        num_queries: candidate.queries,
        batch_proof_of_work_bits: 0,
        commit_proof_of_work_bits: 0,
        query_proof_of_work_bits: candidate.query_pow,
        mmcs: ExtMmcs::new(mmcs.clone()),
    };
    let pcs = Pcs::new(
        Radix2DitParallel::default(),
        mmcs,
        fri,
        RANDOM_CODEWORDS,
        rng(),
    );
    Config::new(pcs, DuplexChallenger::new(perm))
}

/// What each candidate costs: a 1→2 payment's proof, its size and times.
/// Slow (one proof per candidate): run it by name.
#[test]
#[ignore = "proves a payment per candidate; run with --ignored"]
fn what_each_candidate_costs() {
    let (statement, witness) = crate::fuzzing::fixture();
    let public = statement.public_values().unwrap();
    eprintln!("\n| configuration | proven bits (1x2) | proof bytes | prove | verify |");
    eprintln!("| --- | ---: | ---: | ---: | ---: |");
    for candidate in CANDIDATES {
        let air = circuit(1, 2, candidate);
        let config = config_for(candidate);
        let trace = air.program.trace(&statement, &witness).unwrap();
        let started = std::time::Instant::now();
        let proof = prove(&config, &air, trace, &public).unwrap();
        let proving = started.elapsed();
        let bytes = postcard::to_allocvec(&proof).unwrap().len();
        let started = std::time::Instant::now();
        verify(&config_for(candidate), &air, &proof, &public).unwrap();
        let verifying = started.elapsed();
        eprintln!(
            "| {} | {:.1} | {} | {:.2} s | {:.0} ms |",
            candidate.name,
            best(&margin(1, 2, candidate)),
            bytes,
            proving.as_secs_f64(),
            verifying.as_secs_f64() * 1000.0
        );
    }
}
