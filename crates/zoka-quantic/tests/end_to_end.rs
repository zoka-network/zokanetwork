//! A Quantic chain from the genesis, without a network: premine, mining,
//! payments with change, coinbase maturity, a second hop, and the frauds a
//! node must reject. Real Halo 2 proofs throughout.

use std::sync::OnceLock;

use orchard::circuit::{ProvingKey, VerifyingKey};
use rand::rngs::StdRng;
use rand::SeedableRng;
use zoka_quantic::builder::{build_coinbase, Payment};
use zoka_quantic::state::{
    block_subsidy, ChainState, ConsensusError, ConsensusParams, Premine, QuanticBlock,
    ATOMS_PER_ZKA, PREMINE_ATOMS,
};
use zoka_quantic::tx::QuanticTransaction;
use zoka_quantic::verify::{proving_key, verify_transactions};
use zoka_quantic::wallet::{QuanticWallet, WalletStateError};
use zoka_quantic::{Network, QuanticAddress, QuanticSpendingKey};

const NETWORK: Network = Network::Regtest;
const ZKA: u64 = ATOMS_PER_ZKA;

fn keys() -> &'static (ProvingKey, VerifyingKey) {
    static KEYS: OnceLock<(ProvingKey, VerifyingKey)> = OnceLock::new();
    KEYS.get_or_init(|| {
        let pk = proving_key();
        let vk = pk.verifying_key();
        (pk, vk)
    })
}

struct Party {
    sk: QuanticSpendingKey,
    wallet: QuanticWallet,
}

impl Party {
    fn new(seed: u8, params: &ConsensusParams) -> Self {
        let sk = QuanticSpendingKey::from_root(&[seed; 32], NETWORK, 0).unwrap();
        let wallet = QuanticWallet::new(sk.full_viewing_key(), params);
        Self { sk, wallet }
    }

    fn address(&self) -> QuanticAddress {
        self.wallet.fvk().address()
    }

    fn pay(&self, to: &QuanticAddress, value: u64, rng: &mut StdRng) -> QuanticTransaction {
        self.wallet
            .create_transfer(
                &self.sk,
                vec![Payment::new(to.clone(), value)],
                0,
                &keys().0,
                rng,
            )
            .unwrap()
    }
}

fn params(team: &QuanticAddress) -> ConsensusParams {
    let mut params = ConsensusParams::new(
        NETWORK,
        Some(Premine {
            recipient: team.clone(),
            value: PREMINE_ATOMS,
        }),
    );
    // Short maturity so the test does not mine 60 blocks.
    params.coinbase_maturity = 2;
    params
}

fn block(
    height: u32,
    miner: &QuanticAddress,
    transactions: Vec<QuanticTransaction>,
    rng: &mut StdRng,
) -> QuanticBlock {
    let fees: u64 = transactions.iter().map(|tx| tx.fee().unwrap()).sum();
    let coinbase = build_coinbase(
        NETWORK,
        height,
        miner,
        block_subsidy(height) + fees,
        false,
        &keys().0,
        rng,
    )
    .unwrap();
    QuanticBlock {
        height,
        coinbase,
        transactions,
    }
}

fn genesis(team: &QuanticAddress, rng: &mut StdRng) -> QuanticBlock {
    QuanticBlock {
        height: 0,
        coinbase: build_coinbase(NETWORK, 0, team, PREMINE_ATOMS, true, &keys().0, rng).unwrap(),
        transactions: vec![],
    }
}

fn apply(
    chain: &mut ChainState,
    parties: &mut [&mut Party],
    block: &QuanticBlock,
    rng: &mut StdRng,
) {
    chain.apply_block(block, &keys().1, &mut *rng).unwrap();
    for party in parties.iter_mut() {
        party.wallet.scan_block(block);
        assert_eq!(party.wallet.anchor().to_bytes(), chain.root().to_bytes());
    }
}

