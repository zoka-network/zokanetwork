//! Deterministic fuzzing: every test starts from the valid inputs of
//! [`Corpus`], deforms them thousands of times and holds the targets to what
//! they promise. The inputs depend only on the seed, so a failure repeats.
//!
//! `ZOKA_FUZZ_SCALE=100` runs a hundred times as many rounds (a long run for
//! a machine with nothing else to do); `ZOKA_FUZZ_SEED` starts elsewhere. An
//! input that breaks a target is kept in `target/fuzz-failures/`.
use super::{
    mutate::{self, Rng},
    tree::capture,
    *,
};
use crate::{ledger::replay_effects, transfer::proof_tree};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::OnceLock,
};

fn corpus() -> &'static Corpus {
    static CORPUS: OnceLock<Corpus> = OnceLock::new();
    CORPUS.get_or_init(|| Corpus::build().expect("the seeds are valid"))
}

fn number(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok()
}
fn rounds(base: usize) -> usize {
    base * number("ZOKA_FUZZ_SCALE").unwrap_or(1).max(1) as usize
}
fn rng(campaign: u64) -> Rng {
    let seed = number("ZOKA_FUZZ_SEED").unwrap_or(0x5a4f_4b41);
    Rng::new(seed ^ campaign.wrapping_mul(0x9e37_79b9_7f4a_7c15))
}

fn keep(campaign: &str, input: &[u8]) -> PathBuf {
    let name: String = crate::wallet::digest(b"ZokaPQFuzzInput_", &[input])[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("fuzz-failures");
    let path = dir.join(format!("{campaign}-{name}.bin"));
    if std::fs::create_dir_all(&dir).is_err() || std::fs::write(&path, input).is_err() {
        eprintln!("{campaign}: the failing input could not be written");
    }
    path
}

/// Runs one round. A panic of the target fails the test and names the file
/// that holds the input.
fn survive<T>(campaign: &str, round: usize, input: &[u8], run: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(run)) {
        Ok(value) => value,
        Err(_) => {
            let path = keep(campaign, input);
            panic!(
                "{campaign}: round {round} broke the target; its input is in {}",
                path.display()
            );
        }
    }
}

#[test]
fn the_tree_writes_what_postcard_writes() {
    use serde::Serialize;
    use std::collections::BTreeMap;
    let corpus = corpus();
    for bytes in [&corpus.coinbase, &corpus.transfer] {
        let tx = Transaction::decode(bytes).unwrap();
        assert_eq!(&capture(&tx).unwrap().encode(), bytes);
    }
    let page = ScanPage::decode(&corpus.page).unwrap();
    assert_eq!(capture(&page).unwrap().encode(), corpus.page);
    let page2 = ScanPage2::decode(&corpus.page2).unwrap();
    assert_eq!(capture(&page2).unwrap().encode(), corpus.page2);
    // A stored state is its format byte and then the postcard of the wallet.
    let wallet = LightWallet::decode(&corpus.wallet).unwrap();
    assert_eq!(corpus.wallet[0], crate::light::STATE_FORMAT);
    assert_eq!(capture(&wallet).unwrap().encode(), corpus.wallet[1..]);
    assert_eq!(proof_tree(&corpus.proof).unwrap().encode(), corpus.proof);

    // The kinds our own formats do not use.
    #[derive(Serialize)]
    enum Kind {
        Unit,
        New(u32),
        Pair(u8, i16),
        Named { x: u64 },
    }
    #[derive(Serialize)]
    struct Odd {
        a: (i8, i16, i32, i64, i128),
        b: (u8, u16, u32, u64, u128),
        c: (f32, f64, bool, char, ()),
        d: (Option<u16>, Option<u16>),
        e: BTreeMap<u8, String>,
        f: Vec<Kind>,
        g: [u32; 3],
    }
    let odd = Odd {
        a: (-7, -300, i32::MIN, i64::MAX, -1),
        b: (200, 60_000, u32::MAX, 1 << 40, u128::MAX),
        c: (1.5, -2.25, true, 'ñ', ()),
        d: (None, Some(500)),
        e: BTreeMap::from([(1, "one".into()), (9, String::new())]),
        f: vec![
            Kind::Unit,
            Kind::New(70_000),
            Kind::Pair(3, -4),
            Kind::Named { x: 9 },
        ],
        g: [0, 127, 128],
    };
    assert_eq!(
        capture(&odd).unwrap().encode(),
        postcard::to_allocvec(&odd).unwrap()
    );
}

