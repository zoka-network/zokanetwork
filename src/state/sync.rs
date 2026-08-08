//! Initial block download (IBD) helpers.
//!
//! When a node boots behind the network — or skips a block in the gossip
//! stream — it needs to walk forward from its local tip to the network's
//! tip one block at a time. The plumbing lives in two places:
//!
//! - [`try_apply_synced_block_payload`]: pure, testable. Takes the bytes
//!   that arrived in a `ZokaMessage::BlockResponse`, parses them, checks
//!   the announced hash matches, and runs them through
//!   `state::chain::apply_block`. Used both for the active sync loop and
//!   for unsolicited block announcements over gossipsub.
//! - [`PeerChainView`]: a mutable observation of the highest block height
//!   any peer has revealed by publishing a block. Used to decide whether
//!   IBD is still needed.
//!
//! The async P2P round-trip (`request_block_with_retry`) lives in
//! `network::p2p` and is called from `main.rs`. This module deliberately
//! keeps the parser / state-mutating code in a non-async, easily-tested
//! function so the consensus rules can be exercised without a libp2p
//! swarm.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::{Duration, Instant};

use libp2p::PeerId;
use serde::Deserialize;
use tracing::info;

use crate::network::message::BlockResponseMessage;
use crate::private_wallet::{PrivateTransaction, PrivateWalletManager};
use crate::state::chain::{
    apply_block, apply_block_with_expected_difficulty, read_chain_head, ChainError, ChainHead,
};
use crate::storage::RocksDbStorage;
use crate::types::Block;

const MAX_PENDING_BLOCK_RESPONSES: usize = 256;
const MAX_PENDING_BLOCKS_PER_HEIGHT: usize = 4;
const MAX_PENDING_BLOCK_AHEAD: u64 = 512;
const MAX_BRANCH_BACKFILL_REQUEST_FAILURES: u8 = 4;

/// Cuantas veces tiene que fallar el pedido hacia adelante antes de aceptar
/// que el camino directo no sirve y ponerse a caminar la ancestria hacia
/// atras.
///
/// El paseo hacia atras existe para un caso real pero raro: que los pares
/// hayan podado `local_head + 1` y ya no lo tengan. La poda en Zoka es un
/// comando manual (`chain-prune`), asi que casi nadie la corre; sin embargo
/// el paseo era el camino por defecto de un nodo simplemente atrasado, que es
/// el caso comun. Pedir la altura siguiente y ver si llega distingue los dos
/// casos sin adivinar, y tres intentos alcanzan para no confundir un par
/// lento con uno que podo.
const FORWARD_GAP_FAILURES_BEFORE_BACKWARD_WALK: u8 = 3;

/// Hasta que distancia por encima de la cabeza local vale la pena caminar la
/// ancestria hacia atras.
///
/// Contar fallos no alcanza: lo que decide si el paseo sirve no es cuantas
/// veces fallo el pedido directo, sino que tan largo seria el paseo. Cuatro
/// timeouts sobre Tor son normales y alcanzaban para disparar una caminata
/// desde la punta -- cien pasos o mil, de a un bloque, mientras la cadena
/// avanza. Eso no es recuperarse: es el mismo pozo por otra puerta.
///
/// El caso que el paseo si resuelve es corto por definicion: unos pocos
/// bloques que los pares ya no tienen. Cuando el candidato esta mucho mas
/// arriba, ningun paseo va a alcanzarlo -- hace falta un snapshot.
const MAX_BACKWARD_WALK_DISTANCE: u64 = 64;

#[derive(Debug, Clone)]
struct PeerChainTip {
    height: u64,
    block_hash: Option<String>,
    cumulative_work: Option<u128>,
}

#[derive(Debug, Clone)]
struct PendingBlockResponse {
    source: Option<PeerId>,
    response: BlockResponseMessage,
}

#[derive(Debug, Clone)]
struct PerceivedHeight {
    height: u64,
    observed_at: Instant,
}

/// `observe()` records a peer-announced height (from an unvalidated
/// `BlockResponse.block_height` or `SyncState.last_height`) before the
/// payload is ever checked against consensus rules. A single malformed or
/// stale announcement can therefore plant a height neither this node nor
/// any honest peer can ever actually deliver, permanently blocking
/// `mining_pause_target` below. Expire the observation instead of trusting
/// it forever: if the chain hasn't caught up within this window, treat it
/// as unreachable and stop waiting on it. Comfortably above the ~120s
/// target block time so a genuine, merely-slow sync isn't cut short.
const PERCEIVED_HEIGHT_STALE_AFTER: Duration = Duration::from_secs(300);

/// Highest block height any peer has revealed since this node booted.
/// Treat 0 as "no peer has announced a block yet" — IBD is then a no-op.
#[derive(Debug, Default, Clone)]
pub struct PeerChainView {
    pub perceived_max_height: u64,
    perceived_max_height_observed_at: Option<Instant>,
    unattributed_perceived_height: Option<PerceivedHeight>,
    source_perceived_heights: HashMap<PeerId, PerceivedHeight>,
    pub branch_backfill_height: Option<u64>,
    pub branch_backfill_ceiling: Option<u64>,
    pub branch_expected_hashes: BTreeMap<u64, String>,
    branch_source: Option<PeerId>,
    branch_tip_hash: Option<String>,
    branch_cumulative_work: Option<u128>,
    branch_backfill_request_failures: u8,
    forward_gap_failures: u8,
    /// Si ya se mando (o se decidio no mandar) un pedido de cabeceras para
    /// el episodio de paseo actual. Evita mandar el mismo pedido en cada
    /// vuelta del bucle mientras se espera la respuesta.
    branch_headers_probe_sent: bool,
    recovery_mode: bool,
    peer_tips: HashMap<PeerId, PeerChainTip>,
    rejected_sources: HashSet<PeerId>,
    pending_block_responses: BTreeMap<u64, BTreeMap<String, PendingBlockResponse>>,
}

impl PeerChainView {
    /// Record a peer-announced height, but only if it's within plausible
    /// reach of `local_height`. A `BlockResponse.block_height` (or
    /// `SyncState.last_height`) arrives unvalidated — straight from the
    /// wire, before the payload is ever checked against consensus rules —
    /// so a single malformed or dishonest announcement could otherwise
    /// plant an unreachable height and park `mining_pause_target`/IBD on it
    /// indefinitely. `MAX_PENDING_BLOCK_AHEAD` is the same ceiling
    /// `remember_block_response` already uses to decide whether a
    /// candidate is worth caching at all, so a height this implausible was
    /// never going to be actionable anyway.
    pub fn observe(&mut self, height: u64, local_height: u64) {
        if height <= local_height.saturating_add(MAX_PENDING_BLOCK_AHEAD) {
            self.observe_unattributed_height(height);
        }
    }

    /// Source-aware form of [`Self::observe`]. Invalid consensus data can
    /// quarantine one author without leaving that author's advertised height
    /// behind as the global sync target.
    pub fn observe_from(&mut self, source: PeerId, height: u64, local_height: u64) {
        if self.rejected_sources.contains(&source)
            || height > local_height.saturating_add(MAX_PENDING_BLOCK_AHEAD)
        {
            return;
        }
        self.observe_source_height(source, height);
    }

    /// Record a sync-target height that is trusted enough to sit far ahead of
    /// the local head: an operator-configured RPC endpoint, or a directly
    /// connected peer's own declared chain head (a `SyncState` message).
    /// Unlike untrusted one-off block announces (see [`observe`]), these can
    /// legitimately be thousands of blocks ahead of a fresh local DB, so they
    /// are not clamped to `local_height + MAX_PENDING_BLOCK_AHEAD` -- without
    /// this a fresh node could never learn the real head over P2P. Block
    /// payloads are still fetched and fully validated before any state is
    /// committed, so a dishonest height only wastes bounded fetch attempts.
    pub fn observe_trusted_height(&mut self, height: u64) {
        self.observe_unattributed_height(height);
    }

    /// Record a directly-connected peer's declared tip without collapsing the
    /// source into the global perceived height. Keeping the peer association is
    /// what lets IBD continue asking one coherent branch for subsequent blocks.
    pub fn observe_trusted_height_from(&mut self, source: PeerId, height: u64) {
        if self.rejected_sources.contains(&source) {
            return;
        }
        let tip = self.peer_tips.entry(source).or_insert(PeerChainTip {
            height,
            block_hash: None,
            cumulative_work: None,
        });
        if height > tip.height {
            tip.height = height;
            tip.block_hash = None;
            // Keep an older cumulative-work observation as a conservative
            // lower bound until this peer publishes a newer BlockAnnounce.
        }
        self.observe_source_height(source, height);
    }

    /// Record a peer's block tip and advertised cumulative work. Work is only
    /// a branch-selection hint; every downloaded block still goes through full
    /// consensus validation before storage or reorg.
    pub fn observe_block_announce_from(
        &mut self,
        source: PeerId,
        height: u64,
        block_hash: &str,
        cumulative_work: Option<u128>,
        local_height: u64,
    ) {
        if self.rejected_sources.contains(&source) {
            return;
        }
        if height <= local_height.saturating_add(MAX_PENDING_BLOCK_AHEAD) {
            self.observe_source_height(source, height);
        }

        let existing = self.peer_tips.get(&source).cloned();
        let should_replace = existing.as_ref().is_none_or(|tip| {
            cumulative_work > tip.cumulative_work
                || (cumulative_work == tip.cumulative_work && height >= tip.height)
        });
        if should_replace {
            let inherited_work =
                cumulative_work.or_else(|| existing.as_ref().and_then(|tip| tip.cumulative_work));
            self.peer_tips.insert(
                source,
                PeerChainTip {
                    height,
                    block_hash: Some(block_hash.to_string()),
                    cumulative_work: inherited_work,
                },
            );
        }

        // Do not mutate an in-flight branch. Newer/higher-work observations
        // remain candidates for the next selection after completion/failure.
    }

    pub fn active_branch_source(&self) -> Option<PeerId> {
        self.branch_source
    }

    pub fn active_branch_cumulative_work(&self) -> Option<u128> {
        self.branch_cumulative_work
    }

    /// While a hash-pinned branch is being reconstructed, discard unrelated
    /// block bodies before consensus/storage work. Exact hashes may be served
    /// by any relay; an as-yet-unpinned forward height must come from the
    /// selected branch source.
    pub fn should_process_block_candidate(
        &self,
        source: Option<&PeerId>,
        height: u64,
        hash: &str,
    ) -> bool {
        if !self.has_active_branch_backfill() {
            return true;
        }
        if let Some(expected) = self.branch_expected_hashes.get(&height) {
            return expected == hash;
        }
        self.branch_backfill_height == Some(height) && self.branch_source.as_ref() == source
    }

    /// Permit checkpoint recovery to walk backward even when the candidate is
    /// below the current (quarantined) local tip. Normal sync keeps rejecting
    /// such stale side-branch traffic.
    pub fn set_recovery_mode(&mut self, enabled: bool) {
        self.recovery_mode = enabled;
    }

    pub fn recovery_mode(&self) -> bool {
        self.recovery_mode
    }

    /// Prefer a peer that has explicitly advertised enough chain and, when
    /// available, the most cumulative work. Height is only a tie-breaker.
    pub fn preferred_peer_for_height(&self, height: u64) -> Option<PeerId> {
        self.peer_tips
            .iter()
            .filter(|(peer, _)| !self.rejected_sources.contains(peer))
            .filter(|(_, tip)| tip.height >= height)
            .max_by(|(left_peer, left_tip), (right_peer, right_tip)| {
                left_tip
                    .cumulative_work
                    .cmp(&right_tip.cumulative_work)
                    .then_with(|| left_tip.height.cmp(&right_tip.height))
                    .then_with(|| left_peer.to_string().cmp(&right_peer.to_string()))
            })
            .map(|(peer, _)| *peer)
    }

    pub fn record_sync_outcome(&mut self, outcome: &SyncOutcome) {
        self.record_sync_outcome_from(None, outcome);
    }

    pub fn record_sync_outcome_from(&mut self, source: Option<PeerId>, outcome: &SyncOutcome) {
        if source
            .as_ref()
            .is_some_and(|peer| self.rejected_sources.contains(peer))
        {
            return;
        }
        // Diagnostic for an intermittent resync stall seen on follower
        // (--seed configured) nodes: a growing height gap with no error in
        // the dashboard log. Applied/AlreadyKnown are the normal steady
        // state and are noisy at this volume, so only the non-trivial
        // outcomes (a block that couldn't apply directly) are logged.
        if !matches!(
            outcome,
            SyncOutcome::Applied(_) | SyncOutcome::AlreadyKnown { .. }
        ) {
            info!("sync: non-trivial outcome {:?}", outcome);
        }
        match outcome {
            SyncOutcome::Applied(head) => {
                // El camino hacia adelante entrego: deja de ser sospechoso.
                self.clear_forward_gap_failures();
                if self.matches_or_learns_branch_hash(head.height, &head.hash, source.as_ref()) {
                    self.advance_branch_backfill(head.height);
                }
            }
            SyncOutcome::AlreadyKnown { height, hash } => {
                if self.matches_or_learns_branch_hash(*height, hash, source.as_ref()) {
                    self.advance_branch_backfill(*height);
                }
            }
            SyncOutcome::BehindOrSibling {
                candidate,
                candidate_hash,
                ..
            } => {
                if self.matches_or_learns_branch_hash(*candidate, candidate_hash, source.as_ref()) {
                    self.advance_branch_backfill(*candidate)
                }
            }
            SyncOutcome::GapAhead {
                local_head,
                candidate,
                candidate_hash,
                parent_hash,
            } => {
                // A recent, source-attributed block is enough to recover even
                // when peers have pruned local_head + 1: pin its immutable
                // parent hash and walk backward until a known ancestor is
                // found. Unsourced callers retain the conservative forward
                // IBD behaviour.
                //
                // Pero solo despues de que el camino directo demuestre que no
                // sirve. GapAhead significa "me llego un bloque mas alto que
                // mi cabeza", o sea SIEMPRE que el nodo va atrasado -- no que
                // haya una bifurcacion. Disparar el paseo aca lo convertia en
                // el camino por defecto del caso comun: un nodo atrasado 240
                // bloques recorria la ancestria de a uno desde la punta, ~50s
                // por paso sobre Tor, para descubrir un enganche que la altura
                // siguiente le habria dado en un pedido. Medido: 16 horas y
                // media sin aplicar un bloque, con la cadena local sana.
                //
                // El paseo sigue intacto para lo que fue pensado -- pares que
                // podaron local_head + 1 -- solo que ahora se llega ahi por
                // evidencia (tres pedidos hacia adelante sin respuesta) en vez
                // de por suposicion. Una bifurcacion de verdad no depende de
                // este contador: la delata MissingParent, mas abajo, que
                // arranca el paseo de inmediato.
                let paseo_corto = *candidate
                    <= local_head.saturating_add(MAX_BACKWARD_WALK_DISTANCE);
                if source.is_some() && self.forward_path_looks_unavailable() && paseo_corto {
                    self.record_branch_parent(
                        *local_head,
                        *candidate,
                        candidate_hash,
                        parent_hash,
                        source,
                    );
                }
            }
            SyncOutcome::MissingParent {
                local_head,
                candidate,
                candidate_hash,
                parent_hash,
            } => {
                let replaying_validated_side_block = self.has_active_branch_backfill()
                    && self.branch_backfill_height == Some(*candidate)
                    && self
                        .branch_expected_hashes
                        .get(candidate)
                        .is_some_and(|expected| expected == candidate_hash)
                    && candidate.checked_sub(1).is_some_and(|parent_height| {
                        self.branch_expected_hashes
                            .get(&parent_height)
                            .is_some_and(|expected| expected == parent_hash)
                    });
                if replaying_validated_side_block {
                    // The known side block was fully validated when its metadata
                    // was first stored, and its pinned parent has just been
                    // replayed. Continue forward instead of walking back over
                    // the same ancestry forever.
                    self.advance_branch_backfill(*candidate);
                } else {
                    self.record_branch_parent(
                        *local_head,
                        *candidate,
                        candidate_hash,
                        parent_hash,
                        source,
                    );
                }
            }
            SyncOutcome::BadPayload(_)
            | SyncOutcome::AnnouncedHashMismatch { .. }
            | SyncOutcome::Rejected(_) => {
                if let Some(source) = source.as_ref() {
                    self.rejected_sources.insert(*source);
                    self.peer_tips.remove(source);
                    self.source_perceived_heights.remove(source);
                    self.recompute_perceived_height();
                }
                if self.has_active_branch_backfill()
                    && self.branch_source.as_ref() == source.as_ref()
                {
                    self.abandon_branch_backfill();
                }
            }
        }
    }

