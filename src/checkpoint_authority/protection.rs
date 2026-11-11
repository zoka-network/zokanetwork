//! Automatic authority protection for the Windows user running ZSilent.
//! Wallet passwords are only used for portable backups, never for node startup.
use super::{
    read_key_seed, read_key_seed_with, AUTHORITY_KEY_FILE, AUTHORITY_MLDSA_KEY_FILE,
    SEALED_ED25519, SEALED_MLDSA,
};
use fs4::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
};
use zeroize::{Zeroize, Zeroizing};

const USER_SCHEMA: &str = "zoka-authority-windows-user-v1";
const BACKUP_SCHEMA: &str = "zoka-authority-backup-v1";
pub const BACKUP_PASSWORD_ENV: &str = "ZOKA_AUTHORITY_BACKUP_PASSWORD";
const FILES: [(&str, &str); 2] = [
    (AUTHORITY_KEY_FILE, SEALED_ED25519),
    (AUTHORITY_MLDSA_KEY_FILE, SEALED_MLDSA),
];

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UserKey {
    schema: String,
    key: String,
    protected_seed_hex: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Backup {
    schema: String,
    encrypted_seeds_hex: String,
}

/// Whether a key file's text is protected for the Windows user.
pub fn is_protected_for_user(text: &str) -> bool {
    is_user_protected(text)
}

pub(super) fn is_user_protected(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| {
            v.get("schema")
                .and_then(|s| s.as_str())
                .map(|s| s == USER_SCHEMA)
        })
        == Some(true)
}

pub(super) fn open_user_key(text: &str, kind: &str) -> Result<Zeroizing<[u8; 32]>, String> {
    let file: UserKey =
        serde_json::from_str(text).map_err(|_| "invalid Windows-protected authority key")?;
    if file.schema != USER_SCHEMA || file.key != kind {
        return Err("wrong Windows-protected authority key type".into());
    }
    let ciphertext = hex::decode(&file.protected_seed_hex)
        .map_err(|_| "damaged Windows-protected authority key")?;
    if ciphertext.len() > 4096 {
        return Err("Windows-protected authority key is too large".into());
    }
    let plain = dpapi(&ciphertext, kind, false)?;
    <[u8; 32]>::try_from(plain.as_slice())
        .map(Zeroizing::new)
        .map_err(|_| "invalid authority seed length".into())
}

fn encode_user_key(seed: &[u8; 32], kind: &str) -> Result<Vec<u8>, String> {
    let protected = dpapi(seed, kind, true)?;
    serde_json::to_vec_pretty(&UserKey {
        schema: USER_SCHEMA.into(),
        key: kind.into(),
        protected_seed_hex: hex::encode(&*protected),
    })
    .map_err(|e| e.to_string())
}

fn lock(dir: &Path) -> Result<File, String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(".authority-protection.lock"))
        .map_err(|e| e.to_string())?;
    file.lock_exclusive().map_err(|e| e.to_string())?;
    Ok(file)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}

/// Both halves are authenticated before touching either. A crash between the
/// replacements is harmless: the next start accepts the already protected half.
/// Staging contains only DPAPI ciphertext; no plaintext backup is created.
pub fn protect(dir: &Path) -> Result<(), String> {
    let _lock = lock(dir)?;
    let seeds = FILES
        .iter()
        .map(|(name, kind)| read_key_seed(&dir.join(name), kind, "authority key"))
        .collect::<Result<Vec<_>, _>>()?;
    for ((name, kind), seed) in FILES.iter().zip(seeds.iter()) {
        protect_one(&dir.join(name), kind, seed)?;
    }
    Ok(())
}

fn protect_one(path: &Path, kind: &str, seed: &[u8; 32]) -> Result<(), String> {
    if path.is_file() {
        let text = Zeroizing::new(fs::read_to_string(path).map_err(|e| e.to_string())?);
        if is_user_protected(&text) {
            if *open_user_key(&text, kind)? != *seed {
                return Err("authority key changed during protection".into());
            }
            return Ok(());
        }
    }
    let staged = path.with_extension("key.windows-protecting");
    if staged.exists() {
        let text = Zeroizing::new(fs::read_to_string(&staged).map_err(|e| e.to_string())?);
        match open_user_key(&text, kind) {
            Ok(saved) if *saved == *seed => {}
            Ok(_) => return Err("unfinished authority protection belongs to another key".into()),
            // A cut may truncate the staged ciphertext. The original seed has
            // already been authenticated from the original file or the backup.
            Err(_) => fs::remove_file(&staged).map_err(|e| e.to_string())?,
        }
    }
    if !staged.exists() {
        write_new(&staged, &encode_user_key(seed, kind)?)?;
    }
    if *read_key_seed_with(&staged, kind, "staged authority key", None)? != *seed {
        return Err("staged authority key failed verification".into());
    }
    if path.exists() {
        replace(&staged, path)?;
    } else {
        fs::rename(&staged, path).map_err(|e| e.to_string())?;
    }
    if *read_key_seed_with(path, kind, "protected authority key", None)? != *seed {
        return Err("protected authority key failed verification".into());
    }
    Ok(())
}

