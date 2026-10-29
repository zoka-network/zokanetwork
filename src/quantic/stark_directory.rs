//! Address directory of a STARK v2 chain: the v1 design
//! (`docs/QUANTIC-DIRECCIONES-CORTAS.md`, [`super::directory`]) on the v2
//! transport, which has request/response sessions instead of gossip.
//!
//! A short address is the digest of a full one, so a node that serves the full
//! address can withhold it but not change it. A record pays for its storage
//! with an Argon2id work stamp, not a fee. The node keeps every valid record it
//! receives. Each outbound session pushes the node's own records (again every
//! [`REPUBLISH_EVERY`]) and the ones it stored lately, and asks its peer for
//! the short addresses an API caller is waiting for. A seed only holds inbound
//! sessions: publishers push to it and payers ask it.

use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    sync::Mutex,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use super::directory::{leading_zero_bits, work_hash, STAMP_BITS};
use super::stark::protocol::wallet::{address_prefix, Address, ShortAddress};
use crate::storage::RocksDbStorage;

/// Records a node keeps at most: about 1.8 GB of full addresses.
pub const MAX_RECORDS: u64 = 1_000_000;
/// A record as JSON: an address of about 1,760 characters and its stamp.
pub const MAX_RECORD_BYTES: usize = 4096;
/// Records an inbound connection may submit per minute. Each stamp takes a
/// crypto worker a few milliseconds to check.
pub const SUBMITS_PER_MINUTE: u32 = 30;
/// Records a session pushes per minute, below the peer's budget.
pub const PUSHES_PER_MINUTE: u32 = 20;
/// Own records go out again on every session this often, for peers that
/// lost them or arrived later.
pub const REPUBLISH_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// How long an API lookup waits for the outbound sessions.
pub const LOOKUP_TIMEOUT: Duration = Duration::from_secs(20);
/// Wanted short addresses one session asks its peer for per cycle.
pub const LOOKUPS_PER_CYCLE: usize = 8;
const MAX_WANTED: usize = 64;
const RELAY_QUEUE: usize = 256;
const MAX_LOCAL: usize = 64;
const RECORD_PREFIX: &str = "quantic:stark:addr:";
const COUNT_KEY: &str = "quantic:stark:addr-count";
/// Addresses published through this node's API, oldest first.
const LOCAL_KEY: &str = "quantic:stark:addr-local";
const STAMP_SALT: &[u8] = b"ZokaQ_AddrStampV2";

/// A full address and its stamp, as it travels and as it is stored: the same
/// JSON as a v1 record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddressRecord {
    pub address: String,
    pub stamp: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DirectoryError {
    /// Not a full v2 address of this chain.
    Address,
    /// A short address where the full one goes.
    ShortAddress,
    Stamp,
    Full,
    Storage(String),
}

impl fmt::Display for DirectoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DirectoryError::Address => f.write_str("not a full v2 address of this chain"),
            DirectoryError::ShortAddress => {
                f.write_str("a short address cannot be published: publish the full one")
            }
            DirectoryError::Stamp => f.write_str("the address record has no valid work stamp"),
            DirectoryError::Full => f.write_str("the address directory of this node is full"),
            DirectoryError::Storage(error) => write!(f, "address directory storage: {error}"),
        }
    }
}

impl std::error::Error for DirectoryError {}

fn storage_error(error: impl fmt::Debug) -> DirectoryError {
    DirectoryError::Storage(format!("{error:?}"))
}

fn stamp_hash(short: &ShortAddress, stamp: u64) -> [u8; 32] {
    let mut input = Vec::with_capacity(56);
    input.extend_from_slice(address_prefix(short.network).as_bytes());
    input.extend_from_slice(&short.digest);
    input.extend_from_slice(&stamp.to_le_bytes());
    work_hash(STAMP_SALT, &input)
}

pub fn stamp_is_valid(short: &ShortAddress, stamp: u64) -> bool {
    leading_zero_bits(&stamp_hash(short, stamp)) >= STAMP_BITS
}

/// About 2^[`STAMP_BITS`] Argon2id evaluations: seconds on a PC.
pub fn find_stamp(short: &ShortAddress) -> u64 {
    let start: u64 = rand::random();
    (0u64..)
        .map(|i| start.wrapping_add(i))
        .find(|stamp| stamp_is_valid(short, *stamp))
        .expect("an unbounded search ends")
}

