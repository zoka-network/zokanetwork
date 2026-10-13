//! Checkpoints firmados por el nodo del equipo.
//!
//! Diseño completo: `docs/CHECKPOINTS-AUTORITATIVOS.md`. En corto: el nodo del
//! equipo firma "en la altura H el bloque canónico es este hash"; los demás
//! nodos verifican la firma con la clave pública compilada para su cadena y no
//! siguen ninguna rama que contradiga el último checkpoint aceptado. La regla
//! la aplica `state::chain`, sumando ese checkpoint a los compilados.
//!
//! Esto no cambia las reglas de validación: un bloque inválido sigue siendo
//! inválido. El checkpoint solo elige cuál de las cadenas válidas es la
//! canónica.

use crate::storage::RocksDbStorage;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;
use zeroize::Zeroize;

/// Versión del formato. La 2 será la firma híbrida Ed25519 + ML-DSA-65.
pub const SIGNED_CHECKPOINT_VERSION: u8 = 1;
/// El firmador publica un checkpoint cada tantos bloques...
pub const AUTHORITY_SIGN_INTERVAL: u64 = 30;
/// ...y firma el bloque que está tantos por debajo de la punta, para no
/// fijar uno que una reorganización corta todavía pueda cambiar.
pub const AUTHORITY_CONFIRMATIONS: u64 = 6;
/// Nombre del archivo con la clave privada del equipo, dentro de la carpeta de
/// datos de la red. Si existe al arrancar, el nodo firma checkpoints.
pub const AUTHORITY_KEY_FILE: &str = "checkpoint-authority.key";
/// Reemplaza la clave pública compilada. Solo para redes locales de prueba.
pub const AUTHORITY_PUBKEY_ENV: &str = "ZOKA_CHECKPOINT_AUTHORITY_PUBKEY";

const SIGNING_DOMAIN: &[u8] = b"zoka-signed-checkpoint-v1";
const LATEST_KEY: &str = "checkpoint:signed:latest";
const BY_HEIGHT_PREFIX: &str = "checkpoint:signed:h:";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedCheckpoint {
    pub version: u8,
    pub chain_id: String,
    pub height: u64,
    pub block_hash: String,
    /// Firma Ed25519 en hex (64 bytes).
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckpointError {
    /// La cadena no tiene clave de autoridad compilada ni configurada.
    NoAuthority(String),
    WrongChain { expected: String, actual: String },
    Version(u8),
    Malformed(&'static str),
    BadSignature,
    /// Hay uno aceptado más nuevo: un checkpoint viejo no lo reemplaza.
    Stale { latest: u64, received: u64 },
    /// Misma altura, otro hash: la clave estaría comprometida o el firmador
    /// se equivocó. Se conserva el primero.
    Conflict {
        height: u64,
        kept: String,
        received: String,
    },
    Storage(String),
}

impl fmt::Display for CheckpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAuthority(chain) => write!(f, "no checkpoint authority key for chain {chain}"),
            Self::WrongChain { expected, actual } => {
                write!(f, "checkpoint is for chain {actual}, this node follows {expected}")
            }
            Self::Version(v) => write!(f, "unsupported signed checkpoint version {v}"),
            Self::Malformed(what) => write!(f, "malformed signed checkpoint: {what}"),
            Self::BadSignature => f.write_str("signed checkpoint signature is not valid"),
            Self::Stale { latest, received } => write!(
                f,
                "signed checkpoint at {received} is older than the accepted one at {latest}"
            ),
            Self::Conflict {
                height,
                kept,
                received,
            } => write!(
                f,
                "conflicting signed checkpoint at {height}: kept {kept}, received {received}"
            ),
            Self::Storage(err) => write!(f, "signed checkpoint storage error: {err}"),
        }
    }
}

impl std::error::Error for CheckpointError {}

/// Resultado de aceptar un checkpoint ya verificado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acceptance {
    /// Es el nuevo checkpoint vigente.
    Accepted,
    /// Ya estaba: no hace falta reenviarlo.
    AlreadyKnown,
}

fn signing_message(chain_id: &str, height: u64, block_hash: &str) -> Vec<u8> {
    let mut message = Vec::with_capacity(SIGNING_DOMAIN.len() + chain_id.len() + block_hash.len() + 11);
    message.extend_from_slice(SIGNING_DOMAIN);
    message.push(0x1f);
    message.extend_from_slice(chain_id.as_bytes());
    message.push(0x1f);
    message.extend_from_slice(&height.to_le_bytes());
    message.push(0x1f);
    message.extend_from_slice(block_hash.as_bytes());
    message
}