/// Password-protected, portable recovery of both authority seeds. The plaintext
/// exists only in zeroizing memory. Existing backups are never overwritten.
pub fn export_backup(dir: &Path, out: &Path, password: &str) -> Result<(), String> {
    if password.is_empty() {
        return Err("backup password cannot be empty".into());
    }
    let _lock = lock(dir)?;
    let mut plain = Zeroizing::new(Vec::with_capacity(64));
    for (name, kind) in FILES {
        plain.extend_from_slice(read_key_seed(&dir.join(name), kind, "authority key")?.as_ref());
    }
    let encrypted =
        crate::wallet::storage::encrypt_data(&plain, password).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(&Backup {
        schema: BACKUP_SCHEMA.into(),
        encrypted_seeds_hex: hex::encode(encrypted),
    })
    .map_err(|e| e.to_string())?;
    if decode_backup(&bytes, password)?.as_slice() != plain.as_slice() {
        return Err("authority backup failed verification".into());
    }
    write_new(out, &bytes)?;
    if decode_backup(&fs::read(out).map_err(|e| e.to_string())?, password)?.as_slice()
        != plain.as_slice()
    {
        return Err("authority backup failed verification after writing".into());
    }
    Ok(())
}

pub fn decode_backup(bytes: &[u8], password: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    if bytes.len() > 4096 || password.is_empty() {
        return Err("invalid authority backup or password".into());
    }
    let file: Backup = serde_json::from_slice(bytes).map_err(|_| "invalid authority backup")?;
    if file.schema != BACKUP_SCHEMA {
        return Err("unknown authority backup format".into());
    }
    let encrypted =
        hex::decode(file.encrypted_seeds_hex).map_err(|_| "damaged authority backup")?;
    if encrypted.len() != 116 || !encrypted.starts_with(b"ZOKAENC1") {
        return Err("damaged authority backup".into());
    }
    let plain = Zeroizing::new(
        crate::wallet::storage::decrypt_data(&encrypted, password)
            .map_err(|_| "wrong authority backup password or damaged file")?,
    );
    if plain.len() != 64 {
        return Err("invalid authority backup length".into());
    }
    Ok(plain)
}

/// The caller verifies these seeds against the selected chain's public keys.
/// Existing keys must match, including either half left by an interrupted restore.
pub fn validate_restore(dir: &Path, seeds: &[u8]) -> Result<(), String> {
    if seeds.len() != 64 {
        return Err("invalid authority backup length".into());
    }
    for ((name, kind), seed) in FILES.iter().zip(seeds.chunks_exact(32)) {
        if dir.join(name).exists()
            && read_key_seed(&dir.join(name), kind, "authority key")?.as_ref() != seed
        {
            return Err(
                "existing checkpoint authority differs from this backup; nothing was changed"
                    .into(),
            );
        }
    }
    Ok(())
}

pub fn restore_backup(dir: &Path, seeds: &[u8]) -> Result<(), String> {
    let _lock = lock(dir)?;
    validate_restore(dir, seeds)?;
    for ((name, kind), seed) in FILES.iter().zip(seeds.chunks_exact(32)) {
        protect_one(
            &dir.join(name),
            kind,
            seed.try_into().map_err(|_| "invalid authority seed")?,
        )?;
    }
    Ok(())
}