#[test]
fn a_chain_from_the_genesis_with_payments_change_and_mining() {
    let mut rng = StdRng::seed_from_u64(2026);
    let team_sk = QuanticSpendingKey::from_root(&[1; 32], NETWORK, 0).unwrap();
    let params = params(&team_sk.full_viewing_key().address());
    let mut team = Party::new(1, &params);
    let mut miner = Party::new(2, &params);
    let mut alice = Party::new(3, &params);
    let mut bob = Party::new(4, &params);
    let mut chain = ChainState::new(params.clone());

    // Height 0: the premine, spendable at once.
    let b0 = genesis(&team.address(), &mut rng);
    apply(
        &mut chain,
        &mut [&mut team, &mut miner, &mut alice, &mut bob],
        &b0,
        &mut rng,
    );
    assert_eq!(team.wallet.balance(), PREMINE_ATOMS);
    assert_eq!(chain.issued(), PREMINE_ATOMS);

    // Height 1: the team pays Alice 1,000 ZKA; the miner collects subsidy + fee.
    let tx1 = team.pay(&alice.address(), 1_000 * ZKA, &mut rng);
    let fee1 = tx1.fee().unwrap();
    assert_eq!(fee1, 200, "2 actions at the conventional fee");
    chain.check_transfer(&tx1, 1).unwrap();
    let b1 = block(1, &miner.address(), vec![tx1], &mut rng);
    apply(
        &mut chain,
        &mut [&mut team, &mut miner, &mut alice, &mut bob],
        &b1,
        &mut rng,
    );
    assert_eq!(alice.wallet.balance(), 1_000 * ZKA);
    assert_eq!(team.wallet.balance(), PREMINE_ATOMS - 1_000 * ZKA - fee1);
    assert_eq!(miner.wallet.balance(), 0);
    assert_eq!(miner.wallet.immature_balance(), 12 * ZKA + fee1);
    assert_eq!(bob.wallet.balance(), 0);

    // Heights 2 and 3: the coinbase of height 1 matures at 3.
    for height in 2..=3 {
        let b = block(height, &miner.address(), vec![], &mut rng);
        apply(
            &mut chain,
            &mut [&mut team, &mut miner, &mut alice, &mut bob],
            &b,
            &mut rng,
        );
    }
    assert_eq!(miner.wallet.balance(), 12 * ZKA + fee1);
    assert_eq!(miner.wallet.immature_balance(), 24 * ZKA);

    // Height 4: Alice pays Bob, and the miner spends its matured coinbase.
    let tx_alice = alice.pay(&bob.address(), 250 * ZKA, &mut rng);
    let tx_miner = miner.pay(&bob.address(), 5 * ZKA, &mut rng);
    let b4 = block(
        4,
        &miner.address(),
        vec![tx_alice.clone(), tx_miner],
        &mut rng,
    );
    apply(
        &mut chain,
        &mut [&mut team, &mut miner, &mut alice, &mut bob],
        &b4,
        &mut rng,
    );
    assert_eq!(bob.wallet.balance(), 255 * ZKA);
    assert_eq!(alice.wallet.balance(), 750 * ZKA - tx_alice.fee().unwrap());

    // A note can be spent once: the same transaction again is refused.
    let replay = block(5, &miner.address(), vec![tx_alice], &mut rng);
    assert_eq!(
        chain.apply_block(&replay, &keys().1, &mut rng).unwrap_err(),
        ConsensusError::SpentNullifier
    );

    // Bob spends what he got from two different senders in one transfer.
    let tx_bob = bob.pay(&alice.address(), 254 * ZKA, &mut rng);
    let b5 = block(5, &miner.address(), vec![tx_bob], &mut rng);
    apply(
        &mut chain,
        &mut [&mut team, &mut miner, &mut alice, &mut bob],
        &b5,
        &mut rng,
    );
    assert_eq!(bob.wallet.balance(), 255 * ZKA - 254 * ZKA - fees_of(&b5));
    assert!(alice.wallet.balance() > 1_000 * ZKA);

    // History: Bob received from two senders and then sent most of it.
    let history = bob.wallet.history();
    let sent = history.iter().find(|h| h.spent > 0).expect("bob's send");
    assert_eq!(sent.net(), -(254 * ZKA as i128) - i128::from(fees_of(&b5)));
    assert_eq!(
        history
            .iter()
            .filter(|h| h.received > 0 && h.spent == 0)
            .count(),
        2
    );
    assert!(miner.wallet.history().iter().any(|h| h.coinbase));

    // All value is accounted for: what the wallets hold is what was issued.
    let held: u64 = [&team, &miner, &alice, &bob]
        .iter()
        .map(|p| p.wallet.balance() + p.wallet.immature_balance())
        .sum();
    assert_eq!(chain.issued(), PREMINE_ATOMS + 5 * 12 * ZKA);
    assert_eq!(held, chain.issued());
}