fn is_block_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl SignedCheckpoint {
    pub fn sign(key: &SigningKey, chain_id: &str, height: u64, block_hash: &str) -> Self {
        let signature = key.sign(&signing_message(chain_id, height, block_hash));
        Self {
            version: SIGNED_CHECKPOINT_VERSION,
            chain_id: chain_id.to_string(),
            height,
            block_hash: block_hash.to_string(),
            signature: hex::encode(signature.to_bytes()),
        }
    }

    /// Forma, sin la firma: lo que se puede revisar antes de saber la clave.
    pub fn validate_shape(&self) -> Result<(), CheckpointError> {
        if self.version != SIGNED_CHECKPOINT_VERSION {
            return Err(CheckpointError::Version(self.version));
        }
        if self.chain_id.trim().is_empty() || self.chain_id.len() > 128 {
            return Err(CheckpointError::Malformed("chain_id"));
        }
        if self.height == 0 {
            return Err(CheckpointError::Malformed("height"));
        }
        if !is_block_hash(&self.block_hash) {
            return Err(CheckpointError::Malformed("block_hash"));
        }
        if self.signature.len() != 128 {
            return Err(CheckpointError::Malformed("signature"));
        }
        Ok(())
    }

    pub fn verify(&self, authority: &VerifyingKey) -> Result<(), CheckpointError> {
        self.validate_shape()?;
        let bytes = hex::decode(&self.signature).map_err(|_| CheckpointError::Malformed("signature"))?;
        let bytes: [u8; 64] = bytes
            .try_into()
            .map_err(|_| CheckpointError::Malformed("signature"))?;
        let signature = Signature::from_bytes(&bytes);
        authority
            .verify_strict(&signing_message(&self.chain_id, self.height, &self.block_hash), &signature)
            .map_err(|_| CheckpointError::BadSignature)
    }

    /// Verifica contra la clave de la cadena que el propio checkpoint declara.
    /// Es lo que puede hacer la capa de red, que no sabe qué cadena sigue el
    /// nodo; la aceptación vuelve a exigir que sea la cadena correcta.
    pub fn verify_for_declared_chain(&self) -> Result<(), CheckpointError> {
        let authority = authority_key_for_chain(&self.chain_id)
            .ok_or_else(|| CheckpointError::NoAuthority(self.chain_id.clone()))?;
        self.verify(&authority)
    }
}

pub fn parse_public_key(hex_key: &str) -> Option<VerifyingKey> {
    let bytes = hex::decode(hex_key.trim()).ok()?;
    let bytes: [u8; 32] = bytes.try_into().ok()?;
    VerifyingKey::from_bytes(&bytes).ok()
}

/// Clave pública de la autoridad para `chain_id`: la de
/// [`AUTHORITY_PUBKEY_ENV`] si está puesta (redes locales de prueba), o la
/// compilada en `economics::CHECKPOINT_AUTHORITY_KEYS`.
pub fn authority_key_for_chain(chain_id: &str) -> Option<VerifyingKey> {
    if let Ok(value) = std::env::var(AUTHORITY_PUBKEY_ENV) {
        if !value.trim().is_empty() {
            return parse_public_key(&value);
        }
    }
    crate::economics::CHECKPOINT_AUTHORITY_KEYS
        .iter()
        .find(|(chain, _)| *chain == chain_id)
        .and_then(|(_, key)| parse_public_key(key))
}

fn read(storage: &RocksDbStorage, key: &str) -> Result<Option<SignedCheckpoint>, CheckpointError> {
    let Some(bytes) = storage
        .get_raw(key)
        .map_err(|e| CheckpointError::Storage(e.to_string()))?
    else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| CheckpointError::Storage(e.to_string()))
}

fn by_height_key(height: u64) -> String {
    format!("{BY_HEIGHT_PREFIX}{height:020}")
}

/// El último checkpoint aceptado. Ya fue verificado al guardarse.
pub fn latest_signed_checkpoint(
    storage: &RocksDbStorage,
) -> Result<Option<SignedCheckpoint>, CheckpointError> {
    read(storage, LATEST_KEY)
}

