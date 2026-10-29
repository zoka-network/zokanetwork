//! Explicit-path local tools for the STARK v2 chains (`--network regtest`, the
//! default, or `testnet`). Wallets and databases record their chain and are
//! never opened as another one. No command selects a public wallet or chain
//! directory by default.
use std::{
    collections::BTreeMap,
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
    Profile,
    protocol::{
        crypto::Digest,
        ledger::{conventional_fee, Ledger},
        model::{Input, Note, MAX_INPUTS},
        transaction::{self, Payment, Transaction, MAX_TRANSACTION_BYTES},
        wallet::{Account, Address, ShortAddress, MEMO_BYTES},
    },
};
use crate::{state::chain::apply_block, storage::RocksDbStorage, types::Block};

pub(crate) const SCHEMA: &str = "zoka-quantic-stark-wallet-v2";
const DB_MARKER: &str = "quantic:stark:chain-id";
pub(crate) const MAX_WALLET_BYTES: usize = 16 * 1024;
const MAX_BLOCK_JSON_BYTES: usize = 2 * stark::protocol::ledger::MAX_BLOCK_BYTES + 128 * 1024;

pub(crate) fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
pub(crate) fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
}
pub(crate) fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}
pub(crate) fn required<'a>(args: &'a [String], name: &str) -> Result<&'a str, String> {
    option(args, name)
        .filter(|v| !v.starts_with("--"))
        .ok_or_else(|| format!("missing {name}"))
}
fn path(args: &[String], name: &str) -> Result<PathBuf, String> {
    required(args, name).map(PathBuf::from)
}
/// `--to`: a full address. A short one is resolved by a node first.
fn recipient(args: &[String], profile: Profile) -> Result<Address, String> {
    let text = required(args, "--to")?.trim();
    Address::decode(text, profile.network).map_err(|e| {
        if ShortAddress::decode(text, profile.network).is_ok() {
            format!("{text} is a short address: resolve it with GET /address/{text} on your node and pay the full address")
        } else {
            error(e)
        }
    })
}
/// `--network`, when given. Wallets and databases already know theirs.
pub(crate) fn network_option(args: &[String]) -> Result<Option<Profile>, String> {
    option(args, "--network").map(Profile::named).transpose()
}
fn integer(args: &[String], name: &str) -> Result<u64, String> {
    required(args, name)?
        .parse()
        .map_err(|_| format!("{name} must be an unsigned integer"))
}

