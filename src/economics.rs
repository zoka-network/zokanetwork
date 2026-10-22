//! Canonical ZOKA mainnet v1 economic parameters.
//!
//! The wallet/accounting layer uses atomic units internally. User-facing CLI
//! and genesis fields keep speaking in ZKA and convert at the boundary.

pub const ECONOMICS_VERSION: &str = "zoka-economics-v2-atoms";
pub const TOKEN_SYMBOL: &str = "ZKA";
// Bumped for the Zenith relaunch genesis reset: the synthetic genesis hash
// (genesis_pow_hash_for_chain) is derived directly from this id, so a stale
// pre-relaunch chain is cryptographically incompatible with the new one —
// not just rejected by protocol version, but unable to even produce a
// matching block-1 parent hash. Defense in depth alongside PROTOCOL_FREEZE_ID.
pub const MAINNET_CHAIN_ID: &str = "zoka-mainnet-zenith-v1";
/// Retirada: nunca funciono como red publica. Se conserva solo porque la
/// eleccion historica del algoritmo de PoW la nombra; el CLI ya no la abre.
pub const TESTNET_CHAIN_ID: &str = "zoka-testnet-v1";

// Zenith (`MAINNET_CHAIN_ID`) se retiró el 21/10/2026: Zoka tiene dos redes,
// Zoka Quantic Testnet y Zoka Quantic Mainnet, las dos desde cero, y ninguna
// hereda nada de Zenith. El chain id no se renombra (de él sale el hash del
// génesis); lo que se muestra dice que ya no corre.
pub const ZENITH_NETWORK_ROLE: &str = "retired";
pub const ZENITH_DISPLAY_NAME: &str = "Zenith (retired)";

/// Rol público de una cadena conocida.
pub fn network_role_for_chain(chain_id: &str) -> Option<&'static str> {
    match chain_id {
        MAINNET_CHAIN_ID => Some(ZENITH_NETWORK_ROLE),
        QUANTIC_TESTNET_CHAIN_ID => Some("testnet"),
        QUANTIC_MAINNET_CHAIN_ID => Some("mainnet"),
        QUANTIC_REGTEST_CHAIN_ID => Some("regtest"),
        _ => None,
    }
}

/// Nombre para personas de una cadena conocida.
pub fn network_display_name_for_chain(chain_id: &str) -> Option<&'static str> {
    match chain_id {
        MAINNET_CHAIN_ID => Some(ZENITH_DISPLAY_NAME),
        QUANTIC_TESTNET_CHAIN_ID => Some("Zoka Quantic Testnet"),
        QUANTIC_MAINNET_CHAIN_ID => Some("Zoka Quantic Mainnet"),
        QUANTIC_REGTEST_CHAIN_ID => Some("Zoka Quantic Regtest (local)"),
        _ => None,
    }
}

/// Emergency mainnet checkpoint used by the 1.4.x hotfix rollout. Nodes that
/// have this checkpoint must not reorganize onto a branch that does not contain
/// this exact block.
pub const MAINNET_EMERGENCY_CHECKPOINT_HEIGHT: u64 = 296;
pub const MAINNET_EMERGENCY_CHECKPOINT_HASH: &str =
    "d8c4b8d5945469c46fb32c002d34c08cb736ef470578f2464c3054ac190ac95c";
/// Canonical blocks mined during the isolated 1.5.0 recovery. These close the
/// post-296 fork window: upgraded nodes reject any branch above 296 that does
/// not contain this exact recovery chain.
pub const MAINNET_RECOVERY_CHECKPOINT_297_HEIGHT: u64 = 297;
pub const MAINNET_RECOVERY_CHECKPOINT_297_HASH: &str =
    "a086b823ec51052473860211327bc7e5bad9262cb68df264ecafc05ee070e22f";
pub const MAINNET_RECOVERY_CHECKPOINT_298_HEIGHT: u64 = 298;
pub const MAINNET_RECOVERY_CHECKPOINT_298_HASH: &str =
    "bee49728c38951fc82dfda00d2b4000980cb7a3b07046d6f6c636bd1f2d98227";
/// Canonical anchor for the 1.6.0 protocol-freeze rollout. At ship time the
/// network had live private forks mined by stale/NAT-isolated 1.5.0 clients
/// (one 40+ blocks ahead of the canonical head by block count); this pin
/// guarantees upgraded nodes never reorganize onto any branch that does not
/// contain the BOOT/VPS2 canonical chain through this block, no matter how
/// much cumulative work such a fork later claims. Verified identical on both
/// infrastructure nodes before pinning.
pub const MAINNET_UPGRADE_CHECKPOINT_1460_HEIGHT: u64 = 1460;
pub const MAINNET_UPGRADE_CHECKPOINT_1460_HASH: &str =
    "39e45d515c49a54420922d1fa258391cfabed01a94948dbaabe312025c0fb314";
/// Canonical anchor for the serverless difficulty reset (1.7.5). By ship time
/// the network had drifted to difficulty 23 (~3.4h/block for the ~1 remaining
/// home miner), effectively stalling the chain. This pin fixes the canonical
/// head at 3127 so the reset difficulty cap below only applies on the real
/// chain, and upgraded nodes never reorg onto a divergent 3127. Verified live
/// on both the host and the follower before pinning.
pub const MAINNET_UPGRADE_CHECKPOINT_3127_HEIGHT: u64 = 3127;
pub const MAINNET_UPGRADE_CHECKPOINT_3127_HASH: &str =
    "b5c20cfbdc9e2f56bb143334c03f7bcc9df60fbd7c2c7ed553b61ee2dd9740df";
/// Recovery anchor for the 1.9.0 fork-alignment release. Live inspection found
/// three incompatible histories at this height. This block was verified on
/// the canonical seed; a node on any other history is quarantined from mining
/// while it ingests the branch containing this anchor.
pub const MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT: u64 = 10_887;
pub const MAINNET_RECOVERY_CHECKPOINT_10887_HASH: &str =
    "d30c20100f6405424d32e5c204799bc10a1197ff5cdc1fe0b04c2c538b11086e";

/// Ancla canónica posterior a la bifurcación observada en 10891.
///
/// El ancla de 10887 fija ese bloque y nada más: cuatro bloques después la
/// cadena volvió a partirse en dos candidatos con el mismo padre
/// (`b8c17acd…`), y ganó `910efde3…` por trabajo acumulado. Un nodo que
/// arrancó desde un snapshot tomado mientras la semilla estaba en la rama
/// perdedora hereda esa rama y **no converge solo**: la divergencia queda a
/// miles de bloques de profundidad y la reorganización por trabajo no llega
/// tan atrás en la práctica. Sin este pin, el único remedio es borrar la base
/// y sincronizar de cero.
///
/// Con el pin, cualquier cadena que no contenga este bloque viola el
/// checkpoint y `checkpoint_forced_reorg_required` la abandona sin importar el
/// trabajo que reclame. Y un snapshot servido desde una rama equivocada se
/// rechaza al importarlo, así que el problema deja de poder propagarse.
///
/// Altura elegida a 3.609 bloques por encima de la bifurcación y 626 por
/// debajo de la punta al momento de fijarla: lejos de donde todavía puede
/// haber reorganizaciones legítimas.
///
/// Verificado idéntico en dos nodos con bases de datos independientes antes de
/// fijarlo — hash, padre, nonce y timestamp coincidentes en los cuatro campos.
/// Ambos comparten vecinos, así que la comprobación es más débil que la de dos
/// infraestructuras separadas: dos máquinas que sincronizan de los mismos dos
/// pares heredarían el mismo error. La confirmación de que 10891 canónico es
/// `910efde3…` sí vino de nodos de otra red.
pub const MAINNET_RECOVERY_CHECKPOINT_14500_HEIGHT: u64 = 14500;
pub const MAINNET_RECOVERY_CHECKPOINT_14500_HASH: &str =
    "5301997c287e9a14f24700b65d568e915c59cd83b9d88c6cbcd994e22eed010f";

