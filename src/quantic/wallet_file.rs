//! Archivos de wallet de Zoka Quantic.
//!
//! Una wallet es la entropía de su frase de recuperación (24 palabras BIP-39,
//! 256 bits: frente a una búsqueda con Grover equivale a 128 bits, ver la etapa
//! 3), cifrada con la contraseña con el mismo esquema que las wallets de Zenith
//! (Argon2 + AES-256-GCM). La dirección va en claro, para mostrarla sin pedir
//! la contraseña. Todo lo demás (claves, notas) sale de la entropía.

use std::path::{Path, PathBuf};

use bip39::{Language, Mnemonic};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;
use zoka_quantic::{Network, QuanticSpendingKey};

pub const WALLET_SCHEMA: &str = "zoka-quantic-wallet-v1";
pub const WALLETS_DIR: &str = "quantic-wallets";
pub const ENTROPY_BYTES: usize = 32;

#[derive(Serialize, Deserialize)]
struct WalletFile {
    schema: String,
    chain_id: String,
    name: String,
    address: String,
    encrypted_entropy_hex: String,
    created_unix_secs: u64,
}

pub fn wallets_dir(local_dir: &str) -> PathBuf {
    PathBuf::from(local_dir).join(WALLETS_DIR)
}

fn wallet_path(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        return Err("wallet name: 1 to 64 letters, digits, '-' or '_'".to_string());
    }
    Ok(dir.join(format!("{name}.json")))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn write_new(dir: &Path, name: &str, network: Network, entropy: &[u8], password: &str) -> Result<String, String> {
    let path = wallet_path(dir, name)?;
    if path.exists() {
        return Err(format!("{} already exists; refusing to overwrite a wallet", path.display()));
    }
    let key = QuanticSpendingKey::from_root(entropy, network, 0).map_err(|e| e.to_string())?;
    let address = key.full_viewing_key().address().encode();
    let encrypted = crate::wallet::storage::encrypt_data(entropy, password).map_err(|e| e.to_string())?;
    let file = WalletFile {
        schema: WALLET_SCHEMA.to_string(),
        chain_id: network.chain_id().to_string(),
        name: name.to_string(),
        address: address.clone(),
        encrypted_entropy_hex: hex::encode(encrypted),
        created_unix_secs: now_secs(),
    };
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, &json))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(address)
}

/// Crea una wallet nueva. Devuelve la frase (secreta: se muestra una vez) y
/// la dirección.
pub fn create(dir: &Path, name: &str, network: Network, password: &str) -> Result<(Zeroizing<String>, String), String> {
    let mut entropy = Zeroizing::new([0u8; ENTROPY_BYTES]);
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, entropy.as_mut());
    let phrase = Mnemonic::from_entropy_in(Language::English, entropy.as_ref())
        .map_err(|e| e.to_string())?
        .to_string();
    let address = write_new(dir, name, network, entropy.as_ref(), password)?;
    Ok((Zeroizing::new(phrase), address))
}

/// Restaura una wallet desde su frase (12 o 24 palabras).
pub fn restore(dir: &Path, name: &str, network: Network, phrase: &str, password: &str) -> Result<String, String> {
    let mnemonic = Mnemonic::parse_in_normalized(Language::English, phrase.trim())
        .map_err(|e| format!("invalid recovery phrase: {e}"))?;
    let entropy = Zeroizing::new(mnemonic.to_entropy());
    write_new(dir, name, network, &entropy, password)
}

/// Solo para regtest: la wallet del premine, cuya raíz es pública a propósito.
pub fn import_regtest_team(dir: &Path, name: &str, password: &str) -> Result<String, String> {
    write_new(dir, name, Network::Regtest, &super::REGTEST_TEAM_ROOT, password)
}

fn read(dir: &Path, name: &str) -> Result<WalletFile, String> {
    let path = wallet_path(dir, name)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let file: WalletFile = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if file.schema != WALLET_SCHEMA {
        return Err(format!("{}: unknown wallet schema {}", path.display(), file.schema));
    }
    Ok(file)
}

/// La dirección guardada, sin pedir la contraseña.
pub fn address(dir: &Path, name: &str) -> Result<String, String> {
    read(dir, name).map(|file| file.address)
}

/// Abre la wallet con su contraseña.
pub fn open(dir: &Path, name: &str, network: Network, password: &str) -> Result<QuanticSpendingKey, String> {
    let file = read(dir, name)?;
    if file.chain_id != network.chain_id() {
        return Err(format!("wallet {name} is for {}, not {}", file.chain_id, network.chain_id()));
    }
    let encrypted = hex::decode(&file.encrypted_entropy_hex).map_err(|_| "corrupt wallet file".to_string())?;
    let entropy = Zeroizing::new(
        crate::wallet::storage::decrypt_data(&encrypted, password).map_err(|_| "wrong password".to_string())?,
    );
    let key = QuanticSpendingKey::from_root(&entropy, network, 0).map_err(|e| e.to_string())?;
    if key.full_viewing_key().address().encode() != file.address {
        return Err("the wallet file does not match its address".to_string());
    }
    Ok(key)
}

pub fn list(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().to_str().and_then(|n| n.strip_suffix(".json")).map(str::to_string))
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wallet_is_created_restored_and_opened_only_with_its_password() {
        let dir = tempfile::tempdir().unwrap();
        let (phrase, address) = create(dir.path(), "main", Network::Regtest, "clave buena").unwrap();
        assert_eq!(phrase.split_whitespace().count(), 24);
        assert!(address.starts_with("zqregtest1"));
        assert_eq!(super::address(dir.path(), "main").unwrap(), address);
        let key = open(dir.path(), "main", Network::Regtest, "clave buena").unwrap();
        assert_eq!(key.full_viewing_key().address().encode(), address);
        assert_eq!(open(dir.path(), "main", Network::Regtest, "otra").unwrap_err(), "wrong password");
        assert!(open(dir.path(), "main", Network::Testnet, "clave buena").is_err());
        assert!(create(dir.path(), "main", Network::Regtest, "x").is_err(), "never overwrites");

        // La misma frase en otra wallet da la misma dirección.
        let restored = restore(dir.path(), "copia", Network::Regtest, &phrase, "otra clave").unwrap();
        assert_eq!(restored, address);
        assert_eq!(list(dir.path()), vec!["copia".to_string(), "main".to_string()]);
        assert!(wallet_path(dir.path(), "../afuera").is_err());
    }

    #[test]
    fn the_regtest_team_wallet_receives_the_regtest_premine() {
        let dir = tempfile::tempdir().unwrap();
        let address = import_regtest_team(dir.path(), "team", "x").unwrap();
        let premine = super::super::consensus_params(Network::Regtest).premine.unwrap();
        assert_eq!(address, premine.recipient.encode());
    }
}
