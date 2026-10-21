//! Archivos de wallet de Zoka Quantic.
//!
//! Una wallet es la entropía de su frase de recuperación (24 palabras BIP-39,
//! 256 bits: frente a una búsqueda con Grover equivale a 128 bits, ver la etapa
//! 3), cifrada con la contraseña con el mismo esquema que las wallets de Zenith
//! (Argon2 + AES-256-GCM). La dirección va en claro, para mostrarla sin pedir
//! la contraseña. Todo lo demás (claves, notas) sale de la entropía.
//!
//! Al lado de cada wallet, `<nombre>.scan` guarda hasta dónde la escaneó
//! (ver `wallet_sync::resume_wallet`), para no volver a recorrer la cadena desde
//! el génesis en cada consulta. Va cifrado con AES-256-GCM y una clave que sale
//! de la entropía: abrirlo pide la contraseña (que abre la entropía) pero no
//! otro Argon2, y sigue valiendo si cambia la contraseña. Si se pierde o se
//! rompe, solo cuesta volver a escanear.

use std::path::{Path, PathBuf};

use bip39::{Language, Mnemonic};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;
use zoka_quantic::{Network, QuanticSpendingKey};

use super::wallet_sync::SavedScan;

pub const WALLET_SCHEMA: &str = "zoka-quantic-wallet-v1";
pub const SCAN_SCHEMA: &str = "zoka-quantic-scan-v1";
pub const WALLETS_DIR: &str = "quantic-wallets";
pub const ENTROPY_BYTES: usize = 32;
const SCAN_KEY_CONTEXT: &str = "zoka-quantic 2026-10-21 wallet scan cache v1";

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

/// Una frase nueva de 24 palabras, sin crear ninguna wallet: para mostrarla y
/// confirmarla antes, y crear la wallet después con [`restore`].
pub fn generate_phrase() -> Result<Zeroizing<String>, String> {
    let mut entropy = Zeroizing::new([0u8; ENTROPY_BYTES]);
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, entropy.as_mut());
    let phrase = Mnemonic::from_entropy_in(Language::English, entropy.as_ref()).map_err(|e| e.to_string())?;
    Ok(Zeroizing::new(phrase.to_string()))
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

fn open_entropy(
    dir: &Path,
    name: &str,
    network: Network,
    password: &str,
) -> Result<(WalletFile, Zeroizing<Vec<u8>>, QuanticSpendingKey), String> {
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
    Ok((file, entropy, key))
}

/// Abre la wallet con su contraseña.
pub fn open(dir: &Path, name: &str, network: Network, password: &str) -> Result<QuanticSpendingKey, String> {
    open_entropy(dir, name, network, password).map(|(_, _, key)| key)
}

/// La clave del archivo de escaneo de una wallet abierta.
pub struct ScanKey(Zeroizing<[u8; 32]>);

/// Abre la wallet y da también la clave de su archivo de escaneo.
pub fn open_for_scan(
    dir: &Path,
    name: &str,
    network: Network,
    password: &str,
) -> Result<(QuanticSpendingKey, ScanKey), String> {
    let (_, entropy, key) = open_entropy(dir, name, network, password)?;
    Ok((key, ScanKey(Zeroizing::new(blake3::derive_key(SCAN_KEY_CONTEXT, &entropy)))))
}

/// Copia la wallet tal como está guardada (la entropía sigue cifrada con su
/// contraseña) a `out`, para respaldarla fuera de la PC. No pisa un archivo.
pub fn backup(dir: &Path, name: &str, out: &Path) -> Result<(), String> {
    read(dir, name)?;
    let bytes = std::fs::read(wallet_path(dir, name)?).map_err(|e| e.to_string())?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)
        .and_then(|mut f| std::io::Write::write_all(&mut f, &bytes))
        .map_err(|e| format!("cannot write {}: {e}", out.display()))
}

/// Trae como `name` una wallet respaldada con [`backup`]: tiene que ser de esta
/// red y abrir con `password`. Devuelve su dirección.
pub fn import_backup(dir: &Path, name: &str, file: &Path, network: Network, password: &str) -> Result<String, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let mut wallet: WalletFile =
        serde_json::from_slice(&bytes).map_err(|_| format!("{} is not a Zoka Quantic wallet backup", file.display()))?;
    if wallet.schema != WALLET_SCHEMA {
        return Err(format!("{}: unknown wallet schema {}", file.display(), wallet.schema));
    }
    if wallet.chain_id != network.chain_id() {
        return Err(format!("the backup is for {}, not {}", wallet.chain_id, network.chain_id()));
    }
    let encrypted = hex::decode(&wallet.encrypted_entropy_hex).map_err(|_| "corrupt wallet backup".to_string())?;
    let entropy = Zeroizing::new(
        crate::wallet::storage::decrypt_data(&encrypted, password).map_err(|_| "wrong password".to_string())?,
    );
    let key = QuanticSpendingKey::from_root(&entropy, network, 0).map_err(|e| e.to_string())?;
    if key.full_viewing_key().address().encode() != wallet.address {
        return Err("the backup does not match its address".to_string());
    }
    let path = wallet_path(dir, name)?;
    if path.exists() {
        return Err(format!("{} already exists; refusing to overwrite a wallet", path.display()));
    }
    wallet.name = name.to_string();
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_vec_pretty(&wallet).map_err(|e| e.to_string())?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, &json))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(wallet.address)
}