/// Evidence gates for publishing the 1.9.0 recovery release. The checkpoint
/// is compiled now so candidate binaries can be exercised, but release
/// artifacts must remain blocked until all thresholds are independently met.
pub const MAINNET_RECOVERY_MIN_CONFIRMATIONS: u64 = 120;
pub const MAINNET_RECOVERY_MIN_RELEASE_HEAD_HEIGHT: u64 =
    MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT + MAINNET_RECOVERY_MIN_CONFIRMATIONS;
pub const MAINNET_RECOVERY_MIN_INDEPENDENT_OBSERVATIONS: usize = 2;
pub const MAINNET_RECOVERY_REQUIRE_RELEASE_ARTIFACT_SHA256: bool = true;
pub const MAINNET_RECOVERY_AUTOMATIC_DB_DELETION: bool = false;
pub const MAINNET_RECOVERY_MISMATCH_ACTION: &str = "quarantine";
/// Clave pública (Ed25519, hex) del nodo del equipo que firma checkpoints, por
/// cadena. Los nodos no siguen ninguna rama que contradiga el último
/// checkpoint firmado con esta clave (`checkpoint_authority`,
/// `docs/CHECKPOINTS-AUTORITATIVOS.md`). Una cadena sin clave no tiene
/// checkpoints firmados: rige solo el trabajo acumulado y los compilados.
pub const CHECKPOINT_AUTHORITY_KEYS: &[(&str, &str)] = &[
    // Zoka Quantic Testnet: la PC del equipo (`zoka checkpoint-keygen --network
    // quantic-testnet`). La privada no sale de esa PC.
    (QUANTIC_TESTNET_CHAIN_ID, "05241771bed7063d0ad2d87ecf9e62e8982a3b06f7bf4e91825cf4b45c46a7bf"),
    (
    MAINNET_CHAIN_ID,
    // Zenith (testnet): la PC del equipo, creada el 13/10/2026 con
    // `zoka checkpoint-keygen`. La privada no sale de esa PC.
    "bf5c92154ee0e250f4419d47cf0a16c263c586244274ccad196234d339c47bd4",
)];

/// Mitad ML-DSA-65 (hex, 1.952 bytes) de la clave de la autoridad, por cadena.
/// Con ella compilada, la cadena solo acepta checkpoints híbridos (versión 2).
/// Zoka Quantic la exige desde el génesis: `zoka checkpoint-keygen --network
/// quantic-testnet` crea las dos mitades en la PC de la autoridad.
pub const CHECKPOINT_AUTHORITY_MLDSA_KEYS: &[(&str, &str)] = &[(
    QUANTIC_TESTNET_CHAIN_ID,
    "41a02787fcecc9ba2d0000af97d63ab39d5237fd06618146d2a126cd96ae766bfdd960260518da02de5e2246bd754c2bb5e9627b321223b43cbe6853ba45bcfcb1762cfcd8a885373d3974f0bd472bb5359f64088308f346ebd5ebd9894d01f18d17c4b0690ca4ad7a0ef9647ec00831aafdb63c74b74fb8006e0e43bfbd8fe81cb60ab535f2087183d90a4af518266eac96e7a6c66c954947c75494e770002e8245d3abc9baca8956e56923ae83706b9a9ba93fc033b7167b911687640ec736d3247e1875f692b6dfbce7451a2caaf153369a41066dc32eaeb618a7de70cd5bc2732900aa0563686682847d26ccaf73b21dfa0ccd0b9b93cb0f6feebd2ed7ed72f187a25169345ee5f8bce10aa4ab6dc9356a4439dc20497d5cc4b46e55110471f246c0872f714b13fc7e17929614b94acd04894f5c660347ae4c6e3bec3d18b2ed95e67792014598cc9dcdf211cbfa67cb7262a167e644960f67f4659d97eed9d7b36f6ebd9a233382cd4ccc2b50b58d8141326520b47b5cbb8f25bd3bff7cdeea43eaad9b2d32eb15c30eea8e66f82597c7329afcfe3cd178678f745d67b6ef468a7fe5baf04ee4196da7101ae7df6d2bc5e6ef1f325b794878a1279c6bf4d849cce5cb8f5251c073125ecee10963dfa2f13bba378c4695d66339997c821408c9ddbac6a3d8b7eb2706cdaad8b95d9eace420b2f4dd96f0224d55cbc14bdd45d195cb8ba7ba15c26787f6fce68bc67cb447c8295c6bb2b87ff69468b912995b709bee449a4cf34fb6197e71e44ba5f888f5cf320bf6622df283622ec19de7c3df3ad1d86ae9f4ac79362d3c2856697ca8f16112531e5e46633abbaf5a78c3f7f3cedc72352f0617d079b0c8090a04bf9770d5863916a87a1b11655fb9ded7fb87867d4cda7f261db51c5447b0a7ed0297fb88e59ff7cbf2e3e4dc3bc779dcafa35a5000daae525ddf3ade7164873d78e6d699195ec00e22fa27a15b42c599878ab078183386517211b8aa041f7d16ba64912d08cbe8a0a7a6ccb6f8052068f53d8aede438f44042f57a3934e2acef44e8f881ec235be80c967124bc84e84efafdb44a84af20d85789444a640e9fdff3a16e243033d121428dba10ea6e26fd97099fc479752a48925472e3c061de07171d91334eeb01e212c2fa0cd692fdb87052a139e3de7811a54a10805d7a0a4332ac8dfcc65b876559f62920be9c43e1fba705e59cab50c6d5d79e2bc07dc06780a685cf50136beb17da0b087f25138ac5bc4f711096152d4b9e28908cf753895dd4ba9cc8b8112898ec3366c23f4ef1781aa74d351ec6a3245e15bd8bae49b1694bc6f4eb98f0afb1a2f49291b6a792479de866508a78a4f0cf167869e8bdf6eaeb809f96a3896fb3060413fc4f918cac80726cdd4cf5e68efb284576c729d259d03c5ba03d8baf672a99d53e634544f917aadb834f3f42a0ea3951678cf0d96f912e4f8eb5790f10396743940603c03d106dc669f401ebde103801f6b22d0bc5ac52d656de31208db111a87bab7085d73e861a040517cd50c406b43230213c5fb37c5709f18243ac2d037ae6b55ba0ccaa55f9d6e5162fc8345db052143b65337949b2b2000ad4f18f9932b84c6067d9294cec6c4ff7c71b9191dd969db0084b56cb760f86b56fb1fab08e42ee466cb9bfb48a10c4ca26a85bdc4262841e242c742dcef755463a951a254e9ee2dd190ae439aa9e9967c0cd4abafc1214318b46318d09d858b1c0bb5969b4950d3bdcb8428ff5fcdae41bb133f9f35d54a10fc585569b3f12d3430e342c8f21e8314fbf33e589e5fd14e40f0e184b7e5e58c251f70798b641975f9a26a1e0caa171edac954a3db7e8f632cd4fe9d259848d60c7dd86004f9796069abe8c5359d84b9444286b5f6d25e67383c6a1abcb66a633a96f76e1c064b1ed7aa238481539d99676c9fae4ae1074e3dc446bb198047b61e5913c3229e770712e68fac6e5be1cba78d71b09a6976890f844cc8c9a159ed706aab6a008c1996bf3f7e17d64ef25d4a992e64f114f503f6363cb0766f64b0a917cc86dd441613d3bf4b7858e7e837311a1a2d131e30618da26a64b4c4548b5ea467e1408c2354a16e8f0628a6513270be66c49ed1e4e132ac5ff56149f21b2c75971f5b7cde0c66692f02637945512c0c882de928307bd196703e946f818316ccc523d581dcd938c49e52f3221f74b89b6d68e60dcfb319575a63f0333296e775b68e268e6820809694b6682bb43584eada2a111a19828eb29e7ff927d80d7a9a7457e01a090b36805dfc21a4ca5c1cc6fac892231547b6b64f9f833a23318d26c2bf1e3bf766bbdba0375a1be71f6e363c742424425aa5d8ebb95d8a4419c4ee554d852603d3f35aad05d2997445df3766aac1571e907b80015a8ec5b3d6941452c41a93f563072c9a5a9b15008fd0ceefc56a874d86ed040c3f0b6dbaae17bb9ed33e2f7a8f31916822c9c35c1ccac8b22e9cff0e1bf50d4df472bfae56bc1d92e1a1565e21fbadd6254309f92905317f75c044c7587efdefaa29d3c62e2f3708fe3e642bc6e7db79e7bf75a340ee6ca0b3d712b683a3804ae75528885d4d0294058f6647403c115fd8b9a77cf11ae4faffe35693ed4b5c6dd895e33e5610aed9320f33a0919562ae0d3ffed9ea211089901cb9a36b02532d734a32a16b907e7917cca248ba8135e3cb718083f5ed5f8900025522238beb309734dc531c082ba2fb602e69118b8bc187d7f9472ae",
)];

