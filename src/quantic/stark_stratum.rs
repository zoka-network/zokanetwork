//! Stratum for a STARK v2 chain: the shared server in `stratum` (XMRig,
//! `rx/0`), with templates built from the STARK node's state and found blocks
//! imported through the same gate as API, peers and the built-in miner.
//!
//! Each payout address costs a coinbase STARK proof per template (seconds and
//! hundreds of MiB), so this backend allows fewer addresses and refreshes less
//! often than the Ironwood one, and proves only while holding a `work` permit.
use std::time::Duration;

use super::{
    stark::{
        self,
        protocol::{ledger::subsidy, wallet::Address},
    },
    stark_api::NodeState,
    stratum::{looks_like_quantic_address, Backend},
};
use crate::{storage::RocksDbStorage, types::Block};

/// At most this many payout addresses get templates at the same time.
pub const MAX_PAYOUT_ADDRESSES: usize = 4;
/// A template is rebuilt when the tip changes or after this long.
pub const TEMPLATE_REFRESH: Duration = Duration::from_secs(60);

/// `zq2`, `zqtest2`, `zqregtest2`: a v2 address of some network.
fn looks_like_v2_address(login: &str) -> bool {
    ["zq2", "zqtest2", "zqregtest2"]
        .iter()
        .any(|prefix| login.starts_with(prefix))
}

pub struct StarkBackend {
    state: NodeState,
    /// Pays logins that are not addresses (worker names), as `--mine-to`.
    default_address: Option<Address>,
}

impl StarkBackend {
    pub fn new(state: NodeState, default_address: Option<Address>) -> Result<Self, String> {
        if default_address
            .as_ref()
            .is_some_and(|address| address.network != state.profile.network)
        {
            return Err(format!(
                "the default payout is not a {} v2 address",
                state.profile.name
            ));
        }
        Ok(Self {
            state,
            default_address,
        })
    }

    /// Wait for a crypto worker slot. Callers are blocking threads.
    fn permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit, String> {
        futures::executor::block_on(self.state.work.clone().acquire_owned())
            .map_err(|e| e.to_string())
    }
}

impl Backend for StarkBackend {
    fn chain_id(&self) -> &str {
        self.state.profile.chain_id
    }

    fn storage(&self) -> &RocksDbStorage {
        &self.state.storage
    }

    fn payout_for(&self, login: &str) -> Result<String, String> {
        let login = login.trim();
        let profile = self.state.profile;
        if let Ok(address) = Address::decode(login, profile.network) {
            return address.encode().map_err(|e| e.to_string());
        }
        // An address of another network, a v1 address or a short v2 one (this
        // node has no v2 directory yet): paying the node's default instead
        // would mine for someone else without the miner noticing.
        if looks_like_v2_address(login) || looks_like_quantic_address(login) {
            return Err(format!(
                "log in with the full {} v2 address of your wallet ({})",
                profile.name,
                profile.chain_id
            ));
        }
        self.default_address
            .as_ref()
            .ok_or_else(|| "log in with your Zoka Quantic v2 address".to_string())?
            .encode()
            .map_err(|e| e.to_string())
    }

    fn template(&self, payout: &str) -> Result<(Block, u64), String> {
        let profile = self.state.profile;
        let to = Address::decode(payout, profile.network).map_err(|e| e.to_string())?;
        let _permit = self.permit()?;
        let (parent, transactions) = {
            let _gate = self.state.gate.lock().map_err(|_| "node gate poisoned")?;
            let transactions = self
                .state
                .mempool
                .lock()
                .map_err(|_| "mempool poisoned")?
                .select();
            (self.state.tip()?, transactions)
        };
        let fees = transactions
            .iter()
            .try_fold(0u64, |sum, tx| sum.checked_add(tx.statement.fee))
            .ok_or("fee overflow")?;
        // As with the built-in miner, a block arriving meanwhile only makes
        // this template stale; it is still valid on its parent.
        let block = stark::template(
            &self.state.storage,
            profile,
            &parent,
            &to,
            transactions,
            super::now_secs(),
        )?;
        let height = u32::try_from(block.header.block_height).map_err(|e| e.to_string())?;
        // Block 1 pays the premine: not a mining reward.
        let reward = if height == 1 {
            fees
        } else {
            subsidy(height).saturating_add(fees)
        };
        Ok((block, reward))
    }

