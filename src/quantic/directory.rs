//! Directorio de direcciones de Zoka Quantic (`docs/QUANTIC-DIRECCIONES-CORTAS.md`).
//!
//! Una dirección corta es el resumen de una completa: para pagarle, la wallet
//! busca la completa acá y la comprueba contra el resumen. El nodo guarda cada
//! dirección completa que alguien publique con un sello de trabajo válido y la
//! difunde por gossip; nadie puede cambiar una dirección guardada (el resumen
//! la ata entera), solo no tenerla.
//!
//! El sello frena el spam sin pedir monedas: un nonce tal que Argon2id (8 MiB)
//! de `prefijo ‖ resumen ‖ nonce` empiece con [`STAMP_BITS`] bits en cero. Son
//! unas 2^8 pruebas para quien publica y una sola para quien verifica.
//!
//! Primera versión, para medir en la testnet: los nodos nuevos se ponen al día
//! porque cada ZSilent vuelve a publicar sus direcciones al arrancar. Si hace
//! falta sincronizar entre nodos o llevarlo a la cadena, se decide antes de la
//! mainnet.

use std::fmt;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use zoka_quantic::address::AddressError;
use zoka_quantic::{Network, QuanticAddress, QuanticShortAddress};

use crate::storage::RocksDbStorage;

/// Bits en cero que pide el sello.
pub const STAMP_BITS: u32 = 8;
/// Memoria de cada prueba de Argon2id, en KiB.
pub const STAMP_MEMORY_KIB: u32 = 8 * 1024;
/// Cuántas direcciones guarda un nodo como mucho (unos 2,6 GB).
pub const MAX_RECORDS: u64 = 2_000_000;
/// Registros de un par que el nodo verifica por minuto; los demás se descartan
/// en silencio. Una persona publica una o dos direcciones por día.
pub const GOSSIP_RECORDS_PER_PEER_PER_MINUTE: u32 = 30;

const RECORD_PREFIX: &str = "quantic:addr:";
const COUNT_KEY: &str = "quantic:addr-count";
/// Las direcciones que se publicaron desde la API de este nodo: el nodo las
/// vuelve a difundir al arrancar y cada [`REPUBLISH_EVERY_SECS`], para que se
/// enteren los nodos que llegaron después.
const LOCAL_KEY: &str = "quantic:addr-local";
pub const REPUBLISH_EVERY_SECS: u64 = 6 * 60 * 60;
/// Tope de direcciones propias que se vuelven a difundir.
const MAX_LOCAL: usize = 64;
const STAMP_SALT: &[u8] = b"ZokaQ_AddrStampV1";

/// Una dirección completa con su sello, como viaja y como se guarda.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressRecord {
    pub address: String,
    pub stamp: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DirectoryError {
    Address(AddressError),
    Stamp,
    Full,
    Storage(String),
}

impl fmt::Display for DirectoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DirectoryError::Address(error) => write!(f, "{error}"),
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

fn stamp_hash(short: &QuanticShortAddress, stamp: u64) -> [u8; 32] {
    static ARGON: OnceLock<argon2::Argon2<'static>> = OnceLock::new();
    let argon = ARGON.get_or_init(|| {
        let params = argon2::Params::new(STAMP_MEMORY_KIB, 1, 1, Some(32)).expect("fixed Argon2 parameters");
        argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
    });
    let mut input = Vec::with_capacity(48);
    input.extend_from_slice(short.network().address_prefix().as_bytes());
    input.extend_from_slice(short.digest());
    input.extend_from_slice(&stamp.to_le_bytes());
    let mut out = [0u8; 32];
    argon
        .hash_password_into(&input, STAMP_SALT, &mut out)
        .expect("Argon2 with fixed parameters and salt");
    out
}

fn leading_zero_bits(bytes: &[u8]) -> u32 {
    let mut bits = 0;
    for byte in bytes {
        if *byte == 0 {
            bits += 8;
        } else {
            return bits + byte.leading_zeros();
        }
    }
    bits
}

pub fn stamp_is_valid(short: &QuanticShortAddress, stamp: u64) -> bool {
    leading_zero_bits(&stamp_hash(short, stamp)) >= STAMP_BITS
}

/// Busca un sello para `short`: unas 2^[`STAMP_BITS`] pruebas de Argon2id,
/// algunos segundos en una PC.
pub fn find_stamp(short: &QuanticShortAddress) -> u64 {
    let start: u64 = rand::random();
    (0u64..)
        .map(|i| start.wrapping_add(i))
        .find(|stamp| stamp_is_valid(short, *stamp))
        .expect("an unbounded search ends")
}

impl AddressRecord {
    /// El registro de `full`, con un sello nuevo.
    pub fn new(full: &QuanticAddress) -> Self {
        Self {
            address: full.encode(),
            stamp: find_stamp(&full.short()),
        }
    }

