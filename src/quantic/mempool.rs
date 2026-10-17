//! Mempool de Zoka Quantic.
//!
//! Admite una transferencia solo si entraría en el próximo bloque: forma de
//! consenso, comisión convencional (regla de retransmisión), ancla entre las
//! raíces finales de los últimos bloques canónicos, nullifiers sin gastar en la
//! cadena ni tomados por otra del mempool, y prueba y firmas válidas. Así un par
//! no puede llenarlo de basura que después ningún bloque acepte.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use zoka_quantic::state::{check_transfer_shape, conventional_fee, MAX_BLOCK_ACTIONS};
use zoka_quantic::tx::{QuanticTransaction, TxId};

use super::{consensus_params, network_for_chain, recent_roots_after, verifying_key};
use crate::storage::RocksDbStorage;

pub const MAX_MEMPOOL_TXS: usize = 2_000;
pub const MAX_MEMPOOL_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MempoolError {
    #[error("transaction does not decode: {0}")]
    Decode(String),
    #[error("transaction breaks a consensus rule: {0}")]
    Rule(String),
    #[error("transaction already in the mempool")]
    AlreadyKnown,
    #[error("a note of this transaction is already spent on chain")]
    Spent,
    #[error("a note of this transaction is spent by another pending one")]
    Conflict,
    #[error("fee {fee} is below the conventional {needed}")]
    FeeTooLow { fee: u64, needed: u64 },
    #[error("mempool is full")]
    Full,
    #[error("mempool storage: {0}")]
    Storage(String),
}

struct Entry {
    tx: QuanticTransaction,
    size: usize,
    fee: u64,
    received: u64,
}

#[derive(Default)]
pub struct QuanticMempool {
    entries: BTreeMap<[u8; 32], Entry>,
    by_nullifier: HashMap<[u8; 32], [u8; 32]>,
    bytes: usize,
}