/// A full address of `network`, telling a short one apart from garbage.
pub fn decode_full(text: &str, network: u8) -> Result<Address, DirectoryError> {
    let text = text.trim();
    match Address::decode(text, network) {
        Ok(full) => Ok(full),
        Err(_) if ShortAddress::decode(text, network).is_ok() => Err(DirectoryError::ShortAddress),
        Err(_) => Err(DirectoryError::Address),
    }
}

impl AddressRecord {
    /// The record of `full`, with a fresh stamp.
    pub fn new(full: &Address) -> Result<Self, DirectoryError> {
        let short = full.short_address().map_err(|_| DirectoryError::Address)?;
        Ok(Self {
            address: full.encode().map_err(|_| DirectoryError::Address)?,
            stamp: find_stamp(&short),
        })
    }

    /// The full address and its short form, if the record is of `network`
    /// and its stamp holds.
    pub fn verify(&self, network: u8) -> Result<(Address, ShortAddress), DirectoryError> {
        let full = decode_full(&self.address, network)?;
        let short = full.short_address().map_err(|_| DirectoryError::Address)?;
        if !stamp_is_valid(&short, self.stamp) {
            return Err(DirectoryError::Stamp);
        }
        Ok((full, short))
    }
}

fn record_key(digest: &[u8; 32]) -> String {
    format!("{RECORD_PREFIX}{}", hex::encode(digest))
}

pub fn count(storage: &RocksDbStorage) -> Result<u64, DirectoryError> {
    Ok(storage
        .get_raw(COUNT_KEY)
        .map_err(storage_error)?
        .and_then(|bytes| <[u8; 8]>::try_from(bytes.as_slice()).ok())
        .map(u64::from_le_bytes)
        .unwrap_or(0))
}

/// The full address of `short` and its record, if this node has it. The
/// digest is checked again on read: a damaged database cannot answer with
/// someone else's address.
pub fn lookup(
    storage: &RocksDbStorage,
    short: &ShortAddress,
) -> Result<Option<(Address, AddressRecord)>, DirectoryError> {
    let Some(bytes) = storage
        .get_raw(&record_key(&short.digest))
        .map_err(storage_error)?
    else {
        return Ok(None);
    };
    let record: AddressRecord = serde_json::from_slice(&bytes).map_err(storage_error)?;
    let full =
        Address::decode(&record.address, short.network).map_err(|_| DirectoryError::Address)?;
    Ok(short.matches(&full).then_some((full, record)))
}

/// Note `short` among the addresses published through this node.
pub fn mark_local(storage: &RocksDbStorage, short: &ShortAddress) -> Result<(), DirectoryError> {
    let mut local = local(storage, short.network)?;
    if local.contains(short) {
        return Ok(());
    }
    local.push(*short);
    if local.len() > MAX_LOCAL {
        local.remove(0);
    }
    let text: Vec<String> = local
        .iter()
        .map(|short| hex::encode(short.digest))
        .collect();
    let bytes = serde_json::to_vec(&text).map_err(storage_error)?;
    storage.put_raw(LOCAL_KEY, bytes).map_err(storage_error)
}

/// The addresses published through this node, oldest first.
pub fn local(storage: &RocksDbStorage, network: u8) -> Result<Vec<ShortAddress>, DirectoryError> {
    let Some(bytes) = storage.get_raw(LOCAL_KEY).map_err(storage_error)? else {
        return Ok(Vec::new());
    };
    let text: Vec<String> = serde_json::from_slice(&bytes).map_err(storage_error)?;
    Ok(text
        .iter()
        .filter_map(|digest| hex::decode(digest).ok()?.try_into().ok())
        .map(|digest| ShortAddress { network, digest })
        .collect())
}

/// What the node keeps in memory next to the stored records. Reads of the
/// records need no lock.
pub struct Directory {
    /// Serializes the check-then-write of [`Directory::store`].
    write: Mutex<()>,
    /// Digests stored lately, newest last: what the sessions relay.
    relay: Mutex<VecDeque<[u8; 32]>>,
    /// Short addresses an API caller is waiting for, until their deadline.
    wanted: Mutex<BTreeMap<[u8; 32], Instant>>,
    /// Bumped when something new is wanted: wakes the sessions.
    asked: watch::Sender<u64>,
    /// Bumped on every new record: wakes the waiting callers.
    stored: watch::Sender<u64>,
}

