//! A real node against deformed input: blocks as a peer sends them and
//! requests as the HTTP API takes them. After every round the node has to be
//! what it was: answering, its locks whole, its chain the one that was proved.
//!
//! The decoders themselves are fuzzed where they live
//! (`crates/zoka-pq-pool/src/fuzzing`); this is the node around them. As
//! there, the rounds depend only on a seed: `ZOKA_FUZZ_SCALE=100` runs a
//! hundred times as many and `ZOKA_FUZZ_SEED` starts elsewhere.
//!
//! Run with: cargo test --release --features quantic-pq --test quantic_node_fuzz
#![cfg(feature = "quantic-pq")]

use axum::{
    body::Body,
    http::{Request, StatusCode, Uri},
    Router,
};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
};
use tower::ServiceExt;
use zoka::{
    checkpoint_authority::SignedCheckpoint,
    quantic::{
        compact_target_for_child, solve_pow,
        stark::{
            self,
            protocol::{
                ledger::{subsidy, ATOMS, PREMINE},
                model::Input,
                transaction::{transfer, Payment, Transaction},
                wallet::Account,
            },
            Profile,
        },
        stark_api::{self, NodeState, DB_MARKER},
        stark_directory::AddressRecord,
    },
    state::chain::{apply_block, ChainError},
    storage::RocksDbStorage,
    types::{Block, PublicKey, Signature},
};

const P: Profile = Profile::REGTEST;
const CHAIN: &str = P.chain_id;

/// SplitMix64: repeatable, nothing more.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

fn number(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok()
}
fn rounds(base: usize) -> usize {
    base * number("ZOKA_FUZZ_SCALE").unwrap_or(1).max(1) as usize
}
fn rng(campaign: u64) -> Rng {
    let seed = number("ZOKA_FUZZ_SEED").unwrap_or(0x5a4f_4b41);
    Rng(seed ^ campaign.wrapping_mul(0x9e37_79b9_7f4a_7c15))
}

/// One to three byte edits.
fn deform_bytes(data: &mut Vec<u8>, rng: &mut Rng) {
    for _ in 0..1 + rng.below(3) {
        if data.is_empty() {
            data.push(rng.next() as u8);
            continue;
        }
        let at = rng.below(data.len());
        match rng.below(7) {
            0 => data[at] ^= 1 << rng.below(8),
            1 => data[at] = rng.next() as u8,
            2 => data.truncate(at),
            3 => {
                data.remove(at);
            }
            4 => data.insert(at, rng.next() as u8),
            5 => {
                let end = (at + 1 + rng.below(32)).min(data.len());
                let copy = data[at..end].to_vec();
                data.splice(at..at, copy);
            }
            _ => data.extend((0..1 + rng.below(8)).map(|_| rng.next() as u8)),
        }
    }
}

/// A node with two blocks, and a proven payment of its first reward that no
/// block holds yet.
struct Node {
    _dir: tempfile::TempDir,
    state: NodeState,
    alice: Account,
    bob: Account,
    tip: Block,
    payment: Transaction,
}

fn node() -> Node {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(RocksDbStorage::new(dir.path()).unwrap());
    storage
        .put_raw(DB_MARKER, CHAIN.as_bytes().to_vec())
        .unwrap();
    let state = NodeState::new(storage.clone()).unwrap();
    let alice = Account::from_seed(&[41; 32], P.network, 0).unwrap();
    let bob = Account::from_seed(&[42; 32], P.network, 0).unwrap();
    let genesis = stark::mine(
        &storage,
        P,
        &P.genesis_parent(),
        &alice.address(),
        vec![],
        1_700_000_000,
    )
    .unwrap();
    apply_block(&storage, CHAIN, &genesis).unwrap();
    let tip = stark::mine(&storage, P, &genesis, &bob.address(), vec![], 1_700_000_120).unwrap();
    apply_block(&storage, CHAIN, &tip).unwrap();
    let ledger = stark::full_ledger(&storage, P, &tip).unwrap();
    let reward = stark::decode(&genesis).unwrap().remove(0);
    let (note, _) = alice
        .receive(reward.statement.commitments[0], &reward.envelopes[0])
        .unwrap();
    let payment = transfer(
        &alice,
        ledger.tree.root(),
        vec![Input {
            key: alice.spend.clone(),
            note,
            path: ledger.tree.path(0).unwrap(),
        }],
        vec![
            Payment::new(bob.address(), ATOMS),
            Payment::new(alice.address(), PREMINE - ATOMS - 200),
        ],
        200,
    )
    .unwrap();
    Node {
        _dir: dir,
        state,
        alice,
        bob,
        tip,
        payment,
    }
}