fn fees_of(block: &QuanticBlock) -> u64 {
    block.transactions.iter().map(|tx| tx.fee().unwrap()).sum()
}

#[test]
fn transactions_survive_their_wire_format_and_any_change_breaks_them() {
    let mut rng = StdRng::seed_from_u64(7);
    let team_sk = QuanticSpendingKey::from_root(&[1; 32], NETWORK, 0).unwrap();
    let params = params(&team_sk.full_viewing_key().address());
    let mut team = Party::new(1, &params);
    let alice = Party::new(3, &params);
    let mut chain = ChainState::new(params.clone());
    let b0 = genesis(&team.address(), &mut rng);
    apply(&mut chain, &mut [&mut team], &b0, &mut rng);

    let tx = team.pay(&alice.address(), 10 * ZKA, &mut rng);
    let bytes = tx.to_bytes();
    assert_eq!(bytes.len(), tx.serialized_size());
    let decoded = QuanticTransaction::from_bytes(&bytes).unwrap();
    assert_eq!(decoded.txid(), tx.txid());
    assert_eq!(decoded.auth_digest(), tx.auth_digest());
    assert_eq!(decoded.to_bytes(), bytes);
    assert!(verify_transactions(&keys().1, [&decoded], &mut rng));

    // 2 actions: about 11 KB, against 59 to 346 KB for a Zenith transaction.
    assert!((10_000..12_500).contains(&bytes.len()), "{}", bytes.len());

    // A flipped bit in an X-Wing ciphertext, in the memo part, in the outgoing
    // part or in the proof: decodes, but no longer verifies.
    let kem_offset = 12 + 4 * 32;
    let memo_offset = kem_offset + 1120 + 100;
    let out_offset = kem_offset + 1120 + 580 + 10;
    let proof_offset = bytes.len() - 64 * 3 - 100;
    for offset in [kem_offset, memo_offset, out_offset, proof_offset] {
        let mut altered = bytes.clone();
        altered[offset] ^= 1;
        let altered = QuanticTransaction::from_bytes(&altered).unwrap();
        assert!(
            !verify_transactions(&keys().1, [&altered], &mut rng),
            "offset {offset}"
        );
    }
    // Trailing bytes, truncation and a padded proof are refused when decoding.
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(QuanticTransaction::from_bytes(&longer).is_err());
    assert!(QuanticTransaction::from_bytes(&bytes[..bytes.len() - 1]).is_err());
}

/// What a wallet shows, to compare a saved one with the original.
fn view(wallet: &QuanticWallet) -> (u64, u64, [u8; 32], u32, usize, Vec<zoka_quantic::wallet::HistoryEntry>) {
    (
        wallet.balance(),
        wallet.immature_balance(),
        wallet.anchor().to_bytes(),
        wallet.next_height(),
        wallet.notes().len(),
        wallet.history(),
    )
}

