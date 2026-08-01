//! Peer-to-peer ZKA → USDT offers.
//!
//! A seller locks ZKA and publishes the USDT price they want for it. A buyer
//! claims the offer, pays USDT to the address in it, and the seller's node —
//! the only party holding the spend key — releases the ZKA once it has seen
//! that payment confirmed on Tron.
//!
//! Scope is deliberately one-directional: ZKA is always what is offered, USDT
//! always what is asked. There is no arbiter and no dispute process, so every
//! outcome here has to be reachable without a human deciding anything.
//!
//! ## What this module is not
//!
//! The ZKA lock is bookkeeping in the seller's own wallet, not an on-chain
//! escrow. Nothing at protocol level stops a modified client from spending a
//! reserved note after collecting USDT. That limitation is inherent to the v1
//! design and has to be surfaced to users rather than papered over here.

use serde::{Deserialize, Serialize};

pub mod reservation;
pub mod store;
pub mod tron;

pub use reservation::{select_notes, Reservation, ReservationError, SpendableNote};
pub use store::{OfferStore, ReconcileReport, StoreError};
pub use tron::{TronConfig, TronTransfer};

/// Generates an offer id: 128 random bits, hex encoded.
///
/// Random rather than derived from the seller and a timestamp. A derived id
/// collides when one seller publishes twice in the same second, and the id is
/// covered by [`Offer::signing_preimage`] — two offers sharing an id would
/// share a signature, so a collision is a forgery vector rather than a nuisance.
pub fn new_offer_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Micro-USDT per USDT. TRC-20 USDT carries six decimals, so amounts are held
/// as integers — a float would silently round prices and, worse, make two
/// nodes disagree about whether a payment matched.
pub const MICRO_USDT_PER_USDT: u64 = 1_000_000;

/// How long a claimed offer waits for payment before it expires and the ZKA
/// is released back to the seller. Long enough to complete a real transfer,
/// short enough that a buyer who walks away does not strand the funds.
pub const CLAIM_WINDOW_SECONDS: u64 = 45 * 60;

/// Delay before a buyer may publish a non-release report. This avoids
/// permanently accusing a seller because a release and the deadline crossed
/// in adjacent polling/gossip cycles.
pub const FAILURE_REPORT_GRACE_SECONDS: u64 = 5 * 60;

/// Matching tolerance on the USDT amount, in basis points (0.5%). Exchanges
/// and wallets shave fees off transfers, so demanding the exact figure would
/// reject honest payments.
pub const AMOUNT_TOLERANCE_BPS: u64 = 50;

/// Smallest publishable offer. Publishing costs nothing but bandwidth, so
/// without a floor the gossip topic is free to flood.
pub const MIN_OFFER_ZKA_ATOMS: u64 = 10 * crate::economics::ATOMS_PER_ZKA;

/// Smallest USDT an offer may ask for. Guards against offers that are
/// unmatchable in practice because the amount rounds below Tron's precision.
pub const MIN_OFFER_MICRO_USDT: u64 = MICRO_USDT_PER_USDT;

/// How many offers one seller may keep open at once. The second half of the
/// anti-spam rule: a floor alone does not stop one address from publishing
/// thousands of minimum-size offers.
pub const MAX_OPEN_OFFERS_PER_ADDRESS: usize = 5;

/// Lifecycle of an offer.
///
/// Terminal states are `Released`, `Expired` and `Cancelled`. Every path out
/// of `Claimed` is reachable without human intervention: either the payment
/// is observed and the ZKA is released, or the window lapses and the lock is
/// dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OfferStatus {
    /// Published and claimable. ZKA is reserved in the seller's wallet.
    Open,
    /// A buyer took it; the payment window is running.
    Claimed,
    /// Payment seen on Tron and verified. Release is imminent.
    PaymentDetected,
    /// ZKA spent to the buyer. Terminal.
    Released,
    /// Claim window lapsed with no verified payment. Terminal.
    Expired,
    /// Seller withdrew it before anyone claimed. Terminal.
    Cancelled,
}

