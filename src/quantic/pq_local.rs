//! Red local del pool STARK (`zoka-quantic-regtest-v2`).
//!
//! No habla con la testnet ni con la mainnet. Guarda una wallet cifrada, mina
//! bloques hacia ella y manda pagos privados en la misma base. El nodo público
//! sigue en Orchard hasta que esta cadena tenga perfil, red y wallets propias.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use bip39::{Language, Mnemonic};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::stark::{self, protocol};
use crate::economics::QUANTIC_PQ_REGTEST_CHAIN_ID;
use crate::state::chain::{apply_block, genesis_parent_for};
use crate::storage::RocksDbStorage;
use protocol::ledger::{conventional_fee, Ledger, ATOMS};
use protocol::model::{Input, Note, MAX_INPUTS};
use protocol::transaction::{transfer, Payment, Transaction};
use protocol::wallet::{Account, Address};

const SCHEMA: &str = "zoka-pq-regtest-wallet-v1";
const DB_MARKER: &str = "quantic:stark:chain-id";
const NETWORK: u8 = 3;
/// Igual que `Ledger::new(3)`: las recompensas de esta red maduran a los 2 bloques.
const REWARD_MATURITY: u32 = 2;
const USAGE: &str = "\
uso: zoka pq-regtest <new|address|balance|mine|send> --dir <carpeta> --name <wallet> [--password-env <VAR>]
  new      crea la wallet (muestra la frase una sola vez)
  address  muestra la dirección zqregtest2, sin contraseña
  balance  átomos gastables e inmaduros (1 moneda = 1000000 átomos)
  mine     mina un bloque hacia esa wallet
  send     --to <dirección> --amount <átomos>  (mina el bloque que lleva el pago)
La carpeta es una cadena local aparte. No es quantic-testnet ni quantic-mainnet.";

#[derive(Serialize, Deserialize)]
struct WalletFile {
    schema: String,
    chain_id: String,
    name: String,
    address: String,
    encrypted_seed_hex: String,
}