#[test]
fn a_saved_wallet_resumes_where_it_stopped() {
    let mut rng = StdRng::seed_from_u64(31);
    let team_sk = QuanticSpendingKey::from_root(&[1; 32], NETWORK, 0).unwrap();
    let params = params(&team_sk.full_viewing_key().address());
    let mut team = Party::new(1, &params);
    let mut miner = Party::new(2, &params);
    let mut alice = Party::new(3, &params);
    let mut bob = Party::new(4, &params);
    let mut chain = ChainState::new(params.clone());

    let b0 = genesis(&team.address(), &mut rng);
    apply(&mut chain, &mut [&mut team, &mut miner, &mut alice], &b0, &mut rng);
    let tx = team.pay(&alice.address(), 40 * ZKA, &mut rng);
    let b1 = block(1, &miner.address(), vec![tx], &mut rng);
    apply(&mut chain, &mut [&mut team, &mut miner, &mut alice], &b1, &mut rng);
    let b2 = block(2, &miner.address(), vec![], &mut rng);
    apply(&mut chain, &mut [&mut team, &mut miner, &mut alice], &b2, &mut rng);
    // The team spent the premine and holds change; Alice holds a payment; the
    // miner holds two coinbases that have not entered the tree yet.
    assert!(miner.wallet.balance() == 0 && miner.wallet.immature_balance() > 0);

    let mut restored = Vec::new();
    for party in [&team, &miner, &alice] {
        let saved = party.wallet.to_state_bytes();
        let wallet = QuanticWallet::from_state_bytes(&saved, &params).expect("restores");
        assert_eq!(view(&wallet), view(&party.wallet));
        assert_eq!(*wallet.to_state_bytes(), *saved, "saving again gives the same bytes");
        restored.push(wallet);
    }
    let [mut team_again, mut miner_again, alice_again] = <[QuanticWallet; 3]>::try_from(restored).unwrap();

    // Both keep scanning the same way: the coinbase of block 1 matures at 3.
    let b3 = block(3, &miner.address(), vec![], &mut rng);
    apply(&mut chain, &mut [&mut team, &mut miner], &b3, &mut rng);
    for wallet in [&mut team_again, &mut miner_again] {
        wallet.scan_block(&b3);
    }
    assert_eq!(view(&miner_again), view(&miner.wallet));
    assert!(miner_again.balance() > 0);

    // And the restored wallets spend: their witnesses lead to the chain's root.
    let from_miner = miner_again
        .create_transfer(&miner.sk, vec![Payment::new(bob.address(), 5 * ZKA)], 0, &keys().0, &mut rng)
        .unwrap();
    let from_team = team_again
        .create_transfer(&team.sk, vec![Payment::new(bob.address(), 7 * ZKA)], 0, &keys().0, &mut rng)
        .unwrap();
    chain.check_transfer(&from_miner, 4).unwrap();
    chain.check_transfer(&from_team, 4).unwrap();
    let b4 = block(4, &miner.address(), vec![from_miner, from_team], &mut rng);
    chain.apply_block(&b4, &keys().1, &mut rng).unwrap();
    bob.wallet.scan_block(&b0);
    for b in [&b1, &b2, &b3, &b4] {
        bob.wallet.scan_block(b);
    }
    assert_eq!(bob.wallet.balance(), 12 * ZKA);

    // A saved wallet only opens for its own chain, whole.
    let saved = alice_again.to_state_bytes();
    let mut other_maturity = params.clone();
    other_maturity.coinbase_maturity = 60;
    assert_eq!(
        QuanticWallet::from_state_bytes(&saved, &other_maturity).unwrap_err(),
        WalletStateError::OtherChain
    );
    assert_eq!(
        QuanticWallet::from_state_bytes(&saved[..saved.len() - 1], &params).unwrap_err(),
        WalletStateError::Truncated
    );
    let mut longer = saved.to_vec();
    longer.push(0);
    assert!(QuanticWallet::from_state_bytes(&longer, &params).is_err());
    let mut wrong_magic = saved.to_vec();
    wrong_magic[0] ^= 1;
    assert_eq!(
        QuanticWallet::from_state_bytes(&wrong_magic, &params).unwrap_err(),
        WalletStateError::NotAWalletState
    );
    let mut future = saved.to_vec();
    future[4] = 9;
    assert_eq!(
        QuanticWallet::from_state_bytes(&future, &params).unwrap_err(),
        WalletStateError::UnknownVersion(9)
    );
    // Alice's notes in someone else's wallet are refused.
    let mut foreign = saved.to_vec();
    let bob_key = bob.wallet.fvk().to_bytes();
    foreign[5..5 + bob_key.len()].copy_from_slice(bob_key.as_ref());
    assert_eq!(
        QuanticWallet::from_state_bytes(&foreign, &params).unwrap_err(),
        WalletStateError::Invalid("note owner")
    );
}

