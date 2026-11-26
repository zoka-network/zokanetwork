//! Firma de releases: el `SHA256SUMS.txt` de cada release lleva una firma
//! híbrida Ed25519 + ML-DSA-65 (`hybrid_signature`), con una clave propia,
//! distinta de la de los checkpoints, y un dominio propio: una firma de release
//! nunca vale como checkpoint ni al revés.
//!
//! La clave privada vive en la PC del equipo, protegida con DPAPI para el
//! usuario de Windows, como la de la autoridad: firmar no pide contraseña y
//! otra cuenta u otra PC no la puede abrir. `backup` deja una copia portátil
//! cifrada con contraseña, y `restore` la vuelve a instalar.
//!
//! Para verificar sin confiar en este binario, `export-pem` da las dos claves
//! públicas en PEM y OpenSSL 3.5 o posterior verifica cada mitad
//! (docs/release-verification.md).
//!
//! Uso: zoka release <keygen | sign <SHA256SUMS.txt> | verify <SHA256SUMS.txt>
//!      | fingerprint | public-key | export-pem | backup --out <archivo> | restore --from <archivo>>
//!      [--dir <carpeta de la clave>] [--pubkey <hex o archivo>] [--sig <archivo>]

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::SigningKey;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::checkpoint_authority::{protection, read_key_seed_with};
use crate::hybrid_signature::{HybridSignature, HybridSigningKey, HybridVerifyingKey};

/// Lo que firma la clave de releases: `dominio ‖ 0x1f ‖ SHA256SUMS.txt`, y el
/// mismo dominio como contexto de ML-DSA.
pub const RELEASE_DOMAIN: &[u8] = b"ZOKA release SHA256SUMS v1";
pub const KIND_ED25519: &str = "release-ed25519";
pub const KIND_MLDSA: &str = "release-ml-dsa-65";
pub const ED25519_FILE: &str = "release-ed25519.key";
pub const MLDSA_FILE: &str = "release-ml-dsa-65.key";
pub const SIGNATURE_SUFFIX: &str = ".sig";
pub const BACKUP_PASSWORD_ENV: &str = "ZOKA_RELEASE_BACKUP_PASSWORD";
pub const ED25519_PEM_FILE: &str = "zoka-release-ed25519.pem";
pub const MLDSA_PEM_FILE: &str = "zoka-release-ml-dsa-65.pem";
const SIGNATURE_HEADER: &str = "ZOKA release signature v1";
const BACKUP_SCHEMA: &str = "zoka-release-key-backup-v1";
const MIN_BACKUP_PASSWORD_CHARS: usize = 12;

/// La clave pública de releases del equipo (`HybridVerifyingKey` en hex).
/// Vacía hasta que se genere: entonces `verify` pide `--pubkey`.
pub const RELEASE_PUBKEY_HEX: &str = "";

// SubjectPublicKeyInfo (RFC 8410 y RFC 9881) sin la clave: lo que antecede a
// los 32 bytes de Ed25519 y a los 1.952 de ML-DSA-65 en un PEM de OpenSSL.
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];
const MLDSA65_SPKI_PREFIX: [u8; 22] = [
    0x30, 0x82, 0x07, 0xb2, 0x30, 0x0b, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04,
    0x03, 0x12, 0x03, 0x82, 0x07, 0xa1, 0x00,
];

/// `%APPDATA%\Zoka\release-signing` en Windows, `~/.zoka/release-signing` si no.
pub fn default_dir() -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata).join("Zoka").join("release-signing");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".zoka")
        .join("release-signing")
}

/// SHA-256 de la clave pública híbrida, en hex: lo que se publica para
/// reconocerla.
pub fn fingerprint(key: &HybridVerifyingKey) -> String {
    hex::encode(Sha256::digest(key.to_bytes()))
}

/// La clave compilada en este binario.
pub fn embedded_key() -> Result<HybridVerifyingKey, String> {
    if RELEASE_PUBKEY_HEX.is_empty() {
        return Err("this zoka has no release key compiled in: pass --pubkey".into());
    }
    HybridVerifyingKey::from_hex(RELEASE_PUBKEY_HEX).map_err(|e| e.to_string())
}

