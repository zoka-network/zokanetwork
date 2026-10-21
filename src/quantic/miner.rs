//! Minero de Zoka Quantic: arma la plantilla (coinbase con su prueba Halo 2,
//! transferencias del mempool) y busca el nonce en varios hilos.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use zoka_quantic::builder::build_coinbase;
use zoka_quantic::orchard::circuit::ProvingKey;
use zoka_quantic::tx::QuanticTransaction;
use zoka_quantic::QuanticAddress;

use super::{assemble_block, consensus_params, network_for_chain, ValidatedBlock, GENESIS_HEIGHT};
use crate::state::chain::genesis_parent_for;
use crate::storage::RocksDbStorage;
use crate::types::Block;

/// La plantilla del próximo bloque encima de la punta canónica. El génesis
/// paga el premine anunciado (salida pública), sea quien sea el que mine.
pub fn build_template(
    storage: &RocksDbStorage,
    chain_id: &str,
    miner: &QuanticAddress,
    proving_key: &ProvingKey,
    transactions: Vec<QuanticTransaction>,
    timestamp: u64,
) -> Result<(Block, ValidatedBlock), String> {
    let network = network_for_chain(chain_id).ok_or_else(|| format!("{chain_id} is not a Quantic chain"))?;
    if miner.network() != network {
        return Err(format!("the mining address is not a {} address", network.chain_id()));
    }
    let params = consensus_params(network);
    let tip = storage.get_chain_height().map_err(|e| format!("{e:?}"))?;
    let parent = if tip == 0 {
        genesis_parent_for(chain_id)
    } else {
        let hash = storage.get_last_block_hash().map_err(|e| format!("{e:?}"))?;
        storage
            .get_block(&hash)
            .map_err(|e| format!("{e:?}"))?
            .ok_or_else(|| format!("missing tip block {hash}"))?
    };
    let height = parent.header.block_height + 1;
    let height32 = u32::try_from(height).map_err(|_| "height out of range".to_string())?;
    // Después de la mediana de los últimos bloques (regla de consenso).
    let timestamp = timestamp.max(super::median_time_past(storage, &parent).map_err(|e| e.to_string())? + 1);
    let build = |txs: &[QuanticTransaction]| -> Result<(Block, ValidatedBlock), String> {
        let fees: u64 = txs.iter().filter_map(QuanticTransaction::fee).sum();
        let (recipient, value, public) = match (&params.premine, height == GENESIS_HEIGHT) {
            (Some(premine), true) => (premine.recipient.clone(), premine.value, true),
            _ => (miner.clone(), params.minted_at(height32) + fees, false),
        };
        let coinbase = build_coinbase(network, height32, &recipient, value, public, proving_key, &mut zoka_quantic::os_rng())
            .map_err(|e| e.to_string())?;
        assemble_block(storage, chain_id, &parent, &coinbase, txs, timestamp).map_err(|e| e.to_string())
    };
    // El génesis no lleva transferencias.
    if height == GENESIS_HEIGHT || transactions.is_empty() {
        return build(&[]);
    }
    // Lo que el mempool dio puede haber quedado viejo (otro bloque gastó la
    // nota, o el ancla salió de la ventana): se filtra contra la punta antes de
    // armar. Si igual algo no entra, el bloque sale sin transferencias.
    let roots = super::recent_roots_after(storage, &parent.header.hash, params.anchor_window)
        .map_err(|e| e.to_string())?;
    let mut nullifiers = std::collections::HashSet::new();
    let usable: Vec<QuanticTransaction> = transactions
        .into_iter()
        .filter(|tx| {
            roots.contains(&tx.anchor().to_bytes())
                && tx.nullifiers().all(|nf| {
                    nullifiers.insert(nf.to_bytes())
                        && !storage
                            .is_nullifier_spent(&hex::encode(nf.to_bytes()))
                            .unwrap_or(true)
                })
        })
        .collect();
    build(&usable).or_else(|_| build(&[]))
}

/// Hashes calculados por este proceso desde que arrancó. El runtime saca de
/// acá el hashrate que muestran la API y `mine-status`.
static HASHES: AtomicU64 = AtomicU64::new(0);

pub fn total_hashes() -> u64 {
    HASHES.load(Ordering::Relaxed)
}

/// Resultado de una búsqueda de nonce.
pub enum PowOutcome {
    Solved,
    /// Se cumplió el plazo o pidieron parar: hay que rearmar la plantilla.
    Interrupted,
}

/// Busca el nonce en `threads` hilos hasta resolverlo, hasta `deadline` o
/// hasta que `stop` se ponga en verdadero. Deja nonce, sello y hash puestos.
pub fn solve_pow_parallel(
    chain_id: &str,
    block: &mut Block,
    compact_target: u32,
    threads: usize,
    deadline: Duration,
    stop: &AtomicBool,
) -> Result<PowOutcome, String> {
    let target = super::difficulty::from_compact(compact_target);
    let target = &target;
    let lanes = threads.max(1);
    let engines = crate::mining::pow::engine_pool_for(chain_id, block.header.block_height, lanes)
        .map_err(|e| e.to_string())?;
    let found = AtomicBool::new(false);
    let started = Instant::now();
    let (sender, receiver) = mpsc::channel::<Result<(Vec<u8>, u64), String>>();
    let header = block.header.clone();
    let result = std::thread::scope(|scope| {
        for (lane, engine) in engines.into_iter().enumerate() {
            let sender = sender.clone();
            let found = &found;
            let header = &header;
            scope.spawn(move || {
                let mut nonce = lane as u64;
                while !found.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
                    if started.elapsed() >= deadline {
                        return;
                    }
                    let input = crate::mining::pow::build_quantic_pow_input(
                        chain_id,
                        &header.prev_hash,
                        &header.merkle_root,
                        &header.state_root,
                        header.block_height,
                        header.timestamp,
                        nonce,
                    );
                    match engine.calculate_hash(&input) {
                        Ok(hash) => {
                            HASHES.fetch_add(1, Ordering::Relaxed);
                            if super::difficulty::hash_meets_target(&hash, target) {
                                found.store(true, Ordering::Relaxed);
                                let _ = sender.send(Ok((hash, nonce)));
                                return;
                            }
                        }
                        Err(error) => {
                            found.store(true, Ordering::Relaxed);
                            let _ = sender.send(Err(error.to_string()));
                            return;
                        }
                    }
                    nonce = nonce.wrapping_add(lanes as u64);
                }
            });
        }
        drop(sender);
        receiver.recv().ok()
    });
    match result {
        Some(Ok((hash, nonce))) => {
            block.header.nonce = nonce;
            block.header.seal = crate::types::Signature(hash);
            block.header.hash = block.compute_hash();
            Ok(PowOutcome::Solved)
        }
        Some(Err(error)) => Err(error),
        None => Ok(PowOutcome::Interrupted),
    }
}
