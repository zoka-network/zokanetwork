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

/// Versión 1: solo Ed25519.
pub const SIGNED_CHECKPOINT_VERSION: u8 = 1;
/// Versión 2: firma híbrida Ed25519 + ML-DSA-65 (etapa 6), las dos obligatorias.
pub const SIGNED_CHECKPOINT_HYBRID_VERSION: u8 = 2;
/// El firmador publica un checkpoint cada tantos bloques...
pub const AUTHORITY_SIGN_INTERVAL: u64 = 30;
/// ...y firma el bloque que está tantos por debajo de la punta, para no
/// fijar uno que una reorganización corta todavía pueda cambiar.
pub const AUTHORITY_CONFIRMATIONS: u64 = 6;
/// Nombre del archivo con la clave privada del equipo, dentro de la carpeta de
/// datos de la red. Si existe al arrancar, el nodo firma checkpoints.
pub const AUTHORITY_KEY_FILE: &str = "checkpoint-authority.key";
/// Semilla ML-DSA-65 del equipo (32 bytes en hex), junto a la Ed25519. Con
/// ella el nodo firma también checkpoints de la versión 2.
pub const AUTHORITY_MLDSA_KEY_FILE: &str = "checkpoint-authority-mldsa.key";
/// Reemplaza la clave pública compilada. Solo para redes locales de prueba.
pub const AUTHORITY_PUBKEY_ENV: &str = "ZOKA_CHECKPOINT_AUTHORITY_PUBKEY";
/// Igual, para la mitad ML-DSA (hex de 1.952 bytes).
pub const AUTHORITY_MLDSA_PUBKEY_ENV: &str = "ZOKA_CHECKPOINT_AUTHORITY_MLDSA_PUBKEY";
/// Largo en hex de la mitad ML-DSA de una firma de la versión 2.
#[cfg(feature = "pq-experimental")]
const MLDSA_SIGNATURE_HEX_LEN: usize = zoka_pq::ML_DSA_65_SIGNATURE_BYTES * 2;

const SIGNING_DOMAIN: &[u8] = b"zoka-signed-checkpoint-v1";
#[cfg(feature = "pq-experimental")]
const SIGNING_DOMAIN_V2: &[u8] = b"zoka-signed-checkpoint-v2";
const LATEST_KEY: &str = "checkpoint:signed:latest";
const BY_HEIGHT_PREFIX: &str = "checkpoint:signed:h:";
/// En el nodo de la autoridad: la última firma de la otra versión, la que su
/// propia política no guarda, para seguir reenviándola a los nodos que la usan.
const COMPAT_LATEST_KEY: &str = "checkpoint:signed:compat:latest";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedCheckpoint {
    pub version: u8,
    pub chain_id: String,
    pub height: u64,
    pub block_hash: String,
    /// Firma Ed25519 en hex (64 bytes). En la versión 2 es la mitad Ed25519.
    pub signature: String,
    /// Mitad ML-DSA-65 de la firma híbrida (versión 2), en hex. Un nodo viejo
    /// ignora el campo y rechaza la versión 2; nada más se rompe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mldsa_signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckpointError {
    /// La cadena no tiene clave de autoridad compilada ni configurada.
    NoAuthority(String),
    WrongChain { expected: String, actual: String },
    Version(u8),
    Malformed(&'static str),
    BadSignature,
    /// Esta cadena ya tiene la clave ML-DSA de la autoridad: solo acepta la
    /// versión 2.
    HybridRequired,
    /// Llegó una versión 2 y este nodo no conoce la mitad ML-DSA de la clave.
    NoHybridAuthority,
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
            Self::HybridRequired => {
                f.write_str("this chain only accepts hybrid (Ed25519 + ML-DSA-65) checkpoints")
            }
            Self::NoHybridAuthority => {
                f.write_str("hybrid checkpoint, but this node has no ML-DSA authority key")
            }
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

/// Las claves públicas de la autoridad de una cadena. Si se conoce la mitad
/// ML-DSA, la cadena solo acepta checkpoints híbridos: aceptar también los de
/// solo Ed25519 dejaría la puerta abierta a quien rompa Ed25519.
#[derive(Clone)]
pub struct AuthorityKeys {
    pub ed25519: VerifyingKey,
    #[cfg(feature = "pq-experimental")]
    pub mldsa: Option<zoka_pq::MlDsa65PublicKey>,
}

impl AuthorityKeys {
    pub fn ed25519_only(ed25519: VerifyingKey) -> Self {
        Self {
            ed25519,
            #[cfg(feature = "pq-experimental")]
            mldsa: None,
        }
    }

    pub fn requires_hybrid(&self) -> bool {
        #[cfg(feature = "pq-experimental")]
        {
            self.mldsa.is_some()
        }
        #[cfg(not(feature = "pq-experimental"))]
        {
            false
        }
    }

    #[cfg(feature = "pq-experimental")]
    pub fn hybrid(&self) -> Option<crate::hybrid_signature::HybridVerifyingKey> {
        self.mldsa.clone().map(|mldsa| {
            crate::hybrid_signature::HybridVerifyingKey::from_parts(self.ed25519, mldsa)
        })
    }
}

impl From<VerifyingKey> for AuthorityKeys {
    fn from(ed25519: VerifyingKey) -> Self {
        Self::ed25519_only(ed25519)
    }
}

impl fmt::Debug for AuthorityKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "AuthorityKeys(ed25519 {}, hybrid {})",
            hex::encode(self.ed25519.to_bytes()),
            self.requires_hybrid()
        )
    }
}