/// La frase de recuperación de la wallet (secreta), con su contraseña.
pub fn recovery_phrase(dir: &Path, name: &str, network: Network, password: &str) -> Result<Zeroizing<String>, String> {
    let (_, entropy, _) = open_entropy(dir, name, network, password)?;
    let mnemonic = Mnemonic::from_entropy_in(Language::English, &entropy).map_err(|e| e.to_string())?;
    Ok(Zeroizing::new(mnemonic.to_string()))
}

/// Vuelve a cifrar la wallet con otra contraseña. El archivo se reemplaza de
/// una vez: un corte en el medio deja el viejo o el nuevo, nunca uno roto.
pub fn change_password(dir: &Path, name: &str, network: Network, old: &str, new: &str) -> Result<(), String> {
    let (mut file, entropy, _) = open_entropy(dir, name, network, old)?;
    let encrypted = crate::wallet::storage::encrypt_data(&entropy, new).map_err(|e| e.to_string())?;
    file.encrypted_entropy_hex = hex::encode(encrypted);
    let json = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
    replace_file(&wallet_path(dir, name)?, &json)
}

fn replace_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = PathBuf::from(format!("{}.tmp", path.display()));
    {
        let mut file = std::fs::File::create(&tmp).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        std::io::Write::write_all(&mut file, bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        file.sync_all().map_err(|e| format!("cannot flush {}: {e}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot replace {}: {e}", path.display()))
}

#[derive(Serialize, Deserialize)]
struct ScanFile {
    schema: String,
    nonce_hex: String,
    ciphertext_hex: String,
}

#[derive(Serialize, Deserialize)]
struct ScanPointJson {
    height: u64,
    last_hash: String,
    state_hex: String,
}

fn scan_path(dir: &Path, name: &str) -> Result<PathBuf, String> {
    Ok(wallet_path(dir, name)?.with_extension("scan"))
}

/// Los puntos de escaneo guardados; vacío si no hay o no se pueden leer.
pub fn load_scan(dir: &Path, name: &str, key: &ScanKey) -> Vec<SavedScan> {
    use aes_gcm::aead::{Aead, Payload};
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
    let read_points = || -> Option<Vec<SavedScan>> {
        let address = read(dir, name).ok()?.address;
        let file: ScanFile = serde_json::from_slice(&std::fs::read(scan_path(dir, name).ok()?).ok()?).ok()?;
        if file.schema != SCAN_SCHEMA {
            return None;
        }
        let nonce = hex::decode(&file.nonce_hex).ok().filter(|nonce| nonce.len() == 12)?;
        let ciphertext = hex::decode(&file.ciphertext_hex).ok()?;
        let cipher = Aes256Gcm::new_from_slice(key.0.as_ref()).ok()?;
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(Nonce::from_slice(&nonce), Payload { msg: &ciphertext, aad: address.as_bytes() })
                .ok()?,
        );
        let points: Vec<ScanPointJson> = serde_json::from_slice(&plaintext).ok()?;
        points
            .into_iter()
            .map(|point| {
                let state = Zeroizing::new(hex::decode(Zeroizing::new(point.state_hex).as_str()).ok()?);
                Some(SavedScan { height: point.height, last_hash: point.last_hash, state })
            })
            .collect()
    };
    read_points().unwrap_or_default()
}

/// Guarda los puntos de escaneo (reemplaza los anteriores).
pub fn save_scan(dir: &Path, name: &str, key: &ScanKey, points: &[SavedScan]) -> Result<(), String> {
    use aes_gcm::aead::{Aead, Payload};
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
    let address = read(dir, name)?.address;
    let json: Vec<ScanPointJson> = points
        .iter()
        .map(|point| ScanPointJson {
            height: point.height,
            last_hash: point.last_hash.clone(),
            state_hex: hex::encode(&*point.state),
        })
        .collect();
    let plaintext = Zeroizing::new(serde_json::to_vec(&json).map_err(|e| e.to_string())?);
    for point in json {
        drop(Zeroizing::new(point.state_hex));
    }
    let mut nonce = [0u8; 12];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut nonce);
    let cipher = Aes256Gcm::new_from_slice(key.0.as_ref()).map_err(|e| e.to_string())?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: &plaintext, aad: address.as_bytes() })
        .map_err(|_| "cannot encrypt the scan state".to_string())?;
    let file = ScanFile {
        schema: SCAN_SCHEMA.to_string(),
        nonce_hex: hex::encode(nonce),
        ciphertext_hex: hex::encode(ciphertext),
    };
    replace_file(&scan_path(dir, name)?, &serde_json::to_vec(&file).map_err(|e| e.to_string())?)
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