pub struct Chain {
    dir: PathBuf,
    storage: RocksDbStorage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Balance {
    pub height: u64,
    pub spendable_atoms: u64,
    pub immature_atoms: u64,
    pub spendable_notes: usize,
    pub immature_notes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mined {
    pub height: u64,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub txid: String,
    pub height: u64,
    pub fee_atoms: u64,
}

struct SpendableNote {
    note: Note,
    index: u32,
    value: u64,
}

struct Position {
    height: u64,
    parent: crate::types::Block,
    ledger: Ledger,
    spendable: Vec<SpendableNote>,
    immature_atoms: u64,
    immature_notes: usize,
}

impl Chain {
    pub fn open(dir: &Path) -> Result<Self, String> {
        let dir = dir.to_path_buf();
        let db = dir.join("chain");
        let fresh = !db.exists()
            || (db.is_dir()
                && std::fs::read_dir(&db)
                    .map_err(|e| e.to_string())?
                    .next()
                    .transpose()
                    .map_err(|e| e.to_string())?
                    .is_none());
        std::fs::create_dir_all(&db).map_err(|e| format!("no pude crear {}: {e}", db.display()))?;
        let storage =
            RocksDbStorage::new(&db).map_err(|e| format!("no pude abrir la cadena: {e}"))?;
        match storage.get_raw(DB_MARKER).map_err(|e| e.to_string())? {
            Some(marker) if marker == QUANTIC_PQ_REGTEST_CHAIN_ID.as_bytes() => {}
            Some(_) => return Err("esa base pertenece a otra red".into()),
            None => {
                if !fresh {
                    validate_existing_v2(&storage)?;
                }
                storage
                    .put_raw(DB_MARKER, QUANTIC_PQ_REGTEST_CHAIN_ID.as_bytes().to_vec())
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(Self { dir, storage })
    }

    pub fn create_wallet(
        &self,
        name: &str,
        password: &str,
    ) -> Result<(Zeroizing<String>, String), String> {
        if password.is_empty() {
            return Err("la contraseña no puede estar vacía".into());
        }
        let path = wallet_path(&self.dir, name)?;
        if path.exists() {
            return Err(format!("{} ya existe; no piso una wallet", path.display()));
        }
        let mut seed = Zeroizing::new([0u8; 32]);
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, seed.as_mut());
        let phrase = Zeroizing::new(
            Mnemonic::from_entropy_in(Language::English, seed.as_ref())
                .map_err(|e| e.to_string())?
                .to_string(),
        );
        let address = account_from_seed(&seed)?
            .address()
            .encode()
            .map_err(|e| e.to_string())?;
        let encrypted = crate::wallet::storage::encrypt_data(seed.as_ref(), password)
            .map_err(|e| e.to_string())?;
        let file = WalletFile {
            schema: SCHEMA.into(),
            chain_id: QUANTIC_PQ_REGTEST_CHAIN_ID.into(),
            name: name.into(),
            address: address.clone(),
            encrypted_seed_hex: hex::encode(encrypted),
        };
        let json = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, &json))
            .map_err(|e| format!("no pude escribir {}: {e}", path.display()))?;
        Ok((phrase, address))
    }

    pub fn address(&self, name: &str) -> Result<String, String> {
        Ok(read_wallet(&self.dir, name)?.address)
    }

    pub fn balance(&self, name: &str, password: &str) -> Result<Balance, String> {
        let account = self.open_account(name, password)?;
        let position = self.position(&account)?;
        Ok(Balance {
            height: position.height,
            spendable_atoms: position.spendable.iter().map(|note| note.value).sum(),
            immature_atoms: position.immature_atoms,
            spendable_notes: position.spendable.len(),
            immature_notes: position.immature_notes,
        })
    }

    /// Mina un bloque vacío hacia la wallet. La recompensa del génesis se puede
    /// gastar enseguida; las siguientes esperan [`REWARD_MATURITY`] bloques.
    pub fn mine_to(&self, name: &str, password: &str, timestamp: u64) -> Result<Mined, String> {
        let account = self.open_account(name, password)?;
        let position = self.position(&account)?;
        self.mine_block(&account.address(), Vec::new(), &position, timestamp)
    }

    /// Arma un pago privado y mina el bloque que lo confirma. En esta red local
    /// la misma wallet mina: no hay mempool compartido con otro nodo.
    pub fn send(
        &self,
        name: &str,
        password: &str,
        to: &str,
        amount: u64,
        timestamp: u64,
    ) -> Result<Sent, String> {
        if amount == 0 {
            return Err("el monto tiene que ser mayor que cero".into());
        }
        let account = self.open_account(name, password)?;
        let destination =
            Address::decode(to, NETWORK).map_err(|e| format!("dirección de destino: {e}"))?;
        let position = self.position(&account)?;
        if position.height == 0 {
            return Err("la cadena está vacía: primero miná un bloque".into());
        }
        let (inputs, fee, change) =
            inputs_for(&account, &position.ledger, &position.spendable, amount)?;
        let mut payments = vec![Payment::new(destination, amount)];
        if change > 0 {
            payments.push(Payment::new(account.address(), change));
        }
        let expiry = u32::try_from(position.height)
            .unwrap_or(u32::MAX)
            .saturating_add(40);
        let payment = transfer(
            &account,
            position.ledger.tree.root(),
            inputs,
            payments,
            fee,
            expiry,
        )
        .map_err(|e| format!("no pude armar el pago: {e}"))?;
        let mut pool = stark::Mempool::default();
        pool.insert(&position.ledger, payment.clone())
            .map_err(|e| format!("el pago no entra al mempool: {e}"))?;
        let mined = self.mine_block(&account.address(), pool.select(), &position, timestamp)?;
        Ok(Sent {
            txid: hex::encode(payment.txid()),
            height: mined.height,
            fee_atoms: fee,
        })
    }

    fn mine_block(
        &self,
        to: &Address,
        txs: Vec<Transaction>,
        position: &Position,
        timestamp: u64,
    ) -> Result<Mined, String> {
        let block = stark::mine(&self.storage, &position.parent, to, txs, timestamp)?;
        let head = apply_block(&self.storage, QUANTIC_PQ_REGTEST_CHAIN_ID, &block)
            .map_err(|e| e.to_string())?;
        Ok(Mined {
            height: head.height,
            hash: head.hash,
        })
    }

    fn open_account(&self, name: &str, password: &str) -> Result<Account, String> {
        let file = read_wallet(&self.dir, name)?;
        let encrypted =
            hex::decode(file.encrypted_seed_hex).map_err(|e| format!("wallet dañada: {e}"))?;
        if !encrypted.starts_with(b"ZOKAENC1") {
            return Err("esta wallet requiere cifrado autenticado; XOR no es válido".into());
        }
        let seed = Zeroizing::new(
            crate::wallet::storage::decrypt_data(&encrypted, password)
                .map_err(|_| "no se pudo abrir la wallet (contraseña o archivo)".to_string())?,
        );
        let seed: &[u8; 32] = seed
            .as_slice()
            .try_into()
            .map_err(|_| "la wallet no guarda una semilla de 32 bytes".to_string())?;
        let account = account_from_seed(seed)?;
        let address = account.address().encode().map_err(|e| e.to_string())?;
        if address != file.address {
            return Err("la dirección guardada no sale de la semilla".into());
        }
        Ok(account)
    }

    fn position(&self, account: &Account) -> Result<Position, String> {
        let height = self.storage.get_chain_height().map_err(|e| e.to_string())?;
        if height == 0 {
            return Ok(Position {
                height,
                parent: genesis_parent_for(QUANTIC_PQ_REGTEST_CHAIN_ID),
                ledger: Ledger::new(NETWORK).map_err(|e| e.to_string())?,
                spendable: Vec::new(),
                immature_atoms: 0,
                immature_notes: 0,
            });
        }
        let mut blocks = Vec::with_capacity(height as usize);
        for at in 1..=height {
            let block = self
                .storage
                .get_canonical_block_by_height(at)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("falta el bloque canónico {at}"))?;
            blocks.push(block);
        }
        let parent = blocks.last().expect("height > 0").clone();
        let ledger = stark::state_after(&self.storage, &parent)?;
        let inserted = commitment_order(&blocks)?;
        if inserted.len() as u64 != ledger.tree.len() {
            return Err("el orden de las notas no coincide con el árbol guardado".into());
        }
        let mut index_of = BTreeMap::new();
        for (index, commitment) in inserted.iter().enumerate() {
            let stored = ledger.tree.commitment(index as u32);
            if stored != Some(*commitment) {
                return Err("una nota del árbol no está donde la cadena dice".into());
            }
            index_of.insert(*commitment, index as u32);
        }
        let mut spendable = Vec::new();
        let mut immature_atoms = 0u64;
        let mut immature_notes = 0usize;
        let mut seen_nullifiers = BTreeSet::new();
        for block in &blocks {
            for tx in stark::decode(block)? {
                for (commitment, envelope) in tx.statement.commitments.iter().zip(&tx.envelopes) {
                    let Ok((note, _)) = account.receive(*commitment, envelope) else {
                        continue;
                    };
                    let nullifier = note
                        .nullifier(&account.spend, NETWORK)
                        .map_err(|e| e.to_string())?;
                    if ledger.spent.contains(&nullifier) || !seen_nullifiers.insert(nullifier) {
                        continue;
                    }
                    match index_of.get(commitment) {
                        Some(index) => {
                            let value = note.value;
                            spendable.push(SpendableNote {
                                note,
                                index: *index,
                                value,
                            });
                        }
                        None => {
                            immature_atoms = immature_atoms.saturating_add(note.value);
                            immature_notes += 1;
                        }
                    }
                }
            }
        }
        spendable.sort_by_key(|note| note.index);
        Ok(Position {
            height,
            parent,
            ledger,
            spendable,
            immature_atoms,
            immature_notes,
        })
    }
}