impl Default for Directory {
    fn default() -> Self {
        Self {
            write: Mutex::new(()),
            relay: Mutex::new(VecDeque::new()),
            wanted: Mutex::new(BTreeMap::new()),
            asked: watch::Sender::new(0),
            stored: watch::Sender::new(0),
        }
    }
}

impl Directory {
    /// Keep a valid record of `network`. Returns its short address and
    /// whether the record was new. A known address costs no stamp check, and
    /// a new one is checked before the lock.
    pub fn store(
        &self,
        storage: &RocksDbStorage,
        network: u8,
        record: &AddressRecord,
    ) -> Result<(ShortAddress, bool), DirectoryError> {
        let full = decode_full(&record.address, network)?;
        let short = full.short_address().map_err(|_| DirectoryError::Address)?;
        let key = record_key(&short.digest);
        if storage.get_raw(&key).map_err(storage_error)?.is_some() {
            return Ok((short, false));
        }
        if !stamp_is_valid(&short, record.stamp) {
            return Err(DirectoryError::Stamp);
        }
        {
            let _write = self
                .write
                .lock()
                .map_err(|_| storage_error("directory lock poisoned"))?;
            if storage.get_raw(&key).map_err(storage_error)?.is_some() {
                return Ok((short, false));
            }
            let count = count(storage)?;
            if count >= MAX_RECORDS {
                return Err(DirectoryError::Full);
            }
            let stored = AddressRecord {
                address: full.encode().map_err(|_| DirectoryError::Address)?,
                stamp: record.stamp,
            };
            let bytes = serde_json::to_vec(&stored).map_err(storage_error)?;
            storage.put_raw(&key, bytes).map_err(storage_error)?;
            storage
                .put_raw(COUNT_KEY, (count + 1).to_le_bytes().to_vec())
                .map_err(storage_error)?;
        }
        if let Ok(mut relay) = self.relay.lock() {
            relay.push_back(short.digest);
            if relay.len() > RELAY_QUEUE {
                relay.pop_front();
            }
        }
        if let Ok(mut wanted) = self.wanted.lock() {
            wanted.remove(&short.digest);
        }
        self.stored.send_modify(|n| *n += 1);
        Ok((short, true))
    }

    /// Ask the outbound sessions for `short` until `deadline`. False when too
    /// many lookups are already waiting.
    pub fn want(&self, short: &ShortAddress, deadline: Instant) -> bool {
        let Ok(mut wanted) = self.wanted.lock() else {
            return false;
        };
        let now = Instant::now();
        wanted.retain(|_, until| *until > now);
        if wanted.len() >= MAX_WANTED && !wanted.contains_key(&short.digest) {
            return false;
        }
        let until = wanted.entry(short.digest).or_insert(deadline);
        *until = (*until).max(deadline);
        drop(wanted);
        self.asked.send_modify(|n| *n += 1);
        true
    }

    /// The short addresses still wanted.
    pub fn wanted(&self) -> Vec<[u8; 32]> {
        let Ok(mut wanted) = self.wanted.lock() else {
            return Vec::new();
        };
        let now = Instant::now();
        wanted.retain(|_, until| *until > now);
        wanted.keys().copied().collect()
    }