/// A transaction edited by someone who can recompute its txid.
fn resealed(mut tx: Transaction) -> Option<Vec<u8>> {
    tx.statement.binding = tx.effects_digest().ok()?;
    tx.encode().ok()
}

/// A payment that is no longer the one that was proved.
fn edit_payment(bytes: &[u8], rng: &mut Rng) -> Option<Vec<u8>> {
    let mut tx = Transaction::decode(bytes).ok()?;
    // Another edit of the round may have put the reward in its place.
    if tx.statement.nullifiers.is_empty()
        || tx.statement.commitments.len() < 2
        || tx.envelopes.len() < 2
    {
        return None;
    }
    match rng.below(9) {
        0 => tx.statement.fee += 1,
        1 => tx.statement.fee = tx.statement.fee.saturating_sub(1),
        2 => tx.statement.commitments.swap(0, 1),
        3 => {
            let again = tx.statement.nullifiers[0];
            tx.statement.nullifiers.push(again);
        }
        4 => tx.statement.nullifiers[0].0[0] ^= 1,
        5 => tx.statement.commitments[0].0[rng.below(16)] ^= 1,
        6 => tx.envelopes[rng.below(2)].encrypted[rng.below(64)] ^= 1,
        7 => tx.height += 1,
        _ => tx.statement.anchor.0[0] ^= 1,
    }
    resealed(tx)
}

/// A reward that is no longer what the block may mint.
fn edit_reward(bytes: &[u8], rng: &mut Rng) -> Option<Vec<u8>> {
    let mut tx = Transaction::decode(bytes).ok()?;
    if tx.openings.is_empty() {
        return None;
    }
    match rng.below(5) {
        0 => {
            tx.statement.minted += 1;
            tx.openings[0].value += 1;
        }
        1 => tx.statement.minted += 1,
        2 => tx.openings[0].value += 1,
        3 => tx.openings[0].hide.0[0] ^= 1,
        _ => tx.height += 1,
    }
    resealed(tx)
}

/// One edit of `block`: its header, its list of spent notes or what it
/// carries. Returns what was done, for the failure message.
fn deform_block(block: &mut Block, rng: &mut Rng) -> &'static str {
    let transactions = &mut block.quantic_transactions;
    match rng.below(18) {
        0 => {
            block.header.timestamp += 1 + rng.below(600) as u64;
            "a later timestamp"
        }
        1 => {
            block.header.timestamp = 0;
            "timestamp zero"
        }
        2 => {
            block.header.timestamp = u64::MAX;
            "the last timestamp"
        }
        3 => {
            block.header.block_height += 1;
            "another height"
        }
        4 => {
            block.header.block_height = u64::MAX;
            "the last height"
        }
        5 => {
            block.header.state_root.push('0');
            "a longer state root"
        }
        6 => {
            block.header.state_root = "quantic-v3:".into();
            "an empty state root"
        }
        7 => {
            block.header.prev_hash = "00".repeat(32);
            "another parent"
        }
        8 => {
            block.private_nullifiers.clear();
            "no spent notes listed"
        }
        9 => {
            let again = block
                .private_nullifiers
                .first()
                .cloned()
                .unwrap_or_default();
            block.private_nullifiers.push(again);
            "a spent note listed twice"
        }
        10 => {
            transactions.pop();
            "the payment dropped"
        }
        11 => {
            let again = transactions.last().cloned().unwrap_or_default();
            transactions.push(again);
            "the payment twice"
        }
        12 => {
            transactions.reverse();
            "the reward last"
        }
        13 => {
            transactions.clear();
            "no transactions"
        }
        14 => {
            if let Some(payment) = transactions.get_mut(1) {
                deform_bytes(payment, rng);
            }
            "payment bytes"
        }
        15 => {
            if let Some(reward) = transactions.get_mut(0) {
                deform_bytes(reward, rng);
            }
            "reward bytes"
        }
        16 => {
            if let Some(edited) = transactions.get(1).and_then(|tx| edit_payment(tx, rng)) {
                transactions[1] = edited;
            }
            "an edited payment"
        }
        _ => {
            if let Some(edited) = transactions.first().and_then(|tx| edit_reward(tx, rng)) {
                transactions[0] = edited;
            }
            "an edited reward"
        }
    }
}