fn key_from_seeds(ed25519: &[u8; 32], mldsa: &[u8; 32]) -> Result<HybridSigningKey, String> {
    HybridSigningKey::from_parts(SigningKey::from_bytes(ed25519), mldsa).map_err(|e| e.to_string())
}

fn seeds(dir: &Path) -> Result<(Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>), String> {
    let ed25519 = read_key_seed_with(
        &dir.join(ED25519_FILE),
        KIND_ED25519,
        "release Ed25519 key",
        None,
    )?;
    let mldsa = read_key_seed_with(&dir.join(MLDSA_FILE), KIND_MLDSA, "release ML-DSA key", None)?;
    Ok((ed25519, mldsa))
}

/// La clave privada guardada en `dir`.
pub fn load_key(dir: &Path) -> Result<HybridSigningKey, String> {
    let (ed25519, mldsa) = seeds(dir)?;
    key_from_seeds(&ed25519, &mldsa)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn refuse_existing_key(dir: &Path) -> Result<(), String> {
    if dir.join(ED25519_FILE).exists() || dir.join(MLDSA_FILE).exists() {
        return Err(format!(
            "{} already holds a release key; nothing was changed",
            dir.display()
        ));
    }
    Ok(())
}

/// Las dos semillas, protegidas para el usuario de Windows. Nunca pisa nada.
fn write_seeds(dir: &Path, ed25519: &[u8; 32], mldsa: &[u8; 32]) -> Result<(), String> {
    refuse_existing_key(dir)?;
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let ed_file = protection::protect_seed_for_user(ed25519, KIND_ED25519)?;
    let ml_file = protection::protect_seed_for_user(mldsa, KIND_MLDSA)?;
    write_new(&dir.join(ED25519_FILE), &ed_file)?;
    write_new(&dir.join(MLDSA_FILE), &ml_file)
}

/// Una clave nueva en `dir`. Se vuelve a leer antes de dar por buena.
pub fn keygen(dir: &Path) -> Result<HybridVerifyingKey, String> {
    refuse_existing_key(dir)?;
    let mut ed25519 = Zeroizing::new([0u8; 32]);
    let mut mldsa = Zeroizing::new([0u8; 32]);
    rand::rngs::OsRng.fill_bytes(&mut ed25519[..]);
    rand::rngs::OsRng.fill_bytes(&mut mldsa[..]);
    let public = key_from_seeds(&ed25519, &mldsa)?.verifying_key();
    write_seeds(dir, &ed25519, &mldsa)?;
    if load_key(dir)?.verifying_key() != public {
        return Err("the saved release key does not read back the same".into());
    }
    Ok(public)
}

/// `SHA256SUMS.txt` → `SHA256SUMS.txt.sig`.
pub fn signature_path(sums: &Path) -> PathBuf {
    let mut name = sums.as_os_str().to_owned();
    name.push(SIGNATURE_SUFFIX);
    PathBuf::from(name)
}

/// El archivo `.sig`: texto, una línea por campo. Las firmas van en hex.
pub fn signature_text(
    file_name: &str,
    bytes: &[u8],
    key: &HybridVerifyingKey,
    signature: &HybridSignature,
) -> String {
    let (ed25519, mldsa) = signature.to_hex_parts();
    format!(
        "{SIGNATURE_HEADER}\nfile: {file_name}\nsha256: {}\nkey: {}\ned25519: {ed25519}\nml-dsa-65: {mldsa}\n",
        hex::encode(Sha256::digest(bytes)),
        fingerprint(key),
    )
}

struct ParsedSignature {
    key: String,
    signature: HybridSignature,
}

fn parse_signature(text: &str) -> Result<ParsedSignature, String> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some(SIGNATURE_HEADER) {
        return Err("this is not a ZOKA release signature".into());
    }
    let (mut key, mut ed25519, mut mldsa) = (None, None, None);
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim().to_string();
            match name.trim() {
                "key" => key = Some(value),
                "ed25519" => ed25519 = Some(value),
                "ml-dsa-65" => mldsa = Some(value),
                _ => {}
            }
        }
    }
    let (Some(key), Some(ed25519), Some(mldsa)) = (key, ed25519, mldsa) else {
        return Err("the release signature is missing a field".into());
    };
    let signature = HybridSignature::from_hex_parts(&ed25519, &mldsa).map_err(|e| e.to_string())?;
    Ok(ParsedSignature { key, signature })
}