/// Las wallets con su dirección, sin pedir contraseñas (se saltean las que no
/// se pueden leer).
pub fn list_with_addresses(dir: &Path) -> Vec<(String, String)> {
    list(dir)
        .into_iter()
        .filter_map(|name| read(dir, &name).ok().map(|file| (name, file.address)))
        .collect()
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

        // La frase generada antes de crear la wallet (lo que hace ZSilent) sirve igual.
        let generated = generate_phrase().unwrap();
        assert_eq!(generated.split_whitespace().count(), 24);
        assert_ne!(*generated, *generate_phrase().unwrap());
        assert!(restore(dir.path(), "nueva", Network::Regtest, &generated, "x").unwrap().starts_with("zqregtest1"));
    }

    #[test]
    fn the_password_changes_and_the_phrase_and_the_scan_survive() {
        let dir = tempfile::tempdir().unwrap();
        let (phrase, address) = create(dir.path(), "main", Network::Regtest, "vieja").unwrap();
        assert_eq!(*recovery_phrase(dir.path(), "main", Network::Regtest, "vieja").unwrap(), *phrase);
        assert!(recovery_phrase(dir.path(), "main", Network::Regtest, "otra").is_err());

        let (key, scan_key) = open_for_scan(dir.path(), "main", Network::Regtest, "vieja").unwrap();
        let wallet = zoka_quantic::wallet::QuanticWallet::new(
            key.full_viewing_key(),
            &super::super::consensus_params(Network::Regtest),
        );
        let point = SavedScan { height: 0, last_hash: "a".repeat(64), state: wallet.to_state_bytes() };
        assert!(load_scan(dir.path(), "main", &scan_key).is_empty(), "nada guardado todavía");
        save_scan(dir.path(), "main", &scan_key, std::slice::from_ref(&point)).unwrap();
        let raw = std::fs::read_to_string(dir.path().join("main.scan")).unwrap();
        assert!(!raw.contains(&hex::encode(&point.state[..32])), "cifrado");
        assert_eq!(list(dir.path()), vec!["main".to_string()], "el archivo de escaneo no es una wallet");

        change_password(dir.path(), "main", Network::Regtest, "vieja", "nueva").unwrap();
        assert_eq!(open(dir.path(), "main", Network::Regtest, "vieja").unwrap_err(), "wrong password");
        assert!(change_password(dir.path(), "main", Network::Regtest, "vieja", "x").is_err());
        let (key, scan_key) = open_for_scan(dir.path(), "main", Network::Regtest, "nueva").unwrap();
        assert_eq!(key.full_viewing_key().address().encode(), address);
        let loaded = load_scan(dir.path(), "main", &scan_key);
        assert_eq!(loaded.len(), 1, "el escaneo sigue valiendo con la contraseña nueva");
        assert_eq!(*loaded[0].state, *point.state);
        assert_eq!(*recovery_phrase(dir.path(), "main", Network::Regtest, "nueva").unwrap(), *phrase);
        assert_eq!(list_with_addresses(dir.path()), vec![("main".to_string(), address)]);

        // Con la clave de otra wallet no se abre (y no rompe: vacío).
        create(dir.path(), "otra", Network::Regtest, "x").unwrap();
        let (_, other_key) = open_for_scan(dir.path(), "otra", Network::Regtest, "x").unwrap();
        assert!(load_scan(dir.path(), "main", &other_key).is_empty());
    }

    #[test]
    fn a_backup_comes_back_only_to_its_network_and_with_its_password() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let (_, address) = create(dir.path(), "main", Network::Regtest, "clave").unwrap();
        let copy = elsewhere.path().join("respaldo.json");
        backup(dir.path(), "main", &copy).unwrap();
        assert!(backup(dir.path(), "main", &copy).is_err(), "no pisa un respaldo");
        assert_eq!(
            std::fs::read(&copy).unwrap(),
            std::fs::read(dir.path().join("main.json")).unwrap(),
            "es el archivo cifrado tal cual"
        );

        let other = tempfile::tempdir().unwrap();
        assert_eq!(import_backup(other.path(), "vuelta", &copy, Network::Regtest, "mala").unwrap_err(), "wrong password");
        assert!(import_backup(other.path(), "vuelta", &copy, Network::Testnet, "clave").is_err());
        assert_eq!(import_backup(other.path(), "vuelta", &copy, Network::Regtest, "clave").unwrap(), address);
        assert!(import_backup(other.path(), "vuelta", &copy, Network::Regtest, "clave").is_err(), "no pisa");
        assert_eq!(open(other.path(), "vuelta", Network::Regtest, "clave").unwrap().full_viewing_key().address().encode(), address);
        assert_eq!(list(other.path()), vec!["vuelta".to_string()]);
    }

    #[test]
    fn the_regtest_team_wallet_receives_the_regtest_premine() {
        let dir = tempfile::tempdir().unwrap();
        let address = import_regtest_team(dir.path(), "team", "x").unwrap();
        let premine = super::super::consensus_params(Network::Regtest).premine.unwrap();
        assert_eq!(address, premine.recipient.encode());
    }
}
