//! Synthetic device benchmark; does not open chain or wallet files.
use serde::Serialize;
use std::{hint::black_box, time::Instant};
use zoka_pq::{MlDsa65SecretKey, MlKem768SecretKey, XWingSecretKey, X_WING_PROFILE};

#[derive(Serialize)]
struct Measurement {
    operation: &'static str,
    iterations: usize,
    mean_us: f64,
    median_us: f64,
    p95_us: f64,
    min_us: f64,
    max_us: f64,
}

fn measure<T>(
    operation: &'static str,
    iterations: usize,
    mut action: impl FnMut() -> T,
) -> Measurement {
    for _ in 0..5 {
        black_box(action());
    }
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        black_box(action());
        samples.push(start.elapsed().as_secs_f64() * 1_000_000.0);
    }
    samples.sort_by(f64::total_cmp);
    Measurement {
        operation,
        iterations,
        mean_us: samples.iter().sum::<f64>() / iterations as f64,
        median_us: (samples[(iterations - 1) / 2] + samples[iterations / 2]) / 2.0,
        p95_us: samples[(iterations * 95).div_ceil(100) - 1],
        min_us: samples[0],
        max_us: samples[iterations - 1],
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let iterations = match args.as_slice() {
        [] => 100,
        [flag, value] if flag == "--iterations" => value.parse::<usize>()?,
        _ => return Err("usage: pq-bench [--iterations 1..100000]".into()),
    };
    if !(1..=100_000).contains(&iterations) {
        return Err("iterations must be in 1..100000".into());
    }
    let kem = MlKem768SecretKey::generate()?;
    let kem_seed = kem.export_seed();
    let kem_public = kem.public_key();
    let (kem_ct, kem_shared) = kem_public.encapsulate();
    assert_eq!(
        kem_shared.expose_secret(),
        kem.decapsulate(&kem_ct).expose_secret()
    );
    let xwing = XWingSecretKey::generate()?;
    let xwing_seed = xwing.export_seed();
    let xwing_public = xwing.public_key();
    let (xwing_ct, xwing_shared) = xwing_public.encapsulate();
    assert_eq!(
        xwing_shared.expose_secret(),
        xwing.decapsulate(&xwing_ct)?.expose_secret()
    );
    let dsa = MlDsa65SecretKey::generate()?;
    let dsa_seed = dsa.export_seed();
    let dsa_public = dsa.public_key();
    let message = [0x5au8; 64];
    let context = b"zoka-pq-benchmark-v1";
    let signature = dsa.sign(&message, context)?;
    dsa_public.verify(&message, context, &signature)?;

    let operations = vec![
        measure("mlkem768_keygen_os_rng", iterations, || {
            MlKem768SecretKey::generate().unwrap()
        }),
        measure("mlkem768_expand_seed", iterations, || {
            MlKem768SecretKey::from_seed(black_box(kem_seed.expose_secret())).unwrap()
        }),
        measure("mlkem768_encapsulate", iterations, || {
            kem_public.encapsulate()
        }),
        measure("mlkem768_decapsulate_expanded_key", iterations, || {
            kem.decapsulate(black_box(&kem_ct))
        }),
        measure(
            "xwing_import_seed_and_expand_public_key",
            iterations,
            || XWingSecretKey::from_seed(black_box(xwing_seed.expose_secret())).unwrap(),
        ),
        measure("xwing_copy_cached_public_key", iterations, || {
            xwing.public_key()
        }),
        measure("xwing_encapsulate", iterations, || {
            xwing_public.encapsulate()
        }),
        measure("xwing_decapsulate_including_expansion", iterations, || {
            xwing.decapsulate(black_box(&xwing_ct)).unwrap()
        }),
        measure("mldsa65_keygen_os_rng", iterations, || {
            MlDsa65SecretKey::generate().unwrap()
        }),
        measure("mldsa65_expand_seed", iterations, || {
            MlDsa65SecretKey::from_seed(black_box(dsa_seed.expose_secret())).unwrap()
        }),
        measure("mldsa65_sign_64_bytes", iterations, || {
            dsa.sign(black_box(&message), context).unwrap()
        }),
        measure("mldsa65_verify_64_bytes", iterations, || {
            dsa_public
                .verify(black_box(&message), context, &signature)
                .unwrap()
        }),
    ];
    let report = serde_json::json!({
        "schema_version": 1,
        "architecture": std::env::consts::ARCH,
        "os": std::env::consts::OS,
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "iterations": iterations,
        "warmup_iterations": 5,
        "xwing_profile": X_WING_PROFILE,
        "notes": "Synthetic repeated inputs. Timings include returned-value drop/zeroization. ML-KEM decapsulation reuses an expanded key; X-Wing re-expands internally. Not a full wallet scan or memory benchmark.",
        "operations": operations,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