/// Adopt only a previously validated STARK v2 database. An existing empty DB
/// has no chain identity to establish, so it is deliberately not adopted.
fn validate_existing_v2(storage: &RocksDbStorage) -> Result<(), String> {
    let height = storage.get_chain_height().map_err(|e| e.to_string())?;
    if height == 0 {
        return Err("base existente sin identidad v2: usá una carpeta nueva".into());
    }
    let genesis = storage
        .get_canonical_block_by_height(1)
        .map_err(|e| e.to_string())?
        .ok_or("falta el génesis de la base existente")?;
    let parent = genesis_parent_for(QUANTIC_PQ_REGTEST_CHAIN_ID);
    if genesis.header.prev_hash != parent.header.hash {
        return Err("el génesis de esa base pertenece a otra red".into());
    }
    let difficulty = super::compact_target_for_child(storage, QUANTIC_PQ_REGTEST_CHAIN_ID, &parent)
        .map_err(|e| e.to_string())?;
    crate::mining::block_validator::validate_pow_block(
        &genesis,
        &crate::mining::block_validator::PowBlockContext {
            chain_id: QUANTIC_PQ_REGTEST_CHAIN_ID,
            parent: &parent,
            parent_total_supply_emitted: 0,
            expected_difficulty_bits: difficulty,
        },
    )
    .map_err(|e| format!("génesis v2 inválido: {e}"))?;
    stark::validate(storage, &genesis, &parent)?;
    stark::state_after(storage, &genesis)?;
    let hash = storage.get_last_block_hash().map_err(|e| e.to_string())?;
    let tip = storage
        .get_canonical_block_by_height(height)
        .map_err(|e| e.to_string())?
        .ok_or("falta la punta canónica de la base existente")?;
    if tip.header.hash != hash {
        return Err("la punta de la base existente no coincide con el índice".into());
    }
    stark::state_after(storage, &tip)?;
    Ok(())
}