    pub fn remember_block_response(
        &mut self,
        outcome: &SyncOutcome,
        response: &BlockResponseMessage,
    ) {
        self.remember_block_response_from(None, outcome, response);
    }

    pub fn remember_block_response_from(
        &mut self,
        source: Option<PeerId>,
        outcome: &SyncOutcome,
        response: &BlockResponseMessage,
    ) {
        let (local_head, candidate, candidate_hash) = match outcome {
            SyncOutcome::GapAhead {
                local_head,
                candidate,
                candidate_hash,
                ..
            } => (*local_head, *candidate, candidate_hash.as_str()),
            SyncOutcome::MissingParent {
                local_head,
                candidate,
                candidate_hash,
                ..
            } => (*local_head, *candidate, candidate_hash.as_str()),
            _ => return,
        };
        if response.block_height != candidate || response.block_hash != candidate_hash {
            return;
        }
        if candidate > local_head.saturating_add(MAX_PENDING_BLOCK_AHEAD) {
            return;
        }

        let responses_at_height = self.pending_block_responses.entry(candidate).or_default();
        if !responses_at_height.contains_key(candidate_hash)
            && responses_at_height.len() >= MAX_PENDING_BLOCKS_PER_HEIGHT
        {
            if let Some(oldest_hash) = responses_at_height.keys().next().cloned() {
                responses_at_height.remove(&oldest_hash);
            }
        }
        responses_at_height.insert(
            candidate_hash.to_string(),
            PendingBlockResponse {
                source,
                response: response.clone(),
            },
        );
        self.trim_pending_block_responses();
    }

    pub fn take_pending_block_response(
        &mut self,
        request: &SyncRequest,
    ) -> Option<BlockResponseMessage> {
        self.take_pending_sourced_block_response(request)
            .map(|(_, response)| response)
    }

    pub fn take_pending_sourced_block_response(
        &mut self,
        request: &SyncRequest,
    ) -> Option<(Option<PeerId>, BlockResponseMessage)> {
        let responses_at_height = self.pending_block_responses.get_mut(&request.height)?;
        let pending = match request.expected_hash.as_deref() {
            Some(expected_hash) => responses_at_height.remove(expected_hash),
            None => {
                let selected_hash = request
                    .preferred_peer
                    .as_ref()
                    .and_then(|preferred| {
                        responses_at_height.iter().find_map(|(hash, pending)| {
                            (pending.source.as_ref() == Some(preferred)).then(|| hash.clone())
                        })
                    })
                    .or_else(|| {
                        (!request.strict_peer_affinity)
                            .then(|| responses_at_height.keys().next().cloned())
                            .flatten()
                    });
                selected_hash.and_then(|hash| responses_at_height.remove(&hash))
            }
        };
        if responses_at_height.is_empty() {
            self.pending_block_responses.remove(&request.height);
        }
        pending.map(|pending| (pending.source, pending.response))
    }

    /// Stash a block body fetched via IBD prefetch pipelining (Fase 4:
    /// several heights requested in parallel instead of one at a time)
    /// before it's known whether it's immediately applicable. The existing
    /// `take_pending_block_response` (used by `run_sync_step`) picks these
    /// up in the same order it would have fetched them one at a time, so
    /// the apply path itself does not change. `local_head` is the chain
    /// height observed when the batch of requests was fired.
    pub fn stash_prefetched_block_response(
        &mut self,
        local_head: u64,
        response: BlockResponseMessage,
    ) {
        self.stash_prefetched_block_response_from(None, local_head, response);
    }

    /// Source-aware form of [`Self::stash_prefetched_block_response`]. The
    /// parallel IBD path must not erase responder identity, otherwise a
    /// missing-parent result can start an unpinned branch and mix subsequent
    /// heights from unrelated forks.
    pub fn stash_prefetched_block_response_from(
        &mut self,
        source: Option<PeerId>,
        local_head: u64,
        response: BlockResponseMessage,
    ) {
        let candidate = response.block_height;
        if candidate <= local_head || candidate > local_head.saturating_add(MAX_PENDING_BLOCK_AHEAD)
        {
            return;
        }
        let candidate_hash = response.block_hash.clone();
        let responses_at_height = self.pending_block_responses.entry(candidate).or_default();
        if !responses_at_height.contains_key(&candidate_hash)
            && responses_at_height.len() >= MAX_PENDING_BLOCKS_PER_HEIGHT
        {
            if let Some(oldest_hash) = responses_at_height.keys().next().cloned() {
                responses_at_height.remove(&oldest_hash);
            }
        }
        responses_at_height.insert(candidate_hash, PendingBlockResponse { source, response });
        self.trim_pending_block_responses();
    }

    /// Peek (without removing) whether a response is already cached for
    /// `height`, so a prefetch step can skip heights it already fetched.
    pub fn has_pending_block_response(&self, height: u64) -> bool {
        self.pending_block_responses
            .get(&height)
            .is_some_and(|m| !m.is_empty())
    }

    fn pending_block_response_count(&self) -> usize {
        self.pending_block_responses
            .values()
            .map(BTreeMap::len)
            .sum()
    }

    pub fn abandon_branch_backfill(&mut self) -> bool {
        let was_active = self.branch_backfill_ceiling.is_some()
            || self.branch_backfill_height.is_some()
            || !self.branch_expected_hashes.is_empty();
        self.branch_backfill_height = None;
        self.branch_backfill_ceiling = None;
        self.branch_expected_hashes.clear();
        self.branch_source = None;
        self.branch_tip_hash = None;
        self.branch_cumulative_work = None;
        self.branch_backfill_request_failures = 0;
        self.branch_headers_probe_sent = false;
        was_active
    }

    pub fn has_active_branch_backfill(&self) -> bool {
        self.branch_backfill_ceiling.is_some()
            || self.branch_backfill_height.is_some()
            || !self.branch_expected_hashes.is_empty()
    }

    /// Si conviene mandar un `GetHeaders` para el episodio de paseo actual.
    ///
    /// El paseo hoy descubre el ancestro comun de a un bloque: cada bloque
    /// completo que baja solo sirve para revelar el hash del padre, que es
    /// el dato que hace falta para pedir el siguiente. Es la parte mas lenta
    /// de recuperarse de una bifurcacion -- un round-trip completo de body
    /// por altura, sobre Tor, con la tasa de fallo que tenga la red ese dia.
    ///
    /// Un pedido de cabeceras devuelve hasta 512 alturas en una sola vuelta,
    /// livianas (el limite de la respuesta es 8 MB, no por body). Con eso se
    /// puede saltar directo al ancestro comun en vez de caminar hacia el.
    pub fn needs_header_probe(&self) -> bool {
        self.has_active_branch_backfill() && !self.branch_headers_probe_sent
    }

    /// Marca que ya se mando (o se decidio no mandar) el pedido de cabeceras
    /// para este episodio, para no repetirlo en cada vuelta del bucle
    /// mientras se espera la respuesta.
    pub fn mark_header_probe_sent(&mut self) {
        self.branch_headers_probe_sent = true;
    }

    /// Aplica una `HeadersResponse::Headers` recibida mientras hay un paseo
    /// activo.
    ///
    /// `common_ancestor_height` es la altura local (ya conocida por este
    /// nodo, porque el locator que la origino salio de su propio storage) del
    /// hash que el par reconocio. `local_cumulative_work` es el trabajo
    /// acumulado de la cabeza local -- o de la rama en curso si ya se
    /// conocia una -- contra el que se mide si vale la pena seguir.
    ///
    /// Si la respuesta no es mejor que lo que ya se tiene, abandona el paseo
    /// ahi mismo: ni un body se descarga para una rama que de todas formas
    /// iba a perder la comparacion de trabajo al final. Si es mejor, deja
    /// `branch_expected_hashes` con el hash de cada altura del rango entero,
    /// asi el paso de descubrimiento (antes: un body por altura) se resuelve
    /// en una sola vuelta.
    pub fn apply_header_chain(
        &mut self,
        common_ancestor_height: u64,
        common_ancestor_hash: &str,
        headers: &[crate::network::header_request_response::HeaderSummary],
        local_cumulative_work: u128,
    ) -> HeaderChainOutcome {
        if !self.has_active_branch_backfill() {
            return HeaderChainOutcome::NotActive;
        }
        if headers.is_empty() {
            return HeaderChainOutcome::Invalid("el par no devolvio cabeceras".to_string());
        }
        if let Err(reason) = validate_header_chain(headers, common_ancestor_hash) {
            return HeaderChainOutcome::Invalid(reason);
        }
        let first_height = headers[0].header.block_height;
        if first_height != common_ancestor_height.saturating_add(1) {
            return HeaderChainOutcome::Invalid(format!(
                "el primer header ({first_height}) no arranca justo despues \
                 del ancestro comun ({common_ancestor_height})"
            ));
        }

        let chain_work = headers
            .last()
            .map(|summary| summary.cumulative_work)
            .unwrap_or(0);
        if chain_work <= local_cumulative_work {
            // Ni un body vale la pena para una rama que ya se sabe que
            // pierde la comparacion de trabajo. Esto es lo que antes costaba
            // horas de caminata para descubrir recien al final.
            self.abandon_branch_backfill();
            return HeaderChainOutcome::NotHeavier;
        }

        for summary in headers {
            self.branch_expected_hashes
                .insert(summary.header.block_height, summary.header.hash.clone());
        }
        self.branch_expected_hashes
            .insert(common_ancestor_height, common_ancestor_hash.to_string());

        let through_height = headers
            .last()
            .map(|summary| summary.header.block_height)
            .unwrap_or(common_ancestor_height);
        self.branch_backfill_ceiling = Some(
            self.branch_backfill_ceiling
                .map(|ceiling| ceiling.max(through_height))
                .unwrap_or(through_height),
        );
        self.branch_backfill_height = Some(common_ancestor_height.saturating_add(1));
        self.branch_backfill_request_failures = 0;

        HeaderChainOutcome::Adopted {
            from_height: common_ancestor_height.saturating_add(1),
            through_height,
        }
    }

    pub fn mining_pause_target(&self, local_height: u64) -> Option<u64> {
        if self.has_active_branch_backfill() {
            return self.branch_backfill_ceiling.or(self.branch_backfill_height);
        }
        if self.perceived_max_height > local_height {
            let stale = self
                .perceived_max_height_observed_at
                .is_some_and(|seen_at| seen_at.elapsed() >= PERCEIVED_HEIGHT_STALE_AFTER);
            if !stale {
                return Some(self.perceived_max_height);
            }
        }
        None
    }

    /// Drop a perceived height neither this node nor any peer has been able
    /// to actually deliver within [`PERCEIVED_HEIGHT_STALE_AFTER`], so a
    /// single bad announcement can't pause mining indefinitely. Call this
    /// once mining resumes after `mining_pause_target` reports the height
    /// as stale, so the next bona fide announcement starts a fresh window.
    pub fn clear_stale_perceived_height(&mut self, local_height: u64) {
        let stale = self
            .perceived_max_height_observed_at
            .is_some_and(|seen_at| seen_at.elapsed() >= PERCEIVED_HEIGHT_STALE_AFTER);
        if stale && self.perceived_max_height > local_height {
            self.unattributed_perceived_height = None;
            self.source_perceived_heights.clear();
            self.perceived_max_height = local_height;
            self.perceived_max_height_observed_at = None;
        }
    }

    fn observe_unattributed_height(&mut self, height: u64) {
        let should_replace = self
            .unattributed_perceived_height
            .as_ref()
            .is_none_or(|observed| height > observed.height);
        if should_replace {
            self.unattributed_perceived_height = Some(PerceivedHeight {
                height,
                observed_at: Instant::now(),
            });
            self.recompute_perceived_height();
        }
    }

    fn observe_source_height(&mut self, source: PeerId, height: u64) {
        if self.rejected_sources.contains(&source) {
            return;
        }
        let should_replace = self
            .source_perceived_heights
            .get(&source)
            .is_none_or(|observed| height > observed.height);
        if should_replace {
            self.source_perceived_heights.insert(
                source,
                PerceivedHeight {
                    height,
                    observed_at: Instant::now(),
                },
            );
            self.recompute_perceived_height();
        }
    }