#[test]
fn the_seeds_reach_the_code_behind_the_decoders() {
    let corpus = corpus();
    for bytes in [&corpus.coinbase, &corpus.transfer] {
        let tx = Transaction::decode(bytes).unwrap();
        tx.verify(NETWORK, tx.height).unwrap();
        transaction(bytes);
    }
    assert!(proof(&corpus.proof));
    assert_eq!(corpus.envelope.len(), ENVELOPE_INPUT_BYTES);
    assert_eq!(envelope(&corpus.envelope), (true, true));
    assert_eq!(
        ENVELOPE_PARTS.iter().map(|(_, len)| len).sum::<usize>(),
        ENVELOPE_INPUT_BYTES
    );

    let reader = reader();
    let mut wallet = LightWallet::new(NETWORK, 1).unwrap();
    wallet
        .add_page(&reader, &ScanPage2::decode(&corpus.page2).unwrap())
        .unwrap();
    assert_eq!(wallet.height(), 4);
    assert!(wallet.balance().spendable_atoms > 0);
    let stored = LightWallet::decode(&corpus.wallet).unwrap();
    assert_eq!(
        stored.balance().spendable_atoms,
        wallet.balance().spendable_atoms
    );
    let mut viewer = Viewer::new(reader.view_key().unwrap());
    viewer
        .add_page(&ScanPage::decode(&corpus.page).unwrap())
        .unwrap();
    assert_eq!(viewer.balance().sent.len(), 1);
    // The payment's change went through `transaction::transfer`, which marks
    // it: the incoming view key collected the first reward and nothing else.
    let mut incoming = IncomingViewer::new(reader.incoming_view_key().unwrap());
    incoming
        .add_page(&ScanPage::decode(&corpus.page).unwrap())
        .unwrap();
    let collected = incoming.receipts();
    assert_eq!(collected.received_atoms, crate::ledger::PREMINE);
    assert_eq!(
        collected.change_atoms,
        viewer.balance().sent[0].change,
        "the wallet's change was not marked"
    );
    assert!(collected.change_atoms > 0);
    scan_page(&corpus.page);
    scan_page(&corpus.page2);
    wallet_state(&corpus.wallet);

    Address::decode(&corpus.texts[0], NETWORK).unwrap();
    ShortAddress::decode(&corpus.texts[1], NETWORK).unwrap();
    ViewKey::decode(&corpus.texts[2], NETWORK).unwrap();
    IncomingViewKey::decode(&corpus.texts[3], NETWORK).unwrap();
    for seed in &corpus.texts {
        text(seed.as_bytes());
    }
}

/// A transaction that decodes, with its txid made to match again: what an
/// attacker who edits one sends, since anyone can recompute a hash.
fn reseal(bytes: &[u8]) -> Option<Vec<u8>> {
    let (mut tx, rest): (Transaction, &[u8]) = postcard::take_from_bytes(bytes).ok()?;
    if !rest.is_empty() {
        return None;
    }
    tx.statement.binding = tx.effects_digest().ok()?;
    tx.encode().ok()
}

#[test]
fn no_transaction_breaks_the_decoder() {
    let corpus = corpus();
    let mut rng = rng(1);
    let donor = &corpus.transfer[..corpus.transfer.len().min(8192)];
    for round in 0..rounds(4000) {
        let mut input = corpus.coinbase.clone();
        mutate::bytes(&mut input, &mut rng, donor);
        survive("transaction", round, &input, || transaction(&input));
    }
    // The payment: most edits land in its proof, and the verifier runs.
    for round in 0..rounds(120) {
        let mut input = corpus.transfer.clone();
        mutate::bytes(&mut input, &mut rng, &corpus.coinbase);
        survive("transaction", round, &input, || transaction(&input));
    }
    // Shapes. The payment goes without its proof: megabytes of bytes that
    // have no shape to change (the proof has its own test).
    let mut bare = Transaction::decode(&corpus.transfer).unwrap();
    bare.proof.proof.clear();
    let trees = [
        capture(&Transaction::decode(&corpus.coinbase).unwrap()).unwrap(),
        capture(&bare).unwrap(),
    ];
    for round in 0..rounds(4000) {
        let mut tree = trees[round % trees.len()].clone();
        if tree.mutate(&mut rng).is_none() {
            continue;
        }
        let input = tree.encode();
        survive("transaction", round, &input, || transaction(&input));
        if let Some(sealed) = reseal(&input) {
            survive("transaction", round, &sealed, || transaction(&sealed));
        }
    }
}