pub const MAINNET_EMERGENCY_CHECKPOINTS: &[(u64, &str)] = &[
    (
        MAINNET_EMERGENCY_CHECKPOINT_HEIGHT,
        MAINNET_EMERGENCY_CHECKPOINT_HASH,
    ),
    (
        MAINNET_RECOVERY_CHECKPOINT_297_HEIGHT,
        MAINNET_RECOVERY_CHECKPOINT_297_HASH,
    ),
    (
        MAINNET_RECOVERY_CHECKPOINT_298_HEIGHT,
        MAINNET_RECOVERY_CHECKPOINT_298_HASH,
    ),
    (
        MAINNET_UPGRADE_CHECKPOINT_1460_HEIGHT,
        MAINNET_UPGRADE_CHECKPOINT_1460_HASH,
    ),
    (
        MAINNET_UPGRADE_CHECKPOINT_3127_HEIGHT,
        MAINNET_UPGRADE_CHECKPOINT_3127_HASH,
    ),
    (
        MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT,
        MAINNET_RECOVERY_CHECKPOINT_10887_HASH,
    ),
    (
        MAINNET_RECOVERY_CHECKPOINT_14500_HEIGHT,
        MAINNET_RECOVERY_CHECKPOINT_14500_HASH,
    ),
];
/// Temporary one-miner recovery window after the 1.4.x checkpoint. The cap is
/// only applied when the local canonical chain contains the checkpoint above.
pub const MAINNET_EMERGENCY_DIFFICULTY_CAP_FROM_HEIGHT: u64 =
    MAINNET_EMERGENCY_CHECKPOINT_HEIGHT + 1;
pub const MAINNET_EMERGENCY_DIFFICULTY_CAP_UNTIL_HEIGHT: u64 = 360;
pub const MAINNET_EMERGENCY_DIFFICULTY_MAX_BITS: u32 = 14;

/// Serverless difficulty reset (1.7.5). The per-block retarget only moves ±1
/// bit, so after the hashrate collapsed to a single home miner it left the
/// network stuck at difficulty 23 (~3.4h/block). Gated on the 3127 checkpoint,
/// this window drops difficulty to a sane 16 bits (~2 min/block at ~700 H/s)
/// from 3128 onward so the chain can move again; the normal retarget still
/// operates *below* the cap, and floats freely again after the window.
pub const MAINNET_RESET_DIFFICULTY_CAP_FROM_HEIGHT: u64 =
    MAINNET_UPGRADE_CHECKPOINT_3127_HEIGHT + 1;
pub const MAINNET_RESET_DIFFICULTY_CAP_UNTIL_HEIGHT: u64 = 10_000;
pub const MAINNET_RESET_DIFFICULTY_MAX_BITS: u32 = 16;

pub const ZKA_DECIMALS: u32 = 6;
pub const ATOMS_PER_ZKA: u64 = 1_000_000;
pub const MAX_SUPPLY_ZKA: u64 = 23_000_000;
pub const INITIAL_BLOCK_REWARD_ZKA: u64 = 12;
pub const MAX_SUPPLY_ATOMS: u64 = MAX_SUPPLY_ZKA * ATOMS_PER_ZKA;
pub const INITIAL_BLOCK_REWARD_ATOMS: u64 = INITIAL_BLOCK_REWARD_ZKA * ATOMS_PER_ZKA;
pub const HALVING_INTERVAL_BLOCKS: u64 = 1_051_200;
pub const TARGET_BLOCK_TIME_SECS: u64 = 120;
/// Coinbase outputs require 60 confirmations before they can be spent.
/// At 120s/block this is ~2 hours, protecting the network against short reorgs
/// invalidating reward payments.
pub const COINBASE_MATURITY_BLOCKS: u64 = 60;