pub(crate) fn secret_line(label: &str) -> Result<Zeroizing<String>, String> {
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
pub(crate) struct WalletFile {
    pub(crate) schema: String,
    pub(crate) chain_id: String,
    pub(crate) address: String,
    pub(crate) encrypted_seed_hex: String,
}

pub(crate) fn read_wallet(path: &Path) -> Result<(WalletFile, Profile), String> {
    let file: WalletFile =
        serde_json::from_slice(&read_bounded(path, MAX_WALLET_BYTES)?).map_err(error)?;
    let profile = Profile::for_chain(&file.chain_id)
        .filter(|_| file.schema == SCHEMA)
        .ok_or("not a STARK v2 wallet")?;
    Address::decode(&file.address, profile.network).map_err(error)?;
    Ok((file, profile))
}

pub(crate) fn create_wallet(
    path: &Path,
    profile: Profile,
    seed: &[u8; 32],
    password: &str,
) -> Result<String, String> {
    let account = Account::from_seed(seed, profile.network, 0).map_err(error)?;
    let address = account.address().encode().map_err(error)?;
    let encrypted = crate::wallet::storage::encrypt_data(seed, password).map_err(error)?;
    let file = WalletFile {
        schema: SCHEMA.into(),
        chain_id: profile.chain_id.into(),
        address: address.clone(),
        encrypted_seed_hex: hex::encode(encrypted),
    };
    write_new(path, &serde_json::to_vec_pretty(&file).map_err(error)?)?;
    Ok(address)
}

pub(crate) fn open_wallet(path: &Path, password: &str) -> Result<(Account, Profile), String> {
    let (account, profile, _) = open_wallet_seed(path, password)?;
    Ok((account, profile))
}

/// Like [`open_wallet`], with the seed: the recovery phrase is its BIP-39 form.
pub(crate) fn open_wallet_seed(
    path: &Path,
    password: &str,
) -> Result<(Account, Profile, Zeroizing<[u8; 32]>), String> {
    let (file, profile) = read_wallet(path)?;
    let encrypted = hex::decode(&file.encrypted_seed_hex).map_err(error)?;
    // The shared decoder supports old XOR wallets. This new schema never does.
    if !encrypted.starts_with(b"ZOKAENC1") {
        return Err("wallet requires authenticated encryption".into());
    }
    let plaintext = Zeroizing::new(
        crate::wallet::storage::decrypt_data(&encrypted, password)
            .map_err(|_| "incorrect password or corrupt wallet".to_string())?,
    );
    let seed: [u8; 32] = plaintext
        .as_slice()
        .try_into()
        .map_err(|_| "invalid seed length".to_string())?;
    let seed = Zeroizing::new(seed);
    let account = Account::from_seed(&seed, profile.network, 0).map_err(error)?;
    if account.address().encode().map_err(error)? != file.address {
        return Err("wallet address mismatch".into());
    }
    Ok((account, profile, seed))
}

/// Refuse existing DBs without a v2 marker, including v1 and public chains, and
/// a marked DB of another chain than `wanted`. Only a genuinely new directory
/// may be initialized by these tools (as `wanted`, regtest when not given).
pub(crate) fn open_db(
    path: &Path,
    wanted: Option<Profile>,
) -> Result<(RocksDbStorage, Profile), String> {
    let new =
        !path.exists() || (path.is_dir() && fs::read_dir(path).map_err(error)?.next().is_none());
    let db = RocksDbStorage::new(path).map_err(error)?;
    let profile = match db.get_raw(DB_MARKER).map_err(error)? {
        Some(marker) => {
            let Some(found) = std::str::from_utf8(&marker)
                .ok()
                .and_then(Profile::for_chain)
            else {
                return Err("database is not a STARK v2 chain; use a new --db directory".into());
            };
            if let Some(wanted) = wanted.filter(|wanted| *wanted != found) {
                return Err(format!(
                    "database belongs to {}, not {}",
                    found.chain_id, wanted.chain_id
                ));
            }
            found
        }
        None if new => {
            let profile = wanted.unwrap_or(Profile::REGTEST);
            db.put_raw(DB_MARKER, profile.chain_id.as_bytes().to_vec())
                .map_err(error)?;
            profile
        }
        None => {
            return Err("database is not a STARK v2 chain; use a new --db directory".into())
        }
    };
    Ok((db, profile))
}

pub(crate) fn tip(db: &RocksDbStorage, profile: Profile) -> Result<Block, String> {
    if db.get_chain_height().map_err(error)? == 0 {
        return Ok(profile.genesis_parent());
    }
    let hash = db.get_last_block_hash().map_err(error)?;
    db.get_block(&hash)
        .map_err(error)?
        .ok_or_else(|| "missing canonical tip".into())
}

pub(crate) struct OwnedNote {
    pub(crate) note: Note,
    /// Position in the note tree; none while a coinbase output is immature.
    pub(crate) index: Option<u32>,
    pub(crate) nullifier: Digest,
}
/// One transaction that touched the wallet: what it paid to the wallet and
/// what it spent of the wallet's notes.
pub(crate) struct Movement {
    pub(crate) txid: [u8; 32],
    pub(crate) height: u64,
    pub(crate) timestamp: u64,
    pub(crate) coinbase: bool,
    pub(crate) received: u64,
    pub(crate) spent: u64,
}
pub(crate) struct Scan {
    pub(crate) state: Ledger,
    /// Unspent notes.
    pub(crate) notes: Vec<OwnedNote>,
    /// Oldest first.
    pub(crate) history: Vec<Movement>,
}
pub(crate) fn scan(db: &RocksDbStorage, profile: Profile, account: &Account) -> Result<Scan, String> {
    let parent = tip(db, profile)?;
    let state = stark::full_ledger(db, profile, &parent)?;
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
    // Every note the wallet ever received, by nullifier: what a later
    // transaction spends of it.
    let mut values = BTreeMap::new();
    let mut history = Vec::new();
    for height in 1..=parent.header.block_height {
        let block = db
            .get_canonical_block_by_height(height)
            .map_err(error)?
            .ok_or("scan requires unpruned canonical blocks")?;
        for tx in stark::decode(&block)? {
            let mut received = 0u64;
            for (cm, envelope) in tx.statement.commitments.iter().zip(&tx.envelopes) {
                if let Ok((note, _memo)) = account.receive(*cm, envelope) {
                    let nullifier = note
                        .nullifier(&account.spend, profile.network)
                        .map_err(error)?;
                    if values.contains_key(&nullifier) {
                        continue;
                    }
                    values.insert(nullifier, note.value);
                    received = received
                        .checked_add(note.value)
                        .ok_or("balance overflow")?;
                    if !state.spent.contains(&nullifier) {
                        notes.push(OwnedNote {
                            note,
                            index: positions.get(cm).copied(),
                            nullifier,
                        });
                    }
                }
            }
            let mut spent = 0u64;
            for nullifier in &tx.statement.nullifiers {
                if let Some(value) = values.get(nullifier) {
                    spent = spent.checked_add(*value).ok_or("balance overflow")?;
                }
            }
            if received > 0 || spent > 0 {
                history.push(Movement {
                    txid: tx.txid(),
                    height,
                    timestamp: block.header.timestamp,
                    coinbase: tx.statement.nullifiers.is_empty(),
                    received,
                    spent,
                });
            }
        }
    }
    Ok(Scan {
        state,
        notes,
        history,
    })
}

/// The largest mature notes first, at most [`MAX_INPUTS`], until they pay
/// `amount` and the fee of a two-output payment. Returns the inputs, their
/// total and the fee.
pub(crate) fn select_inputs(
    mut notes: Vec<OwnedNote>,
    amount: u64,
) -> Result<(Vec<OwnedNote>, u64, u64), String> {
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
    Ok((selected, total, fee))
}

/// The most one payment can send: the eight largest mature notes less the
/// payment's fee.
pub(crate) fn max_sendable(notes: &[OwnedNote]) -> u64 {
    let mut values: Vec<u64> = notes
        .iter()
        .filter(|note| note.index.is_some())
        .map(|note| note.note.value)
        .collect();
    values.sort_unstable_by(|a, b| b.cmp(a));
    values.truncate(MAX_INPUTS);
    let total = values.iter().fold(0u64, |sum, value| sum.saturating_add(*value));
    total.saturating_sub(conventional_fee(values.len().max(1), 2))
}

/// Build a payment of `amount` to `to` and reserve its inputs in the
/// wallet's pending file before returning it. The caller holds the wallet
/// lock. Returns the transaction, its fee and its expiry height.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pay(
    db: &RocksDbStorage,
    profile: Profile,
    wallet: &Path,
    account: &Account,
    password: &str,
    to: Address,
    amount: u64,
    memo: Option<&str>,
) -> Result<(Transaction, u64, u32), String> {
    if amount == 0 {
        return Err("amount must be greater than zero".into());
    }
    if memo.is_some_and(|memo| memo.len() > MEMO_BYTES) {
        return Err("memo exceeds 512 UTF-8 bytes".into());
    }
    let Scan { state, notes, .. } = scan(db, profile, account)?;
    if let Some(tx) = pending(wallet, password, profile)? {
        if live_pending(&tx, &state) {
            return Err("wallet already has a pending payment; mine it or use abandon".into());
        }
        fs::remove_file(pending_path(wallet)).map_err(error)?;
    }
    let (selected, total, fee) = select_inputs(notes, amount)?;
    let needed = amount.checked_add(fee).ok_or("amount overflow")?;
    let mut payment = Payment::new(to, amount);
    if let Some(memo) = memo {
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
        account,
        state.tree.root(),
        inputs,
        vec![payment, Payment::new(account.address(), total - needed)],
        fee,
        expiry,
    )
    .map_err(error)?;
    state.check_transfer(&tx).map_err(error)?;
    let bytes = tx.encode().map_err(error)?;
    let encrypted = crate::wallet::storage::encrypt_data(&bytes, password).map_err(error)?;
    // Reserve before export. If export fails, pending-export recovers the same
    // transaction without creating a conflicting payment or exposing a seed.
    write_new(&pending_path(wallet), &encrypted)?;
    Ok((tx, fee, expiry))
}

