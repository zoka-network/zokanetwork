//! Persistent store for the offers this node published or learned about.
//!
//! Two rules drive the design.
//!
//! First, **reservations must never outlive their offer**. A reserved note is
//! unspendable, so a crash between reserving ZKA and writing the offer would
//! strand those funds with nothing left to release them. [`OfferStore::reconcile`]
//! runs at startup and frees any reservation whose offer is gone or finished.
//!
//! Second, **a claim is a one-shot**. Two buyers paying for the same ZKA is
//! unrecoverable without an arbiter, and this design has none, so claiming is
//! a compare-and-set that the second caller loses.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{
    Offer, OfferStatus, PaymentFailureReport, CLAIM_WINDOW_SECONDS, MAX_OPEN_OFFERS_PER_ADDRESS,
};

/// Why an operation on the store was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    UnknownOffer,
    /// Someone else claimed it first, or it is no longer open.
    NotClaimable,
    /// The seller already has the maximum number of open offers.
    TooManyOpenOffers,
    DuplicateOfferId,
    ConflictingOffer,
    InvalidTransition,
    SellerMismatch,
    /// The offer failed its own shape validation.
    Invalid(&'static str),
    Io(String),
}

#[derive(Default, Serialize, Deserialize)]
struct OfferFile {
    #[serde(default)]
    offers: Vec<Offer>,
    #[serde(default)]
    payment_failures: Vec<PaymentFailureReport>,
}

/// Offers held by this node, persisted to disk.
pub struct OfferStore {
    path: PathBuf,
    offers: HashMap<String, Offer>,
    payment_failures: Vec<PaymentFailureReport>,
}

impl OfferStore {
    fn transition_allowed(from: OfferStatus, to: OfferStatus) -> bool {
        from == to
            || matches!(
                (from, to),
                (
                    OfferStatus::Open,
                    OfferStatus::Claimed | OfferStatus::Cancelled
                ) | (
                    OfferStatus::Claimed,
                    OfferStatus::PaymentDetected | OfferStatus::Released | OfferStatus::Expired
                ) | (OfferStatus::PaymentDetected, OfferStatus::Released)
            )
    }