impl OfferStatus {
    /// Whether this offer can still be claimed by a buyer.
    pub fn is_claimable(self) -> bool {
        matches!(self, OfferStatus::Open)
    }

    /// Whether the offer has finished and holds no reservation any more.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            OfferStatus::Released | OfferStatus::Expired | OfferStatus::Cancelled
        )
    }

    /// Whether the seller's ZKA is still locked in this state.
    pub fn holds_reservation(self) -> bool {
        !self.is_terminal()
    }
}

/// A published ZKA → USDT offer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub id: String,
    /// Seller's ZKA address; also the identity their signature is checked
    /// against, so nobody can publish or cancel offers on their behalf.
    pub seller_address: String,
    pub zka_atoms: u64,
    pub micro_usdt: u64,
    /// TRC-20 address the buyer must pay. Declared by the seller, and the only
    /// address a payment is matched against.
    pub usdt_receive_address: String,
    pub status: OfferStatus,
    pub created_at: u64,
    /// Set when claimed; the offer expires at this instant if no payment is
    /// verified. `None` while the offer is still open.
    pub claim_expires_at: Option<u64>,
    pub claimed_by: Option<String>,
    /// Notes reserved in the seller's wallet backing this offer. Kept so a
    /// restart can tell which reservations still have an offer behind them.
    #[serde(default)]
    pub reserved_note_ids: Vec<String>,
    /// Cuánto suman esas notas. Es **mayor o igual** a `zka_atoms`, casi siempre
    /// mayor: una nota privada es indivisible, así que respaldar una venta de 20
    /// ZKA puede exigir inmovilizar una nota de 27,99 entera.
    ///
    /// Se guarda porque sin este número la interfaz no puede explicar por qué el
    /// saldo disponible bajó más que lo vendido. El excedente vuelve como vuelto
    /// al cerrarse la operación, pero mientras tanto no se puede gastar, y el
    /// usuario merece verlo en vez de deducirlo.
    #[serde(default)]
    pub reserved_atoms: u64,
}

/// Public evidence that a seller did not release after an on-chain payment.
///
/// Publishing this deliberately links the buyer's Tron transaction to the
/// trade; the UI must obtain explicit confirmation before sending it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentFailureReport {
    pub offer_id: String,
    pub seller_address: String,
    pub buyer_address: String,
    pub tron_tx_hash: String,
    pub timestamp: u64,
    #[serde(default)]
    pub signature: Vec<u8>,
}

impl PaymentFailureReport {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.offer_id.trim().is_empty()
            || self.seller_address.trim().is_empty()
            || self.buyer_address.trim().is_empty()
        {
            return Err("payment failure report is missing trade identities");
        }
        if self.tron_tx_hash.len() != 64
            || !self
                .tron_tx_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("payment failure report needs a 64-character Tron transaction hash");
        }
        Ok(())
    }
}

/// Why an offer was rejected. Rejections are values rather than strings so
/// the UI can explain each case in its own words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfferRejection {
    EmptyId,
    EmptySellerAddress,
    EmptyUsdtAddress,
    MalformedUsdtAddress,
    ZkaBelowMinimum,
    UsdtBelowMinimum,
    ClaimedWithoutBuyer,
    ClaimedWithoutDeadline,
    OpenWithBuyer,
}

impl OfferRejection {
    pub fn as_str(self) -> &'static str {
        match self {
            OfferRejection::EmptyId => "offer id is empty",
            OfferRejection::EmptySellerAddress => "seller address is empty",
            OfferRejection::EmptyUsdtAddress => "USDT receive address is empty",
            OfferRejection::MalformedUsdtAddress => {
                "USDT receive address is not a TRON base58 address"
            }
            OfferRejection::ZkaBelowMinimum => "offer is below the minimum ZKA amount",
            OfferRejection::UsdtBelowMinimum => "offer asks less than the minimum USDT",
            OfferRejection::ClaimedWithoutBuyer => "claimed offer has no buyer",
            OfferRejection::ClaimedWithoutDeadline => "claimed offer has no expiry",
            OfferRejection::OpenWithBuyer => "open offer already names a buyer",
        }
    }
}