/// Consensus cap for private transaction hash commitments in one block.
/// Wallets can split large sends across multiple private transactions, but a
/// block must keep a bounded commitment list so validation remains predictable.
pub const PRIVATE_TX_HASHES_PER_BLOCK_LIMIT: usize = 128;
/// Canonical private bodies remain disabled until a reviewed fork height is
/// fixed in release binaries. `None` keeps the historical hash-only format.
///
/// The on-device prover build (`mobile` feature) activates from genesis so the
/// staging app can exercise the compact v2-i1 profile against the test nodes
/// (which already run with these activations enabled). This ONLY affects the
/// phone's local gate — node/server binaries never enable `mobile`, so their
/// consensus default is the non-mobile constant below (env-overridable).
///
/// Zenith (testnet). La 2.1.0 la había fijado en 30.284 (una semana de margen
/// para que actualizaran otros nodos); la 2.1.2, el 14/10/2026, la adelantó a
/// 25.570, la punta de ese día (25.448) más unas seis horas de bloques: no había
/// otros nodos en la red y la dificultad trabada en 17 bits volvía la semana
/// once días. Ver `docs/RELEASE-2.1.0-testnet.md`.
#[cfg(not(feature = "mobile"))]
pub const PRIVATE_TX_CANONICAL_BUNDLE_ACTIVATION_HEIGHT: Option<u64> = Some(25_570);
#[cfg(feature = "mobile")]
pub const PRIVATE_TX_CANONICAL_BUNDLE_ACTIVATION_HEIGHT: Option<u64> = Some(0);
/// Compact mobile Groth16 profile remains disabled until the final i1 CRS,
/// adversarial vectors and coordinated rollout height are fixed.
#[cfg(not(feature = "mobile"))]
pub const PRIVATE_TX_V2_I1_ACTIVATION_HEIGHT: Option<u64> = None;
#[cfg(feature = "mobile")]
pub const PRIVATE_TX_V2_I1_ACTIVATION_HEIGHT: Option<u64> = Some(0);
/// Defense-in-depth limits for the future canonical private body bundle.
pub const PRIVATE_TX_CANONICAL_BODY_BYTES_LIMIT: usize = 1024 * 1024;
pub const PRIVATE_TX_CANONICAL_BUNDLE_BYTES_LIMIT: usize = 8 * 1024 * 1024;

pub fn private_tx_canonical_bundle_activation_height() -> Option<u64> {
    activation_height_from_env(
        "ZOKA_PRIVATE_TX_CANONICAL_BUNDLE_ACTIVATION_HEIGHT",
        PRIVATE_TX_CANONICAL_BUNDLE_ACTIVATION_HEIGHT,
    )
}

pub fn private_tx_v2_i1_activation_height() -> Option<u64> {
    activation_height_from_env(
        "ZOKA_PRIVATE_TX_V2_I1_ACTIVATION_HEIGHT",
        PRIVATE_TX_V2_I1_ACTIVATION_HEIGHT,
    )
}

fn activation_height_from_env(var: &str, default: Option<u64>) -> Option<u64> {
    let Ok(raw) = std::env::var(var) else {
        return default;
    };
    let raw = raw.trim();
    if raw.is_empty()
        || raw.eq_ignore_ascii_case("none")
        || raw.eq_ignore_ascii_case("null")
        || raw.eq_ignore_ascii_case("off")
    {
        return None;
    }
    Some(
        raw.parse::<u64>()
            .unwrap_or_else(|_| panic!("{var} must be an unsigned block height or 'none'")),
    )
}

pub fn private_tx_canonical_bundle_required(height: u64) -> bool {
    private_tx_canonical_bundle_required_at(height, private_tx_canonical_bundle_activation_height())
}

pub fn private_tx_canonical_bundle_required_at(
    height: u64,
    activation_height: Option<u64>,
) -> bool {
    activation_height.is_some_and(|activation| height >= activation)
}

pub fn private_tx_v2_i1_active(height: u64) -> bool {
    private_tx_v2_i1_active_at(
        height,
        private_tx_v2_i1_activation_height(),
        private_tx_canonical_bundle_activation_height(),
    )
}

pub fn private_tx_v2_i1_active_at(
    height: u64,
    activation_height: Option<u64>,
    canonical_bundle_activation_height: Option<u64>,
) -> bool {
    activation_height.is_some_and(|activation| height >= activation)
        && private_tx_canonical_bundle_required_at(height, canonical_bundle_activation_height)
}

/// Funciones post-cuanticas de Zoka Quantic que se prueban primero en Zenith.
///
/// Ninguna cambia el consenso de Zenith sin una altura de activacion fijada en
/// los binarios. Hoy todas estan en `None` (tambien en la build `mobile`): la
/// cadena valida exactamente como antes. La variable de entorno de cada una
/// existe, como en las activaciones de arriba, para ensayar en redes locales;
/// un nodo de la red publica que la use se separa de los demas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PqFeature {
    /// Direcciones y salidas con encapsulado hibrido X-Wing (etapa 3).
    HybridAddresses,
    /// Gastos probados en un circuito, sin key images ni CLSAG (etapa 5).
    SpendCircuitWithoutKeyImages,
    /// Snapshots con firma Ed25519 + ML-DSA-65, las dos obligatorias (etapa 6).
    HybridSnapshotSignatures,
}

/// Zenith (testnet), versión 2.1.2: 60 bloques (unas tres horas) después de los
/// bundles canónicos, para verlos andando antes de sumar la v2. La 2.1.0 la
/// había fijado en 31.004.
pub const PQ_HYBRID_ADDRESSES_ACTIVATION_HEIGHT: Option<u64> = Some(25_630);
pub const PQ_SPEND_CIRCUIT_ACTIVATION_HEIGHT: Option<u64> = None;
pub const PQ_HYBRID_SNAPSHOT_SIGNATURES_ACTIVATION_HEIGHT: Option<u64> = None;

impl PqFeature {
    pub const ALL: [PqFeature; 3] = [
        PqFeature::HybridAddresses,
        PqFeature::SpendCircuitWithoutKeyImages,
        PqFeature::HybridSnapshotSignatures,
    ];

    /// Identificador estable para logs, diagnosticos y documentacion.
    pub fn id(self) -> &'static str {
        match self {
            PqFeature::HybridAddresses => "pq-hybrid-addresses",
            PqFeature::SpendCircuitWithoutKeyImages => "pq-spend-circuit",
            PqFeature::HybridSnapshotSignatures => "pq-hybrid-snapshot-signatures",
        }
    }

    pub fn activation_env_var(self) -> &'static str {
        match self {
            PqFeature::HybridAddresses => "ZOKA_PQ_HYBRID_ADDRESSES_ACTIVATION_HEIGHT",
            PqFeature::SpendCircuitWithoutKeyImages => "ZOKA_PQ_SPEND_CIRCUIT_ACTIVATION_HEIGHT",
            PqFeature::HybridSnapshotSignatures => {
                "ZOKA_PQ_HYBRID_SNAPSHOT_SIGNATURES_ACTIVATION_HEIGHT"
            }
        }
    }

    pub fn compiled_activation_height(self) -> Option<u64> {
        match self {
            PqFeature::HybridAddresses => PQ_HYBRID_ADDRESSES_ACTIVATION_HEIGHT,
            PqFeature::SpendCircuitWithoutKeyImages => PQ_SPEND_CIRCUIT_ACTIVATION_HEIGHT,
            PqFeature::HybridSnapshotSignatures => PQ_HYBRID_SNAPSHOT_SIGNATURES_ACTIVATION_HEIGHT,
        }
    }
}

pub fn pq_feature_activation_height(feature: PqFeature) -> Option<u64> {
    activation_height_from_env(
        feature.activation_env_var(),
        feature.compiled_activation_height(),
    )
}

pub fn pq_feature_active(feature: PqFeature, height: u64) -> bool {
    pq_feature_active_at(height, pq_feature_activation_height(feature))
}

pub fn pq_feature_active_at(height: u64, activation_height: Option<u64>) -> bool {
    activation_height.is_some_and(|activation| height >= activation)
}