#[test]
fn the_rules_of_the_coinbase_and_the_premine_are_enforced() {
    let mut rng = StdRng::seed_from_u64(9);
    let team_sk = QuanticSpendingKey::from_root(&[1; 32], NETWORK, 0).unwrap();
    let params = params(&team_sk.full_viewing_key().address());
    let team = Party::new(1, &params);
    let thief = Party::new(6, &params);

    // A genesis that pays someone else, or pays the team without making the
    // output public, is not the announced premine.
    for (to, public) in [(thief.address(), true), (team.address(), false)] {
        let mut chain = ChainState::new(params.clone());
        let fake = QuanticBlock {
            height: 0,
            coinbase: build_coinbase(NETWORK, 0, &to, PREMINE_ATOMS, public, &keys().0, &mut rng)
                .unwrap(),
            transactions: vec![],
        };
        assert_eq!(
            chain.apply_block(&fake, &keys().1, &mut rng).unwrap_err(),
            ConsensusError::PremineMismatch
        );
    }
    let mut chain = ChainState::new(params.clone());
    let too_much = QuanticBlock {
        height: 0,
        coinbase: build_coinbase(
            NETWORK,
            0,
            &team.address(),
            PREMINE_ATOMS + 1,
            true,
            &keys().0,
            &mut rng,
        )
        .unwrap(),
        transactions: vec![],
    };
    assert!(matches!(
        chain
            .apply_block(&too_much, &keys().1, &mut rng)
            .unwrap_err(),
        ConsensusError::CoinbaseValue { .. }
    ));
    chain
        .apply_block(&genesis(&team.address(), &mut rng), &keys().1, &mut rng)
        .unwrap();

    // A miner that takes one atom more than subsidy + fees.
    let greedy = QuanticBlock {
        height: 1,
        coinbase: build_coinbase(
            NETWORK,
            1,
            &thief.address(),
            block_subsidy(1) + 1,
            false,
            &keys().0,
            &mut rng,
        )
        .unwrap(),
        transactions: vec![],
    };
    assert!(matches!(
        chain.apply_block(&greedy, &keys().1, &mut rng).unwrap_err(),
        ConsensusError::CoinbaseValue { .. }
    ));
    // A coinbase for another height, and a block out of order.
    let wrong_height = QuanticBlock {
        height: 1,
        coinbase: build_coinbase(
            NETWORK,
            2,
            &thief.address(),
            block_subsidy(2),
            false,
            &keys().0,
            &mut rng,
        )
        .unwrap(),
        transactions: vec![],
    };
    assert_eq!(
        chain
            .apply_block(&wrong_height, &keys().1, &mut rng)
            .unwrap_err(),
        ConsensusError::CoinbaseHeight
    );
    let skipped = block(2, &thief.address(), vec![], &mut rng);
    assert!(matches!(
        chain
            .apply_block(&skipped, &keys().1, &mut rng)
            .unwrap_err(),
        ConsensusError::WrongHeight { .. }
    ));
    // The honest block 1 still goes in.
    chain
        .apply_block(
            &block(1, &thief.address(), vec![], &mut rng),
            &keys().1,
            &mut rng,
        )
        .unwrap();
    assert_eq!(chain.issued(), PREMINE_ATOMS + 12 * ZKA);
}