/// Firma `sums` con la clave de `dir` y deja `sums.sig` al lado (nunca lo
/// pisa). Con `expected`, se niega si la clave no es esa.
pub fn sign_file(
    sums: &Path,
    dir: &Path,
    expected: Option<&HybridVerifyingKey>,
) -> Result<PathBuf, String> {
    let bytes = fs::read(sums).map_err(|e| format!("cannot read {}: {e}", sums.display()))?;
    parse_sums(&bytes)?;
    let key = load_key(dir)?;
    let public = key.verifying_key();
    if let Some(expected) = expected {
        if &public != expected {
            return Err(format!(
                "the release key in {} (fingerprint {}) is not the one compiled into this zoka ({})",
                dir.display(),
                fingerprint(&public),
                fingerprint(expected)
            ));
        }
    }
    let signature = key
        .sign(RELEASE_DOMAIN, &bytes)
        .map_err(|e| e.to_string())?;
    public
        .verify(RELEASE_DOMAIN, &bytes, &signature)
        .map_err(|e| format!("the new signature does not verify: {e}"))?;
    let name = sums
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{} has no file name", sums.display()))?;
    let out = signature_path(sums);
    write_new(&out, signature_text(name, &bytes, &public, &signature).as_bytes())?;
    Ok(out)
}

/// Lo que se comprobó de cada archivo de la lista.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileCheck {
    Ok(String),
    Mismatch(String),
    Missing(String),
    /// Un nombre con carpetas: no se lee nada fuera de la carpeta de la lista.
    Skipped(String),
}

#[derive(Debug)]
pub struct Verification {
    pub fingerprint: String,
    pub files: Vec<FileCheck>,
}

impl Verification {
    pub fn all_present_match(&self) -> bool {
        !self
            .files
            .iter()
            .any(|check| matches!(check, FileCheck::Mismatch(_)))
    }
}

/// `<64 hex>  nombre` o `<64 hex> *nombre`, como `sha256sum`.
fn parse_sum_line(line: &str) -> Option<(String, String)> {
    let (hash, rest) = line.split_at_checked(64)?;
    if !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let name = rest.strip_prefix("  ").or_else(|| rest.strip_prefix(" *"))?;
    if name.is_empty() {
        return None;
    }
    Some((hash.to_ascii_lowercase(), name.to_string()))
}

fn parse_sums(bytes: &[u8]) -> Result<Vec<(String, String)>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "the checksum list is not UTF-8")?;
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        entries.push(
            parse_sum_line(line).ok_or_else(|| format!("unreadable checksum line: {line}"))?,
        );
    }
    if entries.is_empty() {
        return Err("the checksum list is empty".into());
    }
    Ok(entries)
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Verifica la firma de `sums` con `key` y, si es válida, el SHA-256 de cada
/// archivo de la lista que esté en la misma carpeta.
pub fn verify_file(
    sums: &Path,
    signature: &Path,
    key: &HybridVerifyingKey,
) -> Result<Verification, String> {
    let bytes = fs::read(sums).map_err(|e| format!("cannot read {}: {e}", sums.display()))?;
    let text = fs::read_to_string(signature)
        .map_err(|e| format!("cannot read {}: {e}", signature.display()))?;
    let parsed = parse_signature(&text)?;
    let expected = fingerprint(key);
    if !parsed.key.eq_ignore_ascii_case(&expected) {
        return Err(format!(
            "signed with key {}, not with the release key {expected}",
            parsed.key
        ));
    }
    key.verify(RELEASE_DOMAIN, &bytes, &parsed.signature)
        .map_err(|e| format!("the signature is NOT valid: {e}"))?;
    let dir = sums.parent().unwrap_or_else(|| Path::new("."));
    let mut files = Vec::new();
    for (hash, name) in parse_sums(&bytes)? {
        if name.contains('/') || name.contains('\\') || name.starts_with('.') {
            files.push(FileCheck::Skipped(name));
            continue;
        }
        let path = dir.join(&name);
        if !path.is_file() {
            files.push(FileCheck::Missing(name));
        } else if file_sha256(&path)? == hash {
            files.push(FileCheck::Ok(name));
        } else {
            files.push(FileCheck::Mismatch(name));
        }
    }
    Ok(Verification {
        fingerprint: expected,
        files,
    })
}