/// The node after a campaign: its tip still extends, its ledger still
/// rebuilds from the blocks and it issued exactly what the schedule says.
fn assert_sound(node: &Node) {
    let storage = &node.state.storage;
    let hash = storage.get_last_block_hash().unwrap();
    let tip = storage
        .get_block(&hash)
        .unwrap()
        .expect("the tip is stored");
    let ledger = stark::full_ledger(storage, P, &tip).unwrap();
    let height = u32::try_from(tip.header.block_height).unwrap();
    assert_eq!(ledger.height, height);
    assert_eq!(ledger.issued, (1..=height).map(subsidy).sum::<u64>());
    let next = stark::mine(
        storage,
        P,
        &tip,
        &node.bob.address(),
        vec![],
        tip.header.timestamp + 120,
    )
    .unwrap();
    apply_block(storage, CHAIN, &next).unwrap();
}

#[test]
fn no_block_of_a_peer_breaks_the_node() {
    let node = node();
    let storage = &node.state.storage;
    let seed = stark::mine(
        storage,
        P,
        &node.tip,
        &node.bob.address(),
        vec![node.payment.clone()],
        1_700_000_240,
    )
    .unwrap();
    let target = compact_target_for_child(storage, CHAIN, &node.tip).unwrap();
    let payment = node.payment.encode().unwrap();
    let reward = subsidy(3) + node.payment.statement.fee;
    let mut rng = rng(11);
    let (mut refused, mut taken) = (0, 0);
    for round in 0..rounds(600) {
        let mut block = seed.clone();
        // Up to two edits, then what any miner can redo: the Merkle root of
        // the new contents and the work on the new header.
        let what: Vec<_> = (0..1 + rng.below(2))
            .map(|_| deform_block(&mut block, &mut rng))
            .collect();
        block.header.merkle_root = block.compute_merkle_root();
        if solve_pow(CHAIN, &mut block, target).is_err() {
            continue;
        }
        let outcome = catch_unwind(AssertUnwindSafe(|| apply_block(storage, CHAIN, &block)))
            .unwrap_or_else(|_| panic!("round {round}: a block with {what:?} broke the node"));
        // What the node holds under this hash afterwards, if anything: the
        // block it validated, which is not always the one just sent (two
        // bodies can share a hash: see the test below).
        let held = match outcome {
            Ok(_) | Err(ChainError::SideBranchNotHeavier { .. }) => {
                taken += 1;
                block.clone()
            }
            Err(ChainError::BlockAlreadyKnown(_)) => storage
                .get_block(&block.header.hash)
                .unwrap()
                .expect("a known block is stored"),
            Err(_) => {
                refused += 1;
                continue;
            }
        };
        // Valid in the node's eyes: then it is the third block, it mints the
        // schedule's reward and its payment is the one that was proved.
        assert_eq!(held.header.block_height, 3, "round {round}: {what:?}");
        assert_eq!(
            held.header.prev_hash, node.tip.header.hash,
            "round {round}: {what:?}"
        );
        let transactions = stark::decode(&held).unwrap();
        assert_eq!(transactions.len(), 2, "round {round}: {what:?}");
        assert_eq!(
            transactions[0].statement.minted, reward,
            "round {round}: {what:?}"
        );
        assert_eq!(
            held.quantic_transactions[1], payment,
            "round {round}: a block with another payment was accepted ({what:?})"
        );
    }
    assert!(
        refused > rounds(600) / 2,
        "only {refused} blocks were refused"
    );
    eprintln!("blocks: {refused} refused, {taken} valid siblings accepted");
    // The block that was mined honestly still enters, and the chain goes on.
    match apply_block(storage, CHAIN, &seed) {
        Ok(_) | Err(ChainError::SideBranchNotHeavier { .. } | ChainError::BlockAlreadyKnown(_)) => {
        }
        Err(error) => panic!("the honest block was refused after the campaign: {error:?}"),
    }
    assert_sound(&node);
}