    /// La dirección completa, si el registro es de `network` y el sello vale.
    pub fn verify(&self, network: Network) -> Result<QuanticAddress, DirectoryError> {
        let full = QuanticAddress::decode_for(self.address.trim(), network).map_err(DirectoryError::Address)?;
        if !stamp_is_valid(&full.short(), self.stamp) {
            return Err(DirectoryError::Stamp);
        }
        Ok(full)
    }
}

fn record_key(short: &QuanticShortAddress) -> String {
    format!(
        "{RECORD_PREFIX}{}{}",
        short.network().address_prefix(),
        hex::encode(short.digest())
    )
}

fn record_count(storage: &RocksDbStorage) -> Result<u64, DirectoryError> {
    Ok(storage
        .get_raw(COUNT_KEY)
        .map_err(storage_error)?
        .and_then(|bytes| <[u8; 8]>::try_from(bytes.as_slice()).ok())
        .map(u64::from_le_bytes)
        .unwrap_or(0))
}

/// Guarda un registro válido de `network`. Devuelve la dirección corta y si el
/// registro era nuevo.
pub fn store(
    storage: &RocksDbStorage,
    network: Network,
    record: &AddressRecord,
) -> Result<(QuanticShortAddress, bool), DirectoryError> {
    let full = record.verify(network)?;
    let short = full.short();
    let key = record_key(&short);
    if storage.get_raw(&key).map_err(storage_error)?.is_some() {
        return Ok((short, false));
    }
    let count = record_count(storage)?;
    if count >= MAX_RECORDS {
        return Err(DirectoryError::Full);
    }
    let stored = AddressRecord {
        address: full.encode(),
        stamp: record.stamp,
    };
    let bytes = serde_json::to_vec(&stored).map_err(storage_error)?;
    storage.put_raw(&key, bytes).map_err(storage_error)?;
    storage
        .put_raw(COUNT_KEY, (count + 1).to_le_bytes().to_vec())
        .map_err(storage_error)?;
    Ok((short, true))
}

/// La dirección completa de `short`, si este nodo la tiene. El resumen se
/// vuelve a comprobar al leer: una base dañada no da una dirección ajena.
pub fn lookup(storage: &RocksDbStorage, short: &QuanticShortAddress) -> Result<Option<QuanticAddress>, DirectoryError> {
    Ok(lookup_record(storage, short)?.map(|(full, _)| full))
}

/// Como [`lookup`], con el registro guardado (para volver a difundirlo).
pub fn lookup_record(
    storage: &RocksDbStorage,
    short: &QuanticShortAddress,
) -> Result<Option<(QuanticAddress, AddressRecord)>, DirectoryError> {
    let Some(bytes) = storage.get_raw(&record_key(short)).map_err(storage_error)? else {
        return Ok(None);
    };
    let record: AddressRecord = serde_json::from_slice(&bytes).map_err(storage_error)?;
    let full = QuanticAddress::decode_for(&record.address, short.network()).map_err(DirectoryError::Address)?;
    Ok(short.matches(&full).then_some((full, record)))
}

/// Anota `short` entre las publicadas desde este nodo.
pub fn mark_local(storage: &RocksDbStorage, short: &QuanticShortAddress) -> Result<(), DirectoryError> {
    let mut local = local_addresses(storage, short.network())?;
    if local.contains(short) {
        return Ok(());
    }
    local.push(*short);
    if local.len() > MAX_LOCAL {
        local.remove(0);
    }
    let text: Vec<String> = local.iter().map(|short| short.encode()).collect();
    let bytes = serde_json::to_vec(&text).map_err(storage_error)?;
    storage.put_raw(LOCAL_KEY, bytes).map_err(storage_error)
}

/// Las direcciones publicadas desde este nodo, de la más vieja a la más nueva.
pub fn local_addresses(storage: &RocksDbStorage, network: Network) -> Result<Vec<QuanticShortAddress>, DirectoryError> {
    let Some(bytes) = storage.get_raw(LOCAL_KEY).map_err(storage_error)? else {
        return Ok(Vec::new());
    };
    let text: Vec<String> = serde_json::from_slice(&bytes).map_err(storage_error)?;
    Ok(text
        .iter()
        .filter_map(|short| QuanticShortAddress::decode_for(short, network).ok())
        .collect())
}

/// Los registros de las direcciones publicadas desde este nodo, para volver a
/// difundirlos.
pub fn local_records(storage: &RocksDbStorage, network: Network) -> Vec<AddressRecord> {
    local_addresses(storage, network)
        .unwrap_or_default()
        .iter()
        .filter_map(|short| lookup_record(storage, short).ok().flatten())
        .map(|(_, record)| record)
        .collect()
}