fn pem(der: &[u8]) -> String {
    let encoded = BASE64.encode(der);
    let mut out = String::from("-----BEGIN PUBLIC KEY-----\n");
    for chunk in encoded.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        out.push('\n');
    }
    out.push_str("-----END PUBLIC KEY-----\n");
    out
}

/// Las dos mitades de la clave pública en PEM (SubjectPublicKeyInfo), para
/// verificar con OpenSSL: (Ed25519, ML-DSA-65).
pub fn public_pems(key: &HybridVerifyingKey) -> (String, String) {
    let mut ed25519 = ED25519_SPKI_PREFIX.to_vec();
    ed25519.extend_from_slice(&key.ed25519().to_bytes());
    let mut mldsa = MLDSA65_SPKI_PREFIX.to_vec();
    mldsa.extend_from_slice(&key.mldsa().to_bytes());
    (pem(&ed25519), pem(&mldsa))
}

#[derive(Serialize, Deserialize)]
struct BackupFile {
    schema: String,
    key: String,
    encrypted_hex: String,
}

fn decode_backup(
    bytes: &[u8],
    password: &str,
) -> Result<(Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>, String), String> {
    let file: BackupFile =
        serde_json::from_slice(bytes).map_err(|_| "this is not a release key backup")?;
    if file.schema != BACKUP_SCHEMA {
        return Err("unknown release key backup format".into());
    }
    let encrypted =
        hex::decode(&file.encrypted_hex).map_err(|_| "the release key backup is damaged")?;
    let plain = Zeroizing::new(
        crate::wallet::storage::decrypt_data(&encrypted, password)
            .map_err(|_| "wrong password or damaged release key backup")?,
    );
    if plain.len() != 64 {
        return Err("the release key backup is damaged".into());
    }
    let mut ed25519 = Zeroizing::new([0u8; 32]);
    let mut mldsa = Zeroizing::new([0u8; 32]);
    ed25519.copy_from_slice(&plain[..32]);
    mldsa.copy_from_slice(&plain[32..]);
    let actual = fingerprint(&key_from_seeds(&ed25519, &mldsa)?.verifying_key());
    if actual != file.key {
        return Err("the release key backup does not match its own fingerprint".into());
    }
    Ok((ed25519, mldsa, file.key))
}

fn check_backup_password(password: &str) -> Result<(), String> {
    if password.chars().count() < MIN_BACKUP_PASSWORD_CHARS {
        return Err(format!(
            "the backup password needs at least {MIN_BACKUP_PASSWORD_CHARS} characters"
        ));
    }
    Ok(())
}

/// Una copia portátil de la clave de `dir`, cifrada con `password`. Se vuelve
/// a abrir antes de dar por buena. Nunca pisa un archivo.
pub fn backup(dir: &Path, out: &Path, password: &str) -> Result<String, String> {
    check_backup_password(password)?;
    let (ed25519, mldsa) = seeds(dir)?;
    let key = fingerprint(&key_from_seeds(&ed25519, &mldsa)?.verifying_key());
    let mut plain = Zeroizing::new([0u8; 64]);
    plain[..32].copy_from_slice(&ed25519[..]);
    plain[32..].copy_from_slice(&mldsa[..]);
    let encrypted = crate::wallet::storage::encrypt_data(&plain[..], password)
        .map_err(|e| format!("cannot encrypt the release key: {e}"))?;
    let json = serde_json::to_vec_pretty(&BackupFile {
        schema: BACKUP_SCHEMA.into(),
        key: key.clone(),
        encrypted_hex: hex::encode(encrypted),
    })
    .map_err(|e| e.to_string())?;
    write_new(out, &json)?;
    let saved = fs::read(out).map_err(|e| e.to_string())?;
    let (_, _, saved_key) = decode_backup(&saved, password)?;
    if saved_key != key {
        return Err("the backup does not read back the same key".into());
    }
    Ok(key)
}

