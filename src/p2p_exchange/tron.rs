//! Verifying that a buyer's USDT payment actually arrived.
//!
//! The seller's node watches the TRC-20 transfers reaching the payout address
//! it published, and releases the ZKA when one satisfies the offer. That
//! decision is final and automatic — there is no arbiter to appeal to — so the
//! matching rules here are deliberately narrow.
//!
//! ## Privacy
//!
//! The query goes to a public Tron API, which learns the caller's IP and which
//! address they are watching. Together that links a network identity to a
//! trade, so [`TronConfig`] carries a SOCKS proxy and the caller is expected to
//! route through Tor. This module never opens sockets itself; it decides *what*
//! counts as payment, and the transport layer decides *how* to ask.

use serde::{Deserialize, Serialize};

use super::{payment_satisfies, Offer};

/// Public TronGrid endpoint. Works without an API key, at a lower rate limit.
pub const DEFAULT_TRONGRID_URL: &str = "https://api.trongrid.io";

/// USDT's TRC-20 contract on Tron mainnet.
///
/// Checked on every transfer: Tron lets anyone deploy a token calling itself
/// "USDT", so matching on the symbol would accept a worthless lookalike and
/// release real ZKA for it.
pub const USDT_TRC20_CONTRACT: &str = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t";

/// Where and how to reach Tron.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TronConfig {
    pub base_url: String,
    /// Additional TronGrid-compatible endpoints tried in order.
    #[serde(default)]
    pub fallback_urls: Vec<String>,
    /// Optional TronGrid API key. Absent means the public rate limit.
    #[serde(default)]
    pub api_key: Option<String>,
    /// SOCKS5 proxy, normally the local Tor daemon. `None` means a direct
    /// request, which exposes the caller's IP to the API operator.
    #[serde(default)]
    pub socks_proxy: Option<String>,
}

impl Default for TronConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_TRONGRID_URL.to_string(),
            fallback_urls: Vec::new(),
            api_key: None,
            socks_proxy: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TronClientError {
    #[error("Tor SOCKS proxy is unavailable; payment verification was not attempted")]
    TorUnavailable,
    #[error("invalid SOCKS proxy address: {0}")]
    InvalidProxy(String),
    #[error("all Tron providers failed: {0}")]
    ProvidersExhausted(String),
}

#[derive(Clone)]
pub struct TronClient {
    config: TronConfig,
    client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct TronGridResponse {
    #[serde(default)]
    data: Vec<TronGridTransfer>,
}

#[derive(Debug, Deserialize)]
struct TronGridTransfer {
    transaction_id: String,
    to: String,
    from: String,
    token_info: TronGridTokenInfo,
    value: String,
    block_timestamp: u64,
}

#[derive(Debug, Deserialize)]
struct TronGridTokenInfo {
    address: String,
}

impl TronClient {
    /// Builds a client that can only connect through the configured SOCKS
    /// proxy. `socks5h` keeps DNS resolution on the Tor side.
    pub fn new(config: TronConfig) -> Result<Self, TronClientError> {
        let proxy = config
            .socks_proxy
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(TronClientError::TorUnavailable)?;
        let proxy_url = if proxy.starts_with("socks5://") || proxy.starts_with("socks5h://") {
            proxy.to_string()
        } else {
            format!("socks5h://{proxy}")
        };
        let proxy = reqwest::Proxy::all(&proxy_url)
            .map_err(|error| TronClientError::InvalidProxy(error.to_string()))?;
        let client = reqwest::Client::builder()
            .proxy(proxy)
            .build()
            .map_err(|error| TronClientError::InvalidProxy(error.to_string()))?;
        Ok(Self { config, client })
    }