/// Found by the campaign above, fixed by the v3 protocol. A block's Merkle
/// tree pairs its leaves and, when a level has an odd number, pairs the last
/// one with itself: a block whose leaves are odd in number and the same block
/// with its last leaf repeated shared one root, one header and one hash
/// (Bitcoin's CVE-2012-2459). A v3 block's root also commits to how many
/// leaves it has; blocks of the older chains keep the roots they had.
#[test]
fn a_repeated_last_leaf_changes_a_v3_block_s_root() {
    let block = |state_root: &str, leaves: usize| {
        let mut block = Block::new(
            "00".repeat(32),
            1_700_000_000,
            0,
            1,
            vec![],
            PublicKey(vec![]),
            Signature(vec![]),
        );
        block.header.state_root = state_root.into();
        block.quantic_transactions = (0..leaves).map(|i| vec![i as u8; 40]).collect();
        block
    };
    let repeated = |block: &Block| {
        let mut twice = block.clone();
        let last = twice.quantic_transactions.last().unwrap().clone();
        twice.quantic_transactions.push(last);
        twice
    };
    for leaves in [3, 5, 7] {
        let v3 = block("quantic-v3:ab", leaves);
        assert_ne!(
            v3.compute_merkle_root(),
            repeated(&v3).compute_merkle_root(),
            "{leaves} leaves"
        );
        let v2 = block("quantic-v2:ab", leaves);
        assert_eq!(
            v2.compute_merkle_root(),
            repeated(&v2).compute_merkle_root(),
            "the older chains keep their roots"
        );
    }
}

/// So the body with the repeated payment no longer passes for the honest
/// block: under the honest header it is refused, and the node stores only
/// the honest body.
#[test]
fn a_body_with_a_repeated_payment_is_not_the_block() {
    let node = node();
    let storage = &node.state.storage;
    let honest = stark::mine(
        storage,
        P,
        &node.tip,
        &node.bob.address(),
        vec![node.payment.clone()],
        1_700_000_240,
    )
    .unwrap();
    let mut twice = honest.clone();
    twice
        .quantic_transactions
        .push(honest.quantic_transactions[1].clone());
    assert_ne!(twice.compute_merkle_root(), honest.header.merkle_root);
    let refused = apply_block(storage, CHAIN, &twice);
    assert!(
        !matches!(
            refused,
            Ok(_) | Err(ChainError::SideBranchNotHeavier { .. } | ChainError::BlockAlreadyKnown(_))
        ),
        "a block that pays twice was not refused"
    );
    assert!(storage.get_block(&honest.header.hash).unwrap().is_none());
    apply_block(storage, CHAIN, &honest).unwrap();
    let stored = storage.get_block(&honest.header.hash).unwrap().unwrap();
    assert_eq!(stored.quantic_transactions, honest.quantic_transactions);
    assert_sound(&node);
}

#[test]
fn no_json_block_breaks_the_node() {
    let node = node();
    let storage = &node.state.storage;
    let seed = stark::mine(
        storage,
        P,
        &node.tip,
        &node.bob.address(),
        vec![node.payment.clone()],
        1_700_000_240,
    )
    .unwrap();
    let json = serde_json::to_vec(&seed).unwrap();
    let again = serde_json::from_slice::<Block>(&json).unwrap();
    assert_eq!(serde_json::to_vec(&again).unwrap(), json);
    let mut rng = rng(12);
    for round in 0..rounds(400) {
        let mut input = json.clone();
        deform_bytes(&mut input, &mut rng);
        let Ok(block) = serde_json::from_slice::<Block>(&input) else {
            continue;
        };
        // The same block written another way (hex in capitals, say).
        if serde_json::to_vec(&block).unwrap() == json {
            continue;
        }
        let outcome = catch_unwind(AssertUnwindSafe(|| apply_block(storage, CHAIN, &block)))
            .unwrap_or_else(|_| panic!("round {round}: a deformed JSON block broke the node"));
        // Its work no longer covers what it says.
        assert!(
            outcome.is_err(),
            "round {round}: a deformed block was accepted"
        );
    }
    apply_block(storage, CHAIN, &seed).unwrap();
    assert_sound(&node);
}