/// Shape check for a TRON base58 address.
///
/// Mainnet addresses start with `T` and are 34 base58 characters. This catches
/// typos and Ethereum-style `0x` addresses pasted by mistake; it does not
/// verify the checksum, so a well-formed but wrong address still passes — the
/// payment simply never arrives and the offer expires.
pub fn is_plausible_tron_address(address: &str) -> bool {
    const BASE58: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    address.len() == 34 && address.starts_with('T') && address.chars().all(|c| BASE58.contains(c))
}

/// Whether `paid` satisfies an offer asking `asked`.
///
/// Underpayment outside the tolerance does not release: the seller keeps their
/// ZKA and the offer expires normally. Overpayment does release — the buyer
/// already sent more than was asked, and holding the ZKA hostage over their
/// mistake would be theft dressed up as strictness.
pub fn payment_satisfies(asked_micro_usdt: u64, paid_micro_usdt: u64) -> bool {
    if paid_micro_usdt >= asked_micro_usdt {
        return true;
    }
    let shortfall = asked_micro_usdt - paid_micro_usdt;
    // Compare against the tolerance without dividing, so small amounts do not
    // round their allowance down to zero.
    shortfall * 10_000 <= asked_micro_usdt * AMOUNT_TOLERANCE_BPS
}

impl Offer {
    /// Validates the offer's shape.
    ///
    /// This is what a receiving node runs before relaying gossip, so it must
    /// reject anything self-inconsistent — an "open" offer that already names
    /// a buyer, or a "claimed" one with no deadline, are how a malicious peer
    /// would try to jam a claim through.
    pub fn validate(&self) -> Result<(), OfferRejection> {
        if self.id.trim().is_empty() {
            return Err(OfferRejection::EmptyId);
        }
        if self.seller_address.trim().is_empty() {
            return Err(OfferRejection::EmptySellerAddress);
        }
        if self.usdt_receive_address.trim().is_empty() {
            return Err(OfferRejection::EmptyUsdtAddress);
        }
        if !is_plausible_tron_address(&self.usdt_receive_address) {
            return Err(OfferRejection::MalformedUsdtAddress);
        }
        if self.zka_atoms < MIN_OFFER_ZKA_ATOMS {
            return Err(OfferRejection::ZkaBelowMinimum);
        }
        if self.micro_usdt < MIN_OFFER_MICRO_USDT {
            return Err(OfferRejection::UsdtBelowMinimum);
        }
        match self.status {
            OfferStatus::Open => {
                if self.claimed_by.is_some() {
                    return Err(OfferRejection::OpenWithBuyer);
                }
            }
            OfferStatus::Claimed | OfferStatus::PaymentDetected => {
                if self.claimed_by.is_none() {
                    return Err(OfferRejection::ClaimedWithoutBuyer);
                }
                if self.claim_expires_at.is_none() {
                    return Err(OfferRejection::ClaimedWithoutDeadline);
                }
            }
            OfferStatus::Released | OfferStatus::Expired | OfferStatus::Cancelled => {}
        }
        Ok(())
    }

    /// Whether the claim window has run out at `now`.
    ///
    /// Only meaningful for a claimed offer: an open one has no deadline, and a
    /// terminal one has already released or freed its reservation.
    pub fn is_expired_at(&self, now: u64) -> bool {
        match (self.status, self.claim_expires_at) {
            (OfferStatus::Claimed, Some(deadline)) => now >= deadline,
            _ => false,
        }
    }