    pub async fn fetch_transfers(
        &self,
        address: &str,
        limit: u32,
    ) -> Result<Vec<TronTransfer>, TronClientError> {
        let path = trc20_transfers_path(address, limit);
        let providers = std::iter::once(self.config.base_url.as_str())
            .chain(self.config.fallback_urls.iter().map(String::as_str));
        let mut failures = Vec::new();

        for provider in providers {
            let provider = provider.trim().trim_end_matches('/');
            if provider.is_empty() {
                continue;
            }
            let mut request = self.client.get(format!("{provider}{path}"));
            if let Some(api_key) = self
                .config
                .api_key
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                request = request.header("TRON-PRO-API-KEY", api_key);
            }
            let response = match request.send().await {
                Ok(response) => response,
                Err(error) => {
                    failures.push(format!("{provider}: {error}"));
                    continue;
                }
            };
            let response = match response.error_for_status() {
                Ok(response) => response,
                Err(error) => {
                    failures.push(format!("{provider}: {error}"));
                    continue;
                }
            };
            let payload = match response.json::<TronGridResponse>().await {
                Ok(payload) => payload,
                Err(error) => {
                    failures.push(format!("{provider}: invalid response: {error}"));
                    continue;
                }
            };
            let mut transfers = Vec::with_capacity(payload.data.len());
            let mut malformed = None;
            for transfer in payload.data {
                match transfer.value.parse::<u64>() {
                    Ok(value) => transfers.push(TronTransfer {
                        transaction_id: transfer.transaction_id,
                        to: transfer.to,
                        from: transfer.from,
                        contract_address: transfer.token_info.address,
                        value,
                        block_timestamp: transfer.block_timestamp,
                    }),
                    Err(error) => {
                        malformed = Some(format!(
                            "{provider}: invalid transfer value '{}': {error}",
                            transfer.value
                        ));
                        break;
                    }
                }
            }
            if let Some(error) = malformed {
                failures.push(error);
                continue;
            }
            return Ok(transfers);
        }

        Err(TronClientError::ProvidersExhausted(failures.join("; ")))
    }

    pub async fn settling_transfer(
        &self,
        offer: &Offer,
        claimed_at_secs: u64,
    ) -> Result<Option<TronTransfer>, TronClientError> {
        let transfers = self
            .fetch_transfers(&offer.usdt_receive_address, 200)
            .await?;
        Ok(find_settling_transfer(offer, &transfers, claimed_at_secs).cloned())
    }
}

impl TronConfig {
    pub fn from_env(socks_proxy: Option<String>) -> Self {
        let base_url = std::env::var("ZOKA_TRON_API_URL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_TRONGRID_URL.to_string());
        let fallback_urls = std::env::var("ZOKA_TRON_FALLBACK_URLS")
            .ok()
            .into_iter()
            .flat_map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|entry| !entry.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect();
        let api_key = std::env::var("ZOKA_TRON_API_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        Self {
            base_url,
            fallback_urls,
            api_key,
            socks_proxy,
        }
    }

    /// Whether payment checks may run.
    ///
    /// Without a proxy the request would tie the user's IP to the address they
    /// are watching. Refusing is the safer failure: an unverified payment
    /// leaves the offer to expire and the ZKA returns to the seller, whereas a
    /// deanonymised trade cannot be undone.
    pub fn is_private(&self) -> bool {
        self.socks_proxy.as_ref().is_some_and(|p| !p.trim().is_empty())
    }
}

/// One TRC-20 transfer as reported by the API.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TronTransfer {
    pub transaction_id: String,
    /// Destination address, base58.
    pub to: String,
    pub from: String,
    /// Token contract the transfer belongs to.
    pub contract_address: String,
    /// Amount in the token's smallest unit — micro-USDT for USDT.
    pub value: u64,
    /// Block time, milliseconds since epoch, as Tron reports it.
    pub block_timestamp: u64,
}

/// Why a transfer did not settle an offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaymentMismatch {
    WrongToken,
    WrongDestination,
    /// Confirmed before the buyer claimed, so it belongs to some other trade.
    PredatesClaim,
    /// Short by more than the tolerance.
    Underpaid,
    /// The offer is not waiting for payment.
    OfferNotClaimed,
}

/// Whether `transfer` settles `offer`, given the claim happened at
/// `claimed_at_secs`.
///
/// Every clause closes a way the wrong transfer could release the ZKA:
///
/// - the contract must be USDT, since anyone can mint a token by that name;
/// - the destination must be the published payout address;
/// - the transfer must postdate the claim, so an unrelated earlier payment to
///   the same address — a previous trade, or the seller's own funds — cannot be
///   replayed to settle this offer;
/// - the amount must clear the tolerance.
///
/// Matching on amount alone was the tempting shortcut and is the reason for
/// the timestamp rule: two offers priced identically would otherwise settle
/// each other from a single payment.
pub fn transfer_settles_offer(
    offer: &Offer,
    transfer: &TronTransfer,
    claimed_at_secs: u64,
) -> Result<(), PaymentMismatch> {
    if !offer.status.holds_reservation() || offer.claimed_by.is_none() {
        return Err(PaymentMismatch::OfferNotClaimed);
    }
    if !transfer.contract_address.eq_ignore_ascii_case(USDT_TRC20_CONTRACT) {
        return Err(PaymentMismatch::WrongToken);
    }
    if transfer.to != offer.usdt_receive_address {
        return Err(PaymentMismatch::WrongDestination);
    }
    // Tron reports milliseconds; offers keep seconds.
    if transfer.block_timestamp / 1_000 < claimed_at_secs {
        return Err(PaymentMismatch::PredatesClaim);
    }
    if !payment_satisfies(offer.micro_usdt, transfer.value) {
        return Err(PaymentMismatch::Underpaid);
    }
    Ok(())
}