struct Call {
    post: bool,
    uri: String,
    kind: &'static str,
    body: Vec<u8>,
}
impl Call {
    fn get(uri: impl Into<String>) -> Self {
        Self {
            post: false,
            uri: uri.into(),
            kind: "",
            body: Vec::new(),
        }
    }
    fn post(uri: &str, kind: &'static str, body: Vec<u8>) -> Self {
        Self {
            post: true,
            uri: uri.into(),
            kind,
            body,
        }
    }
    fn request(&self) -> Option<Request<Body>> {
        let uri: Uri = self.uri.parse().ok()?;
        let mut builder = Request::builder()
            .method(if self.post { "POST" } else { "GET" })
            .uri(uri)
            .header("host", "127.0.0.1:9140");
        if !self.kind.is_empty() {
            builder = builder.header("content-type", self.kind);
        }
        builder.body(Body::from(self.body.clone())).ok()
    }
}

const NUMBERS: [&str; 10] = [
    "0",
    "1",
    "-1",
    "4294967295",
    "4294967296",
    "18446744073709551615",
    "18446744073709551616",
    "99999999999999999999999999999999",
    "1e9",
    "0x10",
];

/// A request line that is still a request line: other numbers where numbers
/// go, and other characters anywhere.
fn deform_uri(uri: &str, rng: &mut Rng) -> String {
    let mut text: Vec<char> = uri.chars().collect();
    match rng.below(6) {
        0 | 1 => {
            // Replace one run of digits.
            let starts: Vec<usize> = (0..text.len())
                .filter(|&i| {
                    text[i].is_ascii_digit() && (i == 0 || !text[i - 1].is_ascii_hexdigit())
                })
                .collect();
            if let Some(&start) = starts.get(rng.below(starts.len().max(1))) {
                let end = (start..text.len())
                    .find(|&i| !text[i].is_ascii_digit())
                    .unwrap_or(text.len());
                let number: Vec<char> = NUMBERS[rng.below(NUMBERS.len())].chars().collect();
                text.splice(start..end, number);
            }
        }
        2 => {
            let at = rng
                .below(text.len().max(1))
                .min(text.len().saturating_sub(1));
            const ALPHABET: &[u8] = b"0123456789abcdefzZ/?&=%+-._~";
            if !text.is_empty() {
                text[at] = ALPHABET[rng.below(ALPHABET.len())] as char;
            }
        }
        3 => text.truncate(1 + rng.below(text.len().max(1))),
        4 => {
            let extra = "f".repeat(1 + rng.below(300));
            text.extend(extra.chars());
        }
        _ => {
            let extra: Vec<char> = ["&count=0", "?from=0", "&from=1&from=2", "%00", "/../"]
                [rng.below(5)]
            .chars()
            .collect();
            text.extend(extra);
        }
    }
    text.into_iter().collect()
}

async fn send(app: &Router, call: &Call) -> Option<StatusCode> {
    let request = call.request()?;
    Some(app.clone().oneshot(request).await.unwrap().status())
}