impl QuanticMempool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, txid: &TxId) -> bool {
        self.entries.contains_key(&txid.0)
    }

    pub fn get(&self, txid: &TxId) -> Option<&QuanticTransaction> {
        self.entries.get(&txid.0).map(|entry| &entry.tx)
    }

    pub fn txids(&self) -> Vec<TxId> {
        self.entries.keys().map(|id| TxId(*id)).collect()
    }

    /// Revisa `tx` contra la punta canónica de `storage` y, si entraría en el
    /// próximo bloque, la guarda.
    pub fn insert(
        &mut self,
        storage: &RocksDbStorage,
        chain_id: &str,
        tx: QuanticTransaction,
        now: u64,
    ) -> Result<TxId, MempoolError> {
        let txid = tx.txid();
        if self.entries.contains_key(&txid.0) {
            return Err(MempoolError::AlreadyKnown);
        }
        let network = network_for_chain(chain_id)
            .ok_or_else(|| MempoolError::Rule(format!("{chain_id} is not a Quantic chain")))?;
        let params = consensus_params(network);
        let tip_height = storage
            .get_chain_height()
            .map_err(|e| MempoolError::Storage(format!("{e:?}")))?;
        let next_height = u32::try_from(tip_height + 1).map_err(|_| MempoolError::Rule("height".into()))?;
        check_transfer_shape(&params, &tx, next_height).map_err(|e| MempoolError::Rule(e.to_string()))?;
        let fee = tx.fee().ok_or_else(|| MempoolError::Rule("not a transfer".into()))?;
        let needed = conventional_fee(tx.actions().len());
        if fee < needed {
            return Err(MempoolError::FeeTooLow { fee, needed });
        }
        if tip_height < super::GENESIS_HEIGHT {
            return Err(MempoolError::Rule("the chain has no genesis yet".into()));
        }
        let tip = storage
            .get_last_block_hash()
            .map_err(|e| MempoolError::Storage(format!("{e:?}")))?;
        let roots = recent_roots_after(storage, &tip, params.anchor_window)
            .map_err(|e| MempoolError::Storage(e.to_string()))?;
        if !roots.contains(&tx.anchor().to_bytes()) {
            return Err(MempoolError::Rule("anchor is not one of the recent tree roots".into()));
        }
        for nf in tx.nullifiers() {
            let spent = storage
                .is_nullifier_spent(&hex::encode(nf.to_bytes()))
                .map_err(|e| MempoolError::Storage(format!("{e:?}")))?;
            if spent {
                return Err(MempoolError::Spent);
            }
            if self.by_nullifier.contains_key(&nf.to_bytes()) {
                return Err(MempoolError::Conflict);
            }
        }
        let size = tx.serialized_size();
        if self.entries.len() >= MAX_MEMPOOL_TXS || self.bytes + size > MAX_MEMPOOL_BYTES {
            return Err(MempoolError::Full);
        }
        if !zoka_quantic::verify::verify_transactions(verifying_key(), [&tx], zoka_quantic::os_rng()) {
            return Err(MempoolError::Rule("invalid proof or signature".into()));
        }
        for nf in tx.nullifiers() {
            self.by_nullifier.insert(nf.to_bytes(), txid.0);
        }
        self.bytes += size;
        self.entries.insert(
            txid.0,
            Entry {
                tx,
                size,
                fee,
                received: now,
            },
        );
        Ok(txid)
    }

    fn remove(&mut self, txid: &[u8; 32]) {
        if let Some(entry) = self.entries.remove(txid) {
            for nf in entry.tx.nullifiers() {
                self.by_nullifier.remove(&nf.to_bytes());
            }
            self.bytes -= entry.size;
        }
    }

    /// Lo que entra en el próximo bloque: mejor comisión por acción primero
    /// (con la convencional todas pagan igual, así que manda la antigüedad),
    /// sin pasarse del tope de acciones del bloque.
    pub fn select(&self, max_actions: usize) -> Vec<QuanticTransaction> {
        let mut entries: Vec<&Entry> = self.entries.values().collect();
        entries.sort_by(|a, b| {
            let per_action = |e: &Entry| e.fee / e.tx.actions().len().max(1) as u64;
            per_action(b)
                .cmp(&per_action(a))
                .then(a.received.cmp(&b.received))
        });
        let limit = max_actions.min(MAX_BLOCK_ACTIONS);
        let mut actions = 0;
        let mut out = Vec::new();
        for entry in entries {
            let count = entry.tx.actions().len();
            if actions + count > limit {
                continue;
            }
            actions += count;
            out.push(entry.tx.clone());
        }
        out
    }

    /// Después de una punta nueva: saca lo que la cadena ya gastó y lo que
    /// quedó con un ancla fuera de la ventana. Devuelve cuántas sacó.
    pub fn revalidate(&mut self, storage: &RocksDbStorage, chain_id: &str) -> usize {
        let Some(network) = network_for_chain(chain_id) else {
            return 0;
        };
        let params = consensus_params(network);
        let Ok(tip) = storage.get_last_block_hash() else {
            return 0;
        };
        let roots: HashSet<[u8; 32]> = recent_roots_after(storage, &tip, params.anchor_window).unwrap_or_default();
        let stale: Vec<[u8; 32]> = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                !roots.contains(&entry.tx.anchor().to_bytes())
                    || entry.tx.nullifiers().any(|nf| {
                        storage
                            .is_nullifier_spent(&hex::encode(nf.to_bytes()))
                            .unwrap_or(true)
                    })
            })
            .map(|(id, _)| *id)
            .collect();
        for id in &stale {
            self.remove(id);
        }
        stale.len()
    }
}

/// El mempool del nodo: lo comparten la API, la red y el minero.
pub fn global() -> &'static Mutex<QuanticMempool> {
    static POOL: OnceLock<Mutex<QuanticMempool>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(QuanticMempool::new()))
}
