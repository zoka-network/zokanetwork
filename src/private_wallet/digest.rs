//! Resumen público de una transacción privada, para wallets livianas.
//!
//! Una transacción privada de Zenith pesa entre 59 y 346 KB porque lleva
//! pruebas, anillo y pruebas de inclusión. Para saber si le toca, una wallet
//! solo necesita las salidas (para intentar reconocerlas con su clave) y qué
//! notas gasta. El resumen lleva eso y nada más; medido el 15/10/2026 sobre dos
//! transacciones reales, sin comprimir: 346 KB → ~3 KB y 59 KB → ~7 KB (las
//! salidas `zkq1` cargan su ciphertext X-Wing de 1.120 bytes).
//!
//! Lo que dice un resumen no se puede comprobar contra el `tx_hash`, que cubre
//! el cuerpo entero. Por eso solo sirve para elegir qué cuerpos bajar: una
//! wallet cuenta una salida o un gasto recién cuando tiene el cuerpo y verificó
//! su hash. Ver `docs/HOJA-DE-RUTA-wallet-liviana.md`.

use super::{
    constant_time_eq_str, derive_note_nullifier, derive_output_secret_scalar, point_to_hex,
    tx_input_nullifiers, tx_outputs, PrivateOutput, PrivateTransaction, PrivateWalletManager,
    PRIVATE_OUTPUT_VERSION_CLASSIC,
};
use crate::types::BlockError;
use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Largo, en caracteres hex, de una marca de gasto: 8 bytes del nullifier o del
/// compromiso que gasta la transacción.
///
/// Alcanza para elegir qué cuerpo bajar (dos marcas iguales por azar tienen
/// una chance en 2^64) y achica mucho el resumen de una transacción con muchas
/// entradas: la del bloque 14.704 gasta 73 notas.
pub const SPEND_TAG_HEX_LEN: usize = 16;

/// Lo público y chico de una transacción privada.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrivateTxDigest {
    pub tx_hash: String,
    /// Marcas ([`spend_tag`]) de los nullifiers y de los compromisos que gasta.
    #[serde(default)]
    pub spend_tags: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<PrivateOutputDigest>,
}

/// Lo que hace falta para intentar reconocer una salida como propia.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrivateOutputDigest {
    pub output_id: String,
    #[serde(
        default = "super::default_private_output_version",
        skip_serializing_if = "super::is_classic_private_output_version"
    )]
    pub output_version: u8,
    /// Clave efímera de una salida v1, en hex.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ephemeral_pubkey: String,
    /// Ciphertext X-Wing de una salida v2 (`zkq1`), en hex.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kem_ciphertext: String,
    pub commitment: String,
    pub one_time_public_key: String,
}

/// Qué tiene que ver una transacción con una wallet, según su resumen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DigestRelevance {
    /// Alguna salida es de la wallet (un pago o un vuelto).
    pub receives: bool,
    /// Gasta alguna nota de la wallet.
    pub spends: bool,
}

impl DigestRelevance {
    pub fn any(&self) -> bool {
        self.receives || self.spends
    }
}

/// Marca de gasto de un nullifier o de un compromiso.
pub fn spend_tag(value: &str) -> String {
    value
        .trim()
        .chars()
        .take(SPEND_TAG_HEX_LEN)
        .collect::<String>()
        .to_ascii_lowercase()
}

impl PrivateTxDigest {
    pub fn from_tx(tx: &PrivateTransaction) -> Self {
        let mut seen = HashSet::new();
        let spend_tags = tx_input_nullifiers(tx)
            .iter()
            .chain(tx.input_commitments.iter())
            .filter(|value| !value.trim().is_empty())
            .map(|value| spend_tag(value))
            .filter(|tag| seen.insert(tag.clone()))
            .collect();
        let outputs = tx_outputs(tx)
            .iter()
            .map(|output| PrivateOutputDigest {
                output_id: output.output_id.clone(),
                output_version: output.output_version,
                ephemeral_pubkey: hex::encode(&output.ephemeral_pubkey),
                kem_ciphertext: hex::encode(&output.kem_ciphertext),
                commitment: output.commitment.clone(),
                one_time_public_key: output.one_time_public_key.clone(),
            })
            .collect();
        Self {
            tx_hash: tx.tx_hash.clone(),
            spend_tags,
            outputs,
        }
    }
}