fn account_from_seed(seed: &[u8; 32]) -> Result<Account, String> {
    Account::from_seed(seed, NETWORK, 0).map_err(|e| e.to_string())
}

fn wallet_path(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        return Err("el nombre de la wallet: de 1 a 64 letras, números, '-' o '_'".into());
    }
    Ok(dir.join("wallets").join(format!("{name}.json")))
}

fn read_wallet(dir: &Path, name: &str) -> Result<WalletFile, String> {
    let path = wallet_path(dir, name)?;
    let bytes = std::fs::read(&path).map_err(|_| format!("no existe la wallet {name}"))?;
    let file: WalletFile =
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if file.schema != SCHEMA || file.chain_id != QUANTIC_PQ_REGTEST_CHAIN_ID {
        return Err("esa wallet no es de la red local postcuántica".into());
    }
    Ok(file)
}

/// El mismo orden en que `Ledger::apply` mete compromisos al árbol.
fn commitment_order(
    blocks: &[crate::types::Block],
) -> Result<Vec<protocol::crypto::Digest>, String> {
    let mut order = Vec::new();
    let mut pending: BTreeMap<u32, Vec<protocol::crypto::Digest>> = BTreeMap::new();
    for block in blocks {
        let txs = stark::decode(block)?;
        if txs.is_empty() {
            return Err(format!(
                "el bloque {} no trae coinbase",
                block.header.block_height
            ));
        }
        let height = u32::try_from(block.header.block_height)
            .map_err(|_| "altura fuera de rango".to_string())?;
        if let Some(matured) = pending.remove(&height) {
            order.extend(matured);
        }
        if height == 1 {
            order.extend(txs[0].statement.commitments.iter().copied());
        } else {
            let at = height
                .checked_add(REWARD_MATURITY)
                .ok_or("la maduración se pasa de rango")?;
            pending.insert(at, txs[0].statement.commitments.clone());
        }
        for tx in txs.iter().skip(1) {
            order.extend(tx.statement.commitments.iter().copied());
        }
    }
    Ok(order)
}