    /// Opens the store at `path`, creating it empty if absent.
    ///
    /// A corrupt file is an error rather than a silent reset: the file records
    /// which notes are reserved, so discarding it would leave those
    /// reservations with nothing to release them.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        let (offers, payment_failures) = if path.exists() {
            let raw = std::fs::read_to_string(&path).map_err(|e| StoreError::Io(e.to_string()))?;
            let parsed: OfferFile =
                serde_json::from_str(&raw).map_err(|e| StoreError::Io(e.to_string()))?;
            (
                parsed
                    .offers
                    .into_iter()
                    .map(|offer| (offer.id.clone(), offer))
                    .collect(),
                parsed.payment_failures,
            )
        } else {
            (HashMap::new(), Vec::new())
        };
        Ok(Self {
            path,
            offers,
            payment_failures,
        })
    }

    fn persist(&self) -> Result<(), StoreError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StoreError::Io(e.to_string()))?;
        }
        let file = OfferFile {
            offers: self.offers.values().cloned().collect(),
            payment_failures: self.payment_failures.clone(),
        };
        let encoded =
            serde_json::to_string_pretty(&file).map_err(|e| StoreError::Io(e.to_string()))?;
        // Write to a temporary file and rename, so a crash mid-write cannot
        // leave a half-written file that would fail to parse on restart and
        // take the reservation bookkeeping with it.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, encoded).map_err(|e| StoreError::Io(e.to_string()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| StoreError::Io(e.to_string()))?;
        Ok(())
    }

    pub fn get(&self, offer_id: &str) -> Option<&Offer> {
        self.offers.get(offer_id)
    }

    pub fn len(&self) -> usize {
        self.offers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.offers.is_empty()
    }

    /// Offers a buyer could still take.
    pub fn open_offers(&self) -> Vec<&Offer> {
        self.offers
            .values()
            .filter(|offer| offer.status.is_claimable())
            .collect()
    }

    pub fn all_offers(&self) -> Vec<&Offer> {
        self.offers.values().collect()
    }

    pub fn record_payment_failure(
        &mut self,
        report: PaymentFailureReport,
    ) -> Result<(), StoreError> {
        report.validate().map_err(StoreError::Invalid)?;
        if self.payment_failures.iter().any(|existing| {
            existing.offer_id == report.offer_id
                || existing
                    .tron_tx_hash
                    .eq_ignore_ascii_case(&report.tron_tx_hash)
        }) {
            return Ok(());
        }
        self.payment_failures.push(report);
        if let Err(error) = self.persist() {
            self.payment_failures.pop();
            return Err(error);
        }
        Ok(())
    }

    pub fn seller_failure_count(&self, seller_address: &str) -> usize {
        self.payment_failures
            .iter()
            .filter(|report| report.seller_address == seller_address)
            .count()
    }

    fn open_count_for(&self, seller_address: &str) -> usize {
        self.offers
            .values()
            .filter(|offer| offer.seller_address == seller_address && offer.status.is_claimable())
            .count()
    }

    /// Records a locally created offer.
    ///
    /// Enforces the per-address cap here rather than at publish time: the cap
    /// exists to keep one seller from flooding the topic, and every published
    /// offer passes through this method.
    pub fn insert_local(&mut self, offer: Offer) -> Result<(), StoreError> {
        offer
            .validate()
            .map_err(|r| StoreError::Invalid(r.as_str()))?;
        if self.offers.contains_key(&offer.id) {
            return Err(StoreError::DuplicateOfferId);
        }
        if offer.status.is_claimable()
            && self.open_count_for(&offer.seller_address) >= MAX_OPEN_OFFERS_PER_ADDRESS
        {
            return Err(StoreError::TooManyOpenOffers);
        }
        let offer_id = offer.id.clone();
        self.offers.insert(offer_id.clone(), offer);
        if let Err(error) = self.persist() {
            self.offers.remove(&offer_id);
            return Err(error);
        }
        Ok(())
    }

    /// Inserts a remote announcement, or treats an identical reannouncement
    /// as a heartbeat. Immutable offer terms can never be replaced.
    pub fn upsert_remote(&mut self, offer: Offer) -> Result<(), StoreError> {
        offer
            .validate()
            .map_err(|rejection| StoreError::Invalid(rejection.as_str()))?;
        if offer.status != OfferStatus::Open {
            return Err(StoreError::Invalid("remote announcement must be open"));
        }
        if let Some(existing) = self.offers.get(&offer.id) {
            return if existing.signing_preimage() == offer.signing_preimage() {
                Ok(())
            } else {
                Err(StoreError::ConflictingOffer)
            };
        }
        self.insert_local(offer)
    }

    pub fn apply_remote_update(
        &mut self,
        offer_id: &str,
        seller_address: &str,
        status: OfferStatus,
        claimed_by: Option<String>,
        claim_expires_at: Option<u64>,
    ) -> Result<(), StoreError> {
        let offer = self
            .offers
            .get_mut(offer_id)
            .ok_or(StoreError::UnknownOffer)?;
        if offer.seller_address != seller_address {
            return Err(StoreError::SellerMismatch);
        }
        if offer.status == status {
            return if offer.claimed_by == claimed_by && offer.claim_expires_at == claim_expires_at {
                Ok(())
            } else {
                Err(StoreError::ConflictingOffer)
            };
        }
        if !Self::transition_allowed(offer.status, status) {
            return Err(StoreError::InvalidTransition);
        }
        let previous = offer.clone();
        offer.status = status;
        offer.claimed_by = claimed_by;
        offer.claim_expires_at = claim_expires_at;
        if let Err(error) = self.persist() {
            self.offers.insert(offer_id.to_string(), previous);
            return Err(error);
        }
        Ok(())
    }

    /// Claims an offer for `buyer` at `now`, starting the payment window.
    ///
    /// Compare-and-set: a second caller gets [`StoreError::NotClaimable`]
    /// rather than overwriting the first buyer. Without that, two people could
    /// each pay USDT for ZKA only one of them can receive.
    pub fn claim(&mut self, offer_id: &str, buyer: &str, now: u64) -> Result<Offer, StoreError> {
        let offer = self
            .offers
            .get_mut(offer_id)
            .ok_or(StoreError::UnknownOffer)?;
        if !offer.status.is_claimable() {
            return Err(StoreError::NotClaimable);
        }
        let previous = offer.clone();
        offer.status = OfferStatus::Claimed;
        offer.claimed_by = Some(buyer.to_string());
        offer.claim_expires_at = Some(now.saturating_add(CLAIM_WINDOW_SECONDS));
        let claimed = offer.clone();
        if let Err(error) = self.persist() {
            self.offers.insert(offer_id.to_string(), previous);
            return Err(error);
        }
        Ok(claimed)
    }

    /// Devuelve al mercado una oferta que el comprador solto.
    ///
    /// Solo el que tiene el reclamo puede soltarlo, y solo mientras la oferta
    /// siga en Claimed. Sin esto, un comprador que se arrepiente deja la venta
    /// bloqueada los 45 minutos completos: nadie mas puede tomarla y el
    /// vendedor no puede cancelarla porque ya no esta abierta.
    ///
    /// La reserva del vendedor no se toca: el ZKA sigue respaldando la oferta,
    /// que vuelve a estar disponible tal como se publico.
    pub fn release_claim(&mut self, offer_id: &str, buyer: &str) -> Result<Offer, StoreError> {
        let offer = self
            .offers
            .get_mut(offer_id)
            .ok_or(StoreError::UnknownOffer)?;
        if offer.status != OfferStatus::Claimed {
            return Err(StoreError::InvalidTransition);
        }
        if offer.claimed_by.as_deref() != Some(buyer) {
            return Err(StoreError::InvalidTransition);
        }
        let previous = offer.clone();
        offer.status = OfferStatus::Open;
        offer.claimed_by = None;
        offer.claim_expires_at = None;
        let liberada = offer.clone();
        if let Err(error) = self.persist() {
            self.offers.insert(offer_id.to_string(), previous);
            return Err(error);
        }
        Ok(liberada)
    }

    /// Moves an offer to a new status.
    pub fn set_status(&mut self, offer_id: &str, status: OfferStatus) -> Result<(), StoreError> {
        let offer = self
            .offers
            .get_mut(offer_id)
            .ok_or(StoreError::UnknownOffer)?;
        if !Self::transition_allowed(offer.status, status) {
            return Err(StoreError::InvalidTransition);
        }
        let previous = offer.clone();
        offer.status = status;
        if let Err(error) = self.persist() {
            self.offers.insert(offer_id.to_string(), previous);
            return Err(error);
        }
        Ok(())
    }

    /// Note ids that still have a live offer behind them.
    pub fn actively_reserved_note_ids(&self) -> Vec<String> {
        self.offers
            .values()
            // `Released` means the private tx was built and announced, not
            // necessarily confirmed in a block. Its inputs stay locked by
            // the tx hash and must survive startup reconciliation so the
            // exact durable tx can be rebroadcast.
            .filter(|offer| {
                offer.status.holds_reservation() || offer.status == OfferStatus::Released
            })
            .flat_map(|offer| offer.reserved_note_ids.iter().cloned())
            .collect()
    }

    /// Startup reconciliation.
    ///
    /// Reports which of `wallet_reserved_note_ids` no longer belong to a live
    /// offer. A due claim deliberately stays live here: after downtime the
    /// seller must perform a final Tron query before deciding that no payment
    /// arrived and freeing the ZKA.
    pub fn reconcile(
        &mut self,
        wallet_reserved_note_ids: &[String],
        _now: u64,
    ) -> Result<ReconcileReport, StoreError> {
        let active = self.actively_reserved_note_ids();
        let orphaned = wallet_reserved_note_ids
            .iter()
            .filter(|id| !active.contains(id))
            .cloned()
            .collect();
        Ok(ReconcileReport {
            orphaned_note_ids: orphaned,
        })
    }

    /// Offers this node should re-announce after a restart.
    pub fn offers_to_reannounce(&self) -> Vec<&Offer> {
        self.open_offers()
    }
}