/// Un registro que llegó por la red: con presupuesto por par, porque verificar
/// el sello lleva unos milisegundos y corre en el bucle del nodo.
pub fn accept_network_record(
    storage: &RocksDbStorage,
    chain_id: &str,
    record: AddressRecord,
    source: Option<libp2p::PeerId>,
) -> Option<String> {
    let network = super::network_for_chain(chain_id)?;
    if let Some(source) = source {
        let minute = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs() / 60)
            .unwrap_or(0);
        if !gossip_budget_allows(&source, minute) {
            return None;
        }
    }
    match store(storage, network, &record) {
        Ok((short, true)) => Some(format!("Address directory: stored {}.", short.encode())),
        Ok((_, false)) => None,
        Err(error) => Some(format!("Ignored address record: {error}")),
    }
}

fn gossip_budget_allows(source: &libp2p::PeerId, minute: u64) -> bool {
    type Table = std::collections::HashMap<libp2p::PeerId, (u64, u32)>;
    static BUDGET: OnceLock<std::sync::Mutex<Table>> = OnceLock::new();
    let Ok(mut table) = BUDGET.get_or_init(Default::default).lock() else {
        return true;
    };
    if table.len() > 1024 {
        table.retain(|_, bucket| bucket.0 == minute);
    }
    let bucket = table.entry(*source).or_insert((minute, 0));
    if bucket.0 != minute {
        *bucket = (minute, 0);
    }
    bucket.1 += 1;
    bucket.1 <= GOSSIP_RECORDS_PER_PEER_PER_MINUTE
}

#[cfg(test)]
mod tests {
    use super::*;
    use zoka_quantic::QuanticSpendingKey;

    fn address(network: Network, seed: u8) -> QuanticAddress {
        QuanticSpendingKey::from_root(&[seed; 32], network, 0)
            .unwrap()
            .full_viewing_key()
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
        let full = address(Network::Testnet, 7);
        let record = AddressRecord::new(&full);
        assert!(stamp_is_valid(&full.short(), record.stamp));
        assert_eq!(store(&storage, Network::Testnet, &record), Ok((full.short(), true)));
        assert_eq!(store(&storage, Network::Testnet, &record), Ok((full.short(), false)));
        assert_eq!(lookup(&storage, &full.short()), Ok(Some(full.clone())));
        assert_eq!(lookup(&storage, &address(Network::Testnet, 8).short()), Ok(None));
        assert_eq!(record_count(&storage), Ok(1));
    }

    #[test]
    fn a_record_without_work_or_of_another_network_is_refused() {
        let (storage, _dir) = storage();
        let full = address(Network::Testnet, 7);
        let short = full.short();
        let bad = (0u64..).find(|stamp| !stamp_is_valid(&short, *stamp)).unwrap();
        let unstamped = AddressRecord { address: full.encode(), stamp: bad };
        assert_eq!(store(&storage, Network::Testnet, &unstamped), Err(DirectoryError::Stamp));
        let record = AddressRecord::new(&full);
        assert!(matches!(
            store(&storage, Network::Mainnet, &record),
            Err(DirectoryError::Address(AddressError::WrongNetwork { .. }))
        ));
        // Una dirección corta no se puede publicar: el directorio es de completas.
        let short_record = AddressRecord { address: short.encode(), stamp: record.stamp };
        assert_eq!(
            store(&storage, Network::Testnet, &short_record),
            Err(DirectoryError::Address(AddressError::ShortAddress))
        );
        assert_eq!(record_count(&storage), Ok(0));
    }

    // El sello de una dirección no sirve para otra: está atado a su resumen.
    #[test]
    fn a_stamp_belongs_to_its_address() {
        let (storage, _dir) = storage();
        let full = address(Network::Testnet, 7);
        let other = address(Network::Testnet, 9);
        let stamp = (0u64..)
            .find(|stamp| stamp_is_valid(&full.short(), *stamp) && !stamp_is_valid(&other.short(), *stamp))
            .unwrap();
        let borrowed = AddressRecord { address: other.encode(), stamp };
        assert_eq!(store(&storage, Network::Testnet, &borrowed), Err(DirectoryError::Stamp));
    }

    #[test]
    fn the_node_remembers_the_addresses_published_through_it() {
        let (storage, _dir) = storage();
        let mine = address(Network::Testnet, 7);
        let theirs = address(Network::Testnet, 8);
        for full in [&mine, &theirs] {
            store(&storage, Network::Testnet, &AddressRecord::new(full)).unwrap();
        }
        mark_local(&storage, &mine.short()).unwrap();
        mark_local(&storage, &mine.short()).unwrap();
        assert_eq!(local_addresses(&storage, Network::Testnet), Ok(vec![mine.short()]));
        let records = local_records(&storage, Network::Testnet);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].address, mine.encode());
    }

    #[test]
    fn the_work_counts_leading_zero_bits() {
        assert_eq!(leading_zero_bits(&[0xff]), 0);
        assert_eq!(leading_zero_bits(&[0x00, 0x80]), 8);
        assert_eq!(leading_zero_bits(&[0x00, 0x0f]), 12);
        assert_eq!(leading_zero_bits(&[0x00, 0x00]), 16);
    }
}
