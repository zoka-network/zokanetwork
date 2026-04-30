//! Generate the Groth16 trusted-setup bundle and write it to disk.
//!
//! Usage:
//! ```text
//! cargo run --release --bin zoka-trusted-setup -- --out ./trusted-setup
//! export ZOKA_TRUSTED_SETUP_DIR=./trusted-setup
//! ```
//!
//! Single-operator setup. The RNG (`OsRng`) produces τ transiently and drops
//! it at the end of this process, but a malicious operator could in
//! principle capture it before it is dropped. For production networks this
//! binary is a stepping stone: it decouples the CRS from the node binary and
//! pins every node to a specific digest, so that when a real multi-party
//! ceremony finishes you only need to swap the artifacts and update the
//! `DIGEST` reference.

use std::path::PathBuf;

use clap::Parser;

use zoka::zk::groth16_backend::{
    generate_setup_bundle_with_osrng, BALANCE_CIRCUIT, MERKLE_CIRCUIT, NULLIFIER_CIRCUIT,
    PRIVATE_TX_CIRCUIT,
};
use zoka::zk::setup_io;

#[derive(Debug, Parser)]
#[command(
    name = "zoka-trusted-setup",
    about = "Generate and serialize the Groth16 CRS bundle"
)]
struct Args {
    /// Directory where the CRS files (`*.params` + `DIGEST`) will be written.
    #[arg(long)]
    out: PathBuf,

    /// Overwrite an existing directory even if it already contains a CRS.
    #[arg(long)]
    force: bool,
}

fn main() {
    let args = Args::parse();

    if args.out.exists() && !args.force {
        let digest_existing = setup_io::digest_path(&args.out);
        if digest_existing.exists() {
            eprintln!(
                "refusing to overwrite existing CRS at {} (pass --force to overwrite)",
                args.out.display()
            );
            std::process::exit(2);
        }
    }

    std::fs::create_dir_all(&args.out).unwrap_or_else(|e| {
        eprintln!("cannot create {}: {}", args.out.display(), e);
        std::process::exit(1);
    });

    eprintln!("generating Groth16 parameters with OsRng…");
    let (balance_params, merkle_params, nullifier_params, private_tx_params) =
        generate_setup_bundle_with_osrng();

    for (circuit, params) in [
        (BALANCE_CIRCUIT, &balance_params),
        (MERKLE_CIRCUIT, &merkle_params),
        (NULLIFIER_CIRCUIT, &nullifier_params),
        (PRIVATE_TX_CIRCUIT, &private_tx_params),
    ] {
        eprintln!("  writing {}…", circuit);
        setup_io::write_circuit_params(&args.out, circuit, params).unwrap_or_else(|e| {
            eprintln!("failed writing {}: {}", circuit, e);
            std::process::exit(1);
        });
    }

    let digest = setup_io::compute_directory_digest(&args.out).unwrap_or_else(|e| {
        eprintln!("failed computing digest: {}", e);
        std::process::exit(1);
    });
    setup_io::write_digest(&args.out, &digest).unwrap_or_else(|e| {
        eprintln!("failed writing DIGEST: {}", e);
        std::process::exit(1);
    });

    println!(
        "trusted setup written to {}\nDIGEST: {}",
        args.out.display(),
        hex::encode(digest)
    );
    println!(
        "\nNow export:\n  ZOKA_TRUSTED_SETUP_DIR={}\nbefore booting zoka.",
        args.out.display()
    );
}