/// Verifica `checkpoint` contra `authority` y, si corresponde, lo guarda como
/// el vigente. Reglas: la cadena tiene que ser `chain_id`; un checkpoint más
/// viejo que el vigente no lo reemplaza; la misma altura con otro hash es un
/// conflicto y se conserva el primero.
pub fn accept_signed_checkpoint(
    storage: &RocksDbStorage,
    chain_id: &str,
    checkpoint: &SignedCheckpoint,
    authority: &VerifyingKey,
) -> Result<Acceptance, CheckpointError> {
    if checkpoint.chain_id != chain_id {
        return Err(CheckpointError::WrongChain {
            expected: chain_id.to_string(),
            actual: checkpoint.chain_id.clone(),
        });
    }
    checkpoint.verify(authority)?;

    if let Some(at_height) = read(storage, &by_height_key(checkpoint.height))? {
        return if at_height.block_hash == checkpoint.block_hash {
            Ok(Acceptance::AlreadyKnown)
        } else {
            Err(CheckpointError::Conflict {
                height: checkpoint.height,
                kept: at_height.block_hash,
                received: checkpoint.block_hash.clone(),
            })
        };
    }
    if let Some(latest) = latest_signed_checkpoint(storage)? {
        if checkpoint.height < latest.height {
            return Err(CheckpointError::Stale {
                latest: latest.height,
                received: checkpoint.height,
            });
        }
    }

    let bytes = serde_json::to_vec(checkpoint).map_err(|e| CheckpointError::Storage(e.to_string()))?;
    storage
        .put_raw(&by_height_key(checkpoint.height), bytes.clone())
        .map_err(|e| CheckpointError::Storage(e.to_string()))?;
    storage
        .put_raw(LATEST_KEY, bytes)
        .map_err(|e| CheckpointError::Storage(e.to_string()))?;
    Ok(Acceptance::Accepted)
}

/// Altura que el firmador tiene que firmar cuando la punta es `tip`, si toca.
/// Firma cada [`AUTHORITY_SIGN_INTERVAL`] bloques, siempre
/// [`AUTHORITY_CONFIRMATIONS`] por debajo de la punta.
pub fn next_height_to_sign(tip: u64, last_signed: Option<u64>) -> Option<u64> {
    let target = tip.checked_sub(AUTHORITY_CONFIRMATIONS)?;
    if target == 0 {
        return None;
    }
    match last_signed {
        Some(last) if target < last.saturating_add(AUTHORITY_SIGN_INTERVAL) => None,
        _ => Some(target),
    }
}

/// Lee la clave privada de la autoridad: 32 bytes en hex.
pub fn load_signing_key(path: &Path) -> Result<SigningKey, String> {
    let mut text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read checkpoint authority key {}: {e}", path.display()))?;
    let decoded = hex::decode(text.trim());
    text.zeroize();
    let mut bytes = decoded.map_err(|_| "checkpoint authority key is not hex".to_string())?;
    let result = <[u8; 32]>::try_from(bytes.as_slice())
        .map(|seed| SigningKey::from_bytes(&seed))
        .map_err(|_| "checkpoint authority key must be 32 bytes".to_string());
    bytes.zeroize();
    result
}

