//! Una wallet de Quantic al día con un nodo, por bloques compactos.
//!
//! Lo que comprueba de cada bloque que le manda el nodo (A5 de la wallet
//! liviana): que siga al anterior, que sus transacciones compactas recompongan
//! la raíz de txid que compromete su `state_root`, y que el árbol de notas que
//! arma la wallet con ellas dé la raíz del árbol del mismo `state_root`. En las
//! redes con génesis fijo, además, que el génesis sea ese. Un nodo no puede
//! quitarle ni cambiarle una salida sin que se note. Lo que no comprueba es la
//! prueba de trabajo de los encabezados: confía en el nodo para eso.

use zeroize::Zeroizing;
use zoka_quantic::compact::{CompactBlock, CompactTx};
use zoka_quantic::wallet::QuanticWallet;
use zoka_quantic::QuanticFullViewingKey;

use super::{consensus_params, fixed_genesis, QuanticRoots, GENESIS_HEIGHT};

/// Un bloque como lo dan `GET /quantic/compact` y `/zoka-quantic/light/1`.
pub use crate::network::quantic_light_request_response::CompactBlockWire as CompactBlockJson;

/// Bloques por pedido a la API.
pub const BATCH: u64 = 500;

/// Cuántos bloques detrás de la punta queda el estado seguro: más que la
/// ventana de anclas (100) y que la distancia entre checkpoints de la
/// autoridad (30), así una reorganización nunca llega más atrás.
pub const SAFE_DEPTH: u64 = 120;
/// Cada cuántos bloques se guarda un punto cerca de la punta.
pub const SAVE_EVERY: u64 = 30;
/// Puntos guardados como mucho (el seguro y los que siguen).
pub const MAX_SAVED_POINTS: usize = 8;

/// Un punto desde el que retomar el escaneo: el estado de la wallet después
/// del bloque `height` y el hash de ese bloque.
///
/// El estado tiene la clave de vista, que muestra toda la historia de la
/// wallet: se guarda cifrado.
#[derive(Clone)]
pub struct SavedScan {
    pub height: u64,
    pub last_hash: String,
    pub state: Zeroizing<Vec<u8>>,
}

impl SavedScan {
    fn of(wallet: &QuanticWallet, height: u64, last_hash: &str) -> Self {
        Self {
            height,
            last_hash: last_hash.to_string(),
            state: wallet.to_state_bytes(),
        }
    }

    fn open(&self, fvk: &QuanticFullViewingKey) -> Option<QuanticWallet> {
        let wallet = QuanticWallet::from_state_bytes(&self.state, &consensus_params(fvk.network())).ok()?;
        (*wallet.fvk().to_bytes() == *fvk.to_bytes() && u64::from(wallet.next_height()) == self.height + 1)
            .then_some(wallet)
    }
}

/// La wallet al día y los puntos para la próxima vez.
pub struct Resumed {
    pub wallet: QuanticWallet,
    pub points: Vec<SavedScan>,
    /// Desde qué altura escaneó (1: desde el génesis; `tip + 1`: nada nuevo).
    pub scanned_from: u64,
    /// El nodo va detrás de lo que la wallet ya vio (está sincronizando): se
    /// devuelve lo último verificado, sin tocar los puntos.
    pub node_behind: bool,
}

/// Lleva la wallet hasta `tip` desde lo guardado. Empieza por el punto más
/// nuevo que siga en la cadena del nodo (pide su bloque y compara el hash); si
/// ninguno sigue —una reorganización más honda que todos—, desde el génesis.
/// Devuelve los puntos a guardar: el último que quedó [`SAFE_DEPTH`] bloques
/// detrás de la punta y los que vienen después, uno cada [`SAVE_EVERY`].
pub fn resume_wallet(
    fvk: QuanticFullViewingKey,
    mut saved: Vec<SavedScan>,
    tip: u64,
    mut fetch: impl FnMut(u64, u64) -> Result<Vec<CompactBlockJson>, String>,
) -> Result<Resumed, String> {
    saved.retain(|point| point.open(&fvk).is_some());
    saved.sort_by_key(|point| std::cmp::Reverse(point.height));
    if !saved.is_empty() && saved.iter().all(|point| point.height > tip) {
        let wallet = saved[0].open(&fvk).expect("checked above");
        return Ok(Resumed {
            wallet,
            scanned_from: tip + 1,
            points: saved,
            node_behind: true,
        });
    }
    let mut start = None;
    for (index, point) in saved.iter().enumerate() {
        if point.height > tip {
            continue;
        }
        let on_chain = fetch(point.height, point.height)?
            .first()
            .is_some_and(|block| block.height == point.height && block.hash == point.last_hash);
        if on_chain {
            start = Some(index);
            break;
        }
    }
    // Los puntos más nuevos que el de partida ya no valen; él y los más viejos sí.
    let (mut wallet, mut last_hash, mut points) = match start {
        Some(index) => {
            let points = saved.split_off(index);
            let wallet = points[0].open(&fvk).expect("checked above");
            (wallet, Some(points[0].last_hash.clone()), points)
        }
        None => {
            let params = consensus_params(fvk.network());
            (QuanticWallet::new(fvk, &params), None, Vec::new())
        }
    };
    let scanned_from = u64::from(wallet.next_height());
    let safe_height = tip.saturating_sub(SAFE_DEPTH);
    while u64::from(wallet.next_height()) <= tip {
        let next = u64::from(wallet.next_height());
        let to = (next + BATCH - 1).min(tip);
        let blocks = fetch(next, to)?;
        if blocks.is_empty() {
            return Err(format!("the node returned no blocks for {next}..{to}"));
        }
        scan_compact_blocks(&mut wallet, &mut last_hash, &blocks, |wallet, block| {
            let near_tip = block.height >= safe_height && block.height % SAVE_EVERY == 0;
            if near_tip || block.height == safe_height || block.height == tip {
                points.push(SavedScan::of(wallet, block.height, &block.hash));
            }
        })?;
    }
    Ok(Resumed {
        wallet,
        points: prune_points(points, safe_height),
        scanned_from,
        node_behind: false,
    })
}