    fn recompute_perceived_height(&mut self) {
        let best = self
            .unattributed_perceived_height
            .iter()
            .chain(self.source_perceived_heights.values())
            .max_by_key(|observed| observed.height);
        match best {
            Some(observed) => {
                self.perceived_max_height = observed.height;
                self.perceived_max_height_observed_at = Some(observed.observed_at);
            }
            None => {
                self.perceived_max_height = 0;
                self.perceived_max_height_observed_at = None;
            }
        }
    }

    /// Un pedido hacia adelante (altura siguiente, sin hash fijado) que no
    /// llego. Si se repite, es la senal de que los pares podaron ese bloque y
    /// hay que buscar la ancestria hacia atras.
    pub fn record_forward_gap_failure(&mut self) {
        self.forward_gap_failures = self.forward_gap_failures.saturating_add(1);
    }

    /// Si el camino hacia adelante entrego algo, deja de ser sospechoso.
    pub fn clear_forward_gap_failures(&mut self) {
        self.forward_gap_failures = 0;
    }

    /// Si conviene dejar de insistir hacia adelante y caminar hacia atras.
    pub fn forward_path_looks_unavailable(&self) -> bool {
        self.forward_gap_failures >= FORWARD_GAP_FAILURES_BEFORE_BACKWARD_WALK
    }

    pub fn record_branch_backfill_request_failure(&mut self) -> bool {
        if !self.has_active_branch_backfill() {
            return false;
        }
        self.branch_backfill_request_failures =
            self.branch_backfill_request_failures.saturating_add(1);
        if self.branch_backfill_request_failures >= MAX_BRANCH_BACKFILL_REQUEST_FAILURES {
            self.abandon_branch_backfill()
        } else {
            false
        }
    }

    fn matches_or_learns_branch_hash(
        &mut self,
        height: u64,
        hash: &str,
        source: Option<&PeerId>,
    ) -> bool {
        if self.branch_backfill_ceiling.is_none() {
            return true;
        }
        if let Some(expected) = self.branch_expected_hashes.get(&height) {
            return expected == hash;
        }
        if self.branch_backfill_height == Some(height) && self.branch_source.as_ref() == source {
            self.branch_expected_hashes.insert(height, hash.to_string());
            return true;
        }
        false
    }

    fn record_branch_parent(
        &mut self,
        local_head: u64,
        candidate: u64,
        candidate_hash: &str,
        parent_hash: &str,
        source: Option<PeerId>,
    ) {
        if candidate <= local_head && !self.has_active_branch_backfill() && !self.recovery_mode {
            return;
        }

        if self.has_active_branch_backfill() {
            let expected_matches = self
                .branch_expected_hashes
                .get(&candidate)
                .is_some_and(|expected| expected == candidate_hash);
            let same_source = self.branch_source.as_ref() == source.as_ref();
            if !expected_matches && !same_source {
                // An active branch is immutable. Higher advertised work can
                // influence the next selection only after this branch
                // completes or exhausts its bounded retries.
                return;
            } else if self
                .branch_expected_hashes
                .get(&candidate)
                .is_some_and(|expected| expected != candidate_hash)
                || (!expected_matches && self.branch_backfill_height != Some(candidate))
            {
                return;
            }
        }

        let _ = self.backfill_before(candidate, candidate_hash, parent_hash, source);
    }

    fn backfill_before(
        &mut self,
        candidate_height: u64,
        candidate_hash: &str,
        parent_hash: &str,
        source: Option<PeerId>,
    ) -> bool {
        if candidate_height <= 1 {
            return false;
        }
        let next_height = candidate_height.saturating_sub(1);
        if self
            .branch_expected_hashes
            .get(&candidate_height)
            .is_some_and(|expected| expected != candidate_hash)
            || self
                .branch_expected_hashes
                .get(&next_height)
                .is_some_and(|expected| expected != parent_hash)
        {
            return false;
        }

        let starting_branch = self.branch_backfill_ceiling.is_none();
        if starting_branch {
            // Un episodio nuevo merece su propio intento de cabeceras: la
            // ultima vez que se pidieron pudo haber sido para una rama que ya
            // se abandono.
            self.branch_headers_probe_sent = false;
            self.branch_source = source;
            if let Some(source) = self.branch_source.as_ref() {
                if let Some(tip) = self.peer_tips.get(source) {
                    self.branch_tip_hash = tip.block_hash.clone();
                    self.branch_cumulative_work = tip.cumulative_work;
                }
            }
        }
        let source_ceiling = self
            .branch_source
            .as_ref()
            .and_then(|source| self.peer_tips.get(source))
            .map(|tip| tip.height)
            .unwrap_or(candidate_height);
        let ceiling = self
            .branch_backfill_ceiling
            .unwrap_or(candidate_height)
            .max(candidate_height)
            .max(source_ceiling);
        self.branch_backfill_ceiling = Some(ceiling);
        self.branch_backfill_height = Some(match self.branch_backfill_height {
            Some(current) => current.min(next_height),
            None => next_height,
        });
        self.branch_expected_hashes
            .insert(candidate_height, candidate_hash.to_string());
        self.branch_expected_hashes
            .insert(next_height, parent_hash.to_string());
        self.branch_backfill_request_failures = 0;
        true
    }

    fn advance_branch_backfill(&mut self, applied_or_known_height: u64) {
        let Some(ceiling) = self.branch_backfill_ceiling else {
            return;
        };
        self.branch_backfill_request_failures = 0;
        if applied_or_known_height < ceiling {
            self.branch_backfill_height = Some(applied_or_known_height.saturating_add(1));
        } else {
            self.abandon_branch_backfill();
        }
    }

    fn trim_pending_block_responses(&mut self) {
        while self.pending_block_response_count() > MAX_PENDING_BLOCK_RESPONSES {
            let Some(height) = self.pending_block_responses.keys().next_back().cloned() else {
                return;
            };
            let remove_height = {
                let Some(responses_at_height) = self.pending_block_responses.get_mut(&height)
                else {
                    return;
                };
                if let Some(hash) = responses_at_height.keys().next_back().cloned() {
                    responses_at_height.remove(&hash);
                }
                responses_at_height.is_empty()
            };
            if remove_height {
                self.pending_block_responses.remove(&height);
            }
        }
    }
}