    fn found(&self, block: Block, reward_atoms: u64) {
        let imported = self.permit().and_then(|_permit| {
            let _gate = self.state.gate.lock().map_err(|_| "node gate poisoned")?;
            super::stark_node::import_mined_locked(&self.state, &block)
        });
        match imported {
            Ok(canonical) => println!(
                "{}",
                serde_json::json!({"stratum_block":block.header.hash,"height":block.header.block_height,
                    "reward_atoms":reward_atoms,"canonical":canonical})
            ),
            Err(reason) => eprintln!("STARK stratum block rejected: {reason}"),
        }
    }

    fn template_refresh(&self) -> Duration {
        TEMPLATE_REFRESH
    }

    fn max_payout_addresses(&self) -> usize {
        MAX_PAYOUT_ADDRESSES
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::{json, Value};
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::{tcp::OwnedWriteHalf, TcpListener, TcpStream},
    };

    use super::*;
    use crate::{mining::pow, quantic::stark::Profile, quantic::stark_cli::open_db};
    use stark::protocol::{ledger::PREMINE, wallet::Account};

    const P: Profile = Profile::REGTEST;

    fn state(dir: &tempfile::TempDir) -> NodeState {
        let (db, _) = open_db(&dir.path().join("chain"), Some(P)).unwrap();
        NodeState::new(std::sync::Arc::new(db)).unwrap()
    }

    #[test]
    fn logins_pay_their_own_v2_address_or_the_default_never_a_mistyped_one() {
        let dir = tempfile::tempdir().unwrap();
        let mine = Account::from_seed(&[52; 32], 3, 0).unwrap().address();
        let other = Account::from_seed(&[52; 32], 2, 0).unwrap().address();
        assert!(StarkBackend::new(state(&dir), Some(other.clone())).is_err());
        let dir = tempfile::tempdir().unwrap();
        let backend = StarkBackend::new(state(&dir), Some(mine.clone())).unwrap();
        let encoded = mine.encode().unwrap();
        assert_eq!(backend.payout_for(&format!(" {encoded} ")).unwrap(), encoded);
        // A worker name mines for the node's --mine-to address.
        assert_eq!(backend.payout_for("rig-01").unwrap(), encoded);
        for login in [
            other.encode().unwrap(),
            mine.short().unwrap(),
            "zqtest1abc".to_string(),
        ] {
            let err = backend.payout_for(&login).err().unwrap();
            assert!(err.contains("full regtest v2 address"), "{err}");
        }
    }