/// Outcome of [`OfferStore::reconcile`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Reserved notes with no live offer behind them; the caller must release
    /// these or the funds stay locked forever.
    pub orphaned_note_ids: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::economics::ATOMS_PER_ZKA;

    fn temp_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "zoka-offers-{}-{}.json",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        path
    }

    fn offer(id: &str, seller: &str) -> Offer {
        Offer {
            id: id.to_string(),
            seller_address: seller.to_string(),
            zka_atoms: 100 * ATOMS_PER_ZKA,
            micro_usdt: 25_000_000,
            usdt_receive_address: "TQ5NMqJjW8dWvJf3zJ8xYqL2mNpR4sT6uV".to_string(),
            status: OfferStatus::Open,
            created_at: 1_751_000_000,
            claim_expires_at: None,
            claimed_by: None,
            reserved_note_ids: vec![format!("note-{id}")],
            reserved_atoms: 0,
            tron_network: None,
        }
    }

    /// Sin soltar el reclamo, un comprador arrepentido dejaba la venta trabada
    /// los 45 minutos completos: nadie mas podia tomarla y el vendedor tampoco
    /// podia cancelarla, porque cancelar exige que este abierta.
    #[test]
    fn a_buyer_can_hand_the_offer_back_to_the_market() {
        let mut store = OfferStore::open(&temp_path("release")).unwrap();
        store.insert_local(offer("offer-1", "zka1vendedor")).unwrap();
        store.claim("offer-1", "zka1comprador", 1_751_000_000).unwrap();

        let liberada = store.release_claim("offer-1", "zka1comprador").unwrap();

        assert_eq!(liberada.status, OfferStatus::Open);
        assert_eq!(liberada.claimed_by, None);
        assert_eq!(liberada.claim_expires_at, None);
    }

    /// Solo quien tiene el reclamo puede soltarlo. Si cualquiera pudiera, se
    /// convertiria en una forma de sacarle la compra a otro.
    #[test]
    fn only_the_claim_holder_can_release_it() {
        let mut store = OfferStore::open(&temp_path("release-holder")).unwrap();
        store.insert_local(offer("offer-1", "zka1vendedor")).unwrap();
        store.claim("offer-1", "zka1comprador", 1_751_000_000).unwrap();

        assert!(store.release_claim("offer-1", "zka1otro").is_err());
        assert_eq!(store.get("offer-1").unwrap().status, OfferStatus::Claimed);
    }

    #[test]
    fn offers_survive_a_restart() {
        let path = temp_path("restart");
        {
            let mut store = OfferStore::open(&path).unwrap();
            store.insert_local(offer("a", "zka1seller")).unwrap();
        }
        // A fresh handle, as after a node restart.
        let store = OfferStore::open(&path).unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(store.get("a").unwrap().micro_usdt, 25_000_000);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_second_buyer_cannot_claim_the_same_offer() {
        // The failure this prevents is unrecoverable: two buyers each paying
        // USDT for ZKA that only one of them can receive, with no arbiter.
        let path = temp_path("claim-race");
        let mut store = OfferStore::open(&path).unwrap();
        store.insert_local(offer("a", "zka1seller")).unwrap();

        assert!(store.claim("a", "zka1buyer1", 1_751_000_100).is_ok());
        assert_eq!(
            store.claim("a", "zka1buyer2", 1_751_000_101),
            Err(StoreError::NotClaimable)
        );
        assert_eq!(
            store.get("a").unwrap().claimed_by.as_deref(),
            Some("zka1buyer1")
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn claiming_starts_a_45_minute_window() {
        let path = temp_path("window");
        let mut store = OfferStore::open(&path).unwrap();
        store.insert_local(offer("a", "zka1seller")).unwrap();

        let claimed = store.claim("a", "zka1buyer", 1_751_000_000).unwrap();
        assert_eq!(
            claimed.claim_expires_at,
            Some(1_751_000_000 + CLAIM_WINDOW_SECONDS)
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_per_address_open_offer_cap_is_enforced() {
        let path = temp_path("cap");
        let mut store = OfferStore::open(&path).unwrap();
        for i in 0..MAX_OPEN_OFFERS_PER_ADDRESS {
            store
                .insert_local(offer(&format!("a{i}"), "zka1seller"))
                .unwrap();
        }
        assert_eq!(
            store.insert_local(offer("over", "zka1seller")),
            Err(StoreError::TooManyOpenOffers)
        );

        // The cap is per address, so another seller is unaffected.
        assert!(store.insert_local(offer("other", "zka1other")).is_ok());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_finished_offer_frees_the_sellers_quota() {
        let path = temp_path("quota");
        let mut store = OfferStore::open(&path).unwrap();
        for i in 0..MAX_OPEN_OFFERS_PER_ADDRESS {
            store
                .insert_local(offer(&format!("a{i}"), "zka1seller"))
                .unwrap();
        }
        store.set_status("a0", OfferStatus::Cancelled).unwrap();

        assert!(store.insert_local(offer("new", "zka1seller")).is_ok());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn duplicate_offer_ids_are_refused() {
        let path = temp_path("dup");
        let mut store = OfferStore::open(&path).unwrap();
        store.insert_local(offer("a", "zka1seller")).unwrap();
        assert_eq!(
            store.insert_local(offer("a", "zka1seller")),
            Err(StoreError::DuplicateOfferId)
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn identical_remote_reannouncements_are_heartbeats_but_conflicts_are_rejected() {
        let path = temp_path("remote-heartbeat");
        let mut store = OfferStore::open(&path).unwrap();
        let original = offer("remote", "zka1remote");
        store.upsert_remote(original.clone()).unwrap();
        store.upsert_remote(original.clone()).unwrap();
        assert_eq!(store.len(), 1);

        let mut conflict = original;
        conflict.micro_usdt += 1;
        assert_eq!(
            store.upsert_remote(conflict),
            Err(StoreError::ConflictingOffer)
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn remote_status_updates_only_move_forward_and_keep_the_seller() {
        let path = temp_path("remote-update");
        let mut store = OfferStore::open(&path).unwrap();
        store.upsert_remote(offer("remote", "zka1remote")).unwrap();
        store
            .apply_remote_update(
                "remote",
                "zka1remote",
                OfferStatus::Claimed,
                Some("zka1buyer".to_string()),
                Some(1_751_002_700),
            )
            .unwrap();
        assert_eq!(store.get("remote").unwrap().status, OfferStatus::Claimed);
        assert_eq!(
            store.apply_remote_update("remote", "zka1remote", OfferStatus::Open, None, None),
            Err(StoreError::InvalidTransition)
        );
        assert_eq!(
            store.apply_remote_update(
                "remote",
                "zka1impostor",
                OfferStatus::Released,
                Some("zka1buyer".to_string()),
                Some(1_751_002_700)
            ),
            Err(StoreError::SellerMismatch)
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn invalid_offers_never_reach_the_file() {
        let path = temp_path("invalid");
        let mut store = OfferStore::open(&path).unwrap();
        let mut bad = offer("a", "zka1seller");
        bad.usdt_receive_address = "0xdeadbeef".to_string();

        assert!(matches!(
            store.insert_local(bad),
            Err(StoreError::Invalid(_))
        ));
        assert!(store.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_failed_persist_does_not_leave_an_offer_in_memory() {
        let root = std::env::temp_dir().join(format!(
            "zoka-offer-store-failure-{}-{}",
            std::process::id(),
            crate::p2p_exchange::new_offer_id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let blocked_parent = root.join("not-a-directory");
        let path = blocked_parent.join("offers.json");
        let mut store = OfferStore::open(&path).unwrap();
        std::fs::write(&blocked_parent, "blocks directory creation").unwrap();

        assert!(matches!(
            store.insert_local(offer("a", "zka1seller")),
            Err(StoreError::Io(_))
        ));
        assert!(store.is_empty());

        let _ = std::fs::remove_file(&blocked_parent);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn failed_claim_and_status_persistence_roll_back_in_memory() {
        let path = temp_path("transition-rollback");
        let mut store = OfferStore::open(&path).unwrap();
        store.insert_local(offer("claim", "zka1seller")).unwrap();
        store.insert_local(offer("cancel", "zka1seller")).unwrap();

        let blocked_parent = path.with_extension("blocked");
        std::fs::write(&blocked_parent, "not a directory").unwrap();
        store.path = blocked_parent.join("offers.json");

        assert!(matches!(
            store.claim("claim", "zka1buyer", 1_751_000_000),
            Err(StoreError::Io(_))
        ));
        assert_eq!(store.get("claim").unwrap().status, OfferStatus::Open);
        assert!(matches!(
            store.set_status("cancel", OfferStatus::Cancelled),
            Err(StoreError::Io(_))
        ));
        assert_eq!(store.get("cancel").unwrap().status, OfferStatus::Open);

        let _ = std::fs::remove_file(&blocked_parent);
        let _ = std::fs::remove_file(&path);
    }

    /// Arrancar sin abrir la wallet no puede soltar las reservas de nadie.
    ///
    /// `--sync-only` existe para que ZSilent empiece a bajar bloques mientras
    /// el usuario todavia esta en el login. En ese modo el gestor de wallets
    /// esta vacio a proposito, asi que la reconciliacion del arranque corre
    /// con una lista de notas reservadas vacia.
    ///
    /// Lo que se fija aca es que eso no toque las ofertas abiertas: las notas
    /// huerfanas se derivan de lo que la WALLET tiene reservado, no de lo que
    /// las ofertas reclaman. Si algun dia se invirtiera esa direccion, un
    /// arranque sin contrasena liberaria las reservas de todas las ofertas del
    /// usuario -- y este test lo cazaria antes de que llegue a nadie.
    #[test]
    fn a_locked_wallet_at_startup_releases_nothing() {
        let mut store = OfferStore::open(&temp_path("sync-only-reconcile")).expect("open");
        let offer = offer("oferta-viva", "zk1vendedor");
        store.insert_local(offer).expect("insert");

        // Arranque en modo solo-sincronizar: la wallet nunca se abrio.
        let sin_wallet: Vec<String> = Vec::new();
        let reporte = store.reconcile(&sin_wallet, 0).expect("reconcile");

        assert!(
            reporte.orphaned_note_ids.is_empty(),
            "una wallet cerrada no puede declarar huerfana ninguna reserva"
        );
        assert_eq!(
            store.open_offers().len(),
            1,
            "la oferta abierta sigue en pie tras un arranque sin wallet"
        );
    }

    #[test]
    fn reconcile_reports_notes_whose_offer_is_gone() {
        // The worst outcome this feature can produce: a reserved note with no
        // offer behind it is unspendable forever, and the user cannot see why
        // their balance is short.
        let path = temp_path("orphan");
        let mut store = OfferStore::open(&path).unwrap();
        store.insert_local(offer("a", "zka1seller")).unwrap();

        let wallet_reserved = vec![
            "note-a".to_string(),     // backed by the live offer
            "note-ghost".to_string(), // no offer at all
        ];

        let report = store.reconcile(&wallet_reserved, 1_751_000_100).unwrap();
        assert_eq!(report.orphaned_note_ids, vec!["note-ghost".to_string()]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reconcile_keeps_a_due_claim_locked_until_the_final_tron_check() {
        let path = temp_path("downtime");
        let mut store = OfferStore::open(&path).unwrap();
        store.insert_local(offer("a", "zka1seller")).unwrap();
        store.claim("a", "zka1buyer", 1_751_000_000).unwrap();

        // Node restarts long after the window closed.
        let report = store
            .reconcile(
                &["note-a".to_string()],
                1_751_000_000 + CLAIM_WINDOW_SECONDS + 10_000,
            )
            .unwrap();

        assert!(report.orphaned_note_ids.is_empty());
        assert_eq!(store.get("a").unwrap().status, OfferStatus::Claimed);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reconcile_keeps_notes_of_offers_still_in_flight() {
        let path = temp_path("inflight");
        let mut store = OfferStore::open(&path).unwrap();
        store.insert_local(offer("a", "zka1seller")).unwrap();
        store.claim("a", "zka1buyer", 1_751_000_000).unwrap();

        // Restart well inside the window: the trade is still live and the
        // reservation must survive.
        let report = store
            .reconcile(&["note-a".to_string()], 1_751_000_060)
            .unwrap();

        assert!(report.orphaned_note_ids.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reconcile_keeps_released_inputs_locked_until_the_tx_confirms() {
        let path = temp_path("released-pending");
        let mut store = OfferStore::open(&path).unwrap();
        let mut pending = offer("released", "zka1seller");
        pending.reserved_note_ids = vec!["note-release".to_string()];
        store.insert_local(pending).unwrap();
        store.claim("released", "zka1buyer", 1_751_000_000).unwrap();
        store
            .set_status("released", OfferStatus::PaymentDetected)
            .unwrap();
        store.set_status("released", OfferStatus::Released).unwrap();

        let report = store
            .reconcile(&["note-release".to_string()], 1_751_009_999)
            .unwrap();
        assert!(report.orphaned_note_ids.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn only_open_offers_are_reannounced_after_a_restart() {
        let path = temp_path("reannounce");
        let mut store = OfferStore::open(&path).unwrap();
        store.insert_local(offer("open", "zka1seller")).unwrap();
        store.insert_local(offer("done", "zka1seller")).unwrap();
        store.claim("done", "zka1buyer", 1_751_000_000).unwrap();
        store
            .set_status("done", OfferStatus::PaymentDetected)
            .unwrap();
        store.set_status("done", OfferStatus::Released).unwrap();

        let ids: Vec<&str> = store
            .offers_to_reannounce()
            .iter()
            .map(|o| o.id.as_str())
            .collect();
        assert_eq!(ids, vec!["open"]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_corrupt_file_is_an_error_rather_than_a_silent_reset() {
        // Discarding the file would drop the record of which notes are
        // reserved, stranding them.
        let path = temp_path("corrupt");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(matches!(OfferStore::open(&path), Err(StoreError::Io(_))));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn claiming_an_unknown_offer_is_refused() {
        let path = temp_path("unknown");
        let mut store = OfferStore::open(&path).unwrap();
        assert_eq!(
            store.claim("nope", "zka1buyer", 1_751_000_000),
            Err(StoreError::UnknownOffer)
        );
        let _ = std::fs::remove_file(&path);
    }
}