/// Crea una clave nueva en `path` y devuelve la pública en hex. No
/// sobrescribe: si el archivo existe es la clave del equipo y perderla
/// obligaría a sacar binarios nuevos.
pub fn generate_signing_key(path: &Path) -> Result<String, String> {
    if path.exists() {
        return Err(format!(
            "{} already exists; refusing to overwrite the checkpoint authority key",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut seed = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut seed);
    let key = SigningKey::from_bytes(&seed);
    seed.zeroize();
    let mut secret_hex = hex::encode(key.to_bytes());
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, secret_hex.as_bytes()));
    secret_hex.zeroize();
    written.map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(hex::encode(key.verifying_key().to_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAIN: &str = "zoka-test-checkpoints";

    fn key(byte: u8) -> SigningKey {
        SigningKey::from_bytes(&[byte; 32])
    }

    fn hash(byte: char) -> String {
        std::iter::repeat(byte).take(64).collect()
    }

    fn storage() -> (RocksDbStorage, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage = RocksDbStorage::new(dir.path().to_str().unwrap()).unwrap();
        (storage, dir)
    }

    #[test]
    fn una_firma_valida_se_verifica_y_una_alterada_no() {
        let k = key(1);
        let cp = SignedCheckpoint::sign(&k, CHAIN, 100, &hash('a'));
        cp.verify(&k.verifying_key()).unwrap();

        let mut otra_altura = cp.clone();
        otra_altura.height = 101;
        assert_eq!(otra_altura.verify(&k.verifying_key()), Err(CheckpointError::BadSignature));

        let mut otro_hash = cp.clone();
        otro_hash.block_hash = hash('b');
        assert_eq!(otro_hash.verify(&k.verifying_key()), Err(CheckpointError::BadSignature));

        let mut otra_cadena = cp.clone();
        otra_cadena.chain_id = "zoka-otra".to_string();
        assert_eq!(otra_cadena.verify(&k.verifying_key()), Err(CheckpointError::BadSignature));

        assert_eq!(cp.verify(&key(2).verifying_key()), Err(CheckpointError::BadSignature));
    }

    #[test]
    fn la_forma_se_revisa_antes_de_la_firma() {
        let k = key(1);
        let mut cp = SignedCheckpoint::sign(&k, CHAIN, 100, &hash('a'));
        cp.version = 2;
        assert_eq!(cp.validate_shape(), Err(CheckpointError::Version(2)));
        let mut cp = SignedCheckpoint::sign(&k, CHAIN, 100, "ABC");
        assert_eq!(cp.validate_shape(), Err(CheckpointError::Malformed("block_hash")));
        cp = SignedCheckpoint::sign(&k, CHAIN, 0, &hash('a'));
        assert_eq!(cp.validate_shape(), Err(CheckpointError::Malformed("height")));
    }

    #[test]
    fn aceptacion_monotona_con_conflictos_y_otra_cadena() {
        let (storage, _dir) = storage();
        let k = key(3);
        let pk = k.verifying_key();

        let cp100 = SignedCheckpoint::sign(&k, CHAIN, 100, &hash('a'));
        assert_eq!(accept_signed_checkpoint(&storage, CHAIN, &cp100, &pk), Ok(Acceptance::Accepted));
        assert_eq!(accept_signed_checkpoint(&storage, CHAIN, &cp100, &pk), Ok(Acceptance::AlreadyKnown));

        let cp130 = SignedCheckpoint::sign(&k, CHAIN, 130, &hash('b'));
        assert_eq!(accept_signed_checkpoint(&storage, CHAIN, &cp130, &pk), Ok(Acceptance::Accepted));
        assert_eq!(latest_signed_checkpoint(&storage).unwrap().unwrap(), cp130);

        // Uno viejo que ya estaba sigue siendo conocido; uno viejo nuevo es viejo.
        assert_eq!(accept_signed_checkpoint(&storage, CHAIN, &cp100, &pk), Ok(Acceptance::AlreadyKnown));
        let cp110 = SignedCheckpoint::sign(&k, CHAIN, 110, &hash('c'));
        assert!(matches!(
            accept_signed_checkpoint(&storage, CHAIN, &cp110, &pk),
            Err(CheckpointError::Stale { latest: 130, received: 110 })
        ));

        // La misma altura con otro hash: se conserva el primero.
        let conflicto = SignedCheckpoint::sign(&k, CHAIN, 130, &hash('d'));
        assert!(matches!(
            accept_signed_checkpoint(&storage, CHAIN, &conflicto, &pk),
            Err(CheckpointError::Conflict { height: 130, .. })
        ));
        assert_eq!(latest_signed_checkpoint(&storage).unwrap().unwrap(), cp130);

        // Otra cadena u otra clave no entran.
        let ajeno = SignedCheckpoint::sign(&k, "zoka-otra", 200, &hash('e'));
        assert!(matches!(
            accept_signed_checkpoint(&storage, CHAIN, &ajeno, &pk),
            Err(CheckpointError::WrongChain { .. })
        ));
        let falsificado = SignedCheckpoint::sign(&key(4), CHAIN, 200, &hash('e'));
        assert_eq!(
            accept_signed_checkpoint(&storage, CHAIN, &falsificado, &pk),
            Err(CheckpointError::BadSignature)
        );
        assert_eq!(latest_signed_checkpoint(&storage).unwrap().unwrap(), cp130);
    }

    #[test]
    fn el_firmador_firma_cada_intervalo_por_debajo_de_la_punta() {
        assert_eq!(next_height_to_sign(5, None), None);
        assert_eq!(next_height_to_sign(6, None), None);
        assert_eq!(next_height_to_sign(100, None), Some(94));
        assert_eq!(next_height_to_sign(100, Some(94)), None);
        assert_eq!(next_height_to_sign(123, Some(94)), None);
        assert_eq!(next_height_to_sign(130, Some(94)), Some(124));
    }

    #[test]
    fn viaja_por_la_red_y_se_sigue_verificando() {
        let k = key(6);
        let cp = SignedCheckpoint::sign(&k, CHAIN, 300, &hash('f'));
        let bytes = crate::network::message::ZokaMessage::SignedCheckpoint(cp.clone())
            .to_bytes()
            .unwrap();
        match crate::network::message::ZokaMessage::from_bytes(&bytes).unwrap() {
            crate::network::message::ZokaMessage::SignedCheckpoint(recibido) => {
                assert_eq!(recibido, cp);
                recibido.verify(&k.verifying_key()).unwrap();
            }
            otro => panic!("variante inesperada: {otro:?}"),
        }
    }

    #[test]
    fn la_clave_se_crea_una_vez_y_se_vuelve_a_leer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(AUTHORITY_KEY_FILE);
        let public = generate_signing_key(&path).unwrap();
        assert!(generate_signing_key(&path).is_err(), "no sobrescribe");
        let leida = load_signing_key(&path).unwrap();
        assert_eq!(hex::encode(leida.verifying_key().to_bytes()), public);
        assert_eq!(parse_public_key(&public), Some(leida.verifying_key()));
    }
}
