//! Explicit-path local tools for the separate STARK regtest chain.
//! No command selects a public wallet or chain directory by default.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{BufRead, Read, Write},
    path::{Path, PathBuf},
};

use bip39::{Language, Mnemonic};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::stark::{
    self,
    protocol::{
        crypto::Digest,
        ledger::{conventional_fee, Ledger},
        model::{Input, Note, MAX_INPUTS},
        transaction::{self, Payment, Transaction, MAX_TRANSACTION_BYTES},
        wallet::{Account, Address, MEMO_BYTES},
    },
};
use crate::{
    economics::QUANTIC_PQ_REGTEST_CHAIN_ID as CHAIN,
    state::chain::{apply_block, genesis_parent_for},
    storage::RocksDbStorage,
    types::Block,
};

const SCHEMA: &str = "zoka-quantic-stark-wallet-v2";
const DB_MARKER: &str = "quantic:stark:chain-id";
const MAX_WALLET_BYTES: usize = 16 * 1024;
const MAX_BLOCK_JSON_BYTES: usize = 2 * stark::protocol::ledger::MAX_BLOCK_BYTES + 128 * 1024;

fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
}
fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}
fn required<'a>(args: &'a [String], name: &str) -> Result<&'a str, String> {
    option(args, name)
        .filter(|v| !v.starts_with("--"))
        .ok_or_else(|| format!("missing {name}"))
}
fn path(args: &[String], name: &str) -> Result<PathBuf, String> {
    required(args, name).map(PathBuf::from)
}
fn integer(args: &[String], name: &str) -> Result<u64, String> {
    required(args, name)?
        .parse()
        .map_err(|_| format!("{name} must be an unsigned integer"))
}

fn secret_line(label: &str) -> Result<Zeroizing<String>, String> {
    let mut line = Zeroizing::new(String::new());
    // Bound stdin before allocation; no interactive echo or secret logging here.
    std::io::stdin()
        .lock()
        .take(4097)
        .read_line(&mut line)
        .map_err(error)?;
    if line.len() > 4096 {
        return Err(format!("{label} is too long"));
    }
    while matches!(line.as_bytes().last().copied(), Some(b'\r' | b'\n')) {
        line.pop();
    }
    if line.is_empty() {
        return Err(format!("{label} cannot be empty"));
    }
    Ok(line)
}
fn password(args: &[String]) -> Result<Zeroizing<String>, String> {
    if flag(args, "--password-stdin") {
        return secret_line("password");
    }
    let value = std::env::var("ZOKA_PQ_PASSWORD")
        .map_err(|_| "set ZOKA_PQ_PASSWORD or pass --password-stdin".to_string())?;
    if value.is_empty() {
        return Err("password cannot be empty".into());
    }
    Ok(Zeroizing::new(value))
}

pub(crate) fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    if file.metadata().map_err(error)?.len() > limit as u64 {
        return Err("file exceeds size limit".into());
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() > limit {
        return Err("file exceeds size limit".into());
    }
    Ok(bytes)
}
pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(error)?;
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|e| {
            format!(
                "cannot create {} (existing files are never overwritten): {e}",
                path.display()
            )
        })?;
    file.write_all(bytes).map_err(error)?;
    file.sync_all().map_err(error)
}

#[derive(Serialize, Deserialize)]
struct WalletFile {
    schema: String,
    chain_id: String,
    address: String,
    encrypted_seed_hex: String,
}

fn read_wallet(path: &Path) -> Result<WalletFile, String> {
    let file: WalletFile =
        serde_json::from_slice(&read_bounded(path, MAX_WALLET_BYTES)?).map_err(error)?;
    if file.schema != SCHEMA || file.chain_id != CHAIN {
        return Err("not a STARK v2 regtest wallet".into());
    }
    Address::decode(&file.address, 3).map_err(error)?;
    Ok(file)
}

fn create_wallet(path: &Path, seed: &[u8; 32], password: &str) -> Result<String, String> {
    let account = Account::from_seed(seed, 3, 0).map_err(error)?;
    let address = account.address().encode().map_err(error)?;
    let encrypted = crate::wallet::storage::encrypt_data(seed, password).map_err(error)?;
    let file = WalletFile {
        schema: SCHEMA.into(),
        chain_id: CHAIN.into(),
        address: address.clone(),
        encrypted_seed_hex: hex::encode(encrypted),
    };
    write_new(path, &serde_json::to_vec_pretty(&file).map_err(error)?)?;
    Ok(address)
}

