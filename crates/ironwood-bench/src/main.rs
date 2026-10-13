//! Medición sintética del circuito de Ironwood (orchard 0.16, Halo 2) para la
//! etapa 5 de Zoka Quantic: cuánto cuesta probar un gasto en el teléfono.
//! Solo datos sintéticos; no abre wallets ni bases. Imprime JSON.
//!
//! Basado en `benches/circuit.rs` del crate. Un bundle con salidas y sin gastos
//! reales sigue llevando acciones completas (gasto ficticio + salida), así que
//! el costo de la prueba es el de un gasto real.
use std::time::Instant;

use orchard::{
    builder::{Builder, BundleType},
    bundle::BundleVersion,
    circuit::{ProvingKey, VerifyingKey},
    keys::{FullViewingKey, Scope, SpendingKey},
    value::NoteValue,
    Anchor, Bundle,
};
use rand::{rand_core::UnwrapErr, rngs::SysRng};

#[allow(non_upper_case_globals)]
const OsRng: UnwrapErr<SysRng> = UnwrapErr(SysRng);

/// Pico de memoria residente en KB (Linux/Android); en Windows no hay /proc.
fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find(|line| line.starts_with("VmHWM:"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse().ok())
}

fn millis(start: Instant) -> u128 {
    start.elapsed().as_millis()
}

fn main() {
    let iterations: usize = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(3);
    let version = BundleVersion::ironwood_v3();
    let rng = OsRng;
    let sk = SpendingKey::from_bytes([7; 32]).unwrap();
    let recipient = FullViewingKey::from(&sk).address_at(0u32, Scope::External);

    let start = Instant::now();
    let pk = ProvingKey::build(version.circuit_version());
    let proving_key_ms = millis(start);
    let start = Instant::now();
    let vk = VerifyingKey::build(version.circuit_version());
    let verifying_key_ms = millis(start);

    let mut rows = Vec::new();
    for outputs in [1usize, 2] {
        let mut prove_ms = Vec::new();
        let mut verify_ms = Vec::new();
        let mut proof_bytes = 0usize;
        let mut actions = 0usize;
        for _ in 0..iterations {
            let mut builder = Builder::new(
                BundleType::DEFAULT,
                version,
                version.default_flags(),
                Anchor::from_bytes([0; 32]).unwrap(),
            )
            .unwrap();
            for _ in 0..outputs {
                builder
                    .add_output(None, recipient, NoteValue::from_raw(10), [0; 512])
                    .unwrap();
            }
            let bundle: Bundle<_, i64> = builder.build(rng).unwrap().unwrap().0;
            actions = bundle.actions().len();
            let start = Instant::now();
            let proven = bundle.create_proof(&pk, rng).unwrap();
            prove_ms.push(millis(start));
            let signed = proven.apply_signatures(rng, [0; 32], &[]).unwrap();
            proof_bytes = signed.authorization().proof().as_ref().len();
            let start = Instant::now();
            assert!(signed.verify_proof(&vk).is_ok(), "proof must verify");
            verify_ms.push(millis(start));
        }
        rows.push(format!(
            "{{\"outputs\":{outputs},\"actions\":{actions},\"proof_bytes\":{proof_bytes},\"prove_ms\":{prove_ms:?},\"verify_ms\":{verify_ms:?}}}"
        ));
    }

    println!(
        "{{\"crate\":\"orchard 0.16.0\",\"bundle\":\"ironwood_v3\",\"target\":\"{}-{}\",\"threads\":{},\"iterations\":{iterations},\"proving_key_ms\":{proving_key_ms},\"verifying_key_ms\":{verifying_key_ms},\"peak_rss_kb\":{},\"runs\":[{}]}}",
        std::env::consts::ARCH,
        std::env::consts::OS,
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
        peak_rss_kb().map(|kb| kb.to_string()).unwrap_or_else(|| "null".to_string()),
        rows.join(",")
    );
}