/// Instala en `dir` la clave de una copia de `backup`, protegida para este
/// usuario de Windows. Nunca pisa una clave.
pub fn restore(backup_file: &Path, dir: &Path, password: &str) -> Result<HybridVerifyingKey, String> {
    let bytes = fs::read(backup_file)
        .map_err(|e| format!("cannot read {}: {e}", backup_file.display()))?;
    let (ed25519, mldsa, _) = decode_backup(&bytes, password)?;
    let public = key_from_seeds(&ed25519, &mldsa)?.verifying_key();
    write_seeds(dir, &ed25519, &mldsa)?;
    if load_key(dir)?.verifying_key() != public {
        return Err("the restored release key does not read back the same".into());
    }
    Ok(public)
}

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|at| args.get(at + 1))
        .map(String::as_str)
}

fn positional(args: &[String]) -> Option<&str> {
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
            continue;
        }
        if arg.starts_with("--") {
            skip = true;
            continue;
        }
        return Some(arg);
    }
    None
}

/// `--pubkey` en hex o como archivo que lo contiene; si no, la de la carpeta
/// de `--dir`; si no, la compilada.
fn chosen_key(args: &[String]) -> Result<HybridVerifyingKey, String> {
    if let Some(value) = option(args, "--pubkey") {
        let text = if Path::new(value).is_file() {
            fs::read_to_string(value).map_err(|e| format!("cannot read {value}: {e}"))?
        } else {
            value.to_string()
        };
        return HybridVerifyingKey::from_hex(text.trim()).map_err(|e| e.to_string());
    }
    if let Some(dir) = option(args, "--dir") {
        return Ok(load_key(Path::new(dir))?.verifying_key());
    }
    embedded_key()
}

fn backup_password() -> Result<Zeroizing<String>, String> {
    std::env::var(BACKUP_PASSWORD_ENV)
        .map(Zeroizing::new)
        .map_err(|_| format!("set {BACKUP_PASSWORD_ENV} to the backup password"))
}