#[test]
fn a_transfer_needs_a_known_anchor_and_cannot_be_reused_across_networks() {
    let mut rng = StdRng::seed_from_u64(11);
    let team_sk = QuanticSpendingKey::from_root(&[1; 32], NETWORK, 0).unwrap();
    let params = params(&team_sk.full_viewing_key().address());
    let mut team = Party::new(1, &params);
    let alice = Party::new(3, &params);
    let mut chain = ChainState::new(params.clone());
    let b0 = genesis(&team.address(), &mut rng);
    apply(&mut chain, &mut [&mut team], &b0, &mut rng);
    let tx = team.pay(&alice.address(), ZKA, &mut rng);

    // Another anchor: the bytes decode, the chain does not know the root.
    let mut bytes = tx.to_bytes();
    let anchor_offset = 12 + tx.actions().len() * 1919 + 1 + 8;
    bytes[anchor_offset] ^= 1;
    let other = QuanticTransaction::from_bytes(&bytes).unwrap();
    assert_eq!(
        chain.check_transfer(&other, 1).unwrap_err(),
        ConsensusError::UnknownAnchor
    );

    // The same transaction declared for mainnet: another txid, so its
    // signatures do not hold, and the regtest chain refuses it anyway.
    let mut bytes = tx.to_bytes();
    bytes[4] = Network::Mainnet.id();
    let moved = QuanticTransaction::from_bytes(&bytes).unwrap();
    assert_ne!(moved.txid(), tx.txid());
    assert!(!verify_transactions(&keys().1, [&moved], &mut rng));
    assert_eq!(
        chain.check_transfer(&moved, 1).unwrap_err(),
        ConsensusError::WrongNetwork
    );

    // Two transfers spending the same note in one block.
    let twin = team.pay(&alice.address(), 2 * ZKA, &mut rng);
    let doubled = block(1, &alice.address(), vec![tx, twin], &mut rng);
    assert_eq!(
        chain
            .apply_block(&doubled, &keys().1, &mut rng)
            .unwrap_err(),
        ConsensusError::DuplicateNullifier
    );
}