#[cfg(windows)]
fn dpapi(bytes: &[u8], kind: &str, encrypt: bool) -> Result<Zeroizing<Vec<u8>>, String> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    };
    let mut input = CRYPT_INTEGER_BLOB {
        cbData: bytes
            .len()
            .try_into()
            .map_err(|_| "authority data too large")?,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let entropy = format!("{USER_SCHEMA}:{kind}");
    let mut context = CRYPT_INTEGER_BLOB {
        cbData: entropy.len() as u32,
        pbData: entropy.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // Current user only: intentionally never CRYPTPROTECT_LOCAL_MACHINE.
    let ok = unsafe {
        if encrypt {
            CryptProtectData(
                &mut input,
                std::ptr::null(),
                &mut context,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &mut input,
                std::ptr::null_mut(),
                &mut context,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 {
        return Err("Windows could not unlock the checkpoint authority for this user profile; restore its encrypted node backup".into());
    }
    let value = unsafe {
        let buffer = std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
        let value = Zeroizing::new(buffer.to_vec());
        buffer.zeroize();
        LocalFree(output.pbData.cast());
        value
    };
    Ok(value)
}

#[cfg(not(windows))]
fn dpapi(_bytes: &[u8], _kind: &str, _encrypt: bool) -> Result<Zeroizing<Vec<u8>>, String> {
    Err("Windows user protection is unavailable on this platform; use a portable encrypted authority backup".into())
}

#[cfg(windows)]
fn replace(from: &Path, to: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // Replaces in one filesystem operation, preserves the original file's ACL,
    // and never moves the original into a new plaintext backup.
    let ok = unsafe {
        ReplaceFileW(
            to.as_ptr(),
            from.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok == 0 {
        return Err(format!(
            "cannot replace authority key: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace(_from: &Path, _to: &Path) -> Result<(), String> {
    Err("Windows user protection is unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        for ((name, _), byte) in FILES.iter().zip([42u8, 43]) {
            fs::write(dir.join(name), hex::encode([byte; 32])).unwrap();
        }
    }

    #[test]
    fn portable_backups_authenticate_both_keys_and_refuse_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        let backup = dir.path().join("backup.json");
        export_backup(dir.path(), &backup, "fixture password").unwrap();
        let bytes = fs::read(&backup).unwrap();
        let seeds = decode_backup(&bytes, "fixture password").unwrap();
        assert_eq!(&seeds[..32], &[42u8; 32]);
        assert_eq!(&seeds[32..], &[43u8; 32]);
        assert!(decode_backup(&bytes, "wrong").is_err());
        let mut tampered: Backup = serde_json::from_slice(&bytes).unwrap();
        tampered.encrypted_seeds_hex.replace_range(100..102, "ff");
        assert!(
            decode_backup(&serde_json::to_vec(&tampered).unwrap(), "fixture password").is_err()
        );
        assert!(export_backup(dir.path(), &backup, "different password").is_err());
        assert_eq!(fs::read(&backup).unwrap(), bytes);
    }

    #[test]
    #[cfg(windows)]
    fn windows_migration_recovers_between_halves_without_plaintext_backups() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        let first = dir.path().join(AUTHORITY_KEY_FILE);
        protect_one(&first, SEALED_ED25519, &[42; 32]).unwrap();
        // Emulate the process exiting before protecting the other half.
        fs::write(
            dir.path()
                .join("checkpoint-authority-mldsa.key.windows-protecting"),
            b"{truncated",
        )
        .unwrap();
        protect(dir.path()).unwrap();
        protect(dir.path()).unwrap();
        for ((name, kind), byte) in FILES.iter().zip([42u8, 43]) {
            let path = dir.path().join(name);
            let bytes = fs::read_to_string(&path).unwrap();
            assert!(is_user_protected(&bytes));
            assert!(!bytes.contains(&hex::encode([byte; 32])));
            assert_eq!(
                *read_key_seed_with(&path, kind, "fixture", None).unwrap(),
                [byte; 32]
            );
            assert!(!dir.path().join(format!("{name}.plain-backup")).exists());
        }
        // The DPAPI context binds ciphertext to the correct half.
        let mut mixed: UserKey =
            serde_json::from_str(&fs::read_to_string(&first).unwrap()).unwrap();
        mixed.key = SEALED_MLDSA.into();
        assert!(open_user_key(&serde_json::to_string(&mixed).unwrap(), SEALED_MLDSA).is_err());
        mixed.key = SEALED_ED25519.into();
        mixed.protected_seed_hex.replace_range(80..82, "ff");
        assert!(open_user_key(&serde_json::to_string(&mixed).unwrap(), SEALED_ED25519).is_err());
    }

    #[test]
    #[cfg(windows)]
    fn portable_restore_survives_restarts_and_refuses_another_authority() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        fixture(&source);
        protect(&source).unwrap();
        let backup = dir.path().join("backup.json");
        export_backup(&source, &backup, "fixture password").unwrap();
        let seeds = decode_backup(&fs::read(&backup).unwrap(), "fixture password").unwrap();
        let restored = dir.path().join("restored");
        restore_backup(&restored, &seeds).unwrap();
        restore_backup(&restored, &seeds).unwrap();
        let before = fs::read(restored.join(AUTHORITY_KEY_FILE)).unwrap();
        let mut other = seeds.to_vec();
        other[50] ^= 1;
        assert!(restore_backup(&restored, &other).is_err());
        assert_eq!(fs::read(restored.join(AUTHORITY_KEY_FILE)).unwrap(), before);
        assert_eq!(
            *read_key_seed(
                &restored.join(AUTHORITY_MLDSA_KEY_FILE),
                SEALED_MLDSA,
                "fixture"
            )
            .unwrap(),
            [43; 32]
        );
    }
}