/// Altura desde la que valen las salidas v2 (`PqFeature::HybridAddresses`).
///
/// Las reglas de la salida v2 son reglas sobre los cuerpos de las
/// transacciones, y los cuerpos solo son datos de consenso con los bundles
/// canónicos: antes de esa altura los bloques comprometen hashes y
/// nulificadores y ningún nodo valida los cuerpos (medido el 12/10 en una red
/// local: un nodo sin la activación aceptó el bloque con la salida v2). Por
/// eso, como `private_tx_v2_i1_active_at`, hacen falta las dos alturas.
pub fn pq_hybrid_outputs_activation_height_from(
    hybrid_activation: Option<u64>,
    canonical_bundle_activation: Option<u64>,
) -> Option<u64> {
    Some(hybrid_activation?.max(canonical_bundle_activation?))
}

pub fn pq_hybrid_outputs_activation_height() -> Option<u64> {
    pq_hybrid_outputs_activation_height_from(
        pq_feature_activation_height(PqFeature::HybridAddresses),
        private_tx_canonical_bundle_activation_height(),
    )
}

pub fn pq_hybrid_outputs_active_at(
    height: u64,
    hybrid_activation: Option<u64>,
    canonical_bundle_activation: Option<u64>,
) -> bool {
    pq_hybrid_outputs_activation_height_from(hybrid_activation, canonical_bundle_activation)
        .is_some_and(|activation| height >= activation)
}

/// Minimum difficulty the per-block retarget can fall to. Keeps the chain
/// viable even with a single low-end miner.
pub const POW_MIN_DIFFICULTY_BITS: u32 = 12;
/// Starting difficulty for the first two blocks (warmup band before two
/// committed timestamps are available for retarget).
pub const POW_INITIAL_DIFFICULTY_BITS: u32 = 12;
/// Hard ceiling for the per-block retarget. 30 bits at RandomX is
/// comfortably achievable by GPU/ASIC-resistant CPUs without freezing
/// the chain.
pub const POW_MAX_DIFFICULTY_BITS: u32 = 30;
pub const POW_KEY_EPOCH_BLOCKS: u64 = 2_048;

/// Canonical public-network PoW algorithm. See `docs/protocol-overview.md`.
/// Mainnet binaries MUST be built with `--features randomx`; nodes that
/// claim to be mainnet but produce blocks tagged with another algorithm id
/// are invalid and must be rejected by other nodes.
pub const POW_ALGORITHM_MAINNET: &str = "zoka-randomx-v1";

/// CPU fallback used by dev / testnet / CI. Not valid in mainnet.
pub const POW_ALGORITHM_TESTNET: &str = "zoka-cpu-fallback-v1";

/// Returns the canonical PoW algorithm id for a given chain id. Used by
/// `mining::pow::engine_for` and by block validation.
/// Zoka Quantic (etapa 5): cadenas nuevas, con Ironwood y X-Wing desde su
/// primer bloque. Los nombres son los de `crates/zoka-quantic` (`Network`).
pub const QUANTIC_MAINNET_CHAIN_ID: &str = "zoka-quantic-v1";
pub const QUANTIC_TESTNET_CHAIN_ID: &str = "zoka-quantic-testnet-v1";
/// Redes locales de prueba de Quantic: prueba de trabajo liviana.
pub const QUANTIC_REGTEST_CHAIN_ID: &str = "zoka-quantic-regtest-v1";

/// El génesis de una red pública de Quantic: el hash de su bloque 1 y la
/// dirección que recibe el premine. Fijo en el binario, como el de Bitcoin:
/// otro bloque 1, aunque pague lo mismo, no es esa red.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuanticGenesis {
    pub block_hash: &'static str,
    pub premine_address: &'static str,
}

/// Creado el 2026-10-22 11:16 UTC con `zoka quantic-genesis`: el premine va a la
/// wallet del equipo de la testnet (`zoka quantic-wallet new --network
/// quantic-testnet`).
pub const QUANTIC_TESTNET_GENESIS: Option<QuanticGenesis> = Some(QuanticGenesis {
    block_hash: "1d204e8116f53d74302db20b943b288d141681a9be463f65a43861dd47c2d242",
    premine_address: "zqtest12LZq3Xm9eSyVqBTizvUZSHdgLkTDTfVyWczUg43hHHQMPrq8zb81QWm3jGmQ5drsVKxpj773FJWvbVsA9nQNmur29cAiC1sQXAYLpgC4D6wC5VFSn6uysKHL2LQy48cFiTguRwwCqKrXmqFCTk9rZB3NAiq2Jwt43cz5U4gBs6ayveaMLNdXR5NckCsBPkx7kk3sRAX4bMKfC6eLKiz4qexyhbZAEY66aMBW46BwtEQR1MouNK36MLYDVeoxNpvKDzZmt7iZCSDKGsHGC4zvTjRHwKwUtG29gA94n44UAS8ZyRg3GEZ57X81eNxfjb38CMEE2sErwAmq2jvsso4CpEnq9RjMJDunNinziZeU5M9QfDA5g4dYVPoQNxgRP4FC8kUjh6wrCBqZp5BrdCKVwTCxJeko32qDgH21VxHApFpmPo9tm6V6MjaWSbAiNHzXQNEs5evGK3hNm2ry5MC7HYZtdo51J81WgoJ5Uu3vFaDu4FiHmFHQ918SdAu8t3mB9qe7FcjDeEBF9eZ45utBjk8f7jRfVx46guu2NuBKwssr3owvN1ejV8Kzn8g8NRe568z23x4gd7C4tyoWzBYefiaM4Pm9ZRw9CibPmYg4K5c2X4KfAiUm4jAHthAgsK2p8RbN4zhzdQbbNaMd3NERHEX99DRPJHbrHqXSUJWnY8tpgXYZoVmyvve5SMXD2Hd7P3JvsqGPHgfCydv7zMxQWddZjwT33kqB9gR8YwHNaBZb2qLPJ2UokJTGjyRAdypxhWw3SW9m3ntc7k8xr5ULxsP66jvBLB2157X9SyBqAKj8MSvMFkqYfhm7MXjX11cc2Gubg12jG5d9vNszSkpXD2LayXzRQdsz8NJLiKR6Jsv4sviszsf8h5bNgASmNTZ8kSz5JpLwovKSkAH6mQctyjPr1k5jxeqxQ5hLsnQVexiHcn5bKJofcpp1Key6xNUd5dCqXyLEfTyQaKAjkZb81essrw35CavvPExK4a5NJ87vFjApaiCRFtV76FKQsFNwYMEgyUM7ZMQhXfimeBRr53LomiHJBGP3xH6QTQPJepBSbaLdNpc6Jgs1ZmnWf7N9QwP49csfe9yQVsJnakovyXUCVG5eYkJbhhhoJDXNJ1NispwbtsXNptcnTWmZBR78C8vWdGiqRpwnaoCuSKMrNjWfF2P7js2MSb7uiYhLZvAz9ckcGsTgaxitq5cxSyyvdux11zFHBYpqkGGgHnk5CKiSbLSvtTJic8ZZaHknGAhABbn6auaL5eCRTLD5AkGQGfkzNrBnCZRfzg7rMQ9NsZLUAHCfzhJS9BqYPXbNTNZ2QoAnA7VaWrmVsKm8Tw9xBeahD3y54hxFu5DD1qLechjhVP9jegyA3bisKDs3G1jhnRWMzynCq3PJue42KsjTFvAHSyifY8tKwzcwyppPGm6eHLyrbxzQYyHTXAqyDn9wd1XXPg4xyKhfpSR9Ep8jA5jzMUN7E8Fg4cg79vK7L7fk4mAmQr3SSDXETuxB7bLTQMmzXYoUerNXLWY3TDd8EeUYEUreddkGat99jceNAAqXNSahnCB52TTYN8MBfYHtKWryb8rFB95iGrij3pfQV6N3wiuVFQDxLxGiVp4dnB2FgEkeaULGfz9k1a2fWMp7LPfEcTGs6j2VMhy396myrG16f5kyh4Zdiv3Sx4dpdkTWRdjEM46k9EFEZoPvMzKPFoUTxvXdcBm3YmHRpa5",
});
/// Mainnet: se crea en la etapa 8.
pub const QUANTIC_MAINNET_GENESIS: Option<QuanticGenesis> = None;