/// Lo firmado, sin el dominio: cadena, altura y hash. La versión 1 firma
/// `dominio v1 ‖ 0x1f ‖ esto` con Ed25519 (los mismos bytes de siempre); la 2,
/// `dominio v2 ‖ 0x1f ‖ esto` con las dos claves.
fn checkpoint_body(chain_id: &str, height: u64, block_hash: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(chain_id.len() + block_hash.len() + 10);
    body.extend_from_slice(chain_id.as_bytes());
    body.push(0x1f);
    body.extend_from_slice(&height.to_le_bytes());
    body.push(0x1f);
    body.extend_from_slice(block_hash.as_bytes());
    body
}

fn signing_message(chain_id: &str, height: u64, block_hash: &str) -> Vec<u8> {
    let body = checkpoint_body(chain_id, height, block_hash);
    let mut message = Vec::with_capacity(SIGNING_DOMAIN.len() + 1 + body.len());
    message.extend_from_slice(SIGNING_DOMAIN);
    message.push(0x1f);
    message.extend_from_slice(&body);
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
            mldsa_signature: None,
        }
    }

    /// Versión 2: las dos firmas sobre el mismo mensaje.
    #[cfg(feature = "pq-experimental")]
    pub fn sign_hybrid(
        key: &crate::hybrid_signature::HybridSigningKey,
        chain_id: &str,
        height: u64,
        block_hash: &str,
    ) -> Self {
        let signature = key
            .sign(SIGNING_DOMAIN_V2, &checkpoint_body(chain_id, height, block_hash))
            .expect("the checkpoint domain is a valid ML-DSA context");
        let (ed25519, mldsa) = signature.to_hex_parts();
        Self {
            version: SIGNED_CHECKPOINT_HYBRID_VERSION,
            chain_id: chain_id.to_string(),
            height,
            block_hash: block_hash.to_string(),
            signature: ed25519,
            mldsa_signature: Some(mldsa),
        }
    }

    /// Forma, sin la firma: lo que se puede revisar antes de saber la clave.
    pub fn validate_shape(&self) -> Result<(), CheckpointError> {
        match (self.version, &self.mldsa_signature) {
            (SIGNED_CHECKPOINT_VERSION, None) => {}
            (SIGNED_CHECKPOINT_VERSION, Some(_)) => {
                return Err(CheckpointError::Malformed("mldsa_signature"))
            }
            #[cfg(feature = "pq-experimental")]
            (SIGNED_CHECKPOINT_HYBRID_VERSION, Some(mldsa)) => {
                if mldsa.len() != MLDSA_SIGNATURE_HEX_LEN {
                    return Err(CheckpointError::Malformed("mldsa_signature"));
                }
            }
            #[cfg(feature = "pq-experimental")]
            (SIGNED_CHECKPOINT_HYBRID_VERSION, None) => {
                return Err(CheckpointError::Malformed("mldsa_signature"))
            }
            (other, _) => return Err(CheckpointError::Version(other)),
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

    pub fn verify(&self, authority: &AuthorityKeys) -> Result<(), CheckpointError> {
        self.validate_shape()?;
        if self.version == SIGNED_CHECKPOINT_VERSION {
            if authority.requires_hybrid() {
                return Err(CheckpointError::HybridRequired);
            }
            let bytes = hex::decode(&self.signature)
                .map_err(|_| CheckpointError::Malformed("signature"))?;
            let bytes: [u8; 64] = bytes
                .try_into()
                .map_err(|_| CheckpointError::Malformed("signature"))?;
            let signature = Signature::from_bytes(&bytes);
            return authority
                .ed25519
                .verify_strict(
                    &signing_message(&self.chain_id, self.height, &self.block_hash),
                    &signature,
                )
                .map_err(|_| CheckpointError::BadSignature);
        }
        #[cfg(feature = "pq-experimental")]
        {
            let hybrid = authority.hybrid().ok_or(CheckpointError::NoHybridAuthority)?;
            let mldsa = self
                .mldsa_signature
                .as_deref()
                .ok_or(CheckpointError::Malformed("mldsa_signature"))?;
            let signature =
                crate::hybrid_signature::HybridSignature::from_hex_parts(&self.signature, mldsa)
                    .map_err(|_| CheckpointError::Malformed("signature"))?;
            hybrid
                .verify(
                    SIGNING_DOMAIN_V2,
                    &checkpoint_body(&self.chain_id, self.height, &self.block_hash),
                    &signature,
                )
                .map_err(|_| CheckpointError::BadSignature)
        }
        #[cfg(not(feature = "pq-experimental"))]
        Err(CheckpointError::Version(self.version))
    }

    /// Verifica contra la clave de la cadena que el propio checkpoint declara.
    /// Es lo que puede hacer la capa de red, que no sabe qué cadena sigue el
    /// nodo; la aceptación vuelve a exigir que sea la cadena correcta.
    pub fn verify_for_declared_chain(&self) -> Result<(), CheckpointError> {
        let authority = authority_keys_for_chain(&self.chain_id)
            .ok_or_else(|| CheckpointError::NoAuthority(self.chain_id.clone()))?;
        self.verify(&authority)
    }
}

pub fn parse_public_key(hex_key: &str) -> Option<VerifyingKey> {
    let bytes = hex::decode(hex_key.trim()).ok()?;
    let bytes: [u8; 32] = bytes.try_into().ok()?;
    VerifyingKey::from_bytes(&bytes).ok()
}

/// Clave pública Ed25519 de la autoridad para `chain_id`: la de
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

/// Las dos claves de la autoridad de `chain_id`. La ML-DSA viene de
/// [`AUTHORITY_MLDSA_PUBKEY_ENV`] o de `economics::CHECKPOINT_AUTHORITY_MLDSA_KEYS`.
/// Si la variable está puesta pero no es una clave válida, no devuelve nada:
/// mejor no aceptar checkpoints que aceptar los de solo Ed25519 por un error.
pub fn authority_keys_for_chain(chain_id: &str) -> Option<AuthorityKeys> {
    let ed25519 = authority_key_for_chain(chain_id)?;
    #[cfg(feature = "pq-experimental")]
    {
        let configured = std::env::var(AUTHORITY_MLDSA_PUBKEY_ENV)
            .ok()
            .filter(|value| !value.trim().is_empty());
        let hex_key = match configured {
            Some(value) => Some(value),
            None => crate::economics::CHECKPOINT_AUTHORITY_MLDSA_KEYS
                .iter()
                .find(|(chain, _)| *chain == chain_id)
                .map(|(_, key)| key.to_string()),
        };
        let mldsa = match hex_key {
            Some(text) => Some(parse_mldsa_public_key(&text)?),
            None => None,
        };
        Some(AuthorityKeys { ed25519, mldsa })
    }
    #[cfg(not(feature = "pq-experimental"))]
    Some(AuthorityKeys::ed25519_only(ed25519))
}

#[cfg(feature = "pq-experimental")]
pub fn parse_mldsa_public_key(hex_key: &str) -> Option<zoka_pq::MlDsa65PublicKey> {
    let bytes = hex::decode(hex_key.trim()).ok()?;
    zoka_pq::MlDsa65PublicKey::from_bytes(&bytes).ok()
}

/// Lo que firma el nodo de la autoridad.
pub struct CheckpointSigner {
    pub ed25519: SigningKey,
    #[cfg(feature = "pq-experimental")]
    pub hybrid: Option<crate::hybrid_signature::HybridSigningKey>,
    /// Firma también la versión 1 para los nodos que todavía no conocen la
    /// clave ML-DSA (los de Zenith anteriores a la etapa 6).
    pub legacy_v1: bool,
}

impl CheckpointSigner {
    pub fn ed25519_only(ed25519: SigningKey) -> Self {
        Self {
            ed25519,
            #[cfg(feature = "pq-experimental")]
            hybrid: None,
            legacy_v1: true,
        }
    }

    /// Las versiones que publica. Primero la que acepta la política de esta
    /// cadena (la que el propio nodo guarda), después la de compatibilidad.
    pub fn sign_all(
        &self,
        authority: &AuthorityKeys,
        chain_id: &str,
        height: u64,
        block_hash: &str,
    ) -> Vec<SignedCheckpoint> {
        let mut out = Vec::new();
        #[cfg(feature = "pq-experimental")]
        if let Some(hybrid) = &self.hybrid {
            out.push(SignedCheckpoint::sign_hybrid(hybrid, chain_id, height, block_hash));
        }
        if self.legacy_v1 || !authority.requires_hybrid() {
            let v1 = SignedCheckpoint::sign(&self.ed25519, chain_id, height, block_hash);
            if authority.requires_hybrid() {
                out.push(v1);
            } else {
                out.insert(0, v1);
            }
        }
        out
    }
}

impl fmt::Debug for CheckpointSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CheckpointSigner([REDACTED])")
    }
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