fn inputs_for(
    account: &Account,
    ledger: &Ledger,
    notes: &[SpendableNote],
    amount: u64,
) -> Result<(Vec<Input>, u64, u64), String> {
    let mut chosen = Vec::new();
    let mut sum = 0u64;
    for note in notes {
        if chosen.len() == MAX_INPUTS {
            break;
        }
        chosen.push(note);
        sum = sum
            .checked_add(note.value)
            .ok_or("los montos se desbordan")?;
        let fee = conventional_fee(chosen.len(), 2);
        let Some(need) = amount.checked_add(fee) else {
            continue;
        };
        if sum < need {
            continue;
        }
        let change = sum - need;
        let inputs = chosen
            .iter()
            .map(|note| {
                Ok(Input {
                    key: account.spend.clone(),
                    note: note.note.clone(),
                    path: ledger.tree.path(note.index).map_err(|e| e.to_string())?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        return Ok((inputs, fee, change));
    }
    Err(format!(
        "fondos insuficientes para {amount} átomos más la comisión (hay {} átomos gastables)",
        notes
            .iter()
            .map(|note| note.value)
            .fold(0u64, u64::saturating_add)
    ))
}

pub fn command(args: &[String]) -> Result<(), String> {
    if args.is_empty() || args[0] == "help" || args[0] == "--help" {
        println!("{USAGE}");
        return Ok(());
    }
    let action = args[0].as_str();
    let dir = required(args, "--dir")?;
    let name = required(args, "--name")?;
    let chain = Chain::open(Path::new(&dir))?;
    match action {
        "new" => {
            let (phrase, address) = chain.create_wallet(&name, &password_from(args)?)?;
            println!("dirección: {address}");
            println!("frase (se muestra una sola vez):\n{}", &*phrase);
            Ok(())
        }
        "address" => {
            println!("{}", chain.address(&name)?);
            Ok(())
        }
        "balance" => {
            let balance = chain.balance(&name, &password_from(args)?)?;
            println!("altura: {}", balance.height);
            println!(
                "gastable: {} átomos ({} notas)",
                balance.spendable_atoms, balance.spendable_notes
            );
            println!(
                "por madurar: {} átomos ({} notas)",
                balance.immature_atoms, balance.immature_notes
            );
            println!("1 moneda = {ATOMS} átomos");
            Ok(())
        }
        "mine" => {
            let mined = chain.mine_to(&name, &password_from(args)?, now_secs())?;
            println!("bloque {} {}", mined.height, mined.hash);
            Ok(())
        }
        "send" => {
            let to = required(args, "--to")?;
            let amount = parse_atoms(&required(args, "--amount")?)?;
            let sent = chain.send(&name, &password_from(args)?, &to, amount, now_secs())?;
            println!("pago {} en el bloque {}", sent.txid, sent.height);
            println!("comisión: {} átomos", sent.fee_atoms);
            Ok(())
        }
        other => Err(format!("acción desconocida: {other}\n{USAGE}")),
    }
}

fn required(args: &[String], name: &str) -> Result<String, String> {
    flag(args, name).ok_or_else(|| format!("falta {name}"))
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1).cloned())
}

fn password_from(args: &[String]) -> Result<Zeroizing<String>, String> {
    let var = required(args, "--password-env")?;
    std::env::var(&var)
        .map(Zeroizing::new)
        .map_err(|_| format!("la variable {var} no está definida"))
}

fn parse_atoms(raw: &str) -> Result<u64, String> {
    let amount: u64 = raw
        .parse()
        .map_err(|_| format!("--amount tiene que ser un entero de átomos, recibido {raw}"))?;
    if amount == 0 {
        return Err("el monto tiene que ser mayor que cero".into());
    }
    Ok(amount)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::ledger::{subsidy, PREMINE};

    #[test]
    fn wallet_file_rejects_a_wrong_password_and_a_second_create() {
        let dir = tempfile::tempdir().unwrap();
        let chain = Chain::open(dir.path()).unwrap();
        let (phrase, address) = chain.create_wallet("alice", "clave buena").unwrap();
        assert!(address.starts_with("zqregtest2"));
        assert_eq!(phrase.split_whitespace().count(), 24);
        assert_eq!(chain.address("alice").unwrap(), address);
        assert!(chain.create_wallet("alice", "otra").is_err());
        assert!(chain.balance("alice", "clave mala").is_err());
        assert_eq!(chain.balance("alice", "clave buena").unwrap().height, 0);
        let path = wallet_path(dir.path(), "alice").unwrap();
        let mut file = read_wallet(dir.path(), "alice").unwrap();
        file.encrypted_seed_hex = hex::encode([0u8; 32]);
        std::fs::write(path, serde_json::to_vec(&file).unwrap()).unwrap();
        assert!(chain
            .open_account("alice", "clave buena")
            .err()
            .unwrap()
            .contains("XOR"));
    }

    #[test]
    fn local_regtest_wallet_mines_and_pays() {
        let dir = tempfile::tempdir().unwrap();
        let chain = Chain::open(dir.path()).unwrap();
        let (_phrase, alice) = chain.create_wallet("alice", "clave-alice").unwrap();
        let (_phrase, bob) = chain.create_wallet("bob", "clave-bob").unwrap();
        let genesis = chain
            .mine_to("alice", "clave-alice", 1_700_000_000)
            .unwrap();
        assert_eq!(genesis.height, 1);
        let mined = chain.balance("alice", "clave-alice").unwrap();
        assert_eq!(mined.spendable_atoms, PREMINE);
        assert_eq!(mined.immature_atoms, 0);
        assert!(chain
            .send("alice", "clave-alice", &bob, PREMINE, 1_700_000_100)
            .is_err());
        assert_eq!(chain.balance("alice", "clave-alice").unwrap().height, 1);

        let sent = chain
            .send("alice", "clave-alice", &bob, ATOMS, 1_700_000_200)
            .unwrap();
        assert_eq!(sent.height, 2);
        assert_eq!(sent.fee_atoms, conventional_fee(1, 2));
        let bob_balance = chain.balance("bob", "clave-bob").unwrap();
        assert_eq!(bob_balance.spendable_atoms, ATOMS);
        assert_eq!(bob_balance.immature_atoms, 0);
        let alice_balance = chain.balance("alice", "clave-alice").unwrap();
        assert_eq!(
            alice_balance.spendable_atoms,
            PREMINE - ATOMS - sent.fee_atoms
        );
        assert_eq!(alice_balance.immature_atoms, subsidy(2) + sent.fee_atoms);
        assert_eq!(alice_balance.height, 2);
        let _ = alice;
        // Adopt a DB produced by the earlier v2 tool, which had no marker.
        chain.storage.delete_raw(DB_MARKER).unwrap();
        drop(chain);
        let adopted = Chain::open(dir.path()).unwrap();
        assert_eq!(
            adopted.storage.get_raw(DB_MARKER).unwrap().unwrap(),
            QUANTIC_PQ_REGTEST_CHAIN_ID.as_bytes()
        );
        assert_eq!(
            adopted.balance("bob", "clave-bob").unwrap().spendable_atoms,
            ATOMS
        );
    }

    #[test]
    fn refuses_unmarked_empty_or_foreign_databases() {
        let empty = tempfile::tempdir().unwrap();
        let storage = RocksDbStorage::new(empty.path().join("chain")).unwrap();
        assert!(Chain::open(empty.path()).is_err());
        assert!(storage.get_raw(DB_MARKER).unwrap().is_none());
        let foreign = tempfile::tempdir().unwrap();
        let storage = RocksDbStorage::new(foreign.path().join("chain")).unwrap();
        let mut genesis = genesis_parent_for(crate::economics::QUANTIC_TESTNET_CHAIN_ID);
        genesis.header.prev_hash = genesis.header.hash.clone();
        genesis.header.block_height = 1;
        storage.save_block(&genesis).unwrap();
        storage.set_chain_height(1).unwrap();
        storage.set_last_block_hash(&genesis.header.hash).unwrap();
        assert!(Chain::open(foreign.path()).is_err());
        assert!(storage.get_raw(DB_MARKER).unwrap().is_none());
    }

    #[test]
    fn duplicate_notes_do_not_inflate_the_wallet_balance() {
        use protocol::model::{Statement, Witness};
        use protocol::wallet::encrypt_note;
        let dir = tempfile::tempdir().unwrap();
        let chain = Chain::open(dir.path()).unwrap();
        chain.create_wallet("alice", "alice password").unwrap();
        chain.create_wallet("bob", "bob password").unwrap();
        chain
            .mine_to("alice", "alice password", 1_700_000_000)
            .unwrap();
        let alice = chain.open_account("alice", "alice password").unwrap();
        let bob = chain.open_account("bob", "bob password").unwrap();
        let position = chain.position(&alice).unwrap();
        let source = &position.spendable[0];
        let fee = conventional_fee(1, 3);
        let (note, envelope) =
            encrypt_note(&bob.address(), ATOMS, &[0; 512], Some(&alice)).unwrap();
        let (change, change_envelope) = encrypt_note(
            &alice.address(),
            PREMINE - 2 * ATOMS - fee,
            &[0; 512],
            Some(&alice),
        )
        .unwrap();
        let witness = Witness {
            inputs: vec![Input {
                key: alice.spend.clone(),
                note: source.note.clone(),
                path: position.ledger.tree.path(source.index).unwrap(),
            }],
            outputs: vec![note.clone(), note, change],
        };
        let statement = Statement::from_witness(
            NETWORK,
            position.ledger.tree.root(),
            fee,
            0,
            [0; 32],
            &witness,
        )
        .unwrap();
        let mut tx = Transaction {
            statement,
            height: 40,
            envelopes: vec![envelope.clone(), envelope, change_envelope],
            proof: protocol::ProofEnvelope {
                version: 2,
                proof: vec![],
            },
        };
        tx.statement.binding = tx.effects_digest().unwrap();
        tx.proof = protocol::prove_transfer(&tx.statement, &witness).unwrap();
        chain
            .mine_block(&alice.address(), vec![tx], &position, 1_700_000_120)
            .unwrap();
        let balance = chain.balance("bob", "bob password").unwrap();
        assert_eq!(balance.spendable_atoms, ATOMS);
        assert_eq!(balance.spendable_notes, 1);
    }
}