/// Positions of a proof that an accepted mutant may differ at: none. A
/// position listed here is one the verifier does not read, which makes two
/// different byte strings proofs of the same payment.
const UNREAD: &[&[usize]] = &[];

fn unread(path: &[usize]) -> bool {
    UNREAD.iter().any(|prefix| path.starts_with(prefix))
}

#[test]
fn only_the_proof_the_prover_made_is_accepted() {
    let corpus = corpus();
    let (statement, _) = fixture();
    let mut rng = rng(2);

    // Another statement, the same proof.
    let original = capture(&statement).unwrap();
    for round in 0..rounds(40) {
        let mut tree = original.clone();
        let Some(edit) = tree.mutate(&mut rng) else {
            continue;
        };
        let bytes = tree.encode();
        let Ok(mutant) = postcard::from_bytes::<Statement>(&bytes) else {
            continue;
        };
        if mutant == statement {
            continue;
        }
        let accepted = survive("statement", round, &bytes, || {
            proof_of(&mutant, &corpus.proof)
        });
        assert!(
            !accepted,
            "a proof was accepted for another statement: {edit:?}"
        );
    }

    // Another proof, the same statement: first with another shape, then with
    // other bytes.
    let original = proof_tree(&corpus.proof).unwrap();
    let mut accepted = Vec::new();
    for round in 0..rounds(500) {
        let input = if round % 2 == 0 {
            let mut tree = original.clone();
            if tree.mutate(&mut rng).is_none() {
                continue;
            }
            tree.encode()
        } else {
            let mut input = corpus.proof.clone();
            mutate::bytes(&mut input, &mut rng, &corpus.transfer);
            input
        };
        if input == corpus.proof {
            continue;
        }
        if survive("proof", round, &input, || proof(&input)) {
            let mutant = proof_tree(&input).expect("an accepted proof decodes");
            accepted.extend(original.differences(&mutant, 16));
        }
    }
    accepted.sort();
    accepted.dedup();
    let unexpected: Vec<_> = accepted.iter().filter(|path| !unread(path)).collect();
    assert!(
        unexpected.is_empty(),
        "proofs that differ from the prover's here were accepted: {unexpected:?}"
    );
}

/// Headers that match whatever the page now holds, as a server that lies
/// would compute them: the wallet has no proof of work to check, so only a
/// signed checkpoint tells such a page from the chain.
fn reseal_page(page: &mut ScanPage2) -> Option<()> {
    let mut ledger = Ledger::new(NETWORK).ok()?;
    let mut accumulator = [0; 32];
    let mut prev_hash = "0".repeat(64);
    for (offset, block) in page.blocks.iter_mut().enumerate() {
        block.height = page.from.checked_add(offset as u32)?;
        let (coinbase, transfers) = block.transactions.split_first()?;
        let commitments =
            |tx: &ScanTransaction2| tx.outputs.iter().map(|o| o.commitment).collect::<Vec<_>>();
        let outputs: Vec<(Vec<Digest>, &[Digest])> = transfers
            .iter()
            .map(|tx| (commitments(tx), tx.nullifiers.as_slice()))
            .collect();
        replay_effects(
            &mut ledger,
            &commitments(coinbase),
            outputs.iter().map(|(c, n)| (c.as_slice(), *n)),
        )
        .ok()?;
        let spent: BTreeSet<Digest> = transfers
            .iter()
            .flat_map(|tx| tx.nullifiers.iter().copied())
            .collect();
        accumulator = accumulate(&accumulator, &spent);
        block.state_root = state_root(&Committed {
            network: NETWORK,
            height: ledger.height,
            issued: ledger.issued,
            notes: ledger.tree.len(),
            root: ledger.tree.root(),
            nullifiers: accumulator,
        });
        block.prev_hash = prev_hash;
        block.hash = block_hash(
            &block.prev_hash,
            &block.merkle_root,
            &block.state_root,
            block.timestamp,
            block.nonce,
            u64::from(block.height),
        );
        prev_hash = block.hash.clone();
    }
    Some(())
}