impl PrivateOutputDigest {
    /// La salida con lo que trae el resumen; lo demás queda vacío. Un hex roto
    /// da `None`: eso lo manda un par que no sabe o que miente, y no es de nadie.
    fn to_scan_output(&self) -> Option<PrivateOutput> {
        Some(PrivateOutput {
            output_id: self.output_id.clone(),
            asset_tag: super::default_asset_tag(),
            asset_tag_commitment: String::new(),
            recipient_commitment: String::new(),
            stealth_address: String::new(),
            ephemeral_pubkey: hex::decode(&self.ephemeral_pubkey).ok()?,
            output_version: if self.output_version == 0 {
                PRIVATE_OUTPUT_VERSION_CLASSIC
            } else {
                self.output_version
            },
            kem_ciphertext: hex::decode(&self.kem_ciphertext).ok()?,
            commitment: self.commitment.clone(),
            amount_commitment: String::new(),
            range_proof: Vec::new(),
            one_time_public_key: self.one_time_public_key.clone(),
            encrypted_amount: Vec::new(),
            commitment_blind: Vec::new(),
            is_change: false,
        })
    }
}

impl PrivateWalletManager {
    /// Qué tiene que ver una transacción con `wallet_name`, mirando solo su
    /// resumen.
    ///
    /// Reconoce las salidas con la misma derivación que el escaneo del cuerpo
    /// entero (`decrypt_private_output_for_wallet`), así que no puede ver menos
    /// que él. Para los gastos compara marcas, así que puede dar un falso
    /// positivo (una chance en 2^64); el cuerpo entero lo descarta después.
    pub fn private_tx_digest_relevance(
        &self,
        wallet_name: &str,
        digest: &PrivateTxDigest,
    ) -> Result<DigestRelevance, BlockError> {
        let wallet = self.private_wallets.get(wallet_name).ok_or_else(|| {
            BlockError::TransactionValidationFailed("Wallet not found".to_string())
        })?;
        let scan_key = if wallet.scan_key.is_empty() {
            &wallet.spending_key
        } else {
            &wallet.scan_key
        };

        let receives = digest.outputs.iter().any(|output| {
            let Some(output) = output.to_scan_output() else {
                return false;
            };
            match derive_output_secret_scalar(&wallet.spending_key, scan_key, &output) {
                Ok(secret) => constant_time_eq_str(
                    &point_to_hex(&(secret * RISTRETTO_BASEPOINT_POINT)),
                    &output.one_time_public_key,
                ),
                Err(_) => false,
            }
        });

        let tags: HashSet<&str> = digest.spend_tags.iter().map(String::as_str).collect();
        let spends = !tags.is_empty()
            && wallet.private_notes.iter().any(|note| {
                let nullifier =
                    derive_note_nullifier(&wallet.spending_key, &note.note_id, &note.commitment);
                tags.contains(spend_tag(&nullifier).as_str())
                    || tags.contains(spend_tag(&note.commitment).as_str())
            });

        Ok(DigestRelevance { receives, spends })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Alice (con 50) le paga a bob; carol mira.
    fn tx_entre(manager: &mut PrivateWalletManager, monto: u64) -> PrivateTransaction {
        let bob = manager.private_wallets["bob"].commitment_key.clone();
        let hash = manager.send_private_transaction("alice", bob, monto).unwrap();
        manager.get_private_transaction(&hash).unwrap().clone()
    }

    fn con_tres_wallets() -> PrivateWalletManager {
        let mut manager = PrivateWalletManager::new();
        manager.create_private_wallet("alice").unwrap();
        let bob = manager.create_private_wallet("bob").unwrap();
        assert_eq!(bob, manager.private_wallets["bob"].commitment_key);
        manager.create_private_wallet("carol").unwrap();
        manager.deposit_to_private_wallet("alice", 50).unwrap();
        manager
    }

    #[test]
    fn el_resumen_ve_lo_mismo_que_el_cuerpo_entero() {
        let mut manager = con_tres_wallets();
        let tx = tx_entre(&mut manager, 10);
        let digest = PrivateTxDigest::from_tx(&tx);

        let bob = manager.private_tx_digest_relevance("bob", &digest).unwrap();
        let carol = manager.private_tx_digest_relevance("carol", &digest).unwrap();
        let alice = manager.private_tx_digest_relevance("alice", &digest).unwrap();

        assert_eq!(bob, DigestRelevance { receives: true, spends: false });
        assert!(!carol.any(), "a carol no le toca nada");
        assert!(alice.spends, "alice gasta su nota");
        assert!(alice.receives, "y recibe el vuelto");

        // Lo que dice el resumen coincide con aplicar el cuerpo entero.
        assert!(manager.apply_relevant_private_transaction_for_wallet("bob", &tx).unwrap());
        assert!(!manager.apply_relevant_private_transaction_for_wallet("carol", &tx).unwrap());
    }

    #[test]
    fn el_resumen_detecta_un_gasto_por_el_compromiso_de_la_nota() {
        // Una wallet restaurada que todavía no tiene el nullifier guardado
        // reconoce su gasto por el compromiso, igual que el escaneo completo.
        let mut manager = con_tres_wallets();
        let antes = manager.get_private_wallet("alice").unwrap().clone();
        let mut tx = tx_entre(&mut manager, 10);
        tx.input_nullifiers = vec!["otro-nullifier".to_string()];
        tx.outputs.clear();

        let mut restaurado = PrivateWalletManager::new();
        restaurado
            .private_wallets
            .insert("alice".to_string(), antes);
        let relevancia = restaurado
            .private_tx_digest_relevance("alice", &PrivateTxDigest::from_tx(&tx))
            .unwrap();
        assert!(relevancia.spends);
    }

    #[test]
    fn el_resumen_no_lleva_pruebas_ni_anillo() {
        let mut manager = con_tres_wallets();
        let tx = tx_entre(&mut manager, 10);
        let completo = serde_json::to_vec(&tx).unwrap();
        let resumen = serde_json::to_vec(&PrivateTxDigest::from_tx(&tx)).unwrap();
        let texto = String::from_utf8(resumen.clone()).unwrap();

        for pesado in ["range_proof", "ring_signature", "ring_members", "zk_proof", "inclusion"] {
            assert!(!texto.contains(pesado), "el resumen trae {pesado}");
        }
        assert!(
            resumen.len() * 4 < completo.len(),
            "el resumen tiene que ser mucho más chico: {} contra {}",
            resumen.len(),
            completo.len()
        );
        // Ida y vuelta por JSON sin perder nada.
        let vuelta: PrivateTxDigest = serde_json::from_slice(&resumen).unwrap();
        assert_eq!(vuelta, PrivateTxDigest::from_tx(&tx));
    }

    #[test]
    fn un_resumen_roto_no_es_de_nadie_ni_rompe_el_escaneo() {
        let mut manager = con_tres_wallets();
        let tx = tx_entre(&mut manager, 10);
        let mut digest = PrivateTxDigest::from_tx(&tx);
        for output in &mut digest.outputs {
            output.ephemeral_pubkey = "no-es-hex".to_string();
            output.kem_ciphertext = "zz".to_string();
        }
        digest.spend_tags.clear();
        let bob = manager.private_tx_digest_relevance("bob", &digest).unwrap();
        assert!(!bob.any());
        assert!(manager
            .private_tx_digest_relevance("nadie", &digest)
            .is_err());
    }

    #[test]
    fn la_marca_de_gasto_son_ocho_bytes_en_minuscula() {
        assert_eq!(spend_tag(" ABCDEF0123456789ffff "), "abcdef0123456789");
        assert_eq!(spend_tag("abc"), "abc");
    }

    #[cfg(feature = "pq-experimental")]
    #[test]
    fn el_resumen_reconoce_un_pago_a_zkq1() {
        let mut manager = con_tres_wallets();
        let bob_zkq1 = manager
            .get_private_wallet("bob")
            .unwrap()
            .hybrid_payment_address()
            .unwrap();
        let hash = manager.send_private_transaction("alice", bob_zkq1, 20).unwrap();
        let tx = manager.get_private_transaction(&hash).unwrap().clone();
        let digest = PrivateTxDigest::from_tx(&tx);
        assert!(digest
            .outputs
            .iter()
            .all(|output| output.output_version == super::super::PRIVATE_OUTPUT_VERSION_HYBRID
                && !output.kem_ciphertext.is_empty()));

        let bob = manager.private_tx_digest_relevance("bob", &digest).unwrap();
        let carol = manager.private_tx_digest_relevance("carol", &digest).unwrap();
        assert!(bob.receives);
        assert!(!carol.any());
    }
}