fn open_wallet(path: &Path, password: &str) -> Result<Account, String> {
    let file = read_wallet(path)?;
    let encrypted = hex::decode(&file.encrypted_seed_hex).map_err(error)?;
    // The shared decoder supports old XOR wallets. This new schema never does.
    if !encrypted.starts_with(b"ZOKAENC1") {
        return Err("wallet requires authenticated encryption".into());
    }
    let plaintext = Zeroizing::new(
        crate::wallet::storage::decrypt_data(&encrypted, password)
            .map_err(|_| "incorrect password or corrupt wallet".to_string())?,
    );
    let seed: &[u8; 32] = plaintext
        .as_slice()
        .try_into()
        .map_err(|_| "invalid seed length".to_string())?;
    let account = Account::from_seed(seed, 3, 0).map_err(error)?;
    if account.address().encode().map_err(error)? != file.address {
        return Err("wallet address mismatch".into());
    }
    Ok(account)
}

/// Refuse existing DBs without the v2 marker, including v1 and public chains.
/// Only a genuinely new directory may be initialized by these tools.
pub(crate) fn open_db(path: &Path) -> Result<RocksDbStorage, String> {
    let new =
        !path.exists() || (path.is_dir() && fs::read_dir(path).map_err(error)?.next().is_none());
    let db = RocksDbStorage::new(path).map_err(error)?;
    match db.get_raw(DB_MARKER).map_err(error)? {
        Some(marker) if marker == CHAIN.as_bytes() => {}
        None if new => {
            db.put_raw(DB_MARKER, CHAIN.as_bytes().to_vec())
                .map_err(error)?;
        }
        _ => {
            return Err(
                "database is not initialized as STARK v2 regtest; use a new --db directory".into(),
            )
        }
    }
    Ok(db)
}

fn tip(db: &RocksDbStorage) -> Result<Block, String> {
    if db.get_chain_height().map_err(error)? == 0 {
        return Ok(genesis_parent_for(CHAIN));
    }
    let hash = db.get_last_block_hash().map_err(error)?;
    db.get_block(&hash)
        .map_err(error)?
        .ok_or_else(|| "missing canonical tip".into())
}

struct OwnedNote {
    note: Note,
    index: Option<u32>,
    nullifier: Digest,
}
struct Scan {
    state: Ledger,
    notes: Vec<OwnedNote>,
}
fn scan(db: &RocksDbStorage, account: &Account) -> Result<Scan, String> {
    let parent = tip(db)?;
    let state = stark::state_after(db, &parent)?;
    let mut positions = BTreeMap::new();
    for index in 0..state.tree.len() {
        let index = u32::try_from(index).map_err(error)?;
        let cm = state
            .tree
            .commitment(index)
            .ok_or("missing note commitment")?;
        positions.entry(cm).or_insert(index);
    }
    let mut notes = Vec::new();
    let mut seen = BTreeSet::new();
    for height in 1..=parent.header.block_height {
        let block = db
            .get_canonical_block_by_height(height)
            .map_err(error)?
            .ok_or("scan requires unpruned canonical blocks")?;
        for tx in stark::decode(&block)? {
            for (cm, envelope) in tx.statement.commitments.iter().zip(&tx.envelopes) {
                if let Ok((note, _memo)) = account.receive(*cm, envelope) {
                    let nullifier = note.nullifier(&account.spend, 3).map_err(error)?;
                    if !state.spent.contains(&nullifier) && seen.insert(nullifier) {
                        notes.push(OwnedNote {
                            note,
                            index: positions.get(cm).copied(),
                            nullifier,
                        });
                    }
                }
            }
        }
    }
    Ok(Scan { state, notes })
}

fn pending_path(wallet: &Path) -> PathBuf {
    let mut name = wallet.as_os_str().to_os_string();
    name.push(".stark-pending");
    PathBuf::from(name)
}
fn pending(wallet: &Path, password: &str) -> Result<Option<Transaction>, String> {
    let path = pending_path(wallet);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = read_bounded(&path, MAX_TRANSACTION_BYTES + 128)?;
    if !bytes.starts_with(b"ZOKAENC1") {
        return Err("pending payment requires authenticated encryption".into());
    }
    let plaintext =
        Zeroizing::new(crate::wallet::storage::decrypt_data(&bytes, password).map_err(error)?);
    let tx = Transaction::decode(&plaintext).map_err(error)?;
    if tx.statement.network != 3 || tx.statement.nullifiers.is_empty() {
        return Err("invalid pending payment".into());
    }
    Ok(Some(tx))
}
fn live_pending(tx: &Transaction, state: &Ledger) -> bool {
    (tx.height == 0 || tx.height > state.height)
        && !tx
            .statement
            .nullifiers
            .iter()
            .any(|nf| state.spent.contains(nf))
}

