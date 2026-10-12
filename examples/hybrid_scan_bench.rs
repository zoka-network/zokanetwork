//! Costo de escanear salidas v1 (ECDH Ristretto) y v2 (X-Wing) con las
//! funciones del nodo, para cerrar D1/D2 con números del teléfono.
//!
//!     cargo run --release --no-default-features --features pq-experimental \
//!         --example hybrid_scan_bench -- --iterations 100
//!
//! Solo datos sintéticos: no lee wallets, cadena ni red. Imprime JSON.

use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT;
use curve25519_dalek::ristretto::CompressedRistretto;
use curve25519_dalek::scalar::Scalar;
use sha2::{Digest, Sha512};
use std::time::Instant;
use zoka::private_wallet::hybrid::{hybrid_output_material, HybridAddress, HybridWalletKeys};

fn hash_to_scalar(parts: &[&[u8]]) -> Scalar {
    let mut hasher = Sha512::new();
    for part in parts {
        hasher.update(part);
        hasher.update([0x1f]);
    }
    let mut wide = [0u8; 64];
    wide.copy_from_slice(&hasher.finalize());
    Scalar::from_bytes_mod_order_wide(&wide)
}

fn stats(name: &str, mut samples: Vec<f64>) -> String {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = samples.len();
    let mean = samples.iter().sum::<f64>() / n as f64;
    let median = if n % 2 == 1 {
        samples[n / 2]
    } else {
        (samples[n / 2 - 1] + samples[n / 2]) / 2.0
    };
    let p95 = samples[((n as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(n - 1)];
    format!(
        "{{\"operation\":\"{name}\",\"iterations\":{n},\"mean_us\":{mean:.2},\"median_us\":{median:.2},\"p95_us\":{p95:.2},\"min_us\":{:.2},\"max_us\":{:.2}}}",
        samples[0],
        samples[n - 1]
    )
}

fn time_us<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let start = Instant::now();
    let value = f();
    (value, start.elapsed().as_secs_f64() * 1e6)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let iterations = args
        .iter()
        .position(|a| a == "--iterations")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100)
        .max(5);

    let me = HybridWalletKeys::from_root_seed(&[7u8; 32]).expect("keys");
    let stranger = HybridWalletKeys::from_root_seed(&[8u8; 32]).expect("keys");
    let my_address = me.address();
    let encoded = my_address.encode();

    let mut results = Vec::new();

    // Dirección v2: codificar y decodificar (validación completa).
    let mut encode = Vec::new();
    let mut decode = Vec::new();
    for _ in 0..iterations {
        encode.push(time_us(|| my_address.encode()).1);
        decode.push(time_us(|| HybridAddress::decode(&encoded).expect("decode")).1);
    }
    results.push(stats("v2_address_encode", encode));
    results.push(stats("v2_address_decode_and_validate", decode));

    // Salidas v2: crear (lo que hace quien envía) y escanear propias y ajenas.
    let mut build = Vec::new();
    let mut outputs = Vec::new();
    for i in 0..iterations {
        let commitment = format!("commitment-{i}");
        let (material, us) = time_us(|| hybrid_output_material(&my_address, &commitment));
        build.push(us);
        outputs.push((commitment, material));
    }
    results.push(stats("v2_output_build", build));

    let mut own = Vec::new();
    let mut foreign = Vec::new();
    for (commitment, material) in &outputs {
        let (found, us) = time_us(|| {
            me.scan(
                &material.kem_ciphertext,
                commitment,
                &material.one_time_public_key,
            )
        });
        assert!(found.is_some(), "own output not recognized");
        own.push(us);
        let (found, us) = time_us(|| {
            stranger.scan(
                &material.kem_ciphertext,
                commitment,
                &material.one_time_public_key,
            )
        });
        assert!(found.is_none(), "foreign output recognized");
        foreign.push(us);
    }
    results.push(stats("v2_scan_own_output", own));
    results.push(stats("v2_scan_foreign_output", foreign));

    // Equivalente v1 de lo que hace hoy la wallet por salida: s·R, derivar el
    // desplazamiento, B + k·G y comparar.
    let scan_secret = hash_to_scalar(&[b"bench-scan"]);
    let spend_public = hash_to_scalar(&[b"bench-spend"]) * RISTRETTO_BASEPOINT_POINT;
    let scan_public = scan_secret * RISTRETTO_BASEPOINT_POINT;
    let mut v1 = Vec::new();
    for i in 0..iterations {
        let r = hash_to_scalar(&[b"bench-ephemeral", &i.to_le_bytes()]);
        let ephemeral = (r * RISTRETTO_BASEPOINT_POINT).compress().to_bytes();
        let commitment = format!("commitment-{i}");
        let expected = {
            let shared = r * scan_public;
            let k = hash_to_scalar(&[
                b"zoka-output-key",
                shared.compress().as_bytes(),
                commitment.as_bytes(),
            ]);
            (spend_public + k * RISTRETTO_BASEPOINT_POINT).compress()
        };
        let (matched, us) = time_us(|| {
            let point = CompressedRistretto(ephemeral).decompress().expect("point");
            let shared = scan_secret * point;
            let k = hash_to_scalar(&[
                b"zoka-output-key",
                shared.compress().as_bytes(),
                commitment.as_bytes(),
            ]);
            (spend_public + k * RISTRETTO_BASEPOINT_POINT).compress() == expected
        });
        assert!(matched);
        v1.push(us);
    }
    results.push(stats("v1_scan_output_equivalent", v1));

    println!(
        "{{\"schema_version\":1,\"tool\":\"hybrid_scan_bench\",\"os\":\"{}\",\"architecture\":\"{}\",\"iterations\":{iterations},\"address_chars\":{},\"kem_ciphertext_bytes\":{},\"operations\":[{}]}}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        encoded.len(),
        outputs[0].1.kem_ciphertext.len(),
        results.join(",")
    );
}
