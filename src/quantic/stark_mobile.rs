//! What the phone wallet (ZKAPriv) asks of the network, without a node of its
//! own: pages of the chain, sending a payment and the short-address
//! directory. Straight to the seeds (iroh, with n0's relays), or only through
//! Tor: the stream transport over a SOCKS port, the embedded Arti's on the
//! phone, to the seeds' onion addresses (`docs/MODO-TOR.md`). Over Tor there
//! is no other way out: if no onion seed answers, nothing is sent.
//!
//! The proof is built on the phone (`zoka_pq_pool::light`); the seed only
//! gets the finished transaction, as from any peer.
use std::{net::SocketAddr, sync::Arc};

use iroh::endpoint::presets;

use super::{
    stark::{
        protocol::{
            scan::{ScanPage2, MAX_SCAN_BLOCKS},
            transaction::Transaction,
            wallet::{Address, ShortAddress},
        },
        Profile,
    },
    stark_directory::AddressRecord,
    stark_net::{self, Client, NodeDescriptor},
    stark_peers::Manifest,
};
use crate::network::stream_transport::{SocksDialer, StreamOnly, StreamTransport};

/// How the phone reaches the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Direct,
    /// Only through Tor, by this SOCKS port.
    Tor(SocketAddr),
}

/// The seeds of the network in this binary (none for a local chain).
pub fn builtin_seeds(profile: Profile) -> Result<Vec<NodeDescriptor>, String> {
    match Manifest::builtin(profile)? {
        Some(manifest) => manifest.seeds(),
        None => Ok(Vec::new()),
    }
}

/// The first of `seeds` that answers on `route`, authenticated by its pinned
/// ML-DSA key like any peer.
pub async fn connect(seeds: &[NodeDescriptor], route: Route) -> Result<Client, String> {
    let seeds = match route {
        Route::Direct => seeds.to_vec(),
        Route::Tor(_) => stark_net::over_tor(seeds.to_vec()),
    };
    let mut last = match route {
        Route::Direct => "no seed to dial".to_string(),
        Route::Tor(_) => "no seed with an onion address".to_string(),
    };
    for seed in &seeds {
        let attempt = match route {
            Route::Direct => Client::connect(seed, presets::N0).await,
            Route::Tor(proxy) => {
                let dialer = StreamTransport::new(None, Arc::new(SocksDialer { proxy }))
                    .map_err(|e| e.to_string())?;
                Client::connect(seed, StreamOnly(dialer)).await
            }
        };
        match attempt {
            Ok(client) => return Ok(client),
            Err(reason) => last = format!("seed {}: {reason}", seed.endpoint_addr.id.fmt_short()),
        }
    }
    Err(last)
}

/// One page of the chain from `from`, at most [`MAX_SCAN_BLOCKS`] blocks.
pub async fn page(client: &mut Client, from: u32) -> Result<ScanPage2, String> {
    client.scan_page(from, MAX_SCAN_BLOCKS).await
}

/// Hand a payment to the network. The answer is its id once the seed's
/// mempool took it; a refusal says why (an expired anchor, say).
pub async fn submit(client: &mut Client, tx: &Transaction) -> Result<[u8; 32], String> {
    client.submit_tx(tx).await
}

/// The full address a short one names, if the directory knows it. The
/// answer is checked against the short address: the directory cannot lie.
pub async fn resolve(
    client: &mut Client,
    short: &ShortAddress,
    network: u8,
) -> Result<Option<Address>, String> {
    let Some(record) = client.get_address(&short.digest).await? else {
        return Ok(None);
    };
    let full = Address::decode(&record.address, network).map_err(|e| e.to_string())?;
    if !short.matches(&full) {
        return Err("the directory answered another address".into());
    }
    Ok(Some(full))
}