/// El último punto a [`SAFE_DEPTH`] o más de la punta y todos los posteriores,
/// sin repetir alturas y como mucho [`MAX_SAVED_POINTS`].
fn prune_points(mut points: Vec<SavedScan>, safe_height: u64) -> Vec<SavedScan> {
    points.sort_by_key(|point| std::cmp::Reverse(point.height));
    points.dedup_by_key(|point| point.height);
    if let Some(safe) = points.iter().position(|point| point.height <= safe_height) {
        points.truncate(safe + 1);
    }
    points.truncate(MAX_SAVED_POINTS);
    points
}

/// Arma la wallet desde el génesis hasta `tip`, pidiendo los bloques con
/// `fetch(from, to)`.
pub fn sync_wallet(
    fvk: QuanticFullViewingKey,
    tip: u64,
    mut fetch: impl FnMut(u64, u64) -> Result<Vec<CompactBlockJson>, String>,
) -> Result<QuanticWallet, String> {
    let params = consensus_params(fvk.network());
    let mut wallet = QuanticWallet::new(fvk, &params);
    let mut previous_hash: Option<String> = None;
    while u64::from(wallet.next_height()) <= tip {
        let next = u64::from(wallet.next_height());
        let to = (next + BATCH - 1).min(tip);
        let blocks = fetch(next, to)?;
        if blocks.is_empty() {
            return Err(format!("the node returned no blocks for {next}..{to}"));
        }
        scan_compact_blocks(&mut wallet, &mut previous_hash, &blocks, |_, _| {})?;
    }
    Ok(wallet)
}

/// Escanea `blocks` en orden, verificando cada uno, a continuación de lo que
/// ya escaneó la wallet: el primero tiene que ser el de su próxima altura y
/// seguir a `previous_hash` (el último bloque escaneado; `None` antes del
/// génesis). Deja en `previous_hash` el hash del último y llama a
/// `after_block` después de cada uno.
///
/// Si un bloque no pasa, devuelve el error y la wallet queda a medias: quien
/// llama la descarta y vuelve a su estado guardado.
pub fn scan_compact_blocks(
    wallet: &mut QuanticWallet,
    previous_hash: &mut Option<String>,
    blocks: &[CompactBlockJson],
    mut after_block: impl FnMut(&QuanticWallet, &CompactBlockJson),
) -> Result<(), String> {
    let genesis_hash = fixed_genesis(wallet.fvk().network()).map(|genesis| genesis.block_hash);
    for block in blocks {
        let next = u64::from(wallet.next_height());
        if block.height != next {
            return Err(format!("expected block {next}, the node sent {}", block.height));
        }
        match previous_hash.as_deref() {
            Some(previous) if block.prev_hash != previous => {
                return Err(format!(
                    "reorg: block {} does not follow the last scanned block",
                    block.height
                ));
            }
            None if block.height != GENESIS_HEIGHT => {
                return Err(format!("block {} needs the hash of the previous one", block.height));
            }
            _ => {}
        }
        if block.height == GENESIS_HEIGHT {
            if let Some(expected) = genesis_hash {
                if block.hash != expected {
                    return Err("the node follows another genesis".to_string());
                }
            }
        }
        let roots = QuanticRoots::parse(&block.state_root)
            .ok_or_else(|| format!("block {} has no Quantic state root", block.height))?;
        let transactions = block
            .transactions
            .iter()
            .map(|hex_tx| {
                hex::decode(hex_tx)
                    .map_err(|_| "compact transaction is not hex".to_string())
                    .and_then(|bytes| CompactTx::from_bytes(&bytes).map_err(|e| e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let compact = CompactBlock {
            height: u32::try_from(block.height).map_err(|_| "height out of range".to_string())?,
            transactions,
        };
        if !compact.matches_header(&roots.txid_root) {
            return Err(format!(
                "block {}: the compact data does not match its transaction root (the node hid or changed something)",
                block.height
            ));
        }
        wallet.scan_compact_block(&compact);
        if wallet.anchor().to_bytes() != roots.tree_root {
            return Err(format!(
                "block {}: the note commitment tree does not match the block",
                block.height
            ));
        }
        *previous_hash = Some(block.hash.clone());
        after_block(wallet, block);
    }
    Ok(())
}