/// Resultado de darle una `HeadersResponse` a
/// [`PeerChainView::apply_header_chain`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderChainOutcome {
    /// No hay paseo activo ahora mismo; la respuesta llego tarde o para un
    /// episodio que ya se abandono.
    NotActive,
    /// La cadena de cabeceras no encaja estructuralmente (no contigua, no
    /// arranca donde deberia, o el trabajo no es creciente).
    Invalid(String),
    /// El par no tiene mas trabajo que la cabeza local: no vale la pena
    /// descargar ni un body.
    NotHeavier,
    /// Se adopto el rango completo; `branch_expected_hashes` ya tiene el
    /// hash de cada altura entre `from_height` y `through_height`.
    Adopted { from_height: u64, through_height: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRequest {
    pub height: u64,
    pub expected_hash: Option<String>,
    pub preferred_peer: Option<PeerId>,
    pub strict_peer_affinity: bool,
}

/// Outcome of feeding a `BlockResponse` payload into the chain. Each
/// variant maps to one human-readable verdict for the dashboard / logs;
/// callers should not rely on the wording but on the variant.
#[derive(Debug, Clone)]
pub enum SyncOutcome {
    /// Block was applied; the chain head moved.
    Applied(ChainHead),
    /// Block was already on this chain; the message was a duplicate.
    AlreadyKnown { height: u64, hash: String },
    /// Block is at a height beyond `local_head + 1`. The local node has
    /// to fetch missing intermediate blocks before this one will fit.
    GapAhead {
        local_head: u64,
        candidate: u64,
        candidate_hash: String,
        parent_hash: String,
    },
    /// Block is at a height already covered by the local chain and was
    /// not heavy enough to replace the current canonical head.
    BehindOrSibling {
        local_head: u64,
        candidate: u64,
        candidate_hash: String,
    },
    /// The block belongs to a side branch whose parent is not in local
    /// storage yet. IBD should walk backward until it finds a known
    /// ancestor, then replay forward.
    MissingParent {
        local_head: u64,
        candidate: u64,
        candidate_hash: String,
        parent_hash: String,
    },
    /// Decoding the payload failed. The peer sent garbage.
    BadPayload(String),
    /// Hash announced in the response does not match the header. The
    /// peer either tampered with the announcement or with the body.
    AnnouncedHashMismatch { announced: String, header: String },
    /// `apply_block` rejected the block. Includes the underlying reason
    /// (invalid PoW, wrong coinbase, etc.).
    Rejected(String),
}

/// Attempt to apply the body of a `BlockResponseMessage` to the chain.
/// Pure aside from reading and writing `storage`; no networking.
pub fn try_apply_synced_block_payload(
    storage: &RocksDbStorage,
    chain_id: &str,
    response: &BlockResponseMessage,
) -> SyncOutcome {
    try_apply_synced_block_payload_inner(storage, chain_id, response, None)
}

/// Same as [`try_apply_synced_block_payload`], but drives validation at an
/// explicit difficulty for unsafe local rehearsals.
pub fn try_apply_synced_block_payload_with_expected_difficulty(
    storage: &RocksDbStorage,
    chain_id: &str,
    response: &BlockResponseMessage,
    expected_difficulty_bits: u32,
) -> SyncOutcome {
    try_apply_synced_block_payload_inner(
        storage,
        chain_id,
        response,
        Some(expected_difficulty_bits),
    )
}

fn try_apply_synced_block_payload_inner(
    storage: &RocksDbStorage,
    chain_id: &str,
    response: &BlockResponseMessage,
    expected_difficulty_bits: Option<u32>,
) -> SyncOutcome {
    let block: Block = match serde_json::from_slice(&response.encrypted_block) {
        Ok(b) => b,
        Err(err) => return SyncOutcome::BadPayload(err.to_string()),
    };

    if block.header.hash != response.block_hash {
        return SyncOutcome::AnnouncedHashMismatch {
            announced: response.block_hash.clone(),
            header: block.header.hash,
        };
    }

    let local_head = match read_chain_head(storage) {
        Ok(h) => h,
        Err(err) => return SyncOutcome::Rejected(format!("read chain head failed: {err}")),
    };
    let local_height = local_head.as_ref().map(|h| h.height).unwrap_or(0);

    let candidate_height = block.header.block_height;
    let candidate_known = storage.block_exists(&block.header.hash).unwrap_or(false);
    let candidate_parent_known = storage
        .block_exists(&block.header.prev_hash)
        .unwrap_or(false);
    if candidate_height > local_height.saturating_add(1)
        && !candidate_known
        && !candidate_parent_known
    {
        return SyncOutcome::GapAhead {
            local_head: local_height,
            candidate: candidate_height,
            candidate_hash: block.header.hash,
            parent_hash: block.header.prev_hash,
        };
    }
    let resolved_expected_difficulty_bits = expected_difficulty_bits.or(response.difficulty_bits);
    let applied = match resolved_expected_difficulty_bits {
        Some(bits) => apply_block_with_expected_difficulty(storage, chain_id, &block, bits),
        None => apply_block(storage, chain_id, &block),
    };
    match applied {
        Ok(head) => {
            persist_available_private_tx_bodies(storage, &block, &response.private_tx_bodies);
            SyncOutcome::Applied(head)
        }
        Err(ChainError::BlockAlreadyKnown(hash)) => {
            // Persist bodies even when block is already known — this lets a
            // node that synced the block header earlier (without bodies)
            // backfill bodies the next time the same block re-circulates.
            persist_available_private_tx_bodies(storage, &block, &response.private_tx_bodies);
            match storage.get_canonical_block_hash_at_height(candidate_height) {
                Ok(Some(canonical_hash)) if canonical_hash == hash => SyncOutcome::AlreadyKnown {
                    height: candidate_height,
                    hash,
                },
                Ok(_) => SyncOutcome::MissingParent {
                    local_head: local_height,
                    candidate: candidate_height,
                    candidate_hash: block.header.hash,
                    parent_hash: block.header.prev_hash,
                },
                Err(err) => SyncOutcome::Rejected(format!(
                    "read canonical hash at height {candidate_height} failed: {err:?}"
                )),
            }
        }
        Err(ChainError::SideBranchNotHeavier { .. }) => SyncOutcome::BehindOrSibling {
            local_head: local_height,
            candidate: candidate_height,
            candidate_hash: block.header.hash,
        },
        Err(ChainError::DoesNotExtendHead { .. }) => SyncOutcome::MissingParent {
            local_head: local_height,
            candidate: candidate_height,
            candidate_hash: block.header.hash,
            parent_hash: block.header.prev_hash,
        },
        Err(err) => SyncOutcome::Rejected(err.to_string()),
    }
}

pub fn next_required_block(
    storage: &RocksDbStorage,
    peer_view: &PeerChainView,
) -> Result<Option<SyncRequest>, String> {
    if let Some(height) = peer_view.branch_backfill_height {
        if height > 0 {
            let expected_hash = peer_view.branch_expected_hashes.get(&height).cloned();
            return Ok(Some(SyncRequest {
                height,
                // The hash already makes a backward request branch-immutable.
                // A gossipsub source may only be the relay, not the node that
                // stores the parent, so let any connected peer serve an exact
                // hash. Forward discovery without a known hash remains pinned
                // to the branch source.
                strict_peer_affinity: peer_view.active_branch_source().is_some()
                    && expected_hash.is_none(),
                expected_hash,
                preferred_peer: peer_view.active_branch_source(),
            }));
        }
    }

    // A quarantined node may already be hundreds of blocks above the recovery
    // checkpoint on an incompatible fork. Asking for local_height + 1 can
    // merely extend that bad fork forever. Bootstrap recovery by requesting
    // the compiled checkpoint by exact hash; once received, MissingParent
    // walks its ancestry backward with strict peer/hash affinity.
    if peer_view.recovery_mode() {
        let height = crate::economics::MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT;
        return Ok(Some(SyncRequest {
            height,
            expected_hash: Some(
                crate::economics::MAINNET_RECOVERY_CHECKPOINT_10887_HASH.to_string(),
            ),
            preferred_peer: peer_view.preferred_peer_for_height(height),
            strict_peer_affinity: false,
        }));
    }

    let local_height = storage
        .get_chain_height()
        .map_err(|e| format!("read chain height failed: {e:?}"))?;
    if local_height >= peer_view.perceived_max_height {
        return Ok(None);
    }
    let next_height = local_height.saturating_add(1);
    let preferred_peer = peer_view.preferred_peer_for_height(next_height);
    Ok(Some(SyncRequest {
        height: next_height,
        expected_hash: None,
        // Preferir al par que anuncio mas trabajo, pero no atarse a el.
        //
        // Esto era afinidad estricta: si el par preferido existia,
        // select_block_request_peers_with_affinity devolvia SOLO ese par,
        // sin importar cuantos otros estuvieran conectados y sanos. Medido
        // en la PC: un par preferido que tardaba en contestar dejaba al nodo
        // reintentandole a el solo, minutos seguidos, con 6-7 pares mas
        // esperando al lado sin que se les preguntara nada.
        //
        // A diferencia del paseo por bifurcacion -- donde la afinidad evita
        // mezclar ramas -- aca no hay ninguna altura con hash fijado
        // todavia (`expected_hash: None`); la respuesta se valida contra las
        // reglas de cadena al aplicarla, sin importar que par la trajo. La
        // preferencia por el par mas al dia sigue existiendo (queda primero
        // en la lista), solo que ahora es un orden, no un candado.
        strict_peer_affinity: false,
        preferred_peer,
    }))
}

/// Convenience: returns the next block height the local chain needs in
/// order to advance, or `None` when it is already caught up to or ahead
/// of `peer_view.perceived_max_height`.
pub fn next_required_height(
    storage: &RocksDbStorage,
    peer_view: &PeerChainView,
) -> Result<Option<u64>, String> {
    Ok(next_required_block(storage, peer_view)?.map(|request| request.height))
}

// ── Headers-first IBD helpers ─────────────────────────────────────────────

/// Builds a chain locator for the `GetHeaders` request.
///
/// Returns a list of block hashes the local node already knows, ordered from
/// the current tip backward with exponentially increasing gaps:
///   [tip, tip-1, tip-2, tip-4, tip-8, …, genesis/checkpoint]
///
/// The responder finds the first hash it recognises on its canonical chain and
/// returns up to `max` headers starting from that point forward.
pub fn build_chain_locator(storage: &RocksDbStorage) -> Vec<String> {
    let tip = match storage.get_chain_height() {
        Ok(h) => h,
        Err(_) => return Vec::new(),
    };
    if tip == 0 {
        return storage
            .get_last_block_hash()
            .ok()
            .filter(|h| !h.is_empty() && h != "0")
            .into_iter()
            .collect();
    }

    let mut heights: Vec<u64> = Vec::new();
    let mut step = 1u64;
    let mut h = tip;
    loop {
        heights.push(h);
        if h == 0 {
            break;
        }
        h = h.saturating_sub(step);
        step = step.saturating_mul(2);
        if heights.len() >= 32 {
            // Always include genesis
            if *heights.last().unwrap() > 0 {
                heights.push(0);
            }
            break;
        }
    }

    heights
        .into_iter()
        .filter_map(|height| {
            storage
                .get_block_by_height(height)
                .ok()
                .flatten()
                .map(|block| block.header.hash)
        })
        .collect()
}

/// Validates a chain of `HeaderSummary` objects received from a peer.
///
/// Checks:
/// 1. The chain is contiguous (each header's `prev_hash` matches the previous
///    header's `hash`).
/// 2. Heights are strictly increasing by 1.
/// 3. `cumulative_work` is strictly increasing.
///
/// This does **not** verify the PoW seal (that requires the RandomX hash
/// function and is done at apply time). It is a cheap structural guard
/// against obvious garbage.
pub fn validate_header_chain(
    headers: &[crate::network::header_request_response::HeaderSummary],
    common_ancestor_hash: &str,
) -> Result<(), String> {
    if headers.is_empty() {
        return Ok(());
    }
    let first = &headers[0];
    if !common_ancestor_hash.is_empty()
        && first.header.prev_hash != common_ancestor_hash
        && first.header.hash != common_ancestor_hash
    {
        return Err(format!(
            "first header prev_hash ({}) does not connect to common ancestor ({})",
            first.header.prev_hash, common_ancestor_hash
        ));
    }
    for window in headers.windows(2) {
        let (prev, next) = (&window[0], &window[1]);
        if next.header.prev_hash != prev.header.hash {
            return Err(format!(
                "broken chain linkage at height {}: prev_hash={} expected={}",
                next.header.block_height, next.header.prev_hash, prev.header.hash
            ));
        }
        if next.header.block_height != prev.header.block_height + 1 {
            return Err(format!(
                "non-contiguous heights: {} → {}",
                prev.header.block_height, next.header.block_height
            ));
        }
        if next.cumulative_work <= prev.cumulative_work {
            return Err(format!(
                "cumulative_work did not increase at height {} ({} → {})",
                next.header.block_height, prev.cumulative_work, next.cumulative_work
            ));
        }
    }
    Ok(())
}

/// Schema-tolerant deserializer for incoming peer block payloads. Used
/// by tests and the sync helper alike.
#[allow(dead_code)]
pub(crate) fn parse_block_payload(bytes: &[u8]) -> Result<Block, String> {
    let mut de = serde_json::Deserializer::from_slice(bytes);
    Block::deserialize(&mut de).map_err(|e| e.to_string())
}

fn strict_private_tx_spend_tags(tx: &PrivateTransaction) -> Result<Vec<String>, String> {
    let primary = tx.nullifier.trim();
    if primary.is_empty() || primary != tx.nullifier.as_str() {
        return Err("private transaction has a missing or non-canonical primary nullifier".into());
    }
    if tx.input_nullifiers.is_empty() {
        return Ok(vec![primary.to_string()]);
    }

    let mut tags = Vec::with_capacity(tx.input_nullifiers.len());
    for tag in &tx.input_nullifiers {
        let trimmed = tag.trim();
        if trimmed.is_empty() || trimmed != tag {
            return Err(
                "private transaction has a missing or non-canonical input nullifier".into(),
            );
        }
        tags.push(trimmed.to_string());
    }
    if tags.first().map(String::as_str) != Some(primary) {
        return Err("private transaction primary nullifier does not match its first input".into());
    }
    Ok(tags)
}

/// Strict private bundle validation for the future height-gated block format.
///
/// Current blocks commit hashes and nullifiers but only carry bodies as
/// best-effort attachments. The height-gated policy calls this before applying
/// any future block whose bodies are canonical data.
#[allow(dead_code)]
pub(crate) fn validate_private_tx_bundle_for_block(
    block: &Block,
    bodies: &[Vec<u8>],
) -> Result<(), String> {
    validate_private_tx_bundle_for_block_at(
        block,
        bodies,
        crate::economics::private_tx_v2_i1_activation_height(),
        crate::economics::private_tx_canonical_bundle_activation_height(),
    )
}

fn validate_private_tx_bundle_for_block_at(
    block: &Block,
    bodies: &[Vec<u8>],
    v2_i1_activation_height: Option<u64>,
    canonical_bundle_activation_height: Option<u64>,
) -> Result<(), String> {
    let mut committed_hashes = HashSet::new();
    for hash in &block.private_tx_hashes {
        let trimmed = hash.trim();
        if trimmed.is_empty() || trimmed != hash {
            return Err("block has a missing or non-canonical private transaction hash".into());
        }
        if !committed_hashes.insert(trimmed.to_string()) {
            return Err(format!("block repeats private transaction hash {trimmed}"));
        }
    }
    if bodies.len() != committed_hashes.len() {
        return Err(format!(
            "private transaction body count mismatch: committed {}, received {}",
            committed_hashes.len(),
            bodies.len()
        ));
    }

    let mut committed_nullifiers = BTreeSet::new();
    for nullifier in &block.private_nullifiers {
        let trimmed = nullifier.trim();
        if trimmed.is_empty() || trimmed != nullifier {
            return Err("block has a missing or non-canonical private nullifier".into());
        }
        if !committed_nullifiers.insert(trimmed.to_string()) {
            return Err(format!("block repeats private nullifier {trimmed}"));
        }
    }

    let mut represented_hashes = HashSet::new();
    let mut derived_nullifiers = BTreeSet::new();
    let mut validator = PrivateWalletManager::new();
    for (expected_hash, body_bytes) in block.private_tx_hashes.iter().zip(bodies) {
        let tx: PrivateTransaction = serde_json::from_slice(body_bytes)
            .map_err(|err| format!("private transaction body deserialize failed: {err}"))?;
        let hash = tx.tx_hash.trim();
        if hash.is_empty() || hash != tx.tx_hash {
            return Err("private transaction body has a missing or non-canonical hash".into());
        }
        if hash != expected_hash {
            return Err(format!(
                "private transaction body hash {hash} does not match committed position {expected_hash}"
            ));
        }
        if !represented_hashes.insert(hash.to_string()) {
            return Err(format!(
                "private transaction hash {hash} has more than one body"
            ));
        }

        validator
            .validate_inbound_private_transaction_at(
                &tx,
                block.header.block_height,
                v2_i1_activation_height,
                canonical_bundle_activation_height,
            )
            .map_err(|err| format!("private transaction {hash} failed validation: {err}"))?;
        for nullifier in strict_private_tx_spend_tags(&tx)? {
            if !derived_nullifiers.insert(nullifier.clone()) {
                return Err(format!(
                    "private transaction bundle repeats nullifier {nullifier}"
                ));
            }
        }
    }

    if represented_hashes != committed_hashes {
        return Err("not every committed private transaction hash has exactly one body".into());
    }
    if derived_nullifiers != committed_nullifiers {
        return Err(format!(
            "private nullifier set mismatch: derived {}, committed {}",
            derived_nullifiers.len(),
            committed_nullifiers.len()
        ));
    }
    Ok(())
}

pub(crate) fn validate_private_tx_bundle_policy_for_block(block: &Block) -> Result<(), String> {
    validate_private_tx_bundle_policy_for_block_at(
        block,
        crate::economics::private_tx_canonical_bundle_activation_height(),
    )
}

fn validate_private_tx_bundle_policy_for_block_at(
    block: &Block,
    activation_height: Option<u64>,
) -> Result<(), String> {
    if crate::economics::private_tx_canonical_bundle_required_at(
        block.header.block_height,
        activation_height,
    ) {
        validate_private_tx_bundle_for_block(block, &block.private_tx_bodies)
    } else if block.private_tx_bodies.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "canonical private transaction bodies are not active at height {}",
            block.header.block_height
        ))
    }
}

/// Persist private transaction bodies that arrived attached to a block
/// payload. Each body's `tx_hash` is cross-checked against the block's
/// `private_tx_hashes` list — bodies that don't correspond to a committed
/// hash are silently dropped (a malicious peer cannot inject arbitrary
/// bodies). Already-stored bodies are not overwritten by this path.
///
/// Failures are logged but never abort the caller: persisting a body is
/// best-effort, the block apply already succeeded.
fn persist_attached_private_tx_bodies(storage: &RocksDbStorage, block: &Block, bodies: &[Vec<u8>]) {
    if bodies.is_empty() || block.private_tx_hashes.is_empty() {
        return;
    }
    let committed: std::collections::HashSet<&str> =
        block.private_tx_hashes.iter().map(|s| s.as_str()).collect();
    for body_bytes in bodies {
        let tx: crate::private_wallet::PrivateTransaction = match serde_json::from_slice(body_bytes)
        {
            Ok(tx) => tx,
            Err(err) => {
                tracing::debug!("ignoring attached private tx body: deserialize failed: {err}");
                continue;
            }
        };
        let hash = tx.tx_hash.trim();
        if hash.is_empty() || !committed.contains(hash) {
            tracing::debug!(
                "ignoring attached private tx body: hash {} not committed in block {}",
                hash,
                block.header.hash
            );
            continue;
        }
        // Skip if we already have a body for this hash (avoid clobbering
        // a copy we trust with a peer-supplied one).
        if matches!(storage.get_private_transaction_bytes(hash), Ok(Some(_))) {
            continue;
        }
        if let Err(err) = storage.save_private_transaction_bytes(hash, body_bytes) {
            tracing::warn!("failed to persist attached private tx body {hash}: {err}");
        }
    }
}

fn persist_available_private_tx_bodies(
    storage: &RocksDbStorage,
    block: &Block,
    attached_bodies: &[Vec<u8>],
) {
    persist_available_private_tx_bodies_at(
        storage,
        block,
        attached_bodies,
        crate::economics::private_tx_canonical_bundle_activation_height(),
    );
}