    /// Digests stored lately, oldest first.
    pub fn relayed(&self) -> Vec<[u8; 32]> {
        self.relay
            .lock()
            .map(|relay| relay.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Changes when something new is wanted.
    pub fn asked(&self) -> watch::Receiver<u64> {
        self.asked.subscribe()
    }

    /// Changes on every new record.
    pub fn stored(&self) -> watch::Receiver<u64> {
        self.stored.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quantic::stark::protocol::wallet::Account;

    const NETWORK: u8 = 3;

    fn address(seed: u8) -> Address {
        Account::from_seed(&[seed; 32], NETWORK, 0)
            .unwrap()
            .address()
    }

    fn storage() -> (RocksDbStorage, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage = RocksDbStorage::new(dir.path().to_str().unwrap()).unwrap();
        (storage, dir)
    }

    #[test]
    fn a_published_address_is_found_by_its_short_address() {
        let (storage, _dir) = storage();
        let directory = Directory::default();
        let full = address(7);
        let short = full.short_address().unwrap();
        let record = AddressRecord::new(&full).unwrap();
        let mut stored = directory.stored();
        assert_eq!(
            directory.store(&storage, NETWORK, &record),
            Ok((short, true))
        );
        assert!(stored.has_changed().unwrap());
        stored.mark_unchanged();
        assert_eq!(
            directory.store(&storage, NETWORK, &record),
            Ok((short, false))
        );
        assert!(!stored.has_changed().unwrap());
        assert_eq!(lookup(&storage, &short).unwrap(), Some((full, record)));
        assert_eq!(
            lookup(&storage, &address(8).short_address().unwrap()),
            Ok(None)
        );
        assert_eq!(count(&storage), Ok(1));
        assert_eq!(directory.relayed(), vec![short.digest]);
    }

    #[test]
    fn a_record_without_work_of_another_network_or_short_is_refused() {
        let (storage, _dir) = storage();
        let directory = Directory::default();
        let full = address(7);
        let short = full.short_address().unwrap();
        let bad = (0u64..)
            .find(|stamp| !stamp_is_valid(&short, *stamp))
            .unwrap();
        let unstamped = AddressRecord {
            address: full.encode().unwrap(),
            stamp: bad,
        };
        assert_eq!(
            directory.store(&storage, NETWORK, &unstamped),
            Err(DirectoryError::Stamp)
        );
        let record = AddressRecord::new(&full).unwrap();
        assert_eq!(
            directory.store(&storage, 2, &record),
            Err(DirectoryError::Address)
        );
        let short_record = AddressRecord {
            address: short.encode(),
            stamp: record.stamp,
        };
        assert_eq!(
            directory.store(&storage, NETWORK, &short_record),
            Err(DirectoryError::ShortAddress)
        );
        let v1 = AddressRecord {
            address: "zqregtest1abc".into(),
            stamp: record.stamp,
        };
        assert_eq!(
            directory.store(&storage, NETWORK, &v1),
            Err(DirectoryError::Address)
        );
        assert_eq!(count(&storage), Ok(0));
        assert!(directory.relayed().is_empty());
    }

    // The stamp of one address does not serve another: it is bound to its
    // digest, and to the v2 salt.
    #[test]
    fn a_stamp_belongs_to_its_address() {
        let (storage, _dir) = storage();
        let directory = Directory::default();
        let full = address(7);
        let other = address(9);
        let (mine, theirs) = (
            full.short_address().unwrap(),
            other.short_address().unwrap(),
        );
        let stamp = (0u64..)
            .find(|stamp| stamp_is_valid(&mine, *stamp) && !stamp_is_valid(&theirs, *stamp))
            .unwrap();
        let borrowed = AddressRecord {
            address: other.encode().unwrap(),
            stamp,
        };
        assert_eq!(
            directory.store(&storage, NETWORK, &borrowed),
            Err(DirectoryError::Stamp)
        );
    }

    #[test]
    fn the_node_remembers_what_was_published_through_it() {
        let (storage, _dir) = storage();
        let directory = Directory::default();
        let mine = address(7).short_address().unwrap();
        for seed in [7, 8] {
            directory
                .store(
                    &storage,
                    NETWORK,
                    &AddressRecord::new(&address(seed)).unwrap(),
                )
                .unwrap();
        }
        mark_local(&storage, &mine).unwrap();
        mark_local(&storage, &mine).unwrap();
        assert_eq!(local(&storage, NETWORK), Ok(vec![mine]));
    }

    #[test]
    fn a_wanted_address_waits_until_it_arrives_or_expires() {
        let (storage, _dir) = storage();
        let directory = Directory::default();
        let full = address(7);
        let short = full.short_address().unwrap();
        let asked = directory.asked();
        let later = Instant::now() + Duration::from_secs(60);
        assert!(directory.want(&short, later));
        assert!(asked.has_changed().unwrap());
        assert_eq!(directory.wanted(), vec![short.digest]);
        directory
            .store(&storage, NETWORK, &AddressRecord::new(&full).unwrap())
            .unwrap();
        assert!(directory.wanted().is_empty());
        let other = address(8).short_address().unwrap();
        assert!(directory.want(&other, Instant::now()));
        assert!(directory.wanted().is_empty());
        for i in 0..MAX_WANTED {
            let digest = blake3::hash(&i.to_le_bytes()).into();
            assert!(directory.want(
                &ShortAddress {
                    network: NETWORK,
                    digest
                },
                later
            ));
        }
        assert!(!directory.want(&other, later));
    }
}