#[test]
fn a_light_wallet_on_compact_blocks_sees_the_same_and_catches_a_lying_node() {
    use zoka_quantic::compact::CompactBlock;

    let mut rng = StdRng::seed_from_u64(13);
    let team_sk = QuanticSpendingKey::from_root(&[1; 32], NETWORK, 0).unwrap();
    let params = params(&team_sk.full_viewing_key().address());
    let mut team = Party::new(1, &params);
    let mut alice = Party::new(3, &params);
    let mut light = Party::new(3, &params);
    let bob = Party::new(4, &params);
    let mut chain = ChainState::new(params.clone());

    let b0 = genesis(&team.address(), &mut rng);
    apply(&mut chain, &mut [&mut team, &mut alice], &b0, &mut rng);
    let c0 = CompactBlock::from_block(&b0);
    assert!(c0.matches_header(&b0.txid_root()));
    light.wallet.scan_compact_block(&c0);

    let tx = team.pay(&alice.address(), 40 * ZKA, &mut rng);
    let b1 = block(1, &bob.address(), vec![tx], &mut rng);
    apply(&mut chain, &mut [&mut team, &mut alice], &b1, &mut rng);
    let c1 = CompactBlock::from_block(&b1);
    assert!(c1.matches_header(&b1.txid_root()));
    light.wallet.scan_compact_block(&c1);

    // Same balance and same tree as the wallet that saw the whole blocks.
    assert_eq!(light.wallet.balance(), 40 * ZKA);
    assert_eq!(light.wallet.balance(), alice.wallet.balance());
    assert_eq!(light.wallet.anchor().to_bytes(), chain.root().to_bytes());
    assert!(light.wallet.notes()[0].memo.is_none());

    // What travels: about a quarter of the whole transactions.
    let full: usize = b1.all_transactions().map(|t| t.to_bytes().len()).sum();
    assert!(
        c1.serialized_size() * 3 < full,
        "{} vs {}",
        c1.serialized_size(),
        full
    );
    for tx in &c1.transactions {
        let bytes = tx.to_bytes();
        let decoded = zoka_quantic::compact::CompactTx::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.txid(), tx.txid());
    }

    // A lying node: hides the transfer, alters a ciphertext or the fee, or
    // reorders. Every version fails against the header.
    let root = b1.txid_root();
    let mut hidden = c1.clone();
    hidden.transactions.pop();
    assert!(!hidden.matches_header(&root));
    let mut altered = c1.clone();
    altered.transactions[1].actions[0].kem[5] ^= 1;
    assert!(!altered.matches_header(&root));
    let mut fee = c1.clone();
    fee.transactions[1].value_balance += 1;
    assert!(!fee.matches_header(&root));
    let mut reordered = c1.clone();
    reordered.transactions.swap(0, 1);
    assert!(!reordered.matches_header(&root));

    // The light wallet spends with the witnesses it built from compact data.
    let spend = light.pay(&bob.address(), 10 * ZKA, &mut rng);
    let b2 = block(2, &bob.address(), vec![spend], &mut rng);
    chain.apply_block(&b2, &keys().1, &mut rng).unwrap();
}

#[test]
fn an_anchor_older_than_the_window_is_refused_and_the_genesis_can_be_block_one() {
    let mut rng = StdRng::seed_from_u64(17);
    let team_sk = QuanticSpendingKey::from_root(&[1; 32], NETWORK, 0).unwrap();
    let mut params = params(&team_sk.full_viewing_key().address());
    // As in the node: the genesis is block 1, and a short window for the test.
    params.genesis_height = 1;
    params.anchor_window = 2;
    let mut team = Party::new(1, &params);
    let miner = Party::new(2, &params);
    let mut chain = ChainState::new(params.clone());

    let genesis = QuanticBlock {
        height: 1,
        coinbase: build_coinbase(
            NETWORK,
            1,
            &team.address(),
            PREMINE_ATOMS,
            true,
            &keys().0,
            &mut rng,
        )
        .unwrap(),
        transactions: vec![],
    };
    apply(&mut chain, &mut [&mut team], &genesis, &mut rng);
    assert_eq!(team.wallet.balance(), PREMINE_ATOMS);
    // Built now, sent too late.
    let late = team.pay(&miner.address(), ZKA, &mut rng);

    // The subsidy counts from the genesis: block 2 is the first mined block.
    for height in 2..=6 {
        let fees = 0;
        let b = QuanticBlock {
            height,
            coinbase: build_coinbase(
                NETWORK,
                height,
                &miner.address(),
                params.minted_at(height) + fees,
                false,
                &keys().0,
                &mut rng,
            )
            .unwrap(),
            transactions: vec![],
        };
        apply(&mut chain, &mut [&mut team], &b, &mut rng);
    }
    assert_eq!(chain.issued(), PREMINE_ATOMS + 5 * 12 * ZKA);
    // Matured coinbases changed the tree: the old root left the window.
    assert_eq!(
        chain.check_transfer(&late, 7).unwrap_err(),
        ConsensusError::UnknownAnchor
    );
    // Rebuilt on the current anchor, the same payment goes in.
    let fresh = team.pay(&miner.address(), ZKA, &mut rng);
    chain.check_transfer(&fresh, 7).unwrap();
}