/// Offer the wallet's own address to the directory, so that its short form
/// works for others. True if the seed took it (or already had it).
pub async fn publish(client: &mut Client, address: &Address) -> Result<bool, String> {
    let record = AddressRecord::new(address).map_err(|e| e.to_string())?;
    client.submit_address(&record).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::stream_transport::{dial_addr, TcpDialer};
    use crate::quantic::{
        stark::protocol::{light::LightWallet, wallet::Account},
        stark_net::{serve, ServerIdentity},
    };
    use iroh::{EndpointAddr, TransportAddr};

    #[test]
    fn the_testnet_seeds_include_one_reachable_over_tor() {
        let seeds = builtin_seeds(Profile::TESTNET).unwrap();
        assert!(!seeds.is_empty());
        let onion = stark_net::over_tor(seeds);
        assert!(!onion.is_empty(), "Tor mode needs an onion seed");
        assert!(onion.iter().all(|seed| seed
            .endpoint_addr
            .addrs
            .iter()
            .all(|a| matches!(a, TransportAddr::Custom(_)))));
        assert!(builtin_seeds(Profile::REGTEST).unwrap().is_empty());
    }

    /// The phone's whole path over the stream transport (Tor mode without
    /// Tor): pages from a seed, a light wallet that reads them, a payment
    /// proved on the "phone", sent and mined, and the directory.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_phone_reads_pays_and_publishes_over_the_stream_transport() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(crate::storage::RocksDbStorage::new(dir.path()).unwrap());
        storage
            .put_raw(
                crate::quantic::stark_api::DB_MARKER,
                Profile::REGTEST.chain_id.as_bytes().to_vec(),
            )
            .unwrap();
        let state = crate::quantic::stark_api::NodeState::new(storage).unwrap();
        let alice = Arc::new(Account::from_seed(&[81; 32], 3, 0).unwrap());
        let bob = Account::from_seed(&[82; 32], 3, 0).unwrap();
        for _ in 0..3 {
            super::super::stark_node::mine_once(&state, alice.address()).await.unwrap();
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap().to_string();
        let transport = StreamTransport::new(Some(&target), Arc::new(TcpDialer)).unwrap();
        let server = serve(
            state.clone(),
            ServerIdentity::from_seed(&[83; 32]).unwrap(),
            StreamOnly(transport.clone()),
        )
        .await
        .unwrap();
        let _listening = transport.listen(listener);
        let mut seed = server.descriptor.clone();
        seed.endpoint_addr = EndpointAddr::new(server.endpoint.id())
            .with_addrs([TransportAddr::Custom(dial_addr(&target).unwrap())]);
        let dialer = StreamTransport::new(None, Arc::new(TcpDialer)).unwrap();
        let mut client = Client::connect(&seed, StreamOnly(dialer)).await.unwrap();

        let mut light = LightWallet::new(3, 1).unwrap();
        light.add_page(&alice, &page(&mut client, light.next()).await.unwrap()).unwrap();
        assert_eq!((light.height(), light.tip()), (3, 3));
        // Past the tip: an empty page, not an error.
        assert!(page(&mut client, 9).await.unwrap().blocks.is_empty());

        let plan = light.plan(&alice, bob.address(), 3_000_000, b"over the stream").unwrap();
        let prover = alice.clone();
        let tx = tokio::task::spawn_blocking(move || plan.prove(&prover).unwrap())
            .await
            .unwrap();
        light.reserve(&tx).unwrap();
        assert_eq!(submit(&mut client, &tx).await.unwrap(), tx.txid());
        super::super::stark_node::mine_once(&state, alice.address()).await.unwrap();
        light.add_page(&alice, &page(&mut client, light.next()).await.unwrap()).unwrap();
        assert!(!light.balance().pending, "mined");
        assert_eq!(light.history().last().unwrap().paid(), 3_000_000);

        // The directory: unknown, published, then resolved and checked.
        let short = bob.address().short_address().unwrap();
        assert!(resolve(&mut client, &short, 3).await.unwrap().is_none());
        assert!(publish(&mut client, &bob.address()).await.unwrap());
        let found = resolve(&mut client, &short, 3).await.unwrap().unwrap();
        assert_eq!(found.encode().unwrap(), bob.address().encode().unwrap());
        server.close().await;
    }
}
