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

/// La clave pública de releases del equipo (`HybridVerifyingKey` en hex),
/// generada el 26/11/2026. La privada vive en la PC del equipo, protegida con
/// DPAPI, con una copia cifrada aparte (`backup`).
pub const RELEASE_PUBKEY_HEX: &str = concat!(
    "01612c6c9bb57d74ccd9bc759c478cc96e4c77d5ee715330f59aeee83fcaba613cbb7abbf792a175232b59ca4a65b648",
    "a714b3625a9a52f9ad3e9640088134adcbc0695d52a9e29f2042c703f3d13c0c4f6266a1ca99db7e79c9be1c01621264",
    "aff37b26fd4089bc84754c5930d6896e1477e7842caf510f9e4ef198e649693f0d1b50efdf526058643b7e3f03359fc3",
    "50b19b1907e68bb76b8441bf7aa7b371f43e44a2fe13022e76c6c01f5ee67ae6b360165177513d0f562aad96105db45f",
    "6a56b0699d002e29e9e5967463069e55a420fa03a6fadfbc010c285a9437ddc0f39cb66deaa31008cae1d96e9999f195",
    "e4fdc6ffedcc8844a5f06e1a84b996da5422f7104e25eb5c085506ee4e07f1d96a5e94b094b7ae8561782c3833d22ae8",
    "aaa338fd84ddf25fa8a19148aa8ba78a09937b7c6bc7d497c919d005574cbb69a736add5f832ff61a9293a708872563b",
    "44bc46aafdd4a65a48d06fff79b5cd474646576b3638c672f70c7e3c8f8cdaeec5da1bc1259d9f0ef04c71f8554457e0",
    "9ee5b046fa9084e9672e3cca367ff671a9c7cd8b476c03e9682ad32b657367f8abb8dc1a8f4be6fa9da27227b13b1e51",
    "bbe245c666610d7ec5244b56a3e159280008b79839a30282e4539a4b8cec4e665a22ea2929ad9fd9d119efcf93f75874",
    "a96c7b512a94cfbe802508ea826327015e8b8096bd70a5495c0944abdd4cf46564a3cff57a449bfc998e56212b8674a6",
    "490977922146b50173114c6c6e0ed52064c66f77aa2772a387ae537868896107cc69f8c92331479e304a3f495b2f6c5c",
    "b7a461a947dd7b5d88533b8b3275f1597ff49001c398b1025dbdfa282baa2a49d69728a17eeaed23c2d37138643e4104",
    "88dfb33e8f73f3d6dd262cbb6f6938bb1fe51bb433419432e472ff30d84c9ace51ce3f503494f0d6e939a5b388e58c6a",
    "7e2ffadce879f864b71b4b620700f1cf6e85ac2f7ab2ee23ed9816ca91988913be79bc49f57146f873aace0f71665e44",
    "d2dd3287eb6922b277f15b3e0df7242a869eff760db6ba63dacd9e6e9066fd318a0fc03e235ad4fc4b37dd8c864684c0",
    "39145a1f04539c1b4d9101a3a1cac2da4eb93f0e904f4c23375958a6ed8e0a8496eb061ff3aab10f44eb67d0204d84d4",
    "dca2539fafbd9a8c3f9885abb112d37e7c11b231638d47c5b4f2bb245350b4ecd4d085d44ef27538938ada893c61b78d",
    "596b72f93cee44b36d525ceb8135f5513fabfd163a3ce31c43614a1009ca538c516c6ac81da95f3ea7c3bf4ea279a039",
    "29d890265cdc6257514d24789815d3c0a6d246339e71bd79f96dc400a150a9fed1317c7ee259de70db32063f05344ed7",
    "90893cd5039a98a1a47214d146e7e5560298b380f8af09951afcd5af990d45cbe3c74635467adbd4a638d681643fb91c",
    "ae7e3a94cb52b020bb1c13f67fd56b882b3b983ebb077d50df3bad6b5a5a49a6338771f96dfa9bf29a182f0248594a15",
    "13ded04a4643d3252e88189430afc18ee52d53b26618749584744bbf2335cc80d981fd0ca4287367c80cf6eb2ce6bc2d",
    "7542c443cbb76a4816baae50b8e34d2e57c6651b81cfca53ebeb780f3bdda65253cb49a2ffc294ac1c8c264c0cc0d62c",
    "f98fd1524917957a4bab69b2ed6eb578ad4b40e55c228d7258f496f907e135c685b8915e2c7f485dcf3edc8ce4665e95",
    "536511eb7287dd2bc3447f8bd9c83d30084e19bd621aaf9615b81ca9baaeccd2cf7a072e688fa352fea11c1800812593",
    "3c2107675552378521a7329266ad9ee29668205f525e95a64f7b06cf191b9ab47f6d5e83a175052c03bb026fdf640d65",
    "80e1414572b4a13180f301e11ad031afbe73978a448df7bfd0fe913adf9f1f20b6e4b8cd07f7fa9adc552bf26f866e8c",
    "3224a72c29c3cc0c1667c5b1ada0532ddce3f4eda98cec8bd2db5f7199a6d7b6fc6164da92233c989906bb4f15457459",
    "f96befb5e4f261d13b99b61b9982ea7974c10283f4a6b3ad97011c2c43c517855e57b13b7c873a048b3b44b0ded216e5",
    "c6a5970b5fea5089c597bdd4f8a200daa31dd0d41df40f59898b671d2c6ffa037059d4a4542121a366798d0a4e3919aa",
    "ebf0c38c5bf1d47b0927287711304ada5b4fe1538a53bcab1df6142125a232ccd9eea6648dfaa11797868dc0a015bc39",
    "e1e5dc72217f3d4148840d6620c1485688c13bc48b3d1fca1183cec635ecaad2021a005522701d0c0ded5f3639126a3a",
    "57f9c8be601b94d3795457eb71108f0fa515dd03a1fa005474ef72776bfc93dcbb43426280b7b4bf7b2ef661d60d7100",
    "317fd13bd00a27ee8fa9a042f2b24014af4708d80304414b9bc7cbd2c635e09b55d3ec02507c714e827c415ace361709",
    "15a70d04b76a72b67a7135aa6292a2c25d4bcbbbb192199d320f71c56a3f7e69d0b7164002eb601c679a9a45d44814b3",
    "7384d8b7c0c82467eac7b4ac810ed7ce6f7445c18a815a1934a06db82aa277fc0265c3a920d76ea997f07e7bb3d9fdf3",
    "6a1cc31bb6cfe162e3a9237b6511fbce4566d59228204380794c3f386305970affbbce05b3078f87b7149e73e6c60f4b",
    "9dc842662cb495389bc9bc30a093afa453c8411eee2bff59c8cdc299274147175f434e7bef51567be3655bc3e2522753",
    "6160572bf166057978149a29f6bb8618f912ff1cfd0a6d5e5325a1ec0e013931dc4674441cc85af6efce83a9c9369893",
    "965ba44faaad2dc72ee545e743933b8ddbf745236658caeaeff46bffeef851b0c385c4f854d8507b8d192bbc6df5b706",
    "e872f8fbda356689da83ded2446701ab07",
);
/// SHA-256 de esa clave: lo que se publica (docs/release-verification.md).
pub const RELEASE_KEY_FINGERPRINT: &str = "61c2afa567d79ad5c43a89c582656f55ffc888a54ac06ddc8445dece63ed5c99";

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
    fn the_compiled_release_key_is_the_published_one() {
        let key = embedded_key().unwrap();
        assert_eq!(fingerprint(&key), RELEASE_KEY_FINGERPRINT);
        assert_eq!(key.to_hex(), RELEASE_PUBKEY_HEX);
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