    /// Canonical bytes a seller signs to authorise this offer.
    ///
    /// Built field by field with explicit separators rather than by serialising
    /// the struct: JSON key order is not guaranteed, so signing the encoded
    /// form would let two nodes derive different preimages for the same offer
    /// and reject each other's valid signatures.
    ///
    /// Covers everything a forger would want to alter — amounts, the payout
    /// address, and the seller's own identity. Mutable lifecycle fields
    /// (`status`, `claimed_by`, `claim_expires_at`) are excluded: they change
    /// legitimately after publication, so including them would invalidate the
    /// signature the moment a buyer claimed.
    pub fn signing_preimage(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"zoka:offer:v1\n");
        out.extend_from_slice(self.id.as_bytes());
        out.push(b'\n');
        out.extend_from_slice(self.seller_address.as_bytes());
        out.push(b'\n');
        out.extend_from_slice(self.zka_atoms.to_be_bytes().as_slice());
        out.extend_from_slice(self.micro_usdt.to_be_bytes().as_slice());
        out.extend_from_slice(self.usdt_receive_address.as_bytes());
        out.push(b'\n');
        out.extend_from_slice(self.created_at.to_be_bytes().as_slice());
        out
    }

    /// Price expressed as micro-USDT per whole ZKA, for display and sorting.
    /// Returns `None` for a zero-amount offer rather than dividing by zero.
    pub fn micro_usdt_per_zka(&self) -> Option<u64> {
        if self.zka_atoms == 0 {
            return None;
        }
        Some(
            (self.micro_usdt as u128 * crate::economics::ATOMS_PER_ZKA as u128
                / self.zka_atoms as u128) as u64,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_offer() -> Offer {
        Offer {
            id: "offer-1".to_string(),
            seller_address: "zka1seller".to_string(),
            zka_atoms: 100 * crate::economics::ATOMS_PER_ZKA,
            micro_usdt: 25 * MICRO_USDT_PER_USDT,
            usdt_receive_address: "TQ5NMqJjW8dWvJf3zJ8xYqL2mNpR4sT6uV".to_string(),
            status: OfferStatus::Open,
            created_at: 1_751_000_000,
            claim_expires_at: None,
            claimed_by: None,
            reserved_note_ids: vec!["note-a".to_string()],
            reserved_atoms: 0,
        }
    }

    #[test]
    fn a_well_formed_open_offer_validates() {
        assert_eq!(valid_offer().validate(), Ok(()));
    }

    #[test]
    fn the_test_address_is_actually_shaped_like_a_tron_address() {
        // Guards the fixture itself: if this were malformed, every other test
        // here would be asserting against a rejection it never intended.
        assert!(is_plausible_tron_address(
            &valid_offer().usdt_receive_address
        ));
    }

    #[test]
    fn ethereum_style_addresses_are_rejected() {
        let mut offer = valid_offer();
        offer.usdt_receive_address = "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string();
        assert_eq!(
            offer.validate(),
            Err(OfferRejection::MalformedUsdtAddress),
            "pasting an ETH address must fail before the offer is published"
        );
    }

    #[test]
    fn tron_addresses_must_be_34_chars_starting_with_t() {
        assert!(!is_plausible_tron_address(""));
        assert!(!is_plausible_tron_address("T"));
        // Right length, wrong prefix.
        assert!(!is_plausible_tron_address(
            "AQ5NMqJjW8dWvJf3zJ8xYqL2mNpR4sT6uV"
        ));
        // Right prefix, one char short.
        assert!(!is_plausible_tron_address(
            "TQ5NMqJjW8dWvJf3zJ8xYqL2mNpR4sT6u"
        ));
        // Contains base58-excluded characters (0, O, I, l).
        assert!(!is_plausible_tron_address(
            "T0OIlqJjW8dWvJf3zJ8xYqL2mNpR4sT6uV"
        ));
    }

    #[test]
    fn offers_below_the_minimum_are_rejected() {
        let mut offer = valid_offer();
        offer.zka_atoms = MIN_OFFER_ZKA_ATOMS - 1;
        assert_eq!(offer.validate(), Err(OfferRejection::ZkaBelowMinimum));

        let mut offer = valid_offer();
        offer.micro_usdt = MIN_OFFER_MICRO_USDT - 1;
        assert_eq!(offer.validate(), Err(OfferRejection::UsdtBelowMinimum));
    }

    #[test]
    fn an_open_offer_naming_a_buyer_is_rejected() {
        // How a peer would try to smuggle in a claim it never made.
        let mut offer = valid_offer();
        offer.claimed_by = Some("zka1buyer".to_string());
        assert_eq!(offer.validate(), Err(OfferRejection::OpenWithBuyer));
    }

    #[test]
    fn a_claimed_offer_needs_both_a_buyer_and_a_deadline() {
        let mut offer = valid_offer();
        offer.status = OfferStatus::Claimed;
        offer.claim_expires_at = Some(1_751_002_700);
        assert_eq!(offer.validate(), Err(OfferRejection::ClaimedWithoutBuyer));

        let mut offer = valid_offer();
        offer.status = OfferStatus::Claimed;
        offer.claimed_by = Some("zka1buyer".to_string());
        assert_eq!(
            offer.validate(),
            Err(OfferRejection::ClaimedWithoutDeadline)
        );
    }

    #[test]
    fn exact_payment_releases() {
        assert!(payment_satisfies(
            25 * MICRO_USDT_PER_USDT,
            25 * MICRO_USDT_PER_USDT
        ));
    }

    #[test]
    fn overpayment_releases() {
        // The buyer already sent more than asked; withholding the ZKA over
        // their mistake would be theft.
        assert!(payment_satisfies(
            25 * MICRO_USDT_PER_USDT,
            30 * MICRO_USDT_PER_USDT
        ));
    }

    #[test]
    fn underpayment_within_tolerance_releases() {
        let asked = 100 * MICRO_USDT_PER_USDT;
        // 0.4% short — inside the 0.5% allowance for transfer fees.
        assert!(payment_satisfies(asked, asked - (asked * 40 / 10_000)));
    }

    #[test]
    fn underpayment_beyond_tolerance_does_not_release() {
        let asked = 100 * MICRO_USDT_PER_USDT;
        // 1% short — outside the allowance.
        assert!(!payment_satisfies(asked, asked - (asked * 100 / 10_000)));
    }

    #[test]
    fn tolerance_does_not_round_away_on_small_amounts() {
        // 1 USDT at 0.5% is 5000 micro-USDT. Computing the allowance by
        // dividing first would floor it toward zero and make the tolerance
        // vanish for small offers.
        let asked = MICRO_USDT_PER_USDT;
        assert!(payment_satisfies(asked, asked - 5_000));
        assert!(!payment_satisfies(asked, asked - 5_001));
    }

    #[test]
    fn paying_nothing_never_releases() {
        assert!(!payment_satisfies(25 * MICRO_USDT_PER_USDT, 0));
    }

    #[test]
    fn a_claimed_offer_expires_once_its_window_passes() {
        let mut offer = valid_offer();
        offer.status = OfferStatus::Claimed;
        offer.claimed_by = Some("zka1buyer".to_string());
        offer.claim_expires_at = Some(1_751_002_700);

        assert!(!offer.is_expired_at(1_751_002_699));
        assert!(offer.is_expired_at(1_751_002_700));
        assert!(offer.is_expired_at(1_751_009_999));

        // Once payment was verified, a temporary release/broadcast failure
        // must never give the seller their ZKA back. The exact release keeps
        // retrying until it confirms.
        offer.status = OfferStatus::PaymentDetected;
        assert!(!offer.is_expired_at(u64::MAX));
    }

    #[test]
    fn an_open_offer_never_expires_on_the_claim_clock() {
        // Only a claim starts the clock; an unclaimed offer has no deadline to
        // trip, so it must not be swept away as expired.
        let offer = valid_offer();
        assert!(!offer.is_expired_at(u64::MAX));
    }

    #[test]
    fn terminal_states_release_their_reservation() {
        assert!(!OfferStatus::Released.holds_reservation());
        assert!(!OfferStatus::Expired.holds_reservation());
        assert!(!OfferStatus::Cancelled.holds_reservation());

        // Anything still in flight keeps the seller's ZKA locked.
        assert!(OfferStatus::Open.holds_reservation());
        assert!(OfferStatus::Claimed.holds_reservation());
        assert!(OfferStatus::PaymentDetected.holds_reservation());
    }

    #[test]
    fn only_open_offers_are_claimable() {
        assert!(OfferStatus::Open.is_claimable());
        for status in [
            OfferStatus::Claimed,
            OfferStatus::PaymentDetected,
            OfferStatus::Released,
            OfferStatus::Expired,
            OfferStatus::Cancelled,
        ] {
            assert!(!status.is_claimable(), "{status:?} must not be claimable");
        }
    }

    #[test]
    fn price_per_zka_is_reported_without_dividing_by_zero() {
        let offer = valid_offer(); // 100 ZKA for 25 USDT
        assert_eq!(offer.micro_usdt_per_zka(), Some(250_000));

        let mut zero = valid_offer();
        zero.zka_atoms = 0;
        assert_eq!(zero.micro_usdt_per_zka(), None);
    }

    #[test]
    fn price_math_does_not_overflow_on_large_offers() {
        let mut offer = valid_offer();
        offer.zka_atoms = 1_000_000 * crate::economics::ATOMS_PER_ZKA;
        offer.micro_usdt = 250_000 * MICRO_USDT_PER_USDT;
        // Would overflow u64 midway if the multiply were not widened.
        assert_eq!(offer.micro_usdt_per_zka(), Some(250_000));
    }

    #[test]
    fn offers_survive_a_json_round_trip() {
        // Offers travel as JSON over gossip, so the wire form has to preserve
        // every field the receiving node validates.
        let offer = valid_offer();
        let encoded = serde_json::to_vec(&offer).unwrap();
        let decoded: Offer = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(offer, decoded);
    }

    #[test]
    fn the_claim_window_is_45_minutes() {
        assert_eq!(CLAIM_WINDOW_SECONDS, 2_700);
    }

    #[test]
    fn offer_ids_are_unique_and_full_width() {
        let a = new_offer_id();
        let b = new_offer_id();

        assert_ne!(a, b);
        assert_eq!(a.len(), 32, "128 bits hex-encoded");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));

        // Two offers published in the same second must not collide — that was
        // exactly the failure of the seller+timestamp scheme this replaced.
        let ids: std::collections::HashSet<String> = (0..500).map(|_| new_offer_id()).collect();
        assert_eq!(ids.len(), 500);
    }

    #[test]
    fn the_signing_preimage_is_stable_for_the_same_offer() {
        let offer = valid_offer();
        assert_eq!(offer.signing_preimage(), offer.signing_preimage());
    }

    #[test]
    fn changing_anything_a_forger_would_target_changes_the_preimage() {
        let base = valid_offer().signing_preimage();

        let mut cheaper = valid_offer();
        cheaper.micro_usdt = 1_000_000;
        assert_ne!(base, cheaper.signing_preimage(), "price must be signed");

        let mut bigger = valid_offer();
        bigger.zka_atoms += 1;
        assert_ne!(base, bigger.signing_preimage(), "ZKA amount must be signed");

        // The payout address is the field an attacker most wants to swap: it
        // is where the buyer's USDT lands.
        let mut redirected = valid_offer();
        redirected.usdt_receive_address = "TXYZNMqJjW8dWvJf3zJ8xYqL2mNpR4sT6u".to_string();
        assert_ne!(
            base,
            redirected.signing_preimage(),
            "payout address must be signed"
        );

        let mut impostor = valid_offer();
        impostor.seller_address = "zka1someoneelse".to_string();
        assert_ne!(base, impostor.signing_preimage(), "seller must be signed");
    }

    #[test]
    fn lifecycle_changes_do_not_invalidate_the_signature() {
        // A claim legitimately mutates status, buyer and deadline. Covering
        // them would break the seller's signature the instant someone claimed.
        let base = valid_offer().signing_preimage();

        let mut claimed = valid_offer();
        claimed.status = OfferStatus::Claimed;
        claimed.claimed_by = Some("zka1buyer".to_string());
        claimed.claim_expires_at = Some(1_751_002_700);

        assert_eq!(base, claimed.signing_preimage());
    }

    #[test]
    fn adjacent_fields_cannot_be_confused_by_shifting_bytes() {
        // Without separators, moving a character between id and seller would
        // produce the same byte run and let two different offers share a
        // signature.
        let mut a = valid_offer();
        a.id = "ab".to_string();
        a.seller_address = "cd".to_string();

        let mut b = valid_offer();
        b.id = "a".to_string();
        b.seller_address = "bcd".to_string();

        assert_ne!(a.signing_preimage(), b.signing_preimage());
    }
}
