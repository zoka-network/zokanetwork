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

pub mod protection;

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
/// Contraseña de las claves de autoridad guardadas cifradas (`seal_key_file`).
/// No hace falta mientras los archivos estén en hex, como se crean.
pub const AUTHORITY_PASSWORD_ENV: &str = "ZOKA_AUTHORITY_PASSWORD";
/// Qué mitad de la clave guarda un archivo cifrado.
pub const SEALED_ED25519: &str = "ed25519";
pub const SEALED_MLDSA: &str = "ml-dsa-65";
const SEALED_KEY_SCHEMA: &str = "zoka-authority-key-v1";
/// Sufijo de la copia en claro que deja `seal_key_file`: la borra quien cifró,
/// después de comprobar que el nodo firma con el archivo nuevo.
pub const PLAIN_BACKUP_SUFFIX: &str = ".plain-backup";
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
        authority_policy(chain_id, AuthorityKeys { ed25519, mldsa })
    }
    #[cfg(not(feature = "pq-experimental"))]
    authority_policy(chain_id, AuthorityKeys::ed25519_only(ed25519))
}

/// Zoka Quantic exige firmas híbridas desde el génesis: una cadena de Quantic
/// sin la mitad ML-DSA de la clave no tiene autoridad (rige solo el trabajo),
/// en vez de aceptar checkpoints de solo Ed25519, que una computadora cuántica
/// podría falsificar. Zenith acepta los dos hasta que se compile su ML-DSA.
pub fn authority_policy(chain_id: &str, keys: AuthorityKeys) -> Option<AuthorityKeys> {
    if crate::economics::is_quantic_chain_id(chain_id) && !keys.requires_hybrid() {
        return None;
    }
    Some(keys)
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

/// Lee la clave privada de la autoridad: 32 bytes en hex, o cifrada con la
/// contraseña de `ZOKA_AUTHORITY_PASSWORD` (`seal_key_file`).
pub fn load_signing_key(path: &Path) -> Result<SigningKey, String> {
    let seed = read_key_seed(path, SEALED_ED25519, "checkpoint authority key")?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Un archivo de clave cifrado. El formato del cifrado es el de la identidad
/// del nodo (`wallet::storage::encrypt_data`: Argon2id y AES-256-GCM).
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SealedKeyFile {
    schema: String,
    /// `SEALED_ED25519` o `SEALED_MLDSA`: para no tomar una mitad por la otra.
    key: String,
    encrypted_seed_hex: String,
}

/// Si el archivo de clave está cifrado. Un archivo en hex no lo está.
pub fn key_file_is_sealed(path: &Path) -> Result<bool, String> {
    let mut text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let sealed = text.trim_start().starts_with('{');
    text.zeroize();
    Ok(sealed)
}

/// Los 32 bytes secretos de un archivo de clave de la autoridad, con la
/// contraseña del entorno si está cifrado. `what` nombra la clave en los
/// errores; `kind` es la mitad que se espera.
pub(crate) fn read_key_seed(
    path: &Path,
    kind: &str,
    what: &str,
) -> Result<zeroize::Zeroizing<[u8; 32]>, String> {
    let password = std::env::var(AUTHORITY_PASSWORD_ENV)
        .ok()
        .map(zeroize::Zeroizing::new);
    read_key_seed_with(path, kind, what, password.as_ref().map(|p| p.as_str()))
}

/// Como `read_key_seed`, con la contraseña dada. Un archivo en hex se lee
/// igual que siempre y no la mira.
pub(crate) fn read_key_seed_with(
    path: &Path,
    kind: &str,
    what: &str,
    password: Option<&str>,
) -> Result<zeroize::Zeroizing<[u8; 32]>, String> {
    let mut text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {what} {}: {e}", path.display()))?;
    let result = if text.trim_start().starts_with('{') {
        if protection::is_user_protected(text.trim()) {
            protection::open_user_key(text.trim(), kind)
        } else {
            open_sealed_key(text.trim(), kind, what, password)
        }
    } else {
        let decoded = hex::decode(text.trim());
        decoded
            .map_err(|_| format!("{what} is not hex"))
            .and_then(|mut bytes| {
                let seed = <[u8; 32]>::try_from(bytes.as_slice())
                    .map(zeroize::Zeroizing::new)
                    .map_err(|_| format!("{what} must be 32 bytes"));
                bytes.zeroize();
                seed
            })
    };
    text.zeroize();
    result
}

fn open_sealed_key(
    text: &str,
    kind: &str,
    what: &str,
    password: Option<&str>,
) -> Result<zeroize::Zeroizing<[u8; 32]>, String> {
    let file: SealedKeyFile = serde_json::from_str(text)
        .map_err(|_| format!("{what} is neither hex nor an encrypted key file"))?;
    if file.schema != SEALED_KEY_SCHEMA {
        return Err(format!("{what}: unknown encrypted key format"));
    }
    if file.key != kind {
        return Err(format!(
            "{what}: this file holds the {} half of the authority key",
            file.key
        ));
    }
    let password = password
        .filter(|p| !p.is_empty())
        .ok_or_else(|| format!("{what} is encrypted: set {AUTHORITY_PASSWORD_ENV}"))?;
    let encrypted = hex::decode(&file.encrypted_seed_hex)
        .map_err(|_| format!("{what}: damaged encrypted key file"))?;
    // Ocho bytes de marca, sal de 16, nonce de 12, la semilla de 32 y la
    // etiqueta GCM de 16. `decrypt_data` también lee un formato viejo sin
    // autenticar: acá no se llega a él.
    if encrypted.len() != 84 || !encrypted.starts_with(b"ZOKAENC1") {
        return Err(format!("{what}: damaged encrypted key file"));
    }
    let plain = zeroize::Zeroizing::new(
        crate::wallet::storage::decrypt_data(&encrypted, password)
            .map_err(|_| format!("{what}: wrong password or damaged file"))?,
    );
    <[u8; 32]>::try_from(plain.as_slice())
        .map(zeroize::Zeroizing::new)
        .map_err(|_| format!("{what} must be 32 bytes"))
}

/// Cifra en su lugar un archivo de clave que está en hex. Devuelve la ruta
/// de la copia en claro que deja al lado (`PLAIN_BACKUP_SUFFIX`).
///
/// Perder esta clave obliga a sacar binarios nuevos, así que no se borra
/// nada: el archivo cifrado se escribe aparte, se vuelve a leer con la
/// contraseña y recién entonces ocupa el lugar del original, que queda con
/// otro nombre. Esa copia en claro la borra quien cifró, cuando haya visto al
/// nodo firmar con el archivo nuevo.
pub fn seal_key_file(path: &Path, kind: &str, what: &str, password: &str) -> Result<std::path::PathBuf, String> {
    if password.is_empty() {
        return Err(format!("{AUTHORITY_PASSWORD_ENV} cannot be empty"));
    }
    if key_file_is_sealed(path)? {
        return Err(format!("{} is already encrypted", path.display()));
    }
    let seed = read_key_seed_with(path, kind, what, None)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{} has no file name", path.display()))?;
    let backup = path.with_file_name(format!("{name}{PLAIN_BACKUP_SUFFIX}"));
    let staged = path.with_file_name(format!("{name}.encrypting"));
    if backup.exists() || staged.exists() {
        return Err(format!(
            "{} or {} already exists; nothing was changed",
            backup.display(),
            staged.display()
        ));
    }
    let encrypted = crate::wallet::storage::encrypt_data(seed.as_ref(), password)
        .map_err(|e| format!("cannot encrypt {what}: {e}"))?;
    let file = SealedKeyFile {
        schema: SEALED_KEY_SCHEMA.into(),
        key: kind.into(),
        encrypted_seed_hex: hex::encode(encrypted),
    };
    let json = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)
        .and_then(|mut out| {
            std::io::Write::write_all(&mut out, &json)?;
            out.sync_all()
        })
        .map_err(|e| format!("cannot write {}: {e}", staged.display()))?;
    // Lo que quedó en el disco tiene que abrir la misma semilla.
    let reread = read_key_seed_with(&staged, kind, what, Some(password));
    if reread.as_ref().map(|s| **s == *seed).unwrap_or(false) {
        std::fs::rename(path, &backup)
            .map_err(|e| format!("cannot move {} aside: {e}; nothing was changed", path.display()))?;
        if let Err(e) = std::fs::rename(&staged, path) {
            // Volver a como estaba: la clave en claro en su lugar.
            let restored = std::fs::rename(&backup, path);
            return Err(format!(
                "cannot put the encrypted key in place: {e}; the original key is {}",
                if restored.is_ok() {
                    "back in place".to_string()
                } else {
                    format!("at {}", backup.display())
                }
            ));
        }
        Ok(backup)
    } else {
        Err(format!(
            "the encrypted copy at {} does not read back; the original key was not touched",
            staged.display()
        ))
    }
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

    #[test]
    fn una_clave_cifrada_se_lee_con_su_contrasena_y_solo_con_ella() {
        const QUE: &str = "checkpoint authority key";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(AUTHORITY_KEY_FILE);
        let public = generate_signing_key(&path).unwrap();
        let en_hex = std::fs::read_to_string(&path).unwrap();
        let semilla = read_key_seed_with(&path, SEALED_ED25519, QUE, None).unwrap();
        // En hex se lee como siempre: la contraseña ni se mira.
        let igual = read_key_seed_with(&path, SEALED_ED25519, QUE, Some("x")).unwrap();
        assert_eq!(*igual, *semilla);
        assert!(!key_file_is_sealed(&path).unwrap());
        // Una contraseña vacía no cifra nada.
        assert!(seal_key_file(&path, SEALED_ED25519, QUE, "").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), en_hex);

        let copia = seal_key_file(&path, SEALED_ED25519, QUE, "frase larga").unwrap();
        // La clave en claro quedó al lado, intacta, y ya no está en el archivo.
        assert_eq!(std::fs::read_to_string(&copia).unwrap(), en_hex);
        assert!(copia.to_string_lossy().ends_with(PLAIN_BACKUP_SUFFIX));
        assert!(key_file_is_sealed(&path).unwrap());
        let cifrado = std::fs::read_to_string(&path).unwrap();
        assert!(!cifrado.contains(en_hex.trim()));
        let abierta = read_key_seed_with(&path, SEALED_ED25519, QUE, Some("frase larga")).unwrap();
        assert_eq!(*abierta, *semilla);
        assert_eq!(
            hex::encode(SigningKey::from_bytes(&abierta).verifying_key().to_bytes()),
            public
        );

        // Sin contraseña, con otra o tomada por la otra mitad, no abre.
        let sin = read_key_seed_with(&path, SEALED_ED25519, QUE, None).unwrap_err();
        assert!(sin.contains(AUTHORITY_PASSWORD_ENV), "{sin}");
        let otra = read_key_seed_with(&path, SEALED_ED25519, QUE, Some("otra frase")).unwrap_err();
        assert!(otra.contains("wrong password"), "{otra}");
        let mitad = read_key_seed_with(&path, SEALED_MLDSA, QUE, Some("frase larga")).unwrap_err();
        assert!(mitad.contains("half"), "{mitad}");
        // No se cifra dos veces.
        assert!(seal_key_file(&path, SEALED_ED25519, QUE, "frase larga").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), cifrado);

        // Un solo dígito cambiado y el cifrado autenticado lo nota.
        let mut archivo: serde_json::Value = serde_json::from_str(&cifrado).unwrap();
        let hexa = archivo["encrypted_seed_hex"].as_str().unwrap().to_string();
        let ultimo = if hexa.ends_with('0') { '1' } else { '0' };
        archivo["encrypted_seed_hex"] = format!("{}{ultimo}", &hexa[..hexa.len() - 1]).into();
        std::fs::write(&path, serde_json::to_vec(&archivo).unwrap()).unwrap();
        let roto = read_key_seed_with(&path, SEALED_ED25519, QUE, Some("frase larga")).unwrap_err();
        assert!(roto.contains("damaged"), "{roto}");
        // Ni un archivo cortado cae a otro formato.
        archivo["encrypted_seed_hex"] = hexa[..hexa.len() - 2].into();
        std::fs::write(&path, serde_json::to_vec(&archivo).unwrap()).unwrap();
        assert!(read_key_seed_with(&path, SEALED_ED25519, QUE, Some("frase larga")).is_err());
    }

    #[test]
    fn cifrar_no_pisa_una_copia_en_claro_que_ya_existe() {
        const QUE: &str = "checkpoint authority key";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(AUTHORITY_KEY_FILE);
        generate_signing_key(&path).unwrap();
        let en_hex = std::fs::read_to_string(&path).unwrap();
        let copia = dir
            .path()
            .join(format!("{AUTHORITY_KEY_FILE}{PLAIN_BACKUP_SUFFIX}"));
        std::fs::write(&copia, "otra cosa").unwrap();
        let error = seal_key_file(&path, SEALED_ED25519, QUE, "frase larga").unwrap_err();
        assert!(error.contains("nothing was changed"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), en_hex);
        assert_eq!(std::fs::read_to_string(&copia).unwrap(), "otra cosa");
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
    fn quantic_no_tiene_autoridad_sin_la_mitad_ml_dsa() {
        use crate::economics::{MAINNET_CHAIN_ID, QUANTIC_MAINNET_CHAIN_ID, QUANTIC_TESTNET_CHAIN_ID};
        let solo_ed25519 = AuthorityKeys::ed25519_only(key(1).verifying_key());
        for quantic in [QUANTIC_MAINNET_CHAIN_ID, QUANTIC_TESTNET_CHAIN_ID] {
            assert!(authority_policy(quantic, solo_ed25519.clone()).is_none(), "{quantic}");
            let hibrida = authority_policy(quantic, hybrid_authority(2)).expect("hybrid authority");
            assert!(hibrida.requires_hybrid());
            // Y entonces solo vale la versión 2: un checkpoint de solo Ed25519 no.
            let v1 = SignedCheckpoint::sign(&key(2), quantic, 30, &hash('a'));
            assert!(v1.verify(&hibrida).is_err());
            SignedCheckpoint::sign_hybrid(&hybrid(2), quantic, 30, &hash('a'))
                .verify(&hibrida)
                .unwrap();
        }
        // Zenith sigue aceptando la clave de solo Ed25519 hasta que se compile su ML-DSA.
        assert!(authority_policy(MAINNET_CHAIN_ID, solo_ed25519).is_some());
    }

    /// Atacante cuántico simulado (docs/QUANTIC-INVENTARIO-CRIPTOGRAFICO.md):
    /// con Shor saca la clave Ed25519 de la autoridad de su pública. Arma una
    /// clave híbrida con esa mitad verdadera y una ML-DSA propia, y firma un
    /// checkpoint que a él le conviene. Ningún nodo de Quantic lo acepta.
    #[cfg(feature = "pq-experimental")]
    #[test]
    fn un_atacante_cuantico_con_la_clave_ed25519_no_mueve_los_checkpoints_de_quantic() {
        use crate::economics::{QUANTIC_MAINNET_CHAIN_ID, QUANTIC_TESTNET_CHAIN_ID};
        let robada = key(2);
        let atacante =
            crate::hybrid_signature::HybridSigningKey::from_parts(robada.clone(), &[0xee; 32]).unwrap();
        for quantic in [QUANTIC_MAINNET_CHAIN_ID, QUANTIC_TESTNET_CHAIN_ID] {
            let autoridad = authority_policy(quantic, hybrid_authority(2)).expect("hybrid authority");
            let (storage, _dir) = storage();
            let v2 = SignedCheckpoint::sign_hybrid(&atacante, quantic, 40, &hash('e'));
            assert_eq!(v2.verify(&autoridad), Err(CheckpointError::BadSignature), "{quantic}");
            assert_eq!(
                accept_signed_checkpoint(&storage, quantic, &v2, &autoridad),
                Err(CheckpointError::BadSignature)
            );
            let v1 = SignedCheckpoint::sign(&robada, quantic, 40, &hash('e'));
            assert_eq!(v1.verify(&autoridad), Err(CheckpointError::HybridRequired), "{quantic}");
            assert_eq!(latest_signed_checkpoint(&storage).unwrap(), None);
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