fn persist_available_private_tx_bodies_at(
    storage: &RocksDbStorage,
    block: &Block,
    attached_bodies: &[Vec<u8>],
    activation_height: Option<u64>,
) {
    if !block.private_tx_bodies.is_empty() {
        match validate_private_tx_bundle_policy_for_block_at(block, activation_height) {
            Ok(()) => persist_attached_private_tx_bodies(storage, block, &block.private_tx_bodies),
            Err(err) => tracing::warn!(
                "refusing to persist invalid canonical private tx bodies for block {}: {err}",
                block.header.hash
            ),
        }
    }
    persist_attached_private_tx_bodies(storage, block, attached_bodies);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::economics;
    use crate::mining::block_validator::build_coinbase_tx;
    use crate::mining::pow;
    use crate::state::chain::{apply_block_with_expected_difficulty, genesis_parent_for};
    use crate::types::{Address, Block, BlockHeader, PrivateCoinbaseOutput, PublicKey, Signature};

    fn open_temp_storage() -> (RocksDbStorage, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let storage = RocksDbStorage::new(dir.path().to_str().unwrap()).expect("rocksdb open");
        (storage, dir)
    }

    fn dummy_private_coinbase(block_height: u64, recipient_address: &str) -> PrivateCoinbaseOutput {
        PrivateCoinbaseOutput {
            block_height,
            recipient_address: recipient_address.to_string(),
            unlock_height: block_height.saturating_add(economics::COINBASE_MATURITY_BLOCKS),
            output_id: format!("coinbase-output-{block_height}"),
            asset_tag: "ZOKA".to_string(),
            asset_tag_commitment: "asset-commitment".to_string(),
            recipient_commitment: "zpriv:0000000000000000000000000000000000000000000000000000000000000000:1111111111111111111111111111111111111111111111111111111111111111".to_string(),
            stealth_address: "stlth_test".to_string(),
            ephemeral_pubkey: vec![1; 32],
            commitment: "coinbase-commitment".to_string(),
            amount_commitment: "coinbase-amount-commitment".to_string(),
            range_proof: vec![2; 32],
            one_time_public_key: "coinbase-one-time-key".to_string(),
            encrypted_amount: vec![3; 8],
            commitment_blind: vec![4; 32],
        }
    }

    fn mine_test_block(
        chain_id: &str,
        parent_hash: &str,
        parent_height: u64,
        parent_supply: u64,
        miner: &Address,
        difficulty_bits: u32,
        timestamp: u64,
    ) -> Block {
        let height = parent_height.saturating_add(1);
        let subsidy = economics::block_reward_for_height(height, parent_supply);
        let coinbase = build_coinbase_tx(miner.clone(), subsidy, height, timestamp);
        let engine = pow::engine_for(chain_id, height).expect("testnet engine");
        let mut nonce = 0u64;
        let (computed, found_nonce) = loop {
            let preimage =
                pow::build_pow_input(chain_id, parent_hash, miner, height, timestamp, nonce);
            let hash = engine.calculate_hash(&preimage).expect("hash");
            if pow::hash_meets_difficulty(&hash, difficulty_bits) {
                break (hash, nonce);
            }
            nonce = nonce.wrapping_add(1);
        };
        let mut block = Block {
            header: BlockHeader {
                hash: String::new(),
                prev_hash: parent_hash.to_string(),
                merkle_root: String::new(),
                state_root: String::from("0"),
                timestamp,
                nonce: found_nonce,
                block_height: height,
                validator: PublicKey(Vec::new()),
                seal: Signature(computed),
            },
            transactions: vec![coinbase],
            private_tx_hashes: Vec::new(),
            private_tx_bodies: Vec::new(),
            private_nullifiers: Vec::new(),
            private_fee_claims: Vec::new(),
            private_coinbase: Some(dummy_private_coinbase(height, miner)),
        };
        block.header.merkle_root = block.compute_merkle_root();
        block.header.hash = block.compute_hash();
        block
    }

    fn response_for(block: &Block) -> BlockResponseMessage {
        BlockResponseMessage {
            block_height: block.header.block_height,
            encrypted_block: serde_json::to_vec(block).expect("serialize block"),
            block_hash: block.header.hash.clone(),
            difficulty_bits: None,
            private_tx_bodies: Vec::new(),
            timestamp: block.header.timestamp,
        }
    }

    fn block_with_private_bundle(
        private_tx_hashes: Vec<String>,
        private_nullifiers: Vec<String>,
    ) -> Block {
        let chain_id = "zoka-test-net";
        let mut block = mine_test_block(
            chain_id,
            &genesis_parent_for(chain_id).header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_000,
        );
        block.private_tx_hashes = private_tx_hashes;
        block.private_nullifiers = private_nullifiers;
        block.header.merkle_root = block.compute_merkle_root();
        block.header.hash = block.compute_hash();
        block
    }

    #[test]
    fn strict_private_bundle_rejects_missing_body() {
        let block = block_with_private_bundle(
            vec!["committed-private-tx".to_string()],
            vec!["committed-nullifier".to_string()],
        );

        let err = validate_private_tx_bundle_for_block(&block, &[])
            .expect_err("strict bundle must require one body per committed hash");

        assert!(err.contains("body count mismatch"));
    }

    #[test]
    fn strict_private_bundle_rejects_duplicate_committed_hash() {
        let block = block_with_private_bundle(
            vec![
                "committed-private-tx".to_string(),
                "committed-private-tx".to_string(),
            ],
            Vec::new(),
        );

        let err = validate_private_tx_bundle_for_block(&block, &[])
            .expect_err("strict bundle must reject duplicate committed hashes");

        assert!(err.contains("repeats private transaction hash"));
    }

    #[test]
    fn strict_private_bundle_rejects_duplicate_committed_nullifier() {
        let block = block_with_private_bundle(
            Vec::new(),
            vec![
                "committed-nullifier".to_string(),
                "committed-nullifier".to_string(),
            ],
        );

        let err = validate_private_tx_bundle_for_block(&block, &[])
            .expect_err("strict bundle must reject duplicate committed nullifiers");

        assert!(err.contains("repeats private nullifier"));
    }

    #[test]
    fn strict_private_bundle_rejects_dormant_v2_i1_profile() {
        let block = block_with_private_bundle(
            vec!["private-v2-disabled".to_string()],
            vec!["nullifier-v2-disabled".to_string()],
        );
        let body = serde_json::to_vec(&serde_json::json!({
            "tx_hash": "private-v2-disabled",
            "commitment": "commitment-v2-disabled",
            "private_proof_version": 2,
            "private_proof_profile": "private_tx_v2_i1",
            "nullifier": "nullifier-v2-disabled",
            "zk_proof": null,
            "timestamp": 1,
            "status": "Pending"
        }))
        .expect("serialize private tx fixture");

        let err = validate_private_tx_bundle_for_block(&block, &[body])
            .expect_err("strict bundle must reject profiles before activation");

        assert!(err.contains("not active at height"));
    }

    #[test]
    fn strict_private_bundle_uses_block_height_for_configured_v2_i1_profile() {
        let block = block_with_private_bundle(
            vec!["private-v2-configured".to_string()],
            vec!["nullifier-v2-configured".to_string()],
        );
        let body = serde_json::to_vec(&serde_json::json!({
            "tx_hash": "private-v2-configured",
            "commitment": "commitment-v2-configured",
            "private_proof_version": 2,
            "private_proof_profile": "private_tx_v2_i1",
            "nullifier": "nullifier-v2-configured",
            "zk_proof": null,
            "timestamp": 1,
            "status": "Pending"
        }))
        .expect("serialize private tx fixture");

        let err = validate_private_tx_bundle_for_block_at(&block, &[body], Some(0), Some(0))
            .expect_err("fixture remains invalid after profile activation");

        assert!(err.contains("hash does not match body"));
        assert!(!err.contains("not active"));
    }

    #[test]
    fn strict_private_bundle_rejects_body_order_mismatch() {
        let block = block_with_private_bundle(
            vec!["private-tx-a".to_string(), "private-tx-b".to_string()],
            vec!["nullifier-a".to_string(), "nullifier-b".to_string()],
        );
        let body_a = serde_json::to_vec(&serde_json::json!({
            "tx_hash": "private-tx-a",
            "commitment": "commitment-a",
            "nullifier": "nullifier-a",
            "zk_proof": null,
            "timestamp": 1,
            "status": "Pending"
        }))
        .expect("serialize private tx fixture");
        let body_b = serde_json::to_vec(&serde_json::json!({
            "tx_hash": "private-tx-b",
            "commitment": "commitment-b",
            "nullifier": "nullifier-b",
            "zk_proof": null,
            "timestamp": 1,
            "status": "Pending"
        }))
        .expect("serialize private tx fixture");

        let err = validate_private_tx_bundle_for_block(&block, &[body_b, body_a])
            .expect_err("strict bundle must preserve hash/body order");

        assert!(err.contains("does not match committed position"));
    }

    #[test]
    fn canonical_private_bundle_policy_rejects_bodies_before_activation() {
        let mut block = block_with_private_bundle(Vec::new(), Vec::new());
        block.private_tx_bodies = vec![b"unexpected-body".to_vec()];

        let err = validate_private_tx_bundle_policy_for_block_at(&block, None)
            .expect_err("dormant policy must reject canonical bodies");

        assert!(err.contains("not active"));
    }

    #[test]
    fn strict_private_bundle_accepts_valid_tx_and_rejects_nullifier_mismatch() {
        let mut manager = PrivateWalletManager::new();
        manager
            .create_private_wallet("alice")
            .expect("alice wallet");
        let bob = manager.create_private_wallet("bob").expect("bob wallet");
        manager
            .deposit_to_private_wallet("alice", 30)
            .expect("fund alice");
        let tx_hash = manager
            .send_private_transaction("alice", bob, 10)
            .expect("build private tx");
        let tx = manager
            .get_private_transaction(&tx_hash)
            .expect("private tx");
        let nullifiers = strict_private_tx_spend_tags(&tx).expect("derived nullifiers");
        let body = serde_json::to_vec(&tx).expect("serialize private tx");
        let block = block_with_private_bundle(vec![tx_hash], nullifiers);

        validate_private_tx_bundle_for_block(&block, std::slice::from_ref(&body))
            .expect("valid strict private bundle");

        let mut canonical = block.clone();
        canonical.private_tx_bodies = vec![body.clone()];
        validate_private_tx_bundle_policy_for_block_at(
            &canonical,
            Some(canonical.header.block_height),
        )
        .expect("activated policy accepts valid canonical private bundle");
        let (storage, _dir) = open_temp_storage();
        persist_available_private_tx_bodies_at(
            &storage,
            &canonical,
            &[],
            Some(canonical.header.block_height),
        );
        assert_eq!(
            storage
                .get_private_transaction_bytes(&canonical.private_tx_hashes[0])
                .expect("read persisted canonical body"),
            Some(body.clone())
        );

        let mut mismatched = block.clone();
        mismatched.private_nullifiers = vec!["different-nullifier".to_string()];
        let err = validate_private_tx_bundle_for_block(&mismatched, &[body])
            .expect_err("strict bundle must derive the committed nullifier set");

        assert!(err.contains("private nullifier set mismatch"));

        let mut tampered_tx = tx;
        tampered_tx.outputs[0].encrypted_amount[0] ^= 1;
        let tampered_body = serde_json::to_vec(&tampered_tx).expect("serialize tampered tx");
        let err = validate_private_tx_bundle_for_block(&block, &[tampered_body])
            .expect_err("strict bundle must reject body drift");

        assert!(err.contains("hash does not match body"));
    }

    #[test]
    fn peer_chain_view_keeps_max_observed_height() {
        let mut view = PeerChainView::default();
        view.observe(5, 0);
        view.observe(3, 0);
        view.observe(12, 0);
        view.observe(11, 0);
        assert_eq!(view.perceived_max_height, 12);
    }

    #[test]
    fn try_apply_rejects_invalid_json_payload() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let response = BlockResponseMessage {
            block_height: 1,
            encrypted_block: b"not json".to_vec(),
            block_hash: "deadbeef".to_string(),
            difficulty_bits: None,
            private_tx_bodies: Vec::new(),
            timestamp: 0,
        };
        match try_apply_synced_block_payload_with_expected_difficulty(
            &storage, chain_id, &response, 4,
        ) {
            SyncOutcome::BadPayload(_) => {}
            other => panic!("expected BadPayload, got: {other:?}"),
        }
    }

    #[test]
    fn try_apply_rejects_announced_hash_mismatch() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let block = mine_test_block(
            chain_id,
            &genesis_parent_for(chain_id).header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_000,
        );
        let mut response = response_for(&block);
        response.block_hash = "deadbeef".to_string();
        match try_apply_synced_block_payload_with_expected_difficulty(
            &storage, chain_id, &response, 4,
        ) {
            SyncOutcome::AnnouncedHashMismatch { announced, header } => {
                assert_eq!(announced, "deadbeef");
                assert_eq!(header, block.header.hash);
            }
            other => panic!("expected AnnouncedHashMismatch, got: {other:?}"),
        }
    }

    #[test]
    fn try_apply_marks_gap_when_block_height_is_ahead_of_local() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        // Build a block at height 3 from a synthetic parent that the
        // chain doesn't have. The DB head is at 0; the block claims
        // height 3 → gap.
        let mut block = mine_test_block(
            chain_id,
            &genesis_parent_for(chain_id).header.hash,
            2,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_000,
        );
        block.header.block_height = 3;
        block.header.hash = block.compute_hash();
        let response = response_for(&block);
        match try_apply_synced_block_payload(&storage, chain_id, &response) {
            SyncOutcome::GapAhead {
                local_head,
                candidate,
                candidate_hash,
                parent_hash,
            } => {
                assert_eq!(local_head, 0);
                assert_eq!(candidate, 3);
                assert_eq!(candidate_hash, block.header.hash);
                assert_eq!(parent_hash, block.header.prev_hash);
            }
            other => panic!("expected GapAhead, got: {other:?}"),
        }
    }

    #[test]
    fn try_apply_marks_already_known_when_block_is_a_duplicate() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let block = mine_test_block(
            chain_id,
            &genesis_parent_for(chain_id).header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_000,
        );
        apply_block_with_expected_difficulty(&storage, chain_id, &block, 4).expect("first apply");
        let response = response_for(&block);
        match try_apply_synced_block_payload(&storage, chain_id, &response) {
            SyncOutcome::AlreadyKnown { height, hash } => {
                assert_eq!(height, block.header.block_height);
                assert_eq!(hash, block.header.hash);
            }
            other => panic!("expected AlreadyKnown, got: {other:?}"),
        }
    }

    #[test]
    fn try_apply_known_body_without_parent_requests_branch_backfill() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let block1 = mine_test_block(
            chain_id,
            &genesis_parent_for(chain_id).header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_000,
        );
        apply_block_with_expected_difficulty(&storage, chain_id, &block1, 4).expect("apply 1");

        let orphan = mine_test_block(
            chain_id,
            "missing-parent-hash",
            1,
            economics::block_reward_for_height(1, 0),
            &"zka1miner".to_string(),
            4,
            1_700_000_120,
        );
        storage
            .save_block_body(&orphan)
            .expect("cache orphan body by hash");

        let response = response_for(&orphan);
        match try_apply_synced_block_payload_with_expected_difficulty(
            &storage, chain_id, &response, 4,
        ) {
            SyncOutcome::MissingParent {
                local_head,
                candidate,
                candidate_hash,
                parent_hash,
            } => {
                assert_eq!(local_head, 1);
                assert_eq!(candidate, 2);
                assert_eq!(candidate_hash, orphan.header.hash);
                assert_eq!(parent_hash, "missing-parent-hash");
            }
            other => panic!("expected MissingParent, got: {other:?}"),
        }
    }

    #[test]
    fn try_apply_known_noncanonical_block_requests_branch_backfill() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let canonical_1 = mine_test_block(
            chain_id,
            &genesis_parent_for(chain_id).header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_000,
        );
        apply_block_with_expected_difficulty(&storage, chain_id, &canonical_1, 4)
            .expect("apply canonical 1");
        let canonical_2 = mine_test_block(
            chain_id,
            &canonical_1.header.hash,
            1,
            economics::block_reward_for_height(1, 0),
            &"zka1miner".to_string(),
            4,
            1_700_000_120,
        );
        apply_block_with_expected_difficulty(&storage, chain_id, &canonical_2, 4)
            .expect("apply canonical 2");

        let side_1 = mine_test_block(
            chain_id,
            &genesis_parent_for(chain_id).header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_777,
        );
        assert!(matches!(
            apply_block_with_expected_difficulty(&storage, chain_id, &side_1, 4),
            Err(ChainError::SideBranchNotHeavier { .. })
        ));
        let side_2 = mine_test_block(
            chain_id,
            &side_1.header.hash,
            1,
            economics::block_reward_for_height(1, 0),
            &"zka1miner".to_string(),
            4,
            1_700_000_897,
        );
        assert!(matches!(
            apply_block_with_expected_difficulty(&storage, chain_id, &side_2, 4),
            Err(ChainError::SideBranchNotHeavier { .. })
        ));

        match try_apply_synced_block_payload_with_expected_difficulty(
            &storage,
            chain_id,
            &response_for(&side_2),
            4,
        ) {
            SyncOutcome::MissingParent {
                local_head,
                candidate,
                candidate_hash,
                parent_hash,
            } => {
                assert_eq!(local_head, 2);
                assert_eq!(candidate, 2);
                assert_eq!(candidate_hash, side_2.header.hash);
                assert_eq!(parent_hash, side_1.header.hash);
            }
            other => panic!("expected known side branch backfill, got: {other:?}"),
        }
    }

    #[test]
    fn try_apply_far_ahead_block_with_known_parent_reaches_consensus_path() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let miner = "zka1miner".to_string();

        let canonical_1 = mine_test_block(
            chain_id,
            &genesis_parent_for(chain_id).header.hash,
            0,
            0,
            &miner,
            4,
            1_700_000_000,
        );
        apply_block_with_expected_difficulty(&storage, chain_id, &canonical_1, 4)
            .expect("apply canonical 1");
        let canonical_1_supply = canonical_1.transactions[0].amount;
        let canonical_2 = mine_test_block(
            chain_id,
            &canonical_1.header.hash,
            1,
            canonical_1_supply,
            &miner,
            4,
            1_700_000_120,
        );
        apply_block_with_expected_difficulty(&storage, chain_id, &canonical_2, 4)
            .expect("apply canonical 2");

        // Keep the validated side branch non-canonical while building a
        // parent two heights beyond the local head.
        storage
            .set_cumulative_work(u128::MAX)
            .expect("raise canonical work for fixture");
        let side_1 = mine_test_block(
            chain_id,
            &genesis_parent_for(chain_id).header.hash,
            0,
            0,
            &miner,
            4,
            1_700_000_777,
        );
        assert!(matches!(
            apply_block_with_expected_difficulty(&storage, chain_id, &side_1, 4),
            Err(ChainError::SideBranchNotHeavier { .. })
        ));
        let side_1_supply = side_1.transactions[0].amount;
        let side_2 = mine_test_block(
            chain_id,
            &side_1.header.hash,
            1,
            side_1_supply,
            &miner,
            4,
            1_700_000_897,
        );
        assert!(matches!(
            apply_block_with_expected_difficulty(&storage, chain_id, &side_2, 4),
            Err(ChainError::SideBranchNotHeavier { .. })
        ));
        let side_2_supply = side_1_supply.saturating_add(side_2.transactions[0].amount);
        let side_3 = mine_test_block(
            chain_id,
            &side_2.header.hash,
            2,
            side_2_supply,
            &miner,
            4,
            1_700_001_017,
        );
        assert!(matches!(
            apply_block_with_expected_difficulty(&storage, chain_id, &side_3, 4),
            Err(ChainError::SideBranchNotHeavier { .. })
        ));
        assert!(storage
            .block_exists(&side_3.header.hash)
            .expect("known side parent"));

        let side_3_supply = side_2_supply.saturating_add(side_3.transactions[0].amount);
        let side_4 = mine_test_block(
            chain_id,
            &side_3.header.hash,
            3,
            side_3_supply,
            &miner,
            4,
            1_700_001_137,
        );
        match try_apply_synced_block_payload_with_expected_difficulty(
            &storage,
            chain_id,
            &response_for(&side_4),
            4,
        ) {
            SyncOutcome::BehindOrSibling {
                candidate,
                candidate_hash,
                ..
            } => {
                assert_eq!(candidate, 4);
                assert_eq!(candidate_hash, side_4.header.hash);
            }
            other => panic!("known-parent side tip must reach consensus path, got: {other:?}"),
        }
    }

    #[test]
    fn try_apply_marks_behind_when_block_height_is_below_local() {
        let chain_id = "zoka-test-net";
        let (storage, _dir) = open_temp_storage();
        let block1 = mine_test_block(
            chain_id,
            &genesis_parent_for(chain_id).header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_000,
        );
        apply_block_with_expected_difficulty(&storage, chain_id, &block1, 4).expect("apply 1");
        let block2 = mine_test_block(
            chain_id,
            &block1.header.hash,
            block1.header.block_height,
            economics::block_reward_for_height(1, 0),
            &"zka1miner".to_string(),
            4,
            1_700_000_120,
        );
        apply_block_with_expected_difficulty(&storage, chain_id, &block2, 4).expect("apply 2");

        // Build a sibling block at height 1 with a DIFFERENT timestamp so
        // it has a different hash. This simulates a peer offering us a
        // forked block at an already-covered height.
        let sibling = mine_test_block(
            chain_id,
            &genesis_parent_for(chain_id).header.hash,
            0,
            0,
            &"zka1miner".to_string(),
            4,
            1_700_000_777,
        );
        let response = response_for(&sibling);
        match try_apply_synced_block_payload_with_expected_difficulty(
            &storage, chain_id, &response, 4,
        ) {
            SyncOutcome::BehindOrSibling {
                local_head,
                candidate,
                candidate_hash,
            } => {
                assert_eq!(local_head, 2);
                assert_eq!(candidate, 1);
                assert_eq!(candidate_hash, sibling.header.hash);
            }
            other => panic!("expected BehindOrSibling, got: {other:?}"),
        }
    }

    #[test]
    fn next_required_height_returns_none_when_caught_up() {
        let (storage, _dir) = open_temp_storage();
        let view = PeerChainView::default();
        let next = next_required_height(&storage, &view).expect("query");
        assert_eq!(next, None);
    }

    #[test]
    fn next_required_height_returns_local_plus_one_when_behind() {
        let (storage, _dir) = open_temp_storage();
        let mut view = PeerChainView::default();
        view.observe(5, 0);
        let next = next_required_height(&storage, &view).expect("query");
        assert_eq!(next, Some(1));
    }

    /// El pedido normal PREFIERE al par que anuncio mas trabajo, pero no se
    /// ata a el en exclusiva: si ese par tarda o falla, cualquier otro par
    /// conectado tiene que poder servir la misma altura sin hash fijado, sin
    /// esperar a que el preferido responda.
    ///
    /// Antes esto era al reves (afinidad estricta por defecto) y el sintoma
    /// medido era un nodo con 6-7 pares sanos clavado minutos porque
    /// insistia solo con el par preferido.
    #[test]
    fn normal_forward_request_prefers_but_does_not_lock_to_one_peer() {
        let (storage, _dir) = open_temp_storage();
        let mut view = PeerChainView::default();
        let source = PeerId::random();
        view.observe_trusted_height_from(source, 5);

        let request = next_required_block(&storage, &view)
            .expect("query")
            .expect("forward request");
        assert_eq!(request.height, 1);
        assert_eq!(request.expected_hash, None);
        assert_eq!(
            request.preferred_peer,
            Some(source),
            "la preferencia por el par mas al dia se mantiene como orden"
        );
        assert!(
            !request.strict_peer_affinity,
            "pero ya no puede ser un candado que deje afuera a los demas pares"
        );
    }

    #[test]
    fn untrusted_observe_ignores_implausibly_far_height() {
        let (storage, _dir) = open_temp_storage();
        let mut view = PeerChainView::default();
        view.observe(MAX_PENDING_BLOCK_AHEAD + 100, 0);

        let next = next_required_height(&storage, &view).expect("query");
        assert_eq!(next, None);
    }

    #[test]
    fn trusted_observe_allows_far_rpc_bootstrap_height() {
        let (storage, _dir) = open_temp_storage();
        let mut view = PeerChainView::default();
        view.observe_trusted_height(MAX_PENDING_BLOCK_AHEAD + 100);

        let next = next_required_height(&storage, &view).expect("query");
        assert_eq!(next, Some(1));
    }

    fn dummy_header_summary(
        height: u64,
        hash: &str,
        prev_hash: &str,
        cumulative_work: u128,
    ) -> crate::network::header_request_response::HeaderSummary {
        crate::network::header_request_response::HeaderSummary {
            header: BlockHeader {
                hash: hash.to_string(),
                prev_hash: prev_hash.to_string(),
                merkle_root: "mr".to_string(),
                state_root: "sr".to_string(),
                timestamp: height * 120,
                nonce: 0,
                block_height: height,
                validator: PublicKey(b"v".to_vec()),
                seal: Signature(b"s".to_vec()),
            },
            difficulty_bits: 20,
            cumulative_work,
        }
    }

    /// Sin paseo activo no hay nada que acelerar.
    #[test]
    fn header_probe_not_needed_without_an_active_walk() {
        let view = PeerChainView::default();
        assert!(!view.needs_header_probe());
    }

    /// Arrancar un paseo pide cabeceras una sola vez, no en cada vuelta del
    /// bucle mientras se espera la respuesta.
    #[test]
    fn a_new_branch_needs_a_header_probe_until_one_is_sent() {
        let mut view = PeerChainView::default();
        let source = Some(PeerId::random());

        view.record_sync_outcome_from(
            source,
            &SyncOutcome::MissingParent {
                local_head: 100,
                candidate: 101,
                candidate_hash: "h101".to_string(),
                parent_hash: "otra-rama".to_string(),
            },
        );
        assert!(view.has_active_branch_backfill());
        assert!(
            view.needs_header_probe(),
            "un paseo recien arrancado deberia pedir cabeceras"
        );

        view.mark_header_probe_sent();
        assert!(
            !view.needs_header_probe(),
            "no hay que repetir el pedido mientras se espera la respuesta"
        );
    }

    /// El caso que mas tiempo ahorra: una rama competidora con menos trabajo
    /// se descarta con las cabeceras solas, sin bajar ni un body.
    ///
    /// Antes, el nodo solo se enteraba de que una rama pesaba menos DESPUES
    /// de caminarla entera hacia atras y volver a aplicar todo hacia
    /// adelante -- horas de descargas para terminar descartandola igual.
    #[test]
    fn a_lighter_competing_branch_gets_abandoned_without_downloading_a_single_body() {
        let mut view = PeerChainView::default();
        let source = Some(PeerId::random());
        view.record_sync_outcome_from(
            source,
            &SyncOutcome::MissingParent {
                local_head: 100,
                candidate: 101,
                candidate_hash: "h101".to_string(),
                parent_hash: "ancestro".to_string(),
            },
        );
        assert!(view.has_active_branch_backfill());

        let headers = vec![dummy_header_summary(101, "h101", "ancestro", 500)];
        let outcome = view.apply_header_chain(100, "ancestro", &headers, /* local_work */ 900);

        assert_eq!(outcome, HeaderChainOutcome::NotHeavier);
        assert!(
            !view.has_active_branch_backfill(),
            "una rama mas liviana no merece seguir caminandose"
        );
    }

    /// El caso principal: una cadena de cabeceras valida y mas pesada
    /// resuelve el rango entero en una sola vuelta, no una por altura.
    #[test]
    fn a_heavier_header_chain_resolves_the_whole_range_in_one_shot() {
        let mut view = PeerChainView::default();
        let source = Some(PeerId::random());
        view.record_sync_outcome_from(
            source,
            &SyncOutcome::MissingParent {
                local_head: 100,
                candidate: 101,
                candidate_hash: "h101".to_string(),
                parent_hash: "ancestro".to_string(),
            },
        );

        let headers = vec![
            dummy_header_summary(101, "h101", "ancestro", 1_100),
            dummy_header_summary(102, "h102", "h101", 1_200),
            dummy_header_summary(103, "h103", "h102", 1_300),
        ];
        let outcome = view.apply_header_chain(100, "ancestro", &headers, /* local_work */ 1_000);

        assert_eq!(
            outcome,
            HeaderChainOutcome::Adopted {
                from_height: 101,
                through_height: 103,
            }
        );
        assert_eq!(view.branch_backfill_height, Some(101));
        assert_eq!(view.branch_backfill_ceiling, Some(103));
        for (height, hash) in [(101, "h101"), (102, "h102"), (103, "h103")] {
            assert_eq!(
                view.branch_expected_hashes.get(&height).map(String::as_str),
                Some(hash),
                "cada altura del rango tiene que quedar con su hash, no solo la primera"
            );
        }
    }

    /// Una cadena de cabeceras que no encaja (un salto de altura, un hash que
    /// no conecta) se rechaza sin tocar el paseo que ya estaba en curso.
    #[test]
    fn an_invalid_header_chain_is_rejected_without_touching_the_walk() {
        let mut view = PeerChainView::default();
        let source = Some(PeerId::random());
        view.record_sync_outcome_from(
            source,
            &SyncOutcome::MissingParent {
                local_head: 100,
                candidate: 101,
                candidate_hash: "h101".to_string(),
                parent_hash: "ancestro".to_string(),
            },
        );
        let pin_antes = view.branch_expected_hashes.clone();

        // Un salto: 101 seguido de 103, sin el 102.
        let headers = vec![
            dummy_header_summary(101, "h101", "ancestro", 1_100),
            dummy_header_summary(103, "h103", "h101", 1_300),
        ];
        let outcome = view.apply_header_chain(100, "ancestro", &headers, 1_000);

        assert!(matches!(outcome, HeaderChainOutcome::Invalid(_)));
        assert!(
            view.has_active_branch_backfill(),
            "una respuesta invalida no puede tirar abajo el paseo que ya andaba"
        );
        assert_eq!(
            view.branch_expected_hashes, pin_antes,
            "el pin original queda intacto ante una respuesta que no sirve"
        );
    }

    /// Una respuesta que llega tarde, para un episodio que ya se cerro, se
    /// ignora en vez de reabrir algo.
    #[test]
    fn apply_header_chain_does_nothing_without_an_active_walk() {
        let mut view = PeerChainView::default();
        let headers = vec![dummy_header_summary(101, "h101", "ancestro", 1_100)];
        let outcome = view.apply_header_chain(100, "ancestro", &headers, 1_000);
        assert_eq!(outcome, HeaderChainOutcome::NotActive);
        assert!(!view.has_active_branch_backfill());
    }

    /// Ir atrasado no es estar bifurcado, y no puede disparar la caminata.
    ///
    /// GapAhead significa "me llego un bloque mas alto que mi cabeza", o sea
    /// SIEMPRE que el nodo va atrasado. Arrancar ahi el paseo hacia atras lo
    /// volvia el camino por defecto del caso comun: un nodo atrasado 240
    /// bloques recorria la ancestria de a uno desde la punta, ~50s por paso
    /// sobre Tor, para descubrir un enganche que la altura siguiente le
    /// habria dado en un pedido. Medido en una PC: 16 horas y media sin
    /// aplicar un bloque, con la cadena local sana.
    #[test]
    fn a_plain_gap_no_longer_starts_a_backward_walk() {
        let mut view = PeerChainView::default();
        let source = Some(PeerId::random());
        view.observe(15882, 15643);

        view.record_sync_outcome_from(
            source,
            &SyncOutcome::GapAhead {
                local_head: 15643,
                candidate: 15881,
                candidate_hash: "h15881".to_string(),
                parent_hash: "h15880".to_string(),
            },
        );

        assert_eq!(
            view.branch_backfill_height, None,
            "estar atrasado se resuelve pidiendo la altura siguiente"
        );
    }

    /// Un paseo largo nunca es la respuesta, por mas que el pedido directo
    /// falle.
    ///
    /// Contar fallos no alcanzaba: cuatro timeouts sobre Tor son normales y
    /// disparaban una caminata desde la punta, cien pasos o mil, de a un
    /// bloque, mientras la cadena avanza. Medido en la PC: con la cabeza en
    /// 15816 y la punta en 15927, el nodo se puso a caminar hacia atras y el
    /// atraso crecio en vez de cerrarse.
    #[test]
    fn a_far_away_candidate_never_starts_a_backward_walk() {
        let mut view = PeerChainView::default();
        let source = Some(PeerId::random());
        view.observe(15927, 15816);
        // El camino directo ya se dio por perdido.
        view.record_forward_gap_failure();
        view.record_forward_gap_failure();
        view.record_forward_gap_failure();

        view.record_sync_outcome_from(
            source,
            &SyncOutcome::GapAhead {
                local_head: 15816,
                candidate: 15926,
                candidate_hash: "h15926".to_string(),
                parent_hash: "h15925".to_string(),
            },
        );

        assert_eq!(
            view.branch_backfill_height, None,
            "110 pasos hacia atras no recuperan a nadie; hace falta un snapshot"
        );
    }

    /// Pero el paseo sigue disponible para lo que fue pensado: pares que
    /// podaron `local_head + 1` y ya no lo tienen. Ahora se llega ahi por
    /// evidencia -- pedidos hacia adelante que no llegan -- en vez de por
    /// suposicion.
    #[test]
    fn a_pruned_peer_still_triggers_the_backward_walk() {
        let mut view = PeerChainView::default();
        let source = Some(PeerId::random());
        view.observe(15882, 15643);

        // Cerca de la cabeza: es el escenario real de poda -- faltan unos
        // pocos bloques, no doscientos. Un paseo largo esta descartado por
        // MAX_BACKWARD_WALK_DISTANCE y tiene su propio test.
        let gap = SyncOutcome::GapAhead {
            local_head: 15643,
            candidate: 15650,
            candidate_hash: "h15650".to_string(),
            parent_hash: "h15649".to_string(),
        };

        // Dos fallos todavia no alcanzan: puede ser un par lento.
        view.record_forward_gap_failure();
        view.record_forward_gap_failure();
        view.record_sync_outcome_from(source, &gap);
        assert_eq!(view.branch_backfill_height, None);

        // Al tercero se acepta que esa altura no la sirve nadie.
        view.record_forward_gap_failure();
        view.record_sync_outcome_from(source, &gap);
        assert_eq!(
            view.branch_backfill_height,
            Some(15649),
            "sin camino directo hay que buscar la ancestria hacia atras"
        );
    }

    /// Una bifurcacion de verdad no depende de ese contador: la delata
    /// MissingParent, que arranca el paseo de inmediato.
    #[test]
    fn a_real_fork_still_walks_back_immediately() {
        let mut view = PeerChainView::default();
        let source = Some(PeerId::random());
        view.observe(15882, 15643);

        view.record_sync_outcome_from(
            source,
            &SyncOutcome::MissingParent {
                local_head: 15643,
                candidate: 15644,
                candidate_hash: "h15644".to_string(),
                parent_hash: "otra-rama".to_string(),
            },
        );

        assert_eq!(
            view.branch_backfill_height,
            Some(15643),
            "un padre que no engancha es evidencia de rama distinta, no hay que esperar"
        );
    }

    /// Y un bloque aplicado limpia la sospecha: el camino directo funciona.
    #[test]
    fn applying_a_block_clears_the_forward_suspicion() {
        let mut view = PeerChainView::default();
        view.record_forward_gap_failure();
        view.record_forward_gap_failure();
        view.record_forward_gap_failure();
        assert!(view.forward_path_looks_unavailable());

        view.record_sync_outcome(&SyncOutcome::Applied(crate::state::chain::ChainHead {
            height: 15644,
            hash: "h15644".to_string(),
            total_supply_emitted: 0,
            cumulative_work: 1,
        }));

        assert!(
            !view.forward_path_looks_unavailable(),
            "si entrego, deja de ser sospechoso"
        );
    }

    #[test]
    fn next_required_height_prefers_branch_backfill() {
        let (storage, _dir) = open_temp_storage();
        let mut view = PeerChainView::default();
        view.observe(20, 0);
        view.branch_backfill_height = Some(7);
        view.branch_backfill_ceiling = Some(12);
        view.branch_expected_hashes
            .insert(7, "branch-h7".to_string());

        let next = next_required_height(&storage, &view).expect("query");
        assert_eq!(next, Some(7));
        let request = next_required_block(&storage, &view)
            .expect("request")
            .expect("some request");
        assert_eq!(request.height, 7);
        assert_eq!(request.expected_hash.as_deref(), Some("branch-h7"));
    }

    #[test]
    fn peer_chain_view_backfills_then_replays_to_ceiling() {
        let mut view = PeerChainView::default();
        view.record_sync_outcome(&SyncOutcome::MissingParent {
            local_head: 11,
            candidate: 12,
            candidate_hash: "h12".to_string(),
            parent_hash: "h11".to_string(),
        });
        assert_eq!(view.branch_backfill_height, Some(11));
        assert_eq!(view.branch_backfill_ceiling, Some(12));
        assert_eq!(
            view.branch_expected_hashes.get(&11).map(String::as_str),
            Some("h11")
        );

        view.record_sync_outcome(&SyncOutcome::MissingParent {
            local_head: 11,
            candidate: 11,
            candidate_hash: "h11".to_string(),
            parent_hash: "h10".to_string(),
        });
        assert_eq!(view.branch_backfill_height, Some(10));
        assert_eq!(view.branch_backfill_ceiling, Some(12));
        assert_eq!(
            view.branch_expected_hashes.get(&10).map(String::as_str),
            Some("h10")
        );

        view.record_sync_outcome(&SyncOutcome::MissingParent {
            local_head: 11,
            candidate: 12,
            candidate_hash: "h12".to_string(),
            parent_hash: "h11".to_string(),
        });
        assert_eq!(view.branch_backfill_height, Some(10));
        assert_eq!(view.branch_backfill_ceiling, Some(12));

        view.record_sync_outcome(&SyncOutcome::BehindOrSibling {
            local_head: 11,
            candidate: 10,
            candidate_hash: "h10".to_string(),
        });
        assert_eq!(view.branch_backfill_height, Some(11));

        view.record_sync_outcome(&SyncOutcome::BehindOrSibling {
            local_head: 11,
            candidate: 11,
            candidate_hash: "h11".to_string(),
        });
        assert_eq!(view.branch_backfill_height, Some(12));

        view.record_sync_outcome(&SyncOutcome::AlreadyKnown {
            height: 12,
            hash: "h12".to_string(),
        });
        assert_eq!(view.branch_backfill_height, None);
        assert_eq!(view.branch_backfill_ceiling, None);
        assert!(view.branch_expected_hashes.is_empty());
    }

    #[test]
    fn known_side_blocks_advance_when_replayed_after_their_pinned_parent() {
        let mut view = PeerChainView::default();
        let source = PeerId::random();
        view.record_sync_outcome_from(
            Some(source),
            &SyncOutcome::MissingParent {
                local_head: 10,
                candidate: 12,
                candidate_hash: "side-12".to_string(),
                parent_hash: "side-11".to_string(),
            },
        );
        view.record_sync_outcome_from(
            Some(source),
            &SyncOutcome::MissingParent {
                local_head: 10,
                candidate: 11,
                candidate_hash: "side-11".to_string(),
                parent_hash: "side-10".to_string(),
            },
        );
        view.record_sync_outcome_from(
            Some(source),
            &SyncOutcome::AlreadyKnown {
                height: 10,
                hash: "side-10".to_string(),
            },
        );
        assert_eq!(view.branch_backfill_height, Some(11));

        view.record_sync_outcome_from(
            Some(source),
            &SyncOutcome::MissingParent {
                local_head: 10,
                candidate: 11,
                candidate_hash: "side-11".to_string(),
                parent_hash: "side-10".to_string(),
            },
        );
        assert_eq!(view.branch_backfill_height, Some(12));
        view.record_sync_outcome_from(
            Some(source),
            &SyncOutcome::MissingParent {
                local_head: 10,
                candidate: 12,
                candidate_hash: "side-12".to_string(),
                parent_hash: "side-11".to_string(),
            },
        );
        assert!(!view.has_active_branch_backfill());
    }

    #[test]
    fn peer_chain_view_does_not_backfill_unsourced_gap_ahead() {
        let mut view = PeerChainView::default();
        view.record_sync_outcome(&SyncOutcome::GapAhead {
            local_head: 0,
            candidate: 5,
            candidate_hash: "h5".to_string(),
            parent_hash: "h4".to_string(),
        });

        assert_eq!(view.branch_backfill_height, None);
        assert_eq!(view.branch_backfill_ceiling, None);
    }

    #[test]
    fn sourced_gap_ahead_starts_hash_pinned_parent_backfill() {
        let (storage, _dir) = open_temp_storage();
        let mut view = PeerChainView::default();
        let source = PeerId::random();
        view.observe_trusted_height_from(source, 5);

        // El paseo hacia atras ya no arranca por el solo hecho de ir
        // atrasado: hace falta que el camino directo falle. Este test es
        // sobre como se comporta el paseo una vez activo, asi que se lo
        // enciende explicitamente.
        view.record_forward_gap_failure();
        view.record_forward_gap_failure();
        view.record_forward_gap_failure();

        view.record_sync_outcome_from(
            Some(source),
            &SyncOutcome::GapAhead {
                local_head: 2,
                candidate: 5,
                candidate_hash: "h5".to_string(),
                parent_hash: "h4".to_string(),
            },
        );

        assert_eq!(view.active_branch_source(), Some(source));
        assert_eq!(view.branch_backfill_height, Some(4));
        assert_eq!(view.branch_backfill_ceiling, Some(5));
        let request = next_required_block(&storage, &view)
            .expect("request query")
            .expect("backfill request");
        assert_eq!(request.height, 4);
        assert_eq!(request.expected_hash.as_deref(), Some("h4"));
        assert_eq!(request.preferred_peer, Some(source));
        assert!(!request.strict_peer_affinity);
    }

    #[test]
    fn branch_forward_discovery_stays_source_affined_after_hash_backfill() {
        let (storage, _dir) = open_temp_storage();
        let mut view = PeerChainView::default();
        let source = PeerId::random();
        view.observe_trusted_height_from(source, 7);
        // El paseo hacia atras ya no arranca por el solo hecho de ir
        // atrasado: hace falta que el camino directo falle. Este test es
        // sobre como se comporta el paseo una vez activo, asi que se lo
        // enciende explicitamente.
        view.record_forward_gap_failure();
        view.record_forward_gap_failure();
        view.record_forward_gap_failure();
        view.record_sync_outcome_from(
            Some(source),
            &SyncOutcome::GapAhead {
                local_head: 3,
                candidate: 5,
                candidate_hash: "h5".to_string(),
                parent_hash: "h4".to_string(),
            },
        );
        view.record_sync_outcome_from(
            Some(PeerId::random()),
            &SyncOutcome::BehindOrSibling {
                local_head: 3,
                candidate: 4,
                candidate_hash: "h4".to_string(),
            },
        );
        view.record_sync_outcome_from(
            Some(PeerId::random()),
            &SyncOutcome::AlreadyKnown {
                height: 5,
                hash: "h5".to_string(),
            },
        );

        let request = next_required_block(&storage, &view)
            .expect("request query")
            .expect("forward branch request");
        assert_eq!(request.height, 6);
        assert_eq!(request.expected_hash, None);
        assert_eq!(request.preferred_peer, Some(source));
        assert!(request.strict_peer_affinity);
    }

    #[test]
    fn competing_sourced_gap_cannot_overwrite_active_branch() {
        let mut view = PeerChainView::default();
        let peer_a = PeerId::random();
        let peer_b = PeerId::random();
        view.observe_trusted_height_from(peer_a, 12);
        view.observe_trusted_height_from(peer_b, 12);

        // El paseo hacia atras ya no arranca por el solo hecho de ir
        // atrasado: hace falta que el camino directo falle. Este test es
        // sobre como se comporta el paseo una vez activo, asi que se lo
        // enciende explicitamente.
        view.record_forward_gap_failure();
        view.record_forward_gap_failure();
        view.record_forward_gap_failure();

        view.record_sync_outcome_from(
            Some(peer_a),
            &SyncOutcome::GapAhead {
                local_head: 10,
                candidate: 12,
                candidate_hash: "a-12".to_string(),
                parent_hash: "a-11".to_string(),
            },
        );
        view.record_sync_outcome_from(
            Some(peer_b),
            &SyncOutcome::GapAhead {
                local_head: 10,
                candidate: 12,
                candidate_hash: "b-12".to_string(),
                parent_hash: "b-11".to_string(),
            },
        );

        assert_eq!(view.active_branch_source(), Some(peer_a));
        assert_eq!(view.branch_backfill_height, Some(11));
        assert_eq!(
            view.branch_expected_hashes.get(&12).map(String::as_str),
            Some("a-12")
        );
        assert_eq!(
            view.branch_expected_hashes.get(&11).map(String::as_str),
            Some("a-11")
        );
    }

    #[test]
    fn rejected_source_cannot_drive_later_branch_selection() {
        let mut view = PeerChainView::default();
        let rejected = PeerId::random();
        let healthy = PeerId::random();
        view.observe_trusted_height_from(rejected, 20);
        view.observe_trusted_height_from(healthy, 18);
        assert_eq!(view.perceived_max_height, 20);

        view.record_sync_outcome_from(
            Some(rejected),
            &SyncOutcome::Rejected("checkpoint mismatch".to_string()),
        );
        assert_eq!(view.perceived_max_height, 18);
        view.observe_trusted_height_from(rejected, 30);
        view.observe_from(rejected, 31, 10);
        view.record_sync_outcome_from(
            Some(rejected),
            &SyncOutcome::GapAhead {
                local_head: 10,
                candidate: 12,
                candidate_hash: "rejected-12".to_string(),
                parent_hash: "rejected-11".to_string(),
            },
        );

        assert_eq!(view.preferred_peer_for_height(12), Some(healthy));
        assert_eq!(view.perceived_max_height, 18);
        assert!(!view.has_active_branch_backfill());
    }

    #[test]
    fn peer_chain_view_caches_gap_ahead_response_for_later_ibd() {
        let mut view = PeerChainView::default();
        let response = BlockResponseMessage {
            block_height: 5,
            encrypted_block: b"cached-block".to_vec(),
            block_hash: "h5".to_string(),
            difficulty_bits: None,
            private_tx_bodies: Vec::new(),
            timestamp: 1_700_000_000,
        };
        let outcome = SyncOutcome::GapAhead {
            local_head: 2,
            candidate: 5,
            candidate_hash: "h5".to_string(),
            parent_hash: "h4".to_string(),
        };

        view.remember_block_response(&outcome, &response);
        assert_eq!(view.pending_block_response_count(), 1);

        let replayed = view
            .take_pending_block_response(&SyncRequest {
                height: 5,
                expected_hash: Some("h5".to_string()),
                preferred_peer: None,
                strict_peer_affinity: false,
            })
            .expect("cached response");
        assert_eq!(replayed.block_hash, "h5");
        assert_eq!(replayed.encrypted_block, b"cached-block".to_vec());
        assert_eq!(view.pending_block_response_count(), 0);
    }

    #[test]
    fn peer_chain_view_does_not_cache_unbounded_future_responses() {
        let mut view = PeerChainView::default();
        let future_height = MAX_PENDING_BLOCK_AHEAD.saturating_add(10);
        let response = BlockResponseMessage {
            block_height: future_height,
            encrypted_block: b"far-future-block".to_vec(),
            block_hash: "too-far".to_string(),
            difficulty_bits: None,
            private_tx_bodies: Vec::new(),
            timestamp: 1_700_000_000,
        };
        let outcome = SyncOutcome::GapAhead {
            local_head: 0,
            candidate: future_height,
            candidate_hash: "too-far".to_string(),
            parent_hash: "parent".to_string(),
        };

        view.remember_block_response(&outcome, &response);
        assert_eq!(view.pending_block_response_count(), 0);
    }

    #[test]
    fn peer_chain_view_ignores_stale_missing_parent_without_active_backfill() {
        let mut view = PeerChainView::default();
        view.record_sync_outcome(&SyncOutcome::MissingParent {
            local_head: 5,
            candidate: 4,
            candidate_hash: "old-side-4".to_string(),
            parent_hash: "old-side-3".to_string(),
        });

        assert_eq!(view.branch_backfill_height, None);
        assert_eq!(view.branch_backfill_ceiling, None);
        assert!(view.branch_expected_hashes.is_empty());
    }

    #[test]
    fn peer_chain_view_ignores_unrelated_known_blocks_while_backfilling() {
        let mut view = PeerChainView::default();
        view.record_sync_outcome(&SyncOutcome::MissingParent {
            local_head: 16,
            candidate: 17,
            candidate_hash: "winner-17".to_string(),
            parent_hash: "winner-16".to_string(),
        });

        view.record_sync_outcome(&SyncOutcome::AlreadyKnown {
            height: 17,
            hash: "local-17".to_string(),
        });

        assert_eq!(view.branch_backfill_height, Some(16));
        assert_eq!(view.branch_backfill_ceiling, Some(17));
    }

    #[test]
    fn peer_chain_view_can_abandon_unavailable_branch_backfill() {
        let mut view = PeerChainView::default();
        view.record_sync_outcome(&SyncOutcome::MissingParent {
            local_head: 6,
            candidate: 7,
            candidate_hash: "gone-7".to_string(),
            parent_hash: "gone-6".to_string(),
        });

        assert!(view.abandon_branch_backfill());
        assert_eq!(view.branch_backfill_height, None);
        assert_eq!(view.branch_backfill_ceiling, None);
        assert!(view.branch_expected_hashes.is_empty());
        assert!(!view.abandon_branch_backfill());
    }

    #[test]
    fn peer_chain_view_keeps_branch_backfill_across_transient_request_misses() {
        let mut view = PeerChainView::default();
        view.record_sync_outcome(&SyncOutcome::MissingParent {
            local_head: 6,
            candidate: 7,
            candidate_hash: "winner-7".to_string(),
            parent_hash: "winner-6".to_string(),
        });

        for _ in 1..MAX_BRANCH_BACKFILL_REQUEST_FAILURES {
            assert!(!view.record_branch_backfill_request_failure());
            assert_eq!(view.branch_backfill_height, Some(6));
            assert_eq!(view.branch_backfill_ceiling, Some(7));
        }

        assert!(view.record_branch_backfill_request_failure());
        assert_eq!(view.branch_backfill_height, None);
        assert_eq!(view.branch_backfill_ceiling, None);
        assert!(view.branch_expected_hashes.is_empty());
    }

    #[test]
    fn peer_chain_view_pauses_mining_when_behind_or_backfilling() {
        let mut view = PeerChainView::default();
        assert_eq!(view.mining_pause_target(3), None);

        view.observe(8, 0);
        assert_eq!(view.mining_pause_target(3), Some(8));
        assert_eq!(view.mining_pause_target(8), None);

        view.record_sync_outcome(&SyncOutcome::MissingParent {
            local_head: 8,
            candidate: 9,
            candidate_hash: "winner-9".to_string(),
            parent_hash: "winner-8".to_string(),
        });
        assert_eq!(view.mining_pause_target(9), Some(9));
    }

    #[test]
    fn peer_chain_view_continues_forward_side_branch_until_peer_tip() {
        let mut view = PeerChainView::default();
        let source = PeerId::random();
        view.observe_trusted_height_from(source, 6);
        view.record_sync_outcome_from(
            Some(source),
            &SyncOutcome::MissingParent {
                local_head: 1,
                candidate: 2,
                candidate_hash: "peer-2".to_string(),
                parent_hash: "peer-1".to_string(),
            },
        );
        assert_eq!(view.branch_backfill_height, Some(1));
        assert_eq!(view.branch_backfill_ceiling, Some(6));

        view.record_sync_outcome_from(
            Some(source),
            &SyncOutcome::BehindOrSibling {
                local_head: 1,
                candidate: 1,
                candidate_hash: "peer-1".to_string(),
            },
        );
        assert_eq!(view.branch_backfill_height, Some(2));

        view.record_sync_outcome_from(
            Some(source),
            &SyncOutcome::BehindOrSibling {
                local_head: 1,
                candidate: 2,
                candidate_hash: "peer-2".to_string(),
            },
        );
        assert_eq!(view.branch_backfill_height, Some(3));
        assert_eq!(view.branch_backfill_ceiling, Some(6));
        assert!(!view.branch_expected_hashes.contains_key(&3));

        view.record_sync_outcome_from(
            Some(source),
            &SyncOutcome::BehindOrSibling {
                local_head: 1,
                candidate: 3,
                candidate_hash: "peer-3".to_string(),
            },
        );
        assert_eq!(view.branch_backfill_height, Some(4));
        assert_eq!(
            view.branch_expected_hashes.get(&3).map(String::as_str),
            Some("peer-3")
        );
    }

    #[test]
    fn active_branch_cannot_be_overwritten_by_stronger_competing_peer() {
        let mut view = PeerChainView::default();
        let peer_a = PeerId::random();
        let peer_b = PeerId::random();
        view.observe_block_announce_from(peer_a, 12, "a-12", Some(100), 10);
        view.observe_block_announce_from(peer_b, 12, "b-12", Some(500), 10);

        view.record_sync_outcome_from(
            Some(peer_a),
            &SyncOutcome::MissingParent {
                local_head: 10,
                candidate: 12,
                candidate_hash: "a-12".to_string(),
                parent_hash: "a-11".to_string(),
            },
        );
        assert_eq!(view.active_branch_source(), Some(peer_a));
        assert_eq!(view.active_branch_cumulative_work(), Some(100));

        view.record_sync_outcome_from(
            Some(peer_b),
            &SyncOutcome::MissingParent {
                local_head: 10,
                candidate: 12,
                candidate_hash: "b-12".to_string(),
                parent_hash: "b-11".to_string(),
            },
        );

        assert_eq!(view.active_branch_source(), Some(peer_a));
        assert_eq!(view.branch_backfill_height, Some(11));
        assert_eq!(view.branch_backfill_ceiling, Some(12));
        assert_eq!(
            view.branch_expected_hashes.get(&12).map(String::as_str),
            Some("a-12")
        );
        assert_eq!(
            view.branch_expected_hashes.get(&11).map(String::as_str),
            Some("a-11")
        );
    }

    #[test]
    fn active_branch_filters_unrelated_bodies_before_consensus_work() {
        let mut view = PeerChainView::default();
        let branch_peer = PeerId::random();
        let relay = PeerId::random();
        view.record_sync_outcome_from(
            Some(branch_peer),
            &SyncOutcome::MissingParent {
                local_head: 10,
                candidate: 12,
                candidate_hash: "winner-12".to_string(),
                parent_hash: "winner-11".to_string(),
            },
        );

        assert!(view.should_process_block_candidate(Some(&relay), 11, "winner-11"));
        assert!(!view.should_process_block_candidate(Some(&relay), 11, "other-11"));
        assert!(!view.should_process_block_candidate(Some(&relay), 10, "unrelated-10"));

        view.branch_backfill_height = Some(13);
        assert!(view.should_process_block_candidate(Some(&branch_peer), 13, "winner-13"));
        assert!(!view.should_process_block_candidate(Some(&relay), 13, "winner-13"));
    }

    #[test]
    fn rejected_active_branch_is_abandoned_for_next_candidate() {
        let mut view = PeerChainView::default();
        let invalid_peer = PeerId::random();
        let recovery_peer = PeerId::random();
        view.observe_block_announce_from(invalid_peer, 12, "invalid-12", Some(500), 10);
        view.observe_block_announce_from(recovery_peer, 12, "valid-12", Some(400), 10);
        view.record_sync_outcome_from(
            Some(invalid_peer),
            &SyncOutcome::MissingParent {
                local_head: 10,
                candidate: 12,
                candidate_hash: "invalid-12".to_string(),
                parent_hash: "invalid-11".to_string(),
            },
        );
        assert_eq!(view.active_branch_source(), Some(invalid_peer));

        view.record_sync_outcome_from(
            Some(invalid_peer),
            &SyncOutcome::Rejected("checkpoint mismatch".to_string()),
        );
        assert!(!view.has_active_branch_backfill());

        view.record_sync_outcome_from(
            Some(recovery_peer),
            &SyncOutcome::MissingParent {
                local_head: 10,
                candidate: 12,
                candidate_hash: "valid-12".to_string(),
                parent_hash: "valid-11".to_string(),
            },
        );
        assert_eq!(view.active_branch_source(), Some(recovery_peer));
        assert_eq!(
            view.branch_expected_hashes.get(&12).map(String::as_str),
            Some("valid-12")
        );
    }

    #[test]
    fn branch_ceiling_comes_from_its_source_not_global_height() {
        let mut view = PeerChainView::default();
        let branch_peer = PeerId::random();
        let taller_other_peer = PeerId::random();
        view.observe_trusted_height_from(branch_peer, 20);
        view.observe_trusted_height_from(taller_other_peer, 50);

        view.record_sync_outcome_from(
            Some(branch_peer),
            &SyncOutcome::MissingParent {
                local_head: 10,
                candidate: 12,
                candidate_hash: "branch-12".to_string(),
                parent_hash: "branch-11".to_string(),
            },
        );

        assert_eq!(view.perceived_max_height, 50);
        assert_eq!(view.branch_backfill_ceiling, Some(20));
        assert_eq!(view.active_branch_source(), Some(branch_peer));
    }

    #[test]
    fn preferred_peer_uses_work_before_height() {
        let mut view = PeerChainView::default();
        let stronger = PeerId::random();
        let taller = PeerId::random();
        view.observe_block_announce_from(stronger, 20, "strong-20", Some(1_000), 0);
        view.observe_block_announce_from(taller, 30, "tall-30", Some(900), 0);

        assert_eq!(view.preferred_peer_for_height(10), Some(stronger));
        assert_eq!(view.preferred_peer_for_height(25), Some(taller));
    }

    #[test]
    fn checkpoint_recovery_can_backfill_below_quarantined_local_tip() {
        let mut view = PeerChainView::default();
        let recovery_peer = PeerId::random();
        view.set_recovery_mode(true);
        view.observe_block_announce_from(
            recovery_peer,
            10_887,
            "checkpoint-valid-10887",
            Some(50_000),
            11_051,
        );

        view.record_sync_outcome_from(
            Some(recovery_peer),
            &SyncOutcome::MissingParent {
                local_head: 11_051,
                candidate: 10_887,
                candidate_hash: "checkpoint-valid-10887".to_string(),
                parent_hash: "checkpoint-valid-10886".to_string(),
            },
        );

        assert_eq!(view.active_branch_source(), Some(recovery_peer));
        assert_eq!(view.branch_backfill_height, Some(10_886));
        assert_eq!(
            view.branch_expected_hashes.get(&10_887).map(String::as_str),
            Some("checkpoint-valid-10887")
        );
    }

    #[test]
    fn checkpoint_recovery_bootstraps_from_the_compiled_anchor_not_local_tip() {
        let (storage, _dir) = open_temp_storage();
        storage
            .set_chain_height(11_051)
            .expect("seed quarantined local height");
        let mut view = PeerChainView::default();
        let recovery_peer = PeerId::random();
        view.set_recovery_mode(true);
        view.observe_trusted_height_from(recovery_peer, 11_200);

        let request = next_required_block(&storage, &view)
            .expect("recovery request")
            .expect("compiled checkpoint must be requested");

        assert_eq!(
            request.height,
            crate::economics::MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT
        );
        assert_eq!(
            request.expected_hash.as_deref(),
            Some(crate::economics::MAINNET_RECOVERY_CHECKPOINT_10887_HASH)
        );
        assert_eq!(request.preferred_peer, Some(recovery_peer));
        assert!(!request.strict_peer_affinity);
    }
}