#[test]
fn no_page_breaks_a_wallet() {
    let corpus = corpus();
    let mut rng = rng(3);
    let seeds = [&corpus.page2, &corpus.page];
    for round in 0..rounds(1500) {
        let mut input = seeds[round % 2].clone();
        mutate::bytes(&mut input, &mut rng, seeds[(round + 1) % 2]);
        survive("scan_page", round, &input, || scan_page(&input));
    }
    let trees = [
        capture(&ScanPage2::decode(&corpus.page2).unwrap()).unwrap(),
        capture(&ScanPage::decode(&corpus.page).unwrap()).unwrap(),
    ];
    let mut read = 0;
    for round in 0..rounds(3000) {
        let mut tree = trees[round % 2].clone();
        // Up to three edits: pages that are wrong in more than one place.
        let edits = (0..1 + rng.below(3))
            .filter(|_| tree.mutate(&mut rng).is_some())
            .count();
        if edits == 0 {
            continue;
        }
        let input = tree.encode();
        survive("scan_page", round, &input, || scan_page(&input));
        // The same page from a server that lies well: its headers match.
        let Ok(mut page) = ScanPage2::decode(&input) else {
            continue;
        };
        if reseal_page(&mut page).is_none() {
            continue;
        }
        let sealed = page.encode().unwrap();
        survive("scan_page", round, &sealed, || scan_page(&sealed));
        let mut wallet = LightWallet::new(NETWORK, 1).unwrap();
        if wallet.add_page(&reader(), &page).is_ok() && wallet.height() > 0 {
            read += 1;
        }
    }
    assert!(
        read > rounds(3000) / 20,
        "only {read} deformed pages were read to the end: the test no longer reaches the wallet"
    );
}

#[test]
fn every_bit_of_an_envelope_is_authenticated() {
    let corpus = corpus();
    let mut rng = rng(4);
    let part_of = |mut at: usize| {
        for (name, len) in ENVELOPE_PARTS {
            if at < len {
                return name;
            }
            at -= len;
        }
        unreachable!("an offset inside the input")
    };
    for round in 0..rounds(2000) {
        let (at, bit) = (rng.below(corpus.envelope.len()), rng.below(8));
        let mut input = corpus.envelope.clone();
        input[at] ^= 1 << bit;
        let opened = survive("envelope", round, &input, || envelope(&input));
        // What the recipient opens and what the sender opens are sealed
        // apart; the commitment and the KEM ciphertext bind both.
        let expected = match part_of(at) {
            "commitment" | "kem" => (false, false),
            "nonce" | "encrypted" => (false, true),
            _ => (true, false),
        };
        assert_eq!(
            opened,
            expected,
            "bit {bit} of byte {at}, in the {}",
            part_of(at)
        );
    }
    for round in 0..rounds(2000) {
        let mut input = corpus.envelope.clone();
        mutate::bytes(&mut input, &mut rng, &corpus.page2);
        survive("envelope", round, &input, || envelope(&input));
    }
}

#[test]
fn no_text_breaks_the_address_decoders() {
    let corpus = corpus();
    let mut rng = rng(5);
    for round in 0..rounds(3000) {
        let seed = &corpus.texts[round % corpus.texts.len()];
        let donor = &corpus.texts[(round + 1) % corpus.texts.len()];
        let mut input = seed.clone().into_bytes();
        mutate::bytes(&mut input, &mut rng, donor.as_bytes());
        survive("text", round, &input, || text(&input));
    }
}

#[test]
fn no_stored_state_breaks_a_wallet() {
    let corpus = corpus();
    let mut rng = rng(6);
    for round in 0..rounds(1500) {
        let mut input = corpus.wallet.clone();
        mutate::bytes(&mut input, &mut rng, &corpus.page2);
        survive("wallet_state", round, &input, || wallet_state(&input));
    }
    let original = capture(&LightWallet::decode(&corpus.wallet).unwrap()).unwrap();
    for round in 0..rounds(3000) {
        let mut tree = original.clone();
        if tree.mutate(&mut rng).is_none() {
            continue;
        }
        // The format byte goes first: without it nothing is even read.
        let mut input = vec![crate::light::STATE_FORMAT];
        input.extend(tree.encode());
        survive("wallet_state", round, &input, || wallet_state(&input));
    }
}

#[test]
fn an_edit_is_found_where_it_was_made() {
    let (statement, _) = fixture();
    let original = capture(&statement).unwrap();
    let mut rng = rng(7);
    let mut edits = 0;
    for _ in 0..200 {
        let mut tree = original.clone();
        let Some(edit) = tree.mutate(&mut rng) else {
            continue;
        };
        edits += 1;
        let found = original.differences(&tree, 64);
        assert!(
            found
                .iter()
                .any(|path| edit.path.starts_with(path) || path.starts_with(&edit.path)),
            "{edit:?} was not found: {found:?}"
        );
        assert_ne!(tree.encode(), original.encode(), "{edit:?} changed nothing");
    }
    assert!(edits > 150);
    assert!(original.differences(&original, 64).is_empty());
}