fn wallet_lock_path(path: &Path) -> Result<PathBuf, String> {
    let mut canonical = fs::canonicalize(path).map_err(error)?.into_os_string();
    canonical.push(".stark-lock");
    Ok(PathBuf::from(canonical))
}

fn wallet_lock(path: &Path) -> Result<File, String> {
    // Lock a persistent sidecar: Windows mandatory locks would prevent the
    // separate handle used by open_wallet from reading a locked wallet file.
    // Never unlink the sidecar, since that could create two independent locks.
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(wallet_lock_path(path)?)
        .map_err(error)?;
    fs4::FileExt::lock_exclusive(&file).map_err(error)?;
    Ok(file)
}

fn send(args: &[String]) -> Result<(), String> {
    let wallet = fs::canonicalize(path(args, "--wallet")?).map_err(error)?;
    let out = path(args, "--out")?;
    if out.exists() {
        return Err("output already exists".into());
    }
    let _lock = wallet_lock(&wallet)?;
    let password = password(args)?;
    let account = open_wallet(&wallet, &password)?;
    let to = Address::decode(required(args, "--to")?, 3).map_err(error)?;
    let amount = integer(args, "--amount-atoms")?;
    if amount == 0 {
        return Err("amount must be greater than zero".into());
    }
    let db = open_db(&path(args, "--db")?)?;
    let Scan { state, mut notes } = scan(&db, &account)?;
    if let Some(tx) = pending(&wallet, &password)? {
        if live_pending(&tx, &state) {
            return Err("wallet already has a pending payment; mine it or use abandon".into());
        }
        fs::remove_file(pending_path(&wallet)).map_err(error)?;
    }
    notes.retain(|note| note.index.is_some());
    notes.sort_by_key(|note| std::cmp::Reverse(note.note.value));
    let mut selected = Vec::new();
    let mut total = 0u64;
    for note in notes.into_iter().take(MAX_INPUTS) {
        total = total
            .checked_add(note.note.value)
            .ok_or("input sum overflow")?;
        selected.push(note);
        let required = amount
            .checked_add(conventional_fee(selected.len(), 2))
            .ok_or("amount overflow")?;
        if total >= required {
            break;
        }
    }
    let fee = conventional_fee(selected.len(), 2);
    let needed = amount.checked_add(fee).ok_or("amount overflow")?;
    if total < needed {
        return Err("insufficient mature funds within the eight-input limit".into());
    }
    let mut payment = Payment::new(to, amount);
    if let Some(memo) = option(args, "--memo") {
        if memo.len() > MEMO_BYTES {
            return Err("memo exceeds 512 UTF-8 bytes".into());
        }
        payment.memo[..memo.len()].copy_from_slice(memo.as_bytes());
    }
    let inputs = selected
        .into_iter()
        .map(|owned| {
            Ok(Input {
                key: account.spend.clone(),
                note: owned.note,
                path: state
                    .tree
                    .path(owned.index.ok_or("immature input")?)
                    .map_err(error)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let expiry = state.height.checked_add(20).ok_or("expiry overflow")?;
    let tx = transaction::transfer(
        &account,
        state.tree.root(),
        inputs,
        vec![payment, Payment::new(account.address(), total - needed)],
        fee,
        expiry,
    )
    .map_err(error)?;
    state.check_transfer(&tx).map_err(error)?;
    let bytes = tx.encode().map_err(error)?;
    let encrypted = crate::wallet::storage::encrypt_data(&bytes, &password).map_err(error)?;
    // Reserve before export. If export fails, pending-export recovers the same
    // transaction without creating a conflicting payment or exposing a seed.
    write_new(&pending_path(&wallet), &encrypted)?;
    write_new(&out, &bytes)?;
    println!(
        "{}",
        serde_json::json!({"txid":hex::encode(tx.txid()),"fee_atoms":fee,"expires_at":expiry,"file":out})
    );
    Ok(())
}

pub fn run(args: &[String]) -> Result<(), String> {
    let command = args.first().map(String::as_str).unwrap_or("help");
    match command {
        "help" | "--help" => {
            println!(
                "ZOKA QUANTIC STARK v2 local regtest\n\
wallet-create --wallet FILE [--password-stdin]\n\
wallet-restore --wallet FILE --phrase-stdin [--password-stdin]\n\
address --wallet FILE\n\
balance --db DIR --wallet FILE [--password-stdin]\n\
send --db DIR --wallet FILE --to ADDRESS --amount-atoms N --out FILE [--memo TEXT]\n\
pending-export --wallet FILE --out FILE | abandon --wallet FILE\n\
mine --db DIR --to ADDRESS [--tx FILE] [--out FILE]\n\
import-block --db DIR --in FILE | export-block --db DIR --height N --out FILE\n\
status --db DIR\n\
Passwords come from ZOKA_PQ_PASSWORD unless --password-stdin is set.\n\
Restore reads 24 English BIP-39 words from stdin after the password, if requested.\n\
Wallet creation never prints the seed: back up the encrypted wallet file.\n\
One atom = 0.000001 ZKA. No public chain or wallet path is selected automatically."
            );
        }
        "wallet-create" | "wallet-restore" => {
            let wallet = path(args, "--wallet")?;
            if wallet.exists() {
                return Err("wallet already exists".into());
            }
            let password = password(args)?;
            let mut seed = Zeroizing::new([0u8; 32]);
            if command == "wallet-restore" {
                if !flag(args, "--phrase-stdin") {
                    return Err("restore requires --phrase-stdin".into());
                }
                let phrase = secret_line("recovery phrase")?;
                let mnemonic = Mnemonic::parse_in_normalized(Language::English, &phrase)
                    .map_err(|_| "invalid recovery phrase".to_string())?;
                let entropy = Zeroizing::new(mnemonic.to_entropy());
                if entropy.len() != 32 {
                    return Err("STARK v2 requires a 24-word / 256-bit recovery phrase".into());
                }
                seed.copy_from_slice(&entropy);
            } else {
                rand::rngs::OsRng
                    .try_fill_bytes(seed.as_mut())
                    .map_err(error)?;
            }
            let address = create_wallet(&wallet, &seed, &password)?;
            println!(
                "{}",
                serde_json::json!({"chain_id":CHAIN,"wallet":wallet,"address":address,"backup":"copy the encrypted wallet file"})
            );
        }
        "address" => {
            println!("{}", read_wallet(&path(args, "--wallet")?)?.address);
        }
        "balance" => {
            let wallet = fs::canonicalize(path(args, "--wallet")?).map_err(error)?;
            let _lock = wallet_lock(&wallet)?;
            let password = password(args)?;
            let account = open_wallet(&wallet, &password)?;
            let db = open_db(&path(args, "--db")?)?;
            let Scan { state, notes } = scan(&db, &account)?;
            let reservation = pending(&wallet, &password)?.filter(|tx| live_pending(tx, &state));
            let mut available = 0u64;
            let mut immature = 0u64;
            let mut reserved = 0u64;
            for owned in notes {
                let total = if owned.index.is_none() {
                    &mut immature
                } else if reservation
                    .as_ref()
                    .is_some_and(|tx| tx.statement.nullifiers.contains(&owned.nullifier))
                {
                    &mut reserved
                } else {
                    &mut available
                };
                *total = total
                    .checked_add(owned.note.value)
                    .ok_or("balance overflow")?;
            }
            println!(
                "{}",
                serde_json::json!({"height":state.height,"available_atoms":available,"immature_atoms":immature,"reserved_atoms":reserved})
            );
        }
        "send" => return send(args),
        "pending-export" | "abandon" => {
            let wallet = fs::canonicalize(path(args, "--wallet")?).map_err(error)?;
            let _lock = wallet_lock(&wallet)?;
            let password = password(args)?;
            open_wallet(&wallet, &password)?;
            if command == "pending-export" {
                let tx = pending(&wallet, &password)?.ok_or("no pending payment")?;
                write_new(&path(args, "--out")?, &tx.encode().map_err(error)?)?;
                println!(
                    "{}",
                    serde_json::json!({"txid":hex::encode(tx.txid()),"action":command})
                );
            } else {
                // Explicit abandonment also releases a partially written
                // reservation after an I/O failure. The wallet is untouched.
                fs::remove_file(pending_path(&wallet)).map_err(error)?;
                println!(
                    "{}",
                    serde_json::json!({"action":command,"note":"An exported payment can still be mined until expiry or until one of its inputs is spent."})
                );
            }
        }
        "mine" => {
            let db = open_db(&path(args, "--db")?)?;
            let to = Address::decode(required(args, "--to")?, 3).map_err(error)?;
            let txs = option(args, "--tx")
                .map(|p| {
                    read_bounded(Path::new(p), MAX_TRANSACTION_BYTES)
                        .and_then(|b| Transaction::decode(&b).map_err(error))
                })
                .transpose()?
                .into_iter()
                .collect();
            if let Some(out) = option(args, "--out") {
                if Path::new(out).exists() {
                    return Err("output already exists".into());
                }
            }
            let block = stark::mine(&db, &tip(&db)?, &to, txs, super::now_secs())?;
            let result = apply_block(&db, CHAIN, &block).map_err(error)?;
            if let Some(out) = option(args, "--out") {
                write_new(Path::new(out), &serde_json::to_vec(&block).map_err(error)?).map_err(
                    |e| {
                        format!(
                            "block {} committed at height {}, but export failed: {e}",
                            result.hash, result.height
                        )
                    },
                )?;
            }
            println!(
                "{}",
                serde_json::json!({"height":result.height,"hash":result.hash,"chain_id":CHAIN})
            );
        }
        "import-block" => {
            let db = open_db(&path(args, "--db")?)?;
            let bytes = read_bounded(&path(args, "--in")?, MAX_BLOCK_JSON_BYTES)?;
            let block: Block = serde_json::from_slice(&bytes).map_err(error)?;
            // Same ingress as gossip/mining: header, PoW, body, STARKs and fork choice.
            match apply_block(&db, CHAIN, &block) {
                Ok(result) => println!(
                    "{}",
                    serde_json::json!({"height":result.height,"hash":result.hash,"canonical":true})
                ),
                Err(crate::state::chain::ChainError::SideBranchNotHeavier { .. }) => {
                    println!(
                        "{}",
                        serde_json::json!({"height":block.header.block_height,"hash":block.header.hash,"canonical":false,"stored":true})
                    );
                }
                Err(crate::state::chain::ChainError::BlockAlreadyKnown(_)) => {
                    println!(
                        "{}",
                        serde_json::json!({"hash":block.header.hash,"already_known":true})
                    );
                }
                Err(e) => return Err(error(e)),
            }
        }
        "export-block" => {
            let db = open_db(&path(args, "--db")?)?;
            let block = db
                .get_canonical_block_by_height(integer(args, "--height")?)
                .map_err(error)?
                .ok_or("unknown height")?;
            write_new(
                &path(args, "--out")?,
                &serde_json::to_vec(&block).map_err(error)?,
            )?;
        }
        "status" => {
            let db = open_db(&path(args, "--db")?)?;
            let parent = tip(&db)?;
            let state = stark::state_after(&db, &parent)?;
            println!(
                "{}",
                serde_json::json!({"chain_id":CHAIN,"height":state.height,"hash":parent.header.hash,"issued_atoms":state.issued,"notes":state.tree.len()})
            );
        }
        _ => {
            return Err(format!(
                "unknown STARK command '{command}'; use quantic-pq help"
            ))
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallet_sidecar_lock_allows_decryption_and_excludes_other_writers() {
        let dir = tempfile::tempdir().unwrap();
        let wallet = dir.path().join("alice.json");
        let expected = create_wallet(&wallet, &[19; 32], "temporary lock test").unwrap();
        let original = fs::read(&wallet).unwrap();
        let subdir = dir.path().join("alias");
        fs::create_dir(&subdir).unwrap();
        let alias = subdir.join("..").join("alice.json");
        let sidecar = wallet_lock_path(&wallet).unwrap();
        assert_eq!(wallet_lock_path(&alias).unwrap(), sidecar);
        // Existing contents must survive every lock acquisition; do not
        // truncate or unlink a lock inode shared with another process.
        fs::write(&sidecar, b"persistent lock file").unwrap();
        let held = wallet_lock(&alias).unwrap();
        assert_eq!(
            open_wallet(&wallet, "temporary lock test")
                .unwrap()
                .address()
                .encode()
                .unwrap(),
            expected
        );
        let competing = OpenOptions::new()
            .read(true)
            .write(true)
            .open(wallet_lock_path(&alias).unwrap())
            .unwrap();
        assert!(fs4::FileExt::try_lock_exclusive(&competing).is_err());
        drop(held);
        fs4::FileExt::try_lock_exclusive(&competing).unwrap();
        drop(competing);
        assert_eq!(fs::read(&sidecar).unwrap(), b"persistent lock file");
        assert_eq!(fs::read(&wallet).unwrap(), original);
        #[cfg(unix)]
        {
            let link = dir.path().join("linked-wallet.json");
            std::os::unix::fs::symlink(&wallet, &link).unwrap();
            assert_eq!(wallet_lock_path(&link).unwrap(), sidecar);
            assert_eq!(
                pending_path(&fs::canonicalize(&link).unwrap()),
                pending_path(&fs::canonicalize(&wallet).unwrap())
            );
        }
    }

    #[test]
    fn encrypted_wallet_refuses_overwrite_wrong_password_and_legacy_format() {
        let dir = tempfile::tempdir().unwrap();
        let wallet = dir.path().join("alice.json");
        let address = create_wallet(&wallet, &[9; 32], "test password").unwrap();
        assert_eq!(
            open_wallet(&wallet, "test password")
                .unwrap()
                .address()
                .encode()
                .unwrap(),
            address
        );
        assert!(open_wallet(&wallet, "wrong").is_err());
        assert!(create_wallet(&wallet, &[8; 32], "different").is_err());
        let mut file = read_wallet(&wallet).unwrap();
        file.encrypted_seed_hex = hex::encode([0; 32]);
        fs::write(&wallet, serde_json::to_vec(&file).unwrap()).unwrap();
        assert!(open_wallet(&wallet, "test password").is_err());
    }
    #[test]
    fn database_marker_keeps_existing_databases_separate() {
        let dir = tempfile::tempdir().unwrap();
        let v2 = dir.path().join("v2");
        let db = open_db(&v2).unwrap();
        drop(db);
        assert!(open_db(&v2).is_ok());
        let foreign = dir.path().join("foreign");
        let db = RocksDbStorage::new(&foreign).unwrap();
        drop(db);
        assert!(open_db(&foreign).is_err());
    }

    #[test]
    fn wallet_scan_finds_private_change_and_honors_pending_payment() {
        use stark::protocol::ledger::{ATOMS, PREMINE};
        let dir = tempfile::tempdir().unwrap();
        let wallet = dir.path().join("alice.json");
        create_wallet(&wallet, &[1; 32], "test password").unwrap();
        let alice = open_wallet(&wallet, "test password").unwrap();
        let bob = Account::from_seed(&[2; 32], 3, 0).unwrap();
        let db = open_db(&dir.path().join("chain")).unwrap();
        let genesis = stark::mine(
            &db,
            &tip(&db).unwrap(),
            &alice.address(),
            vec![],
            1_700_000_000,
        )
        .unwrap();
        apply_block(&db, CHAIN, &genesis).unwrap();
        let initial = scan(&db, &alice).unwrap();
        assert_eq!(initial.notes.len(), 1);
        assert_eq!(initial.notes[0].note.value, PREMINE);
        assert_eq!(initial.notes[0].index, Some(0));
        assert!(scan(&db, &bob).unwrap().notes.is_empty());
        let tx = transaction::transfer(
            &alice,
            initial.state.tree.root(),
            vec![Input {
                key: alice.spend.clone(),
                note: initial.notes[0].note.clone(),
                path: initial.state.tree.path(0).unwrap(),
            }],
            vec![
                Payment::new(bob.address(), ATOMS),
                Payment::new(alice.address(), PREMINE - ATOMS - 200),
            ],
            200,
            20,
        )
        .unwrap();
        let encrypted =
            crate::wallet::storage::encrypt_data(&tx.encode().unwrap(), "test password").unwrap();
        write_new(&pending_path(&wallet), &encrypted).unwrap();
        let restored = pending(&wallet, "test password").unwrap().unwrap();
        assert_eq!(restored.txid(), tx.txid());
        assert!(live_pending(&restored, &initial.state));
        assert!(pending(&wallet, "wrong password").is_err());
        let paid = stark::mine(&db, &genesis, &bob.address(), vec![tx], 1_700_000_120).unwrap();
        apply_block(&db, CHAIN, &paid).unwrap();
        let after = scan(&db, &alice).unwrap();
        assert!(!live_pending(&restored, &after.state));
        assert_eq!(after.notes.len(), 1);
        assert_eq!(after.notes[0].note.value, PREMINE - ATOMS - 200);
        let received = scan(&db, &bob).unwrap();
        assert_eq!(
            received
                .notes
                .iter()
                .filter(|n| n.index.is_some())
                .map(|n| n.note.value)
                .sum::<u64>(),
            ATOMS
        );
        assert_eq!(
            received
                .notes
                .iter()
                .filter(|n| n.index.is_none())
                .map(|n| n.note.value)
                .sum::<u64>(),
            12 * ATOMS + 200
        );
    }
}
