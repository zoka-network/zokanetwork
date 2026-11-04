//! Lo que una billetera liviana de Zoka Quantic necesita del nodo, sin el nodo.
//!
//! - [`checkpoint`]: verifica un checkpoint firmado por la autoridad de la
//!   cadena con la firma híbrida (Ed25519 + ML-DSA-65) que Quantic exige.
//! - [`directory`]: arma y verifica el registro de una dirección corta, con su
//!   estampilla de trabajo Argon2id.
//!
//! Replica el formato del nodo (`src/checkpoint_authority.rs`,
//! `src/hybrid_signature.rs`, `src/quantic/stark_directory.rs`). El nodo tiene
//! pruebas que comparan los dos comportamientos.

pub mod checkpoint;
pub mod directory;
