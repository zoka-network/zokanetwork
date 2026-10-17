//! Una wallet de Quantic al día con un nodo, por bloques compactos.
//!
//! Lo que comprueba de cada bloque que le manda el nodo (A5 de la wallet
//! liviana): que sus transacciones compactas recompongan la raíz de txid que
//! compromete su `state_root`, y que el árbol de notas que arma la wallet con
//! ellas dé la raíz del árbol del mismo `state_root`. Un nodo no puede quitarle
//! ni cambiarle una salida sin que se note. Lo que no comprueba es la prueba de
//! trabajo de los encabezados: confía en el nodo propio para eso.

use zoka_quantic::compact::{CompactBlock, CompactTx};
use zoka_quantic::wallet::QuanticWallet;
use zoka_quantic::QuanticFullViewingKey;

use super::{consensus_params, QuanticRoots, GENESIS_HEIGHT};

/// Un bloque como lo dan `GET /quantic/compact` y `/zoka-quantic/light/1`.
pub use crate::network::quantic_light_request_response::CompactBlockWire as CompactBlockJson;

/// Bloques por pedido a la API.
pub const BATCH: u64 = 500;

/// Arma la wallet desde el génesis hasta `tip`, pidiendo los bloques con
/// `fetch(from, to)`.
pub fn sync_wallet(
    fvk: QuanticFullViewingKey,
    tip: u64,
    mut fetch: impl FnMut(u64, u64) -> Result<Vec<CompactBlockJson>, String>,
) -> Result<QuanticWallet, String> {
    let params = consensus_params(fvk.network());
    let mut wallet = QuanticWallet::new(fvk, &params);
    let mut next = GENESIS_HEIGHT;
    let mut previous_hash: Option<String> = None;
    while next <= tip {
        let to = (next + BATCH - 1).min(tip);
        let blocks = fetch(next, to)?;
        if blocks.is_empty() {
            return Err(format!("the node returned no blocks for {next}..{to}"));
        }
        for block in blocks {
            if block.height != next {
                return Err(format!("expected block {next}, the node sent {}", block.height));
            }
            if let Some(previous) = &previous_hash {
                if &block.prev_hash != previous {
                    return Err(format!("block {} does not follow the previous one", block.height));
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
            previous_hash = Some(block.hash);
            next += 1;
        }
    }
    Ok(wallet)
}