    async fn call(
        writer: &mut OwnedWriteHalf,
        lines: &mut tokio::io::Lines<BufReader<tokio::net::tcp::OwnedReadHalf>>,
        request: Value,
    ) -> Value {
        let mut line = request.to_string();
        line.push('\n');
        writer.write_all(line.as_bytes()).await.unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(300), lines.next_line())
            .await
            .expect("stratum reply")
            .unwrap()
            .unwrap();
        serde_json::from_str(&reply).unwrap()
    }

    /// What XMRig does with a job: put a 32-bit nonce at offset 39 of the blob
    /// and compare the top 64 bits of the hash (little endian) to the target.
    fn solve(job: &Value) -> (String, String) {
        let mut blob = hex::decode(job["blob"].as_str().unwrap()).unwrap();
        let seed = hex::decode(job["seed_hash"].as_str().unwrap()).unwrap();
        let target: [u8; 8] = hex::decode(job["target"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let target = u64::from_le_bytes(target);
        let engine = pow::build_engine(P.pow_algorithm(), &seed).unwrap();
        let offset = pow::QUANTIC_POW_NONCE_OFFSET;
        for nonce in 0u32.. {
            blob[offset..offset + 4].copy_from_slice(&nonce.to_le_bytes());
            let hash = engine.calculate_hash(&blob).unwrap();
            if u64::from_le_bytes(hash[24..32].try_into().unwrap()) < target {
                return (hex::encode(nonce.to_le_bytes()), hex::encode(hash));
            }
        }
        unreachable!("a 32-bit nonce space always holds an easy share")
    }

    // End to end over TCP on the regtest chain (CPU PoW, hence serve_listener
    // rather than the RandomX-only serve_backend).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn xmrig_messages_mine_a_stark_block_that_pays_the_login() {
        let dir = tempfile::tempdir().unwrap();
        let state = state(&dir);
        let miner = Account::from_seed(&[51; 32], 3, 0).unwrap();
        let backend = Arc::new(StarkBackend::new(state.clone(), None).unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bind = listener.local_addr().unwrap();
        tokio::spawn(crate::quantic::stratum::serve_listener(listener, backend));
        let (reader, mut writer) = TcpStream::connect(bind).await.unwrap().into_split();
        let mut lines = BufReader::new(reader).lines();

        let refused = call(&mut writer, &mut lines, json!({"id":1,"method":"login",
            "params":{"login":"rig-01","pass":"x","agent":"XMRig/6.25.0"}})).await;
        assert!(refused["error"]["message"].as_str().unwrap().contains("v2 address"));

        let address = miner.address().encode().unwrap();
        let login = call(&mut writer, &mut lines, json!({"id":2,"method":"login",
            "params":{"login":address,"pass":"x","agent":"XMRig/6.25.0","algo":["rx/0"]}})).await;
        assert!(login["error"].is_null(), "{login}");
        let session = login["result"]["id"].as_str().unwrap().to_string();
        let job = login["result"]["job"].clone();
        assert_eq!(job["height"], 1);
        assert_eq!(job["algo"], "rx/0");
        assert_eq!(
            hex::decode(job["blob"].as_str().unwrap()).unwrap().len(),
            pow::QUANTIC_POW_BLOB_BYTES
        );

        // On an empty chain the template is not stale: no new job (each one a
        // coinbase proof) until there is a block.
        assert!(tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .is_err());

        let (nonce, result) = solve(&job);
        // A nonce is hashed once per job, even when its result was wrong (so a
        // miner cannot make the node recompute it); try the bad result on another.
        let mut other: [u8; 4] = hex::decode(&nonce).unwrap().try_into().unwrap();
        other[0] ^= 1;
        let rejected = call(&mut writer, &mut lines, json!({"id":3,"method":"submit",
            "params":{"id":session,"job_id":job["job_id"],"nonce":hex::encode(other),"result":result}})).await;
        assert!(rejected["error"]["message"].as_str().unwrap().contains("does not match"));
        // In regtest the share target is the block target: this share is block 1.
        let accepted = call(&mut writer, &mut lines, json!({"id":4,"method":"submit",
            "params":{"id":session,"job_id":job["job_id"],"nonce":nonce,"result":result}})).await;
        assert_eq!(accepted["result"]["status"], "OK", "{accepted}");
        let again = call(&mut writer, &mut lines, json!({"id":5,"method":"submit",
            "params":{"id":session,"job_id":job["job_id"],"nonce":nonce,"result":result}})).await;
        assert!(again["error"]["message"].as_str().unwrap().contains("duplicate"));

        let block = state.storage.get_canonical_block_by_height(1).unwrap().unwrap();
        assert_eq!(state.storage.get_chain_height().unwrap(), 1);
        let reward = stark::decode(&block).unwrap().remove(0);
        let (note, _) = miner
            .receive(reward.statement.commitments[0], &reward.envelopes[0])
            .unwrap();
        assert_eq!(note.value, PREMINE);

        // The new tip brings this login a job on top of block 1.
        let notice = tokio::time::timeout(Duration::from_secs(300), lines.next_line())
            .await
            .expect("job notification")
            .unwrap()
            .unwrap();
        let notice: Value = serde_json::from_str(&notice).unwrap();
        assert_eq!(notice["method"], "job");
        assert_eq!(notice["params"]["height"], 2);
        assert_ne!(notice["params"]["job_id"], job["job_id"]);
        let status = crate::quantic::stratum::status();
        assert!(status.accepted_shares >= 1 && status.blocks_found >= 1);
    }
}