async fn alive(app: &Router) -> bool {
    send(app, &Call::get("/status")).await == Some(StatusCode::OK)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_request_breaks_the_api() {
    let node = node();
    let app = stark_api::router(node.state.clone());
    assert!(alive(&app).await);

    let payment = node.payment.encode().unwrap();
    let txid = hex::encode(node.payment.txid());
    let short = node.alice.address().short().unwrap();
    let record = {
        let address = node.bob.address();
        tokio::task::spawn_blocking(move || AddressRecord::new(&address).unwrap())
            .await
            .unwrap()
    };
    let checkpoint = SignedCheckpoint::sign(
        &ed25519_dalek::SigningKey::from_bytes(&[9; 32]),
        CHAIN,
        1,
        &node.tip.header.prev_hash,
    );
    let block = serde_json::to_vec(&node.tip).unwrap();
    let seeds = [
        Call::get("/status"),
        Call::get("/blocks/recent?count=5"),
        Call::get("/blocks/stats?count=10&before=2"),
        Call::get("/blocks/1"),
        Call::get("/blocks/2/summary"),
        Call::get(format!("/blocks/hash/{}/summary", node.tip.header.hash)),
        Call::get("/scan?from=1&count=10"),
        Call::get("/scan2?from=1&count=10"),
        Call::get("/supply"),
        Call::get("/supply/total"),
        Call::get("/network/history"),
        Call::get("/checkpoint"),
        Call::get(format!("/transactions/{txid}")),
        Call::get(format!("/address/{short}?local=1")),
        Call::get(format!(
            "/mining/template?address={}",
            node.bob.address().encode().unwrap()
        )),
        Call::post("/transactions", "application/octet-stream", payment.clone()),
        Call::post("/blocks", "application/json", block),
        Call::post(
            "/checkpoint",
            "application/json",
            serde_json::to_vec(&checkpoint).unwrap(),
        ),
        Call::post(
            "/address",
            "application/json",
            serde_json::to_vec(&record).unwrap(),
        ),
    ];
    // Each seed is a request the API knows: none is a missing route.
    for call in seeds.iter().filter(|call| !call.post) {
        let status = send(&app, call).await.unwrap();
        assert!(
            status != StatusCode::NOT_FOUND
                || call.uri.starts_with("/transactions/")
                || call.uri.starts_with("/address/"),
            "{} answered {status}",
            call.uri
        );
    }

    let mut rng = rng(13);
    for round in 0..rounds(1500) {
        let seed = &seeds[rng.below(seeds.len())];
        let mut call = Call {
            post: seed.post,
            uri: seed.uri.clone(),
            kind: seed.kind,
            body: seed.body.clone(),
        };
        match rng.below(if seed.post { 5 } else { 2 }) {
            0 | 1 => call.uri = deform_uri(&call.uri, &mut rng),
            2 => deform_bytes(&mut call.body, &mut rng),
            3 if seed.uri == "/transactions" => {
                if let Some(edited) = edit_payment(&call.body, &mut rng) {
                    call.body = edited;
                }
            }
            3 => deform_bytes(&mut call.body, &mut rng),
            _ => {
                call.kind = [
                    "",
                    "text/plain",
                    "application/json",
                    "application/octet-stream",
                ][rng.below(4)]
            }
        }
        let Some(status) = send(&app, &call).await else {
            continue;
        };
        assert_ne!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "round {round}: {} {} failed inside the node",
            if call.post { "POST" } else { "GET" },
            call.uri
        );
        assert!(
            alive(&app).await,
            "round {round}: the node stopped answering after {} {}",
            if call.post { "POST" } else { "GET" },
            call.uri
        );
    }

    // The public front takes the same requests through its own admission.
    let public = stark_api::public_router(node.state.clone());
    for round in 0..rounds(300) {
        let seed = &seeds[rng.below(seeds.len())];
        let Ok(uri) = deform_uri(&seed.uri, &mut rng).parse::<Uri>() else {
            continue;
        };
        let method = ["GET", "POST", "HEAD", "OPTIONS", "PUT", "DELETE"][rng.below(6)];
        let length = NUMBERS[rng.below(NUMBERS.len())];
        let client = [
            "203.0.113.7",
            "2001:db8::1",
            "::ffff:203.0.113.7",
            "not an address",
            "",
        ][rng.below(5)];
        let mut builder = Request::builder().method(method).uri(uri);
        if rng.below(2) == 0 {
            builder = builder.header("content-length", length);
        }
        if !client.is_empty() {
            builder = builder.header("cf-connecting-ip", client);
        }
        if !seed.kind.is_empty() {
            builder = builder.header("content-type", seed.kind);
        }
        let Ok(request) = builder.body(Body::from(seed.body.clone())) else {
            continue;
        };
        let status = public.clone().oneshot(request).await.unwrap().status();
        assert_ne!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "round {round}: public {method}"
        );
    }
    assert!(alive(&app).await);

    // After all of it the node still takes the real payment and mines it.
    node.state
        .mempool
        .lock()
        .unwrap()
        .revalidate(&stark::branch(&node.state.storage, P, &current_tip(&node)).unwrap());
    let status = send(
        &app,
        &Call::post("/transactions", "application/octet-stream", payment),
    )
    .await
    .unwrap();
    let held = node
        .state
        .mempool
        .lock()
        .unwrap()
        .contains(&node.payment.txid());
    assert!(
        held,
        "the real payment is not in the mempool after the campaign (last answer {status})"
    );
    let tip = current_tip(&node);
    let selected = node.state.mempool.lock().unwrap().select();
    let paid = stark::mine(
        &node.state.storage,
        P,
        &tip,
        &node.bob.address(),
        selected,
        tip.header.timestamp + 120,
    )
    .unwrap();
    assert_eq!(paid.quantic_transactions.len(), 2);
    apply_block(&node.state.storage, CHAIN, &paid).unwrap();
    assert_sound(&node);
}

fn current_tip(node: &Node) -> Block {
    let storage = &node.state.storage;
    let hash = storage.get_last_block_hash().unwrap();
    storage
        .get_block(&hash)
        .unwrap()
        .expect("the tip is stored")
}