pub fn run(args: &[String]) -> Result<(), String> {
    let Some(command) = args.first() else {
        return Err(USAGE.into());
    };
    let rest = &args[1..];
    let dir = option(rest, "--dir")
        .map(PathBuf::from)
        .unwrap_or_else(default_dir);
    match command.as_str() {
        "keygen" => {
            let public = keygen(&dir)?;
            println!("Release key created in {} (protected for this Windows user).", dir.display());
            println!("Fingerprint: {}", fingerprint(&public));
            println!("Public key:  {}", public.to_hex());
            println!("Make a portable backup now: zoka release backup --out <file> (password in {BACKUP_PASSWORD_ENV}).");
            Ok(())
        }
        "sign" => {
            let sums = positional(rest).ok_or("pass the SHA256SUMS.txt to sign")?;
            let expected = embedded_key().ok();
            if expected.is_none() {
                eprintln!("warning: this zoka has no release key compiled in; signing without that check");
            }
            let out = sign_file(Path::new(sums), &dir, expected.as_ref())?;
            println!("Signed: {}", out.display());
            Ok(())
        }
        "verify" => {
            let sums = PathBuf::from(positional(rest).ok_or("pass the SHA256SUMS.txt to verify")?);
            let signature = option(rest, "--sig")
                .map(PathBuf::from)
                .unwrap_or_else(|| signature_path(&sums));
            let key = chosen_key(rest)?;
            let verification = verify_file(&sums, &signature, &key)?;
            println!(
                "Signature OK (Ed25519 + ML-DSA-65), release key {}",
                verification.fingerprint
            );
            for check in &verification.files {
                match check {
                    FileCheck::Ok(name) => println!("  OK        {name}"),
                    FileCheck::Mismatch(name) => println!("  MISMATCH  {name}"),
                    FileCheck::Missing(name) => println!("  not here  {name}"),
                    FileCheck::Skipped(name) => println!("  skipped   {name}"),
                }
            }
            if verification.all_present_match() {
                Ok(())
            } else {
                Err("a file does not match the signed checksum: do not run it".into())
            }
        }
        "fingerprint" => {
            println!("{}", fingerprint(&chosen_key(rest)?));
            Ok(())
        }
        "public-key" => {
            println!("{}", chosen_key(rest)?.to_hex());
            Ok(())
        }
        "export-pem" => {
            let key = chosen_key(rest)?;
            let out = option(rest, "--out-dir").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
            let (ed25519, mldsa) = public_pems(&key);
            write_new(&out.join(ED25519_PEM_FILE), ed25519.as_bytes())?;
            write_new(&out.join(MLDSA_PEM_FILE), mldsa.as_bytes())?;
            println!("Wrote {ED25519_PEM_FILE} and {MLDSA_PEM_FILE} in {}", out.display());
            Ok(())
        }
        "backup" => {
            let out = option(rest, "--out").ok_or("pass --out <file>")?;
            let password = backup_password()?;
            let key = backup(&dir, Path::new(out), &password)?;
            println!("Backup of release key {key} written to {out}. Keep it apart from this PC.");
            Ok(())
        }
        "restore" => {
            let from = option(rest, "--from").ok_or("pass --from <backup file>")?;
            let password = backup_password()?;
            let public = restore(Path::new(from), &dir, &password)?;
            println!("Release key {} restored in {}", fingerprint(&public), dir.display());
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

pub const USAGE: &str = "zoka release <keygen | sign <SHA256SUMS.txt> | verify <SHA256SUMS.txt> | fingerprint | public-key | export-pem | backup --out <file> | restore --from <file>> [--dir <key folder>] [--pubkey <hex or file>] [--sig <file>] [--out-dir <folder>]";

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_key_dir(dir: &Path, ed25519: u8, mldsa: u8) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(ED25519_FILE), hex::encode([ed25519; 32])).unwrap();
        fs::write(dir.join(MLDSA_FILE), hex::encode([mldsa; 32])).unwrap();
    }

    fn release_folder(root: &Path) -> PathBuf {
        let release = root.join("release");
        fs::create_dir_all(&release).unwrap();
        fs::write(release.join("a.zip"), b"first file").unwrap();
        fs::write(release.join("b.tar.gz"), b"second file").unwrap();
        let sums = format!(
            "{}  a.zip\n{} *b.tar.gz\n{}  c.msi\n",
            hex::encode(Sha256::digest(b"first file")),
            hex::encode(Sha256::digest(b"second file")),
            hex::encode(Sha256::digest(b"not here")),
        );
        fs::write(release.join("SHA256SUMS.txt"), sums).unwrap();
        release.join("SHA256SUMS.txt")
    }

    #[test]
    fn a_signed_list_verifies_and_checks_every_file_next_to_it() {
        let root = tempfile::tempdir().unwrap();
        let keys = root.path().join("keys");
        hex_key_dir(&keys, 1, 2);
        let sums = release_folder(root.path());
        let public = load_key(&keys).unwrap().verifying_key();
        let sig = sign_file(&sums, &keys, Some(&public)).unwrap();
        assert_eq!(sig, signature_path(&sums));
        let verification = verify_file(&sums, &sig, &public).unwrap();
        assert_eq!(
            verification.files,
            vec![
                FileCheck::Ok("a.zip".into()),
                FileCheck::Ok("b.tar.gz".into()),
                FileCheck::Missing("c.msi".into()),
            ]
        );
        assert!(verification.all_present_match());
        assert_eq!(verification.fingerprint, fingerprint(&public));

        // Un archivo cambiado después de firmar se marca.
        fs::write(sums.parent().unwrap().join("a.zip"), b"tampered").unwrap();
        let verification = verify_file(&sums, &sig, &public).unwrap();
        assert_eq!(verification.files[0], FileCheck::Mismatch("a.zip".into()));
        assert!(!verification.all_present_match());
    }

    #[test]
    fn a_changed_list_another_key_or_another_domain_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let keys = root.path().join("keys");
        hex_key_dir(&keys, 3, 4);
        let sums = release_folder(root.path());
        let key = load_key(&keys).unwrap();
        let public = key.verifying_key();
        let sig = sign_file(&sums, &keys, None).unwrap();

        // Otra clave: la huella no coincide.
        let other_dir = root.path().join("other");
        hex_key_dir(&other_dir, 5, 6);
        let other = load_key(&other_dir).unwrap().verifying_key();
        assert!(verify_file(&sums, &sig, &other).unwrap_err().contains("not with the release key"));

        // La misma clave, otro dominio (por ejemplo, el de un checkpoint).
        let bytes = fs::read(&sums).unwrap();
        let wrong = key.sign(b"zoka-checkpoint-v2", &bytes).unwrap();
        let wrong_sig = root.path().join("wrong.sig");
        fs::write(&wrong_sig, signature_text("SHA256SUMS.txt", &bytes, &public, &wrong)).unwrap();
        assert!(verify_file(&sums, &wrong_sig, &public).unwrap_err().contains("NOT valid"));

        // La lista cambiada después de firmar.
        let mut changed = bytes.clone();
        changed[0] = if changed[0] == b'0' { b'1' } else { b'0' };
        fs::write(&sums, &changed).unwrap();
        assert!(verify_file(&sums, &sig, &public).unwrap_err().contains("NOT valid"));
    }

    #[test]
    fn signing_never_overwrites_and_checks_the_compiled_key() {
        let root = tempfile::tempdir().unwrap();
        let keys = root.path().join("keys");
        hex_key_dir(&keys, 7, 8);
        let sums = release_folder(root.path());
        let other_dir = root.path().join("other");
        hex_key_dir(&other_dir, 9, 10);
        let other = load_key(&other_dir).unwrap().verifying_key();
        assert!(sign_file(&sums, &keys, Some(&other)).unwrap_err().contains("is not the one compiled"));
        assert!(!signature_path(&sums).exists());
        sign_file(&sums, &keys, None).unwrap();
        assert!(sign_file(&sums, &keys, None).unwrap_err().contains("cannot create"));
    }

    #[test]
    fn checksum_lines_and_names_outside_the_folder() {
        assert_eq!(
            parse_sum_line(&format!("{}  x.zip", "A".repeat(64))),
            Some(("a".repeat(64), "x.zip".into()))
        );
        assert_eq!(
            parse_sum_line(&format!("{} *x.zip", "b".repeat(64))),
            Some(("b".repeat(64), "x.zip".into()))
        );
        assert_eq!(parse_sum_line(&format!("{} x.zip", "c".repeat(64))), None);
        assert_eq!(parse_sum_line("nothexnothexnothex  x.zip"), None);

        let root = tempfile::tempdir().unwrap();
        let keys = root.path().join("keys");
        hex_key_dir(&keys, 11, 12);
        let release = root.path().join("release");
        fs::create_dir_all(&release).unwrap();
        let sums = release.join("SHA256SUMS.txt");
        fs::write(&sums, format!("{}  ../keys/{ED25519_FILE}\n", "d".repeat(64))).unwrap();
        let public = load_key(&keys).unwrap().verifying_key();
        let sig = sign_file(&sums, &keys, None).unwrap();
        let verification = verify_file(&sums, &sig, &public).unwrap();
        assert_eq!(
            verification.files,
            vec![FileCheck::Skipped(format!("../keys/{ED25519_FILE}"))]
        );
    }

    #[test]
    fn the_pem_keys_carry_the_raw_public_keys() {
        let root = tempfile::tempdir().unwrap();
        let keys = root.path().join("keys");
        hex_key_dir(&keys, 13, 14);
        let public = load_key(&keys).unwrap().verifying_key();
        let (ed25519, mldsa) = public_pems(&public);
        let der = |pem: &str| {
            let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
            BASE64.decode(body).unwrap()
        };
        let ed = der(&ed25519);
        assert_eq!(ed.len(), 44);
        assert_eq!(&ed[12..], &public.ed25519().to_bytes());
        let ml = der(&mldsa);
        assert_eq!(ml.len(), 1974);
        assert_eq!(&ml[22..], &public.mldsa().to_bytes()[..]);
    }

    #[cfg(windows)]
    #[test]
    fn keygen_backup_and_restore_keep_the_same_key_and_never_overwrite() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("keys");
        let public = keygen(&dir).unwrap();
        assert!(keygen(&dir).unwrap_err().contains("already holds"));
        let text = fs::read_to_string(dir.join(ED25519_FILE)).unwrap();
        assert!(protection::is_protected_for_user(&text));

        let out = root.path().join("release-key.backup");
        assert!(backup(&dir, &out, "short").unwrap_err().contains("at least"));
        let key = backup(&dir, &out, "a long enough password").unwrap();
        assert_eq!(key, fingerprint(&public));
        assert!(backup(&dir, &out, "a long enough password").is_err());

        let restored_dir = root.path().join("restored");
        assert!(restore(&out, &restored_dir, "the wrong password!").unwrap_err().contains("wrong password"));
        let restored = restore(&out, &restored_dir, "a long enough password").unwrap();
        assert_eq!(restored, public);
        assert!(restore(&out, &restored_dir, "a long enough password").is_err());
    }
}