/// Dificultad fija de las redes locales de Quantic (como el regtest de
/// Bitcoin): sin reajuste, para que una red de prueba en una PC produzca
/// bloques al ritmo que se le pide. Nunca vale en la testnet ni en mainnet.
pub const QUANTIC_REGTEST_DIFFICULTY_BITS: u32 = 8;

pub fn is_quantic_chain_id(chain_id: &str) -> bool {
    chain_id == QUANTIC_MAINNET_CHAIN_ID
        || chain_id == QUANTIC_TESTNET_CHAIN_ID
        || chain_id == QUANTIC_REGTEST_CHAIN_ID
}

/// Las cadenas públicas usan RandomX: Zenith y las dos de Quantic.
pub fn chain_uses_randomx(chain_id: &str) -> bool {
    chain_id == MAINNET_CHAIN_ID
        || chain_id == TESTNET_CHAIN_ID
        || chain_id == QUANTIC_MAINNET_CHAIN_ID
        || chain_id == QUANTIC_TESTNET_CHAIN_ID
}

pub fn pow_algorithm_for_chain(chain_id: &str) -> &'static str {
    if chain_uses_randomx(chain_id) {
        POW_ALGORITHM_MAINNET
    } else {
        POW_ALGORITHM_TESTNET
    }
}

pub fn zka_to_atoms(zka: u64) -> Option<u64> {
    zka.checked_mul(ATOMS_PER_ZKA)
}

pub fn parse_zka_amount(raw: &str) -> Result<u64, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("amount cannot be empty".to_string());
    }
    if raw.starts_with('-') {
        return Err("amount cannot be negative".to_string());
    }
    let parts = raw.split('.').collect::<Vec<_>>();
    if parts.len() > 2 {
        return Err("amount must contain at most one decimal point".to_string());
    }
    let whole = if parts[0].is_empty() {
        0
    } else {
        parts[0]
            .parse::<u64>()
            .map_err(|_| format!("invalid whole-ZKA amount: {}", parts[0]))?
    };
    let whole_atoms = zka_to_atoms(whole).ok_or_else(|| "amount overflows u64".to_string())?;
    let fractional_atoms = if parts.len() == 2 {
        let frac = parts[1];
        if frac.len() > ZKA_DECIMALS as usize {
            return Err(format!(
                "amount has more than {ZKA_DECIMALS} decimal places"
            ));
        }
        let mut padded = frac.to_string();
        while padded.len() < ZKA_DECIMALS as usize {
            padded.push('0');
        }
        if padded.is_empty() {
            0
        } else {
            padded
                .parse::<u64>()
                .map_err(|_| format!("invalid fractional-ZKA amount: {frac}"))?
        }
    } else {
        0
    };
    whole_atoms
        .checked_add(fractional_atoms)
        .ok_or_else(|| "amount overflows u64".to_string())
}

pub fn format_zka_amount(atoms: u64) -> String {
    let whole = atoms / ATOMS_PER_ZKA;
    let frac = atoms % ATOMS_PER_ZKA;
    if frac == 0 {
        return whole.to_string();
    }
    let mut frac_text = format!("{frac:0width$}", width = ZKA_DECIMALS as usize);
    while frac_text.ends_with('0') {
        frac_text.pop();
    }
    format!("{whole}.{frac_text}")
}

/// Atomic-unit block subsidy before cap clipping.
pub fn halving_reward_for_height(height: u64) -> u64 {
    let era = height / HALVING_INTERVAL_BLOCKS;
    if era >= u64::BITS as u64 {
        0
    } else {
        (INITIAL_BLOCK_REWARD_ZKA >> era).saturating_mul(ATOMS_PER_ZKA)
    }
}

/// Final block subsidy after applying the hard cap.
pub fn block_reward_for_height(height: u64, total_supply_emitted: u64) -> u64 {
    let remaining = MAX_SUPPLY_ATOMS.saturating_sub(total_supply_emitted);
    if remaining == 0 {
        return 0;
    }
    halving_reward_for_height(height).min(remaining)
}