/// The first transfer in `transfers` that settles `offer`.
///
/// Returns the earliest qualifying payment rather than the largest: the buyer
/// paid once, and a later transfer to the same address belongs to whatever
/// trade came next.
pub fn find_settling_transfer<'a>(
    offer: &Offer,
    transfers: &'a [TronTransfer],
    claimed_at_secs: u64,
) -> Option<&'a TronTransfer> {
    transfers
        .iter()
        .filter(|t| transfer_settles_offer(offer, t, claimed_at_secs).is_ok())
        .min_by_key(|t| t.block_timestamp)
}

/// Path for listing TRC-20 transfers into `address`.
///
/// Kept beside the matching rules so the request and the rules that judge its
/// answer stay in step.
pub fn trc20_transfers_path(address: &str, limit: u32) -> String {
    format!(
        "/v1/accounts/{address}/transactions/trc20?only_to=true&limit={}&contract_address={}",
        limit.clamp(1, 200),
        USDT_TRC20_CONTRACT
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::economics::ATOMS_PER_ZKA;
    use crate::p2p_exchange::{OfferStatus, MICRO_USDT_PER_USDT};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const PAYOUT: &str = "TQ5NMqJjW8dWvJf3zJ8xYqL2mNpR4sT6uV";
    const CLAIMED_AT: u64 = 1_751_000_000;

    fn claimed_offer() -> Offer {
        Offer {
            id: "offer-1".to_string(),
            seller_address: "zka1seller".to_string(),
            zka_atoms: 100 * ATOMS_PER_ZKA,
            micro_usdt: 25 * MICRO_USDT_PER_USDT,
            usdt_receive_address: PAYOUT.to_string(),
            status: OfferStatus::Claimed,
            created_at: CLAIMED_AT - 500,
            claim_expires_at: Some(CLAIMED_AT + 2_700),
            claimed_by: Some("zka1buyer".to_string()),
            reserved_note_ids: vec!["note-a".to_string()],
        }
    }

    fn transfer() -> TronTransfer {
        TronTransfer {
            transaction_id: "tron-tx-1".to_string(),
            to: PAYOUT.to_string(),
            from: "TBuyerAddress111111111111111111111".to_string(),
            contract_address: USDT_TRC20_CONTRACT.to_string(),
            value: 25 * MICRO_USDT_PER_USDT,
            block_timestamp: (CLAIMED_AT + 60) * 1_000,
        }
    }

    async fn spawn_mock_socks_http(
        responses: Vec<(u16, String)>,
    ) -> (
        String,
        tokio::sync::mpsc::UnboundedReceiver<String>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (requests_tx, requests_rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            for (status, body) in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut greeting = [0u8; 2];
                socket.read_exact(&mut greeting).await.unwrap();
                let mut methods = vec![0u8; greeting[1] as usize];
                socket.read_exact(&mut methods).await.unwrap();
                socket.write_all(&[5, 0]).await.unwrap();

                let mut request_head = [0u8; 4];
                socket.read_exact(&mut request_head).await.unwrap();
                let destination = match request_head[3] {
                    1 => {
                        let mut ip = [0u8; 4];
                        socket.read_exact(&mut ip).await.unwrap();
                        std::net::Ipv4Addr::from(ip).to_string()
                    }
                    3 => {
                        let mut len = [0u8; 1];
                        socket.read_exact(&mut len).await.unwrap();
                        let mut host = vec![0u8; len[0] as usize];
                        socket.read_exact(&mut host).await.unwrap();
                        String::from_utf8(host).unwrap()
                    }
                    atyp => panic!("unexpected SOCKS address type {atyp}"),
                };
                let mut port = [0u8; 2];
                socket.read_exact(&mut port).await.unwrap();
                socket
                    .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80])
                    .await
                    .unwrap();

                let mut request = Vec::new();
                loop {
                    let mut byte = [0u8; 1];
                    socket.read_exact(&mut byte).await.unwrap();
                    request.push(byte[0]);
                    if request.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                let request = format!(
                    "SOCKS-Destination: {destination}\n{}",
                    String::from_utf8(request).unwrap()
                );
                requests_tx.send(request).unwrap();

                let reason = if status == 200 {
                    "OK"
                } else {
                    "Service Unavailable"
                };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        (address.to_string(), requests_rx, task)
    }

    #[test]
    fn a_matching_payment_settles_the_offer() {
        assert_eq!(
            transfer_settles_offer(&claimed_offer(), &transfer(), CLAIMED_AT),
            Ok(())
        );
    }

    #[test]
    fn a_lookalike_token_does_not_settle() {
        // Anyone can deploy a token calling itself USDT. Accepting one would
        // release real ZKA for something worthless.
        let mut fake = transfer();
        fake.contract_address = "TFakeUSDTContract11111111111111111".to_string();
        assert_eq!(
            transfer_settles_offer(&claimed_offer(), &fake, CLAIMED_AT),
            Err(PaymentMismatch::WrongToken)
        );
    }

    #[test]
    fn a_payment_to_another_address_does_not_settle() {
        let mut elsewhere = transfer();
        elsewhere.to = "TSomeoneElse11111111111111111111111".to_string();
        assert_eq!(
            transfer_settles_offer(&claimed_offer(), &elsewhere, CLAIMED_AT),
            Err(PaymentMismatch::WrongDestination)
        );
    }

    #[test]
    fn a_payment_predating_the_claim_does_not_settle() {
        // The rule that stops an older payment to the same address — an
        // earlier trade, or the seller's own funds — from being replayed to
        // settle this offer for free.
        let mut earlier = transfer();
        earlier.block_timestamp = (CLAIMED_AT - 1) * 1_000;
        assert_eq!(
            transfer_settles_offer(&claimed_offer(), &earlier, CLAIMED_AT),
            Err(PaymentMismatch::PredatesClaim)
        );
    }

    #[test]
    fn underpayment_beyond_tolerance_does_not_settle() {
        let mut short = transfer();
        short.value = 20 * MICRO_USDT_PER_USDT;
        assert_eq!(
            transfer_settles_offer(&claimed_offer(), &short, CLAIMED_AT),
            Err(PaymentMismatch::Underpaid)
        );
    }

    #[test]
    fn small_underpayment_within_tolerance_settles() {
        let mut fee_shaved = transfer();
        // 0.2% short, the kind of shortfall a transfer fee produces.
        fee_shaved.value = 25 * MICRO_USDT_PER_USDT - 50_000;
        assert_eq!(
            transfer_settles_offer(&claimed_offer(), &fee_shaved, CLAIMED_AT),
            Ok(())
        );
    }

    #[test]
    fn overpayment_settles() {
        let mut generous = transfer();
        generous.value = 30 * MICRO_USDT_PER_USDT;
        assert_eq!(
            transfer_settles_offer(&claimed_offer(), &generous, CLAIMED_AT),
            Ok(())
        );
    }

    #[test]
    fn an_unclaimed_offer_cannot_be_settled() {
        // Without this an open offer could be released by a stray payment,
        // handing the ZKA to nobody in particular.
        let mut open = claimed_offer();
        open.status = OfferStatus::Open;
        open.claimed_by = None;
        assert_eq!(
            transfer_settles_offer(&open, &transfer(), CLAIMED_AT),
            Err(PaymentMismatch::OfferNotClaimed)
        );
    }

    #[test]
    fn an_already_released_offer_cannot_be_settled_twice() {
        let mut done = claimed_offer();
        done.status = OfferStatus::Released;
        assert_eq!(
            transfer_settles_offer(&done, &transfer(), CLAIMED_AT),
            Err(PaymentMismatch::OfferNotClaimed)
        );
    }

    #[test]
    fn the_earliest_qualifying_payment_is_chosen() {
        // The buyer paid once. A later transfer to the same address belongs to
        // whatever trade came next, so picking by size or by last-seen would
        // attribute someone else's payment to this offer.
        let mut later = transfer();
        later.transaction_id = "tron-tx-2".to_string();
        later.block_timestamp = (CLAIMED_AT + 600) * 1_000;
        later.value = 40 * MICRO_USDT_PER_USDT;

        let transfers = vec![later, transfer()];
        let found = find_settling_transfer(&claimed_offer(), &transfers, CLAIMED_AT);
        assert_eq!(found.map(|t| t.transaction_id.as_str()), Some("tron-tx-1"));
    }

    #[test]
    fn no_qualifying_payment_returns_none() {
        let mut wrong = transfer();
        wrong.value = 1;
        assert!(find_settling_transfer(&claimed_offer(), &[wrong], CLAIMED_AT).is_none());
    }

    #[test]
    fn config_without_a_proxy_is_not_private() {
        // The caller must refuse to query in this state: an unverified payment
        // only costs an expired offer, while a deanonymised trade is permanent.
        let mut config = TronConfig::default();
        assert!(!config.is_private());

        config.socks_proxy = Some("   ".to_string());
        assert!(!config.is_private(), "blank proxy must not count as private");

        config.socks_proxy = Some("127.0.0.1:9050".to_string());
        assert!(config.is_private());
    }

    #[test]
    fn the_default_endpoint_is_public_trongrid_without_a_key() {
        let config = TronConfig::default();
        assert_eq!(config.base_url, DEFAULT_TRONGRID_URL);
        assert!(config.api_key.is_none(), "must work with no key configured");
    }

    #[test]
    fn the_transfer_query_pins_the_usdt_contract_and_direction() {
        let path = trc20_transfers_path(PAYOUT, 50);
        assert!(path.contains(PAYOUT));
        assert!(path.contains("only_to=true"), "incoming transfers only");
        assert!(path.contains(USDT_TRC20_CONTRACT));
        // Absurd limits are clamped rather than forwarded.
        assert!(trc20_transfers_path(PAYOUT, 99_999).contains("limit=200"));
        assert!(trc20_transfers_path(PAYOUT, 0).contains("limit=1"));
    }

    #[test]
    fn client_refuses_to_exist_without_tor() {
        assert!(matches!(
            TronClient::new(TronConfig::default()),
            Err(TronClientError::TorUnavailable)
        ));
    }

    #[tokio::test]
    async fn transfers_are_fetched_through_socks_and_decoded() {
        let body = serde_json::json!({
            "data": [{
                "transaction_id": "tron-tx-1",
                "to": PAYOUT,
                "from": "TBuyerAddress111111111111111111111",
                "token_info": { "address": USDT_TRC20_CONTRACT },
                "value": (25 * MICRO_USDT_PER_USDT).to_string(),
                "block_timestamp": (CLAIMED_AT + 60) * 1_000
            }]
        })
        .to_string();
        let (proxy, mut requests, task) = spawn_mock_socks_http(vec![(200, body)]).await;
        let client = TronClient::new(TronConfig {
            base_url: "http://tron-primary.test".to_string(),
            fallback_urls: Vec::new(),
            api_key: Some("secret-test-key".to_string()),
            socks_proxy: Some(proxy),
        })
        .unwrap();

        let settling = client
            .settling_transfer(&claimed_offer(), CLAIMED_AT)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(settling.transaction_id, "tron-tx-1");
        let request = requests.recv().await.unwrap();
        assert!(request.contains("SOCKS-Destination: tron-primary.test"));
        assert!(request.contains(&trc20_transfers_path(PAYOUT, 200)));
        assert!(request
            .to_ascii_lowercase()
            .contains("tron-pro-api-key: secret-test-key"));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_failed_primary_uses_the_configured_fallback() {
        let body = serde_json::json!({ "data": [] }).to_string();
        let (proxy, mut requests, task) = spawn_mock_socks_http(vec![
            (503, "{}".to_string()),
            (200, body),
        ])
        .await;
        let client = TronClient::new(TronConfig {
            base_url: "http://tron-primary.test".to_string(),
            fallback_urls: vec!["http://tron-fallback.test".to_string()],
            api_key: None,
            socks_proxy: Some(proxy),
        })
        .unwrap();

        assert!(client.fetch_transfers(PAYOUT, 50).await.unwrap().is_empty());
        let primary = requests.recv().await.unwrap();
        let fallback = requests.recv().await.unwrap();
        assert!(primary.contains("SOCKS-Destination: tron-primary.test"));
        assert!(fallback.contains("SOCKS-Destination: tron-fallback.test"));
        task.await.unwrap();
    }
}