/// En el nodo de la autoridad: guarda la firma de la versión que su política
/// no acepta, para reenviarla junto con la vigente.
pub fn store_compat_checkpoint(
    storage: &RocksDbStorage,
    checkpoint: &SignedCheckpoint,
) -> Result<(), CheckpointError> {
    let bytes = serde_json::to_vec(checkpoint).map_err(|e| CheckpointError::Storage(e.to_string()))?;
    storage
        .put_raw(COMPAT_LATEST_KEY, bytes)
        .map_err(|e| CheckpointError::Storage(e.to_string()))
}

pub fn latest_compat_checkpoint(
    storage: &RocksDbStorage,
) -> Result<Option<SignedCheckpoint>, CheckpointError> {
    read(storage, COMPAT_LATEST_KEY)
}

/// Verifica `checkpoint` contra `authority` y, si corresponde, lo guarda como
/// el vigente. Reglas: la cadena tiene que ser `chain_id`; un checkpoint más
/// viejo que el vigente no lo reemplaza; la misma altura con otro hash es un
/// conflicto y se conserva el primero.
pub fn accept_signed_checkpoint(
    storage: &RocksDbStorage,
    chain_id: &str,
    checkpoint: &SignedCheckpoint,
    authority: &AuthorityKeys,
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
        cp.verify(&k.verifying_key().into()).unwrap();

        let mut otra_altura = cp.clone();
        otra_altura.height = 101;
        assert_eq!(otra_altura.verify(&k.verifying_key().into()), Err(CheckpointError::BadSignature));

        let mut otro_hash = cp.clone();
        otro_hash.block_hash = hash('b');
        assert_eq!(otro_hash.verify(&k.verifying_key().into()), Err(CheckpointError::BadSignature));

        let mut otra_cadena = cp.clone();
        otra_cadena.chain_id = "zoka-otra".to_string();
        assert_eq!(otra_cadena.verify(&k.verifying_key().into()), Err(CheckpointError::BadSignature));

        assert_eq!(cp.verify(&key(2).verifying_key().into()), Err(CheckpointError::BadSignature));
    }

    #[test]
    fn la_forma_se_revisa_antes_de_la_firma() {
        let k = key(1);
        let mut cp = SignedCheckpoint::sign(&k, CHAIN, 100, &hash('a'));
        cp.version = 3;
        assert_eq!(cp.validate_shape(), Err(CheckpointError::Version(3)));
        cp.version = 2;
        // Una versión 2 sin la mitad ML-DSA está mal formada (o, sin X-Wing ni
        // ML-DSA en la build, es una versión que no se entiende).
        #[cfg(feature = "pq-experimental")]
        assert_eq!(cp.validate_shape(), Err(CheckpointError::Malformed("mldsa_signature")));
        #[cfg(not(feature = "pq-experimental"))]
        assert_eq!(cp.validate_shape(), Err(CheckpointError::Version(2)));
        let mut cp = SignedCheckpoint::sign(&k, CHAIN, 100, &hash('a'));
        cp.mldsa_signature = Some("00".repeat(10));
        assert_eq!(cp.validate_shape(), Err(CheckpointError::Malformed("mldsa_signature")));
        let mut cp = SignedCheckpoint::sign(&k, CHAIN, 100, "ABC");
        assert_eq!(cp.validate_shape(), Err(CheckpointError::Malformed("block_hash")));
        cp = SignedCheckpoint::sign(&k, CHAIN, 0, &hash('a'));
        assert_eq!(cp.validate_shape(), Err(CheckpointError::Malformed("height")));
    }

    #[test]
    fn aceptacion_monotona_con_conflictos_y_otra_cadena() {
        let (storage, _dir) = storage();
        let k = key(3);
        let pk: AuthorityKeys = k.verifying_key().into();

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
                recibido.verify(&k.verifying_key().into()).unwrap();
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

    #[cfg(feature = "pq-experimental")]
    fn hybrid(byte: u8) -> crate::hybrid_signature::HybridSigningKey {
        crate::hybrid_signature::HybridSigningKey::from_parts(key(byte), &[byte ^ 0x33; 32]).unwrap()
    }

    #[cfg(feature = "pq-experimental")]
    fn hybrid_authority(byte: u8) -> AuthorityKeys {
        let public = hybrid(byte).verifying_key();
        AuthorityKeys {
            ed25519: *public.ed25519(),
            mldsa: Some(public.mldsa().clone()),
        }
    }

    #[cfg(feature = "pq-experimental")]
    #[test]
    fn un_checkpoint_hibrido_exige_las_dos_firmas() {
        let k = hybrid(7);
        let authority = hybrid_authority(7);
        let cp = SignedCheckpoint::sign_hybrid(&k, CHAIN, 120, &hash('a'));
        assert_eq!(cp.version, SIGNED_CHECKPOINT_HYBRID_VERSION);
        cp.verify(&authority).unwrap();

        // Alterar cualquiera de las dos mitades, o lo firmado, lo invalida.
        let mut ed = cp.clone();
        ed.signature = SignedCheckpoint::sign(&key(7), CHAIN, 120, &hash('a')).signature;
        assert_eq!(ed.verify(&authority), Err(CheckpointError::BadSignature));
        let mut ml = cp.clone();
        ml.mldsa_signature = SignedCheckpoint::sign_hybrid(&hybrid(8), CHAIN, 120, &hash('a')).mldsa_signature;
        assert_eq!(ml.verify(&authority), Err(CheckpointError::BadSignature));
        let mut altura = cp.clone();
        altura.height = 121;
        assert_eq!(altura.verify(&authority), Err(CheckpointError::BadSignature));

        // La firma Ed25519 de un v2 no sirve como v1: el dominio es otro.
        let mut como_v1 = cp.clone();
        como_v1.version = SIGNED_CHECKPOINT_VERSION;
        como_v1.mldsa_signature = None;
        assert_eq!(
            como_v1.verify(&key(7).verifying_key().into()),
            Err(CheckpointError::BadSignature)
        );
    }

    #[cfg(feature = "pq-experimental")]
    #[test]
    fn con_la_clave_ml_dsa_compilada_ya_no_vale_la_version_1() {
        let k = hybrid(9);
        let v1 = SignedCheckpoint::sign(k.ed25519(), CHAIN, 150, &hash('b'));
        // Un nodo que conoce la mitad ML-DSA rechaza la v1 aunque la firma Ed25519 sea buena...
        assert_eq!(v1.verify(&hybrid_authority(9)), Err(CheckpointError::HybridRequired));
        // ...y uno que no la conoce no puede verificar una v2.
        let v2 = SignedCheckpoint::sign_hybrid(&k, CHAIN, 150, &hash('b'));
        assert_eq!(
            v2.verify(&k.ed25519().verifying_key().into()),
            Err(CheckpointError::NoHybridAuthority)
        );
        v1.verify(&k.ed25519().verifying_key().into()).unwrap();
    }

    #[cfg(feature = "pq-experimental")]
    #[test]
    fn el_firmador_publica_las_dos_versiones_mientras_haya_nodos_viejos() {
        let authority = hybrid_authority(5);
        let signer = CheckpointSigner {
            ed25519: key(5),
            hybrid: Some(hybrid(5)),
            legacy_v1: true,
        };
        let all = signer.sign_all(&authority, CHAIN, 180, &hash('c'));
        assert_eq!(all.len(), 2);
        // Primero la que acepta la política de la cadena: la que guarda el nodo.
        assert_eq!(all[0].version, SIGNED_CHECKPOINT_HYBRID_VERSION);
        all[0].verify(&authority).unwrap();
        all[1].verify(&key(5).verifying_key().into()).unwrap();
        // Una cadena nueva, sin nodos viejos: solo la híbrida.
        let quantic = CheckpointSigner {
            legacy_v1: false,
            ..signer
        };
        let only = quantic.sign_all(&authority, CHAIN, 180, &hash('c'));
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].version, SIGNED_CHECKPOINT_HYBRID_VERSION);
        // Sin la clave ML-DSA en la cadena: la v1 primero.
        let v1_first = CheckpointSigner::ed25519_only(key(5)).sign_all(&key(5).verifying_key().into(), CHAIN, 180, &hash('c'));
        assert_eq!(v1_first.len(), 1);
        assert_eq!(v1_first[0].version, SIGNED_CHECKPOINT_VERSION);
    }

    #[cfg(feature = "pq-experimental")]
    #[test]
    fn un_hibrido_viaja_por_la_red_y_un_nodo_viejo_lo_ignora_sin_romperse() {
        let k = hybrid(6);
        let cp = SignedCheckpoint::sign_hybrid(&k, CHAIN, 300, &hash('f'));
        let bytes = crate::network::message::ZokaMessage::SignedCheckpoint(cp.clone())
            .to_bytes()
            .unwrap();
        // Lo que ve un nodo de antes de la etapa 6: el mismo JSON sin el campo nuevo.
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(value["SignedCheckpoint"]["mldsa_signature"].is_string());
        match crate::network::message::ZokaMessage::from_bytes(&bytes).unwrap() {
            crate::network::message::ZokaMessage::SignedCheckpoint(recibido) => {
                assert_eq!(recibido, cp);
                recibido.verify(&hybrid_authority(6)).unwrap();
            }
            otro => panic!("variante inesperada: {otro:?}"),
        }
        // Un v1 sigue serializándose exactamente como antes (sin el campo).
        let v1 = SignedCheckpoint::sign(&key(6), CHAIN, 300, &hash('f'));
        let json = serde_json::to_string(&v1).unwrap();
        assert!(!json.contains("mldsa_signature"));
    }

    #[cfg(feature = "pq-experimental")]
    #[test]
    fn la_compatibilidad_se_guarda_aparte() {
        let (storage, _dir) = storage();
        assert_eq!(latest_compat_checkpoint(&storage).unwrap(), None);
        let v1 = SignedCheckpoint::sign(&key(2), CHAIN, 60, &hash('a'));
        store_compat_checkpoint(&storage, &v1).unwrap();
        assert_eq!(latest_compat_checkpoint(&storage).unwrap(), Some(v1));
        assert_eq!(latest_signed_checkpoint(&storage).unwrap(), None);
    }
}