pub(crate) fn pending_path(wallet: &Path) -> PathBuf {
    let mut name = wallet.as_os_str().to_os_string();
    name.push(".stark-pending");
    PathBuf::from(name)
}
pub(crate) fn pending(
    wallet: &Path,
    password: &str,
    profile: Profile,
) -> Result<Option<Transaction>, String> {
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
    if tx.statement.network != profile.network || tx.statement.nullifiers.is_empty() {
        return Err("invalid pending payment".into());
    }
    Ok(Some(tx))
}
pub(crate) fn live_pending(tx: &Transaction, state: &Ledger) -> bool {
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

pub(crate) fn wallet_lock(path: &Path) -> Result<File, String> {
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
    let (account, profile) = open_wallet(&wallet, &password)?;
    let to = recipient(args, profile)?;
    let amount = integer(args, "--amount-atoms")?;
    if amount == 0 {
        return Err("amount must be greater than zero".into());
    }
    let (db, _) = open_db(&path(args, "--db")?, Some(profile))?;
    let (tx, fee, expiry) = pay(
        &db,
        profile,
        &wallet,
        &account,
        &password,
        to,
        amount,
        option(args, "--memo"),
    )?;
    write_new(&out, &tx.encode().map_err(error)?)?;
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
                "ZOKA QUANTIC STARK v2 (regtest by default; --network testnet for the RandomX chain)\n\
wallet-create --wallet FILE [--network NAME] [--password-stdin]\n\
wallet-restore --wallet FILE --phrase-stdin [--network NAME] [--password-stdin]\n\
address --wallet FILE [--short]\n\
balance --db DIR --wallet FILE [--password-stdin]\n\
send --db DIR --wallet FILE --to ADDRESS --amount-atoms N --out FILE [--memo TEXT]\n\
pending-export --wallet FILE --out FILE | abandon --wallet FILE\n\
mine --db DIR --to ADDRESS [--tx FILE] [--out FILE] [--network NAME]\n\
import-block --db DIR --in FILE | export-block --db DIR --height N --out FILE\n\
status --db DIR\n\
A new --db takes --network (regtest if omitted); an existing one keeps its chain.\n\
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
            let profile = network_option(args)?.unwrap_or(Profile::REGTEST);
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
            let address = create_wallet(&wallet, profile, &seed, &password)?;
            println!(
                "{}",
                serde_json::json!({"chain_id":profile.chain_id,"wallet":wallet,"address":address,"backup":"copy the encrypted wallet file"})
            );
        }
        "address" => {
            let (file, profile) = read_wallet(&path(args, "--wallet")?)?;
            if flag(args, "--short") {
                let full = Address::decode(&file.address, profile.network).map_err(error)?;
                println!("{}", full.short().map_err(error)?);
            } else {
                println!("{}", file.address);
            }
        }
        "balance" => {
            let wallet = fs::canonicalize(path(args, "--wallet")?).map_err(error)?;
            let _lock = wallet_lock(&wallet)?;
            let password = password(args)?;
            let (account, profile) = open_wallet(&wallet, &password)?;
            let (db, _) = open_db(&path(args, "--db")?, Some(profile))?;
            let Scan { state, notes, .. } = scan(&db, profile, &account)?;
            let reservation =
                pending(&wallet, &password, profile)?.filter(|tx| live_pending(tx, &state));
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
            let (_, profile) = open_wallet(&wallet, &password)?;
            if command == "pending-export" {
                let tx = pending(&wallet, &password, profile)?.ok_or("no pending payment")?;
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
            let (db, profile) = open_db(&path(args, "--db")?, network_option(args)?)?;
            let to = recipient(args, profile)?;
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
            let block = stark::mine(
                &db,
                profile,
                &tip(&db, profile)?,
                &to,
                txs,
                super::now_secs(),
            )?;
            let result = apply_block(&db, profile.chain_id, &block).map_err(error)?;
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
                serde_json::json!({"height":result.height,"hash":result.hash,"chain_id":profile.chain_id})
            );
        }
        "import-block" => {
            let (db, profile) = open_db(&path(args, "--db")?, network_option(args)?)?;
            let bytes = read_bounded(&path(args, "--in")?, MAX_BLOCK_JSON_BYTES)?;
            let block: Block = serde_json::from_slice(&bytes).map_err(error)?;
            // Same ingress as gossip/mining: header, PoW, body, STARKs and fork choice.
            match apply_block(&db, profile.chain_id, &block) {
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
            let (db, _) = open_db(&path(args, "--db")?, network_option(args)?)?;
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
            let (db, profile) = open_db(&path(args, "--db")?, network_option(args)?)?;
            let parent = tip(&db, profile)?;
            let branch = stark::branch(&db, profile, &parent)?;
            let state = branch.state();
            println!(
                "{}",
                serde_json::json!({"chain_id":profile.chain_id,"height":state.height,"hash":parent.header.hash,"issued_atoms":state.issued,"notes":state.notes})
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

    const P: Profile = Profile::REGTEST;
    const CHAIN: &str = P.chain_id;

    #[test]
    fn wallet_sidecar_lock_allows_decryption_and_excludes_other_writers() {
        let dir = tempfile::tempdir().unwrap();
        let wallet = dir.path().join("alice.json");
        let expected = create_wallet(&wallet, P, &[19; 32], "temporary lock test").unwrap();
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
                .0
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
        let address = create_wallet(&wallet, P, &[9; 32], "test password").unwrap();
        assert_eq!(
            open_wallet(&wallet, "test password")
                .unwrap()
                .0
                .address()
                .encode()
                .unwrap(),
            address
        );
        assert!(open_wallet(&wallet, "wrong").is_err());
        assert!(create_wallet(&wallet, P, &[8; 32], "different").is_err());
        let (mut file, _) = read_wallet(&wallet).unwrap();
        file.encrypted_seed_hex = hex::encode([0; 32]);
        fs::write(&wallet, serde_json::to_vec(&file).unwrap()).unwrap();
        assert!(open_wallet(&wallet, "test password").is_err());
    }
    #[test]
    fn database_marker_keeps_existing_databases_separate() {
        let dir = tempfile::tempdir().unwrap();
        let v2 = dir.path().join("v2");
        let (db, profile) = open_db(&v2, None).unwrap();
        assert_eq!(profile, P);
        drop(db);
        assert!(open_db(&v2, None).is_ok());
        assert!(open_db(&v2, Some(P)).is_ok());
        let err = open_db(&v2, Some(Profile::TESTNET)).err().unwrap();
        assert!(err.contains("belongs to zoka-quantic-regtest-v2"), "{err}");
        let testnet = dir.path().join("testnet");
        let (db, profile) = open_db(&testnet, Some(Profile::TESTNET)).unwrap();
        assert_eq!(profile, Profile::TESTNET);
        drop(db);
        assert_eq!(open_db(&testnet, None).unwrap().1, Profile::TESTNET);
        assert!(open_db(&testnet, Some(P)).is_err());
        let foreign = dir.path().join("foreign");
        let db = RocksDbStorage::new(&foreign).unwrap();
        drop(db);
        assert!(open_db(&foreign, None).is_err());
    }

    #[test]
    fn a_wallet_records_its_chain_and_address_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let wallet = dir.path().join("miner.json");
        let address = create_wallet(&wallet, Profile::TESTNET, &[3; 32], "test password").unwrap();
        assert!(address.starts_with("zqtest2"), "{address}");
        let (account, profile) = open_wallet(&wallet, "test password").unwrap();
        assert_eq!(profile, Profile::TESTNET);
        assert_eq!(account.network, 2);
        let local = dir.path().join("local.json");
        let address = create_wallet(&local, P, &[3; 32], "test password").unwrap();
        assert!(address.starts_with("zqregtest2"), "{address}");
        let args = |values: &[&str]| values.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(network_option(&args(&["--network", "testnet"])).unwrap(), Some(Profile::TESTNET));
        assert_eq!(network_option(&args(&[])).unwrap(), None);
        assert!(network_option(&args(&["--network", "mainnet"])).is_err());
        // `--to` takes a full address; a short one is named as such.
        let full = Address::decode(&address, P.network).unwrap();
        assert_eq!(recipient(&args(&["--to", &address]), P).unwrap(), full);
        let short = full.short().unwrap();
        let refused = recipient(&args(&["--to", &short]), P).err().unwrap();
        assert!(refused.contains("is a short address"), "{refused}");
        assert!(recipient(&args(&["--to", "zqregtest2abc"]), P).is_err());
    }

    #[test]
    fn wallet_scan_finds_private_change_and_honors_pending_payment() {
        use stark::protocol::ledger::{ATOMS, PREMINE};
        let dir = tempfile::tempdir().unwrap();
        let wallet = dir.path().join("alice.json");
        create_wallet(&wallet, P, &[1; 32], "test password").unwrap();
        let (alice, _) = open_wallet(&wallet, "test password").unwrap();
        let bob = Account::from_seed(&[2; 32], 3, 0).unwrap();
        let (db, _) = open_db(&dir.path().join("chain"), None).unwrap();
        let genesis = stark::mine(
            &db,
            P,
            &tip(&db, P).unwrap(),
            &alice.address(),
            vec![],
            1_700_000_000,
        )
        .unwrap();
        apply_block(&db, CHAIN, &genesis).unwrap();
        let initial = scan(&db, P, &alice).unwrap();
        assert_eq!(initial.notes.len(), 1);
        assert_eq!(initial.notes[0].note.value, PREMINE);
        assert_eq!(initial.notes[0].index, Some(0));
        assert!(scan(&db, P, &bob).unwrap().notes.is_empty());
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
        let restored = pending(&wallet, "test password", P).unwrap().unwrap();
        assert_eq!(restored.txid(), tx.txid());
        assert!(live_pending(&restored, &initial.state));
        assert!(pending(&wallet, "wrong password", P).is_err());
        let paid = stark::mine(&db, P, &genesis, &bob.address(), vec![tx], 1_700_000_120).unwrap();
        apply_block(&db, CHAIN, &paid).unwrap();
        let after = scan(&db, P, &alice).unwrap();
        assert!(!live_pending(&restored, &after.state));
        assert_eq!(after.notes.len(), 1);
        assert_eq!(after.notes[0].note.value, PREMINE - ATOMS - 200);
        let received = scan(&db, P, &bob).unwrap();
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