pub fn halving_blocks_remaining(height: u64) -> u64 {
    let offset = height % HALVING_INTERVAL_BLOCKS;
    if offset == 0 {
        HALVING_INTERVAL_BLOCKS
    } else {
        HALVING_INTERVAL_BLOCKS - offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emission_v1_halving_schedule_is_stable() {
        assert_eq!(halving_reward_for_height(0), 12 * ATOMS_PER_ZKA);
        assert_eq!(
            halving_reward_for_height(HALVING_INTERVAL_BLOCKS - 1),
            12 * ATOMS_PER_ZKA
        );
        assert_eq!(
            halving_reward_for_height(HALVING_INTERVAL_BLOCKS),
            6 * ATOMS_PER_ZKA
        );
        assert_eq!(
            halving_reward_for_height(HALVING_INTERVAL_BLOCKS * 2),
            3 * ATOMS_PER_ZKA
        );
        assert_eq!(
            halving_reward_for_height(HALVING_INTERVAL_BLOCKS * 3),
            ATOMS_PER_ZKA
        );
        assert_eq!(halving_reward_for_height(HALVING_INTERVAL_BLOCKS * 4), 0);
    }

    #[test]
    fn emission_v1_never_exceeds_hard_cap() {
        assert_eq!(block_reward_for_height(0, 0), 12 * ATOMS_PER_ZKA);
        assert_eq!(block_reward_for_height(0, MAX_SUPPLY_ATOMS - 5), 5);
        assert_eq!(block_reward_for_height(0, MAX_SUPPLY_ATOMS), 0);
        assert_eq!(block_reward_for_height(0, MAX_SUPPLY_ATOMS + 1), 0);
    }

    #[test]
    fn emission_v1_maturity_and_timing_are_frozen() {
        assert_eq!(TOKEN_SYMBOL, "ZKA");
        assert_eq!(ATOMS_PER_ZKA, 1_000_000);
        assert_eq!(MAX_SUPPLY_ZKA, 23_000_000);
        assert_eq!(MAX_SUPPLY_ATOMS, 23_000_000_000_000);
        assert_eq!(INITIAL_BLOCK_REWARD_ZKA, 12);
        assert_eq!(INITIAL_BLOCK_REWARD_ATOMS, 12_000_000);
        assert_eq!(HALVING_INTERVAL_BLOCKS, 1_051_200);
        assert_eq!(TARGET_BLOCK_TIME_SECS, 120);
        assert_eq!(COINBASE_MATURITY_BLOCKS, 60);
    }

    /// El ancla nueva tiene que estar en la tabla, no solo declarada: la
    /// constante suelta no la usa nadie, y un pin que no se valida no protege
    /// de nada. Es exactamente el error que dejaria la bifurcacion sin cerrar
    /// mientras el codigo aparenta lo contrario.
    #[test]
    fn the_14500_anchor_is_actually_enforced() {
        assert_eq!(MAINNET_RECOVERY_CHECKPOINT_14500_HEIGHT, 14500);
        assert_eq!(
            MAINNET_RECOVERY_CHECKPOINT_14500_HASH,
            "5301997c287e9a14f24700b65d568e915c59cd83b9d88c6cbcd994e22eed010f"
        );
        assert!(MAINNET_EMERGENCY_CHECKPOINTS.contains(&(
            MAINNET_RECOVERY_CHECKPOINT_14500_HEIGHT,
            MAINNET_RECOVERY_CHECKPOINT_14500_HASH,
        )));
    }

    /// Las anclas se recorren en orden y cada una corta ramas por encima de la
    /// anterior. Fuera de orden, una altura menor se evaluaria despues de una
    /// mayor y el razonamiento sobre que rama sobrevive deja de valer.
    #[test]
    fn anchors_stay_in_ascending_height_order() {
        let alturas: Vec<u64> = MAINNET_EMERGENCY_CHECKPOINTS.iter().map(|(h, _)| *h).collect();
        let mut ordenadas = alturas.clone();
        ordenadas.sort_unstable();
        assert_eq!(alturas, ordenadas, "anclas fuera de orden: {alturas:?}");
    }

    #[test]
    fn mainnet_1_9_recovery_anchor_and_release_gates_are_frozen() {
        assert_eq!(MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT, 10_887);
        assert_eq!(
            MAINNET_RECOVERY_CHECKPOINT_10887_HASH,
            "d30c20100f6405424d32e5c204799bc10a1197ff5cdc1fe0b04c2c538b11086e"
        );
        assert!(MAINNET_EMERGENCY_CHECKPOINTS.contains(&(
            MAINNET_RECOVERY_CHECKPOINT_10887_HEIGHT,
            MAINNET_RECOVERY_CHECKPOINT_10887_HASH,
        )));
        assert_eq!(MAINNET_RECOVERY_MIN_CONFIRMATIONS, 120);
        assert_eq!(MAINNET_RECOVERY_MIN_RELEASE_HEAD_HEIGHT, 11_007);
        assert_eq!(MAINNET_RECOVERY_MIN_INDEPENDENT_OBSERVATIONS, 2);
        assert!(MAINNET_RECOVERY_REQUIRE_RELEASE_ARTIFACT_SHA256);
        assert!(!MAINNET_RECOVERY_AUTOMATIC_DB_DELETION);
        assert_eq!(MAINNET_RECOVERY_MISMATCH_ACTION, "quarantine");

        assert!(MAINNET_EMERGENCY_CHECKPOINTS
            .windows(2)
            .all(|pair| pair[0].0 < pair[1].0));
        assert!(MAINNET_EMERGENCY_CHECKPOINTS
            .iter()
            .all(|(_, hash)| hash.len() == 64
                && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                && !hash.bytes().any(|byte| byte.is_ascii_uppercase())));
    }

    // Esta prueba describe la configuracion de consenso de los nodos (desde la
    // 2.1.0, con la altura de Zenith fijada). El build `mobile` enciende los
    // bundles desde genesis a proposito -- es la compuerta local del telefono,
    // documentada en economics.rs -- asi que aca no puede pasar y no deberia
    // intentarlo.
    //
    // Sin este marcado, `cargo test --features mobile` quedaba con cinco rojos
    // permanentes y dejaba de servir como senal: un fallo real se perdia entre
    // fallos esperados. Se descubrio usandolo justamente para validar un
    // protocolo nuevo.
    #[cfg(not(feature = "mobile"))]
    #[test]
    fn canonical_private_bundle_activates_at_the_zenith_height() {
        // Zenith 2.1.0: antes de la altura la cadena valida exactamente como
        // antes; desde ahí cada bloque lleva los cuerpos de sus tx privadas.
        assert_eq!(PRIVATE_TX_CANONICAL_BUNDLE_ACTIVATION_HEIGHT, Some(25_570));
        if std::env::var_os("ZOKA_PRIVATE_TX_CANONICAL_BUNDLE_ACTIVATION_HEIGHT").is_none() {
            assert!(!private_tx_canonical_bundle_required(0));
            assert!(!private_tx_canonical_bundle_required(25_569));
            assert!(private_tx_canonical_bundle_required(25_570));
        }
        assert!(!private_tx_canonical_bundle_required_at(9, Some(10)));
        assert!(private_tx_canonical_bundle_required_at(10, Some(10)));
        assert!(private_tx_canonical_bundle_required_at(11, Some(10)));
    }

    // Esta prueba describe la configuracion de consenso, donde estas
    // activaciones estan dormidas. El build `mobile` las enciende desde genesis
    // a proposito -- es la compuerta local del telefono, documentada en
    // economics.rs -- asi que aca no puede pasar y no deberia intentarlo.
    //
    // Sin este marcado, `cargo test --features mobile` quedaba con cinco rojos
    // permanentes y dejaba de servir como senal: un fallo real se perdia entre
    // fallos esperados. Se descubrio usandolo justamente para validar un
    // protocolo nuevo.
    #[cfg(not(feature = "mobile"))]
    #[test]
    fn compact_private_tx_profile_stays_dormant_until_height_is_fixed() {
        assert_eq!(PRIVATE_TX_V2_I1_ACTIVATION_HEIGHT, None);
        assert!(!private_tx_v2_i1_active(0));
        assert!(!private_tx_v2_i1_active(u64::MAX));
        assert!(!private_tx_v2_i1_active_at(9, Some(10), Some(10)));
        assert!(private_tx_v2_i1_active_at(10, Some(10), Some(10)));
        assert!(private_tx_v2_i1_active_at(11, Some(10), Some(10)));
        assert!(!private_tx_v2_i1_active_at(10, Some(10), None));
    }

    #[test]
    fn activation_height_env_override_is_explicit() {
        const VAR: &str = "ZOKA_TEST_ONLY_PRIVATE_TX_ACTIVATION_HEIGHT";
        std::env::remove_var(VAR);
        assert_eq!(activation_height_from_env(VAR, None), None);
        assert_eq!(activation_height_from_env(VAR, Some(12)), Some(12));

        std::env::set_var(VAR, "42");
        assert_eq!(activation_height_from_env(VAR, None), Some(42));

        std::env::set_var(VAR, "none");
        assert_eq!(activation_height_from_env(VAR, Some(42)), None);

        std::env::remove_var(VAR);
    }

    // Incluir las primitivas post-cuanticas no puede cambiar Zenith: ninguna
    // funcion tiene altura hasta que se acuerde y se fije en los binarios
    // (la 2.1.0 fijo la de las direcciones hibridas). Vale tambien para la
    // build `mobile`, a diferencia de v2-i1.
    #[test]
    fn pq_features_activate_only_where_a_height_is_fixed() {
        // Zenith 2.1.0 fija solo la de las direcciones híbridas; las demás
        // siguen dormidas hasta que se acuerde la suya.
        assert_eq!(
            PqFeature::HybridAddresses.compiled_activation_height(),
            Some(25_630)
        );
        for feature in [
            PqFeature::SpendCircuitWithoutKeyImages,
            PqFeature::HybridSnapshotSignatures,
        ] {
            assert_eq!(
                feature.compiled_activation_height(),
                None,
                "{}",
                feature.id()
            );
            if std::env::var_os(feature.activation_env_var()).is_none() {
                assert!(!pq_feature_active(feature, 0), "{}", feature.id());
                assert!(!pq_feature_active(feature, u64::MAX), "{}", feature.id());
            }
        }
        assert!(!pq_feature_active_at(u64::MAX, None));
        assert!(!pq_feature_active_at(9, Some(10)));
        assert!(pq_feature_active_at(10, Some(10)));
        assert!(pq_feature_active_at(11, Some(10)));
    }

    #[test]
    fn pq_feature_ids_and_env_vars_are_unique() {
        let ids: std::collections::BTreeSet<_> = PqFeature::ALL.iter().map(|f| f.id()).collect();
        let vars: std::collections::BTreeSet<_> = PqFeature::ALL
            .iter()
            .map(|f| f.activation_env_var())
            .collect();
        assert_eq!(ids.len(), PqFeature::ALL.len());
        assert_eq!(vars.len(), PqFeature::ALL.len());
        assert!(vars.iter().all(|v| v.starts_with("ZOKA_PQ_")));
    }

    // Una salida v2 es una regla sobre cuerpos: sin bundles canónicos ningún
    // bloque la valida, así que hacen falta las dos alturas y manda la mayor.
    #[test]
    fn hybrid_outputs_need_canonical_bundles_too() {
        assert_eq!(pq_hybrid_outputs_activation_height_from(None, None), None);
        assert_eq!(
            pq_hybrid_outputs_activation_height_from(Some(80), None),
            None
        );
        assert_eq!(
            pq_hybrid_outputs_activation_height_from(None, Some(70)),
            None
        );
        assert_eq!(
            pq_hybrid_outputs_activation_height_from(Some(80), Some(70)),
            Some(80)
        );
        assert_eq!(
            pq_hybrid_outputs_activation_height_from(Some(60), Some(70)),
            Some(70)
        );
        assert!(!pq_hybrid_outputs_active_at(79, Some(80), Some(70)));
        assert!(pq_hybrid_outputs_active_at(80, Some(80), Some(70)));
        assert!(!pq_hybrid_outputs_active_at(u64::MAX, Some(80), None));
        // Zenith 2.1.0: la v2 rige desde la mayor de las dos alturas, la suya.
        // En la build `mobile` los bundles valen desde 0 y el resultado es el
        // mismo.
        if std::env::var_os(PqFeature::HybridAddresses.activation_env_var()).is_none()
            && std::env::var_os("ZOKA_PRIVATE_TX_CANONICAL_BUNDLE_ACTIVATION_HEIGHT").is_none()
        {
            assert_eq!(pq_hybrid_outputs_activation_height(), Some(25_630));
        }
    }

    // Dos redes, las de Quantic; Zenith no se presenta como ninguna de ellas.
    #[test]
    fn the_two_networks_are_quantic_and_zenith_is_shown_as_retired() {
        assert_eq!(MAINNET_CHAIN_ID, "zoka-mainnet-zenith-v1");
        assert_eq!(network_role_for_chain(MAINNET_CHAIN_ID), Some("retired"));
        assert_eq!(network_display_name_for_chain(MAINNET_CHAIN_ID), Some("Zenith (retired)"));
        assert_eq!(network_role_for_chain(QUANTIC_TESTNET_CHAIN_ID), Some("testnet"));
        assert_eq!(network_display_name_for_chain(QUANTIC_TESTNET_CHAIN_ID), Some("Zoka Quantic Testnet"));
        assert_eq!(network_role_for_chain(QUANTIC_MAINNET_CHAIN_ID), Some("mainnet"));
        assert_eq!(network_display_name_for_chain(QUANTIC_MAINNET_CHAIN_ID), Some("Zoka Quantic Mainnet"));
        assert_eq!(network_role_for_chain(TESTNET_CHAIN_ID), None);
    }

    #[test]
    fn zka_amount_parser_accepts_fractional_atoms() {
        assert_eq!(parse_zka_amount("0").unwrap(), 0);
        assert_eq!(parse_zka_amount("1").unwrap(), ATOMS_PER_ZKA);
        assert_eq!(parse_zka_amount("1.25").unwrap(), 1_250_000);
        assert_eq!(parse_zka_amount("0.000001").unwrap(), 1);
        assert!(parse_zka_amount("0.0000001").is_err());
        assert_eq!(format_zka_amount(1_250_000), "1.25");
        assert_eq!(format_zka_amount(1), "0.000001");
    }

    #[test]
    fn testnet_economics_artifact_matches_code_constants() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("docs")
            .join("economics.testnet.v1.json");
        let raw = std::fs::read_to_string(path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(json["version"], ECONOMICS_VERSION);
        assert_eq!(json["chain_id"], TESTNET_CHAIN_ID);
        assert_eq!(json["token"]["symbol"], TOKEN_SYMBOL);
        assert_eq!(json["token"]["decimals"], ZKA_DECIMALS);
        assert_eq!(json["token"]["atoms_per_zka"], ATOMS_PER_ZKA);
        assert_eq!(json["supply"]["max_supply_zka"], MAX_SUPPLY_ZKA);
        assert_eq!(json["supply"]["max_supply_atoms"], MAX_SUPPLY_ATOMS);
        assert_eq!(
            json["emission"]["initial_block_reward_zka"],
            INITIAL_BLOCK_REWARD_ZKA
        );
        assert_eq!(
            json["emission"]["initial_block_reward_atoms"],
            INITIAL_BLOCK_REWARD_ATOMS
        );
        assert_eq!(
            json["emission"]["halving_interval_blocks"],
            HALVING_INTERVAL_BLOCKS
        );
        assert_eq!(
            json["emission"]["target_block_time_seconds"],
            TARGET_BLOCK_TIME_SECS
        );
        assert_eq!(
            json["emission"]["coinbase_maturity_blocks"],
            COINBASE_MATURITY_BLOCKS
        );
        assert_eq!(json["pow"]["algorithm_v1"], POW_ALGORITHM_MAINNET);
    }

    #[test]
    fn testnet_v1_mirrors_mainnet_pow_and_economics() {
        assert_eq!(
            pow_algorithm_for_chain(TESTNET_CHAIN_ID),
            POW_ALGORITHM_MAINNET
        );
        assert_eq!(MAX_SUPPLY_ZKA, 23_000_000);
        assert_eq!(INITIAL_BLOCK_REWARD_ATOMS, 12_000_000);
        assert_eq!(COINBASE_MATURITY_BLOCKS, 60);
    }
}
