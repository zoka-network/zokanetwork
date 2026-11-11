//! Checkpoints firmados de Zoka Quantic, como los verifica el nodo.

use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::fmt;
use zoka_pq::{MlDsa65PublicKey, MlDsa65Signature, ML_DSA_65_SIGNATURE_BYTES};

/// La versión 2: una firma híbrida, la de una autoridad de una sola clave.
pub const SIGNED_CHECKPOINT_HYBRID_VERSION: u8 = 2;
/// La versión 3: k de n firmas híbridas, la de una autoridad de varias claves.
pub const SIGNED_CHECKPOINT_COSIGNED_VERSION: u8 = 3;
/// Cuántas claves puede tener una autoridad de k de n.
pub const MAX_COSIGNERS: usize = 8;
const SIGNING_DOMAIN_V2: &[u8] = b"zoka-signed-checkpoint-v2";
const SIGNING_DOMAIN_V3: &[u8] = b"zoka-signed-checkpoint-v3";

/// La cadena de la testnet v3.
pub const QUANTIC_PQ_TESTNET_CHAIN_ID: &str = "zoka-quantic-testnet-v3";
/// La testnet v2: misma autoridad, con la versión 2. Sus checkpoints reales se
/// guardan como vectores de prueba.
pub const QUANTIC_PQ_TESTNET_V2_CHAIN_ID: &str = "zoka-quantic-testnet-v2";
/// La testnet v3 es de k de n (versión 3): cuántas firmas hacen falta y las
/// claves que no son la de abajo, que es la primera. Igual que
/// `economics::CHECKPOINT_AUTHORITY_COSIGNERS` en el nodo.
pub const QUANTIC_TESTNET_THRESHOLD: usize = 1;
pub const QUANTIC_TESTNET_COSIGNERS: &[(&str, &str)] = &[];
/// Las dos mitades de la clave pública de la autoridad de las testnets de
/// Quantic (las mismas que compila el nodo en `economics.rs`).
pub const QUANTIC_TESTNET_AUTHORITY: &str = "05241771bed7063d0ad2d87ecf9e62e8982a3b06f7bf4e91825cf4b45c46a7bf";
pub const QUANTIC_TESTNET_AUTHORITY_MLDSA: &str = "41a02787fcecc9ba2d0000af97d63ab39d5237fd06618146d2a126cd96ae766bfdd960260518da02de5e2246bd754c2bb5e9627b321223b43cbe6853ba45bcfcb1762cfcd8a885373d3974f0bd472bb5359f64088308f346ebd5ebd9894d01f18d17c4b0690ca4ad7a0ef9647ec00831aafdb63c74b74fb8006e0e43bfbd8fe81cb60ab535f2087183d90a4af518266eac96e7a6c66c954947c75494e770002e8245d3abc9baca8956e56923ae83706b9a9ba93fc033b7167b911687640ec736d3247e1875f692b6dfbce7451a2caaf153369a41066dc32eaeb618a7de70cd5bc2732900aa0563686682847d26ccaf73b21dfa0ccd0b9b93cb0f6feebd2ed7ed72f187a25169345ee5f8bce10aa4ab6dc9356a4439dc20497d5cc4b46e55110471f246c0872f714b13fc7e17929614b94acd04894f5c660347ae4c6e3bec3d18b2ed95e67792014598cc9dcdf211cbfa67cb7262a167e644960f67f4659d97eed9d7b36f6ebd9a233382cd4ccc2b50b58d8141326520b47b5cbb8f25bd3bff7cdeea43eaad9b2d32eb15c30eea8e66f82597c7329afcfe3cd178678f745d67b6ef468a7fe5baf04ee4196da7101ae7df6d2bc5e6ef1f325b794878a1279c6bf4d849cce5cb8f5251c073125ecee10963dfa2f13bba378c4695d66339997c821408c9ddbac6a3d8b7eb2706cdaad8b95d9eace420b2f4dd96f0224d55cbc14bdd45d195cb8ba7ba15c26787f6fce68bc67cb447c8295c6bb2b87ff69468b912995b709bee449a4cf34fb6197e71e44ba5f888f5cf320bf6622df283622ec19de7c3df3ad1d86ae9f4ac79362d3c2856697ca8f16112531e5e46633abbaf5a78c3f7f3cedc72352f0617d079b0c8090a04bf9770d5863916a87a1b11655fb9ded7fb87867d4cda7f261db51c5447b0a7ed0297fb88e59ff7cbf2e3e4dc3bc779dcafa35a5000daae525ddf3ade7164873d78e6d699195ec00e22fa27a15b42c599878ab078183386517211b8aa041f7d16ba64912d08cbe8a0a7a6ccb6f8052068f53d8aede438f44042f57a3934e2acef44e8f881ec235be80c967124bc84e84efafdb44a84af20d85789444a640e9fdff3a16e243033d121428dba10ea6e26fd97099fc479752a48925472e3c061de07171d91334eeb01e212c2fa0cd692fdb87052a139e3de7811a54a10805d7a0a4332ac8dfcc65b876559f62920be9c43e1fba705e59cab50c6d5d79e2bc07dc06780a685cf50136beb17da0b087f25138ac5bc4f711096152d4b9e28908cf753895dd4ba9cc8b8112898ec3366c23f4ef1781aa74d351ec6a3245e15bd8bae49b1694bc6f4eb98f0afb1a2f49291b6a792479de866508a78a4f0cf167869e8bdf6eaeb809f96a3896fb3060413fc4f918cac80726cdd4cf5e68efb284576c729d259d03c5ba03d8baf672a99d53e634544f917aadb834f3f42a0ea3951678cf0d96f912e4f8eb5790f10396743940603c03d106dc669f401ebde103801f6b22d0bc5ac52d656de31208db111a87bab7085d73e861a040517cd50c406b43230213c5fb37c5709f18243ac2d037ae6b55ba0ccaa55f9d6e5162fc8345db052143b65337949b2b2000ad4f18f9932b84c6067d9294cec6c4ff7c71b9191dd969db0084b56cb760f86b56fb1fab08e42ee466cb9bfb48a10c4ca26a85bdc4262841e242c742dcef755463a951a254e9ee2dd190ae439aa9e9967c0cd4abafc1214318b46318d09d858b1c0bb5969b4950d3bdcb8428ff5fcdae41bb133f9f35d54a10fc585569b3f12d3430e342c8f21e8314fbf33e589e5fd14e40f0e184b7e5e58c251f70798b641975f9a26a1e0caa171edac954a3db7e8f632cd4fe9d259848d60c7dd86004f9796069abe8c5359d84b9444286b5f6d25e67383c6a1abcb66a633a96f76e1c064b1ed7aa238481539d99676c9fae4ae1074e3dc446bb198047b61e5913c3229e770712e68fac6e5be1cba78d71b09a6976890f844cc8c9a159ed706aab6a008c1996bf3f7e17d64ef25d4a992e64f114f503f6363cb0766f64b0a917cc86dd441613d3bf4b7858e7e837311a1a2d131e30618da26a64b4c4548b5ea467e1408c2354a16e8f0628a6513270be66c49ed1e4e132ac5ff56149f21b2c75971f5b7cde0c66692f02637945512c0c882de928307bd196703e946f818316ccc523d581dcd938c49e52f3221f74b89b6d68e60dcfb319575a63f0333296e775b68e268e6820809694b6682bb43584eada2a111a19828eb29e7ff927d80d7a9a7457e01a090b36805dfc21a4ca5c1cc6fac892231547b6b64f9f833a23318d26c2bf1e3bf766bbdba0375a1be71f6e363c742424425aa5d8ebb95d8a4419c4ee554d852603d3f35aad05d2997445df3766aac1571e907b80015a8ec5b3d6941452c41a93f563072c9a5a9b15008fd0ceefc56a874d86ed040c3f0b6dbaae17bb9ed33e2f7a8f31916822c9c35c1ccac8b22e9cff0e1bf50d4df472bfae56bc1d92e1a1565e21fbadd6254309f92905317f75c044c7587efdefaa29d3c62e2f3708fe3e642bc6e7db79e7bf75a340ee6ca0b3d712b683a3804ae75528885d4d0294058f6647403c115fd8b9a77cf11ae4faffe35693ed4b5c6dd895e33e5610aed9320f33a0919562ae0d3ffed9ea211089901cb9a36b02532d734a32a16b907e7917cca248ba8135e3cb718083f5ed5f8900025522238beb309734dc531c082ba2fb602e69118b8bc187d7f9472ae";

/// Un checkpoint firmado, con los campos del JSON del nodo (`GET /checkpoint`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedCheckpoint {
    pub version: u8,
    pub chain_id: String,
    pub height: u64,
    pub block_hash: String,
    /// Versión 2: la mitad Ed25519, en hex (64 bytes).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub signature: String,
    /// Versión 2: la mitad ML-DSA-65, en hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mldsa_signature: Option<String>,
    /// Versión 3: las firmas de las claves de la autoridad, en orden de clave.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cosignatures: Vec<Cosignature>,
}

/// La firma híbrida de una de las claves de una autoridad de k de n.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cosignature {
    /// Qué clave firmó: su lugar en [`Authority`].
    pub signer: u8,
    pub ed25519: String,
    pub mldsa: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckpointError {
    /// La cadena no tiene autoridad conocida por una billetera liviana.
    NoAuthority(String),
    Version(u8),
    Malformed(&'static str),
    BadSignature,
    /// Una versión 3 válida, con menos firmas de las que hacen falta.
    Incomplete { have: usize, need: usize },
}

impl fmt::Display for CheckpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAuthority(chain) => write!(f, "no checkpoint authority for chain {chain}"),
            Self::Version(v) => write!(f, "unsupported checkpoint version {v}"),
            Self::Malformed(what) => write!(f, "malformed checkpoint {what}"),
            Self::BadSignature => f.write_str("the checkpoint signature is not valid"),
            Self::Incomplete { have, need } => write!(
                f,
                "the checkpoint has {have} authority signatures, {need} are needed"
            ),
        }
    }
}

impl std::error::Error for CheckpointError {}

/// La clave pública de una autoridad: sus dos mitades.
#[derive(Clone)]
pub struct AuthorityKey {
    ed25519: VerifyingKey,
    mldsa: MlDsa65PublicKey,
}

impl AuthorityKey {
    pub fn from_hex(ed25519: &str, mldsa: &str) -> Option<Self> {
        let ed: [u8; 32] = hex::decode(ed25519.trim()).ok()?.try_into().ok()?;
        let ed25519 = VerifyingKey::from_bytes(&ed).ok()?;
        let mldsa = MlDsa65PublicKey::from_bytes(&hex::decode(mldsa.trim()).ok()?).ok()?;
        Some(Self { ed25519, mldsa })
    }

    /// Las dos mitades en hex, como las compila el nodo.
    pub fn to_hex(&self) -> (String, String) {
        (
            hex::encode(self.ed25519.as_bytes()),
            hex::encode(self.mldsa.to_bytes()),
        )
    }

    /// La primera clave de la autoridad compilada para `chain_id`, si es una
    /// cadena de Quantic que una billetera liviana sigue. La autoridad entera
    /// (con las demás claves de un k de n) es [`Authority::for_chain`].
    pub fn for_chain(chain_id: &str) -> Option<Self> {
        (chain_id == QUANTIC_PQ_TESTNET_CHAIN_ID || chain_id == QUANTIC_PQ_TESTNET_V2_CHAIN_ID)
            .then(|| Self::from_hex(QUANTIC_TESTNET_AUTHORITY, QUANTIC_TESTNET_AUTHORITY_MLDSA))
            .flatten()
    }

    /// Las dos mitades sobre `domain ‖ 0x1f ‖ body`; ML-DSA con el dominio
    /// como contexto.
    fn verify_parts(
        &self,
        domain: &[u8],
        body: &[u8],
        ed: &str,
        mldsa: &str,
    ) -> Result<(), CheckpointError> {
        let ed: [u8; 64] = hex::decode(ed)
            .map_err(|_| CheckpointError::Malformed("signature"))?
            .try_into()
            .map_err(|_| CheckpointError::Malformed("signature"))?;
        let mldsa = MlDsa65Signature::from_bytes(
            &hex::decode(mldsa).map_err(|_| CheckpointError::Malformed("mldsa_signature"))?,
        )
        .map_err(|_| CheckpointError::Malformed("mldsa_signature"))?;
        let mut signed = Vec::with_capacity(domain.len() + 1 + body.len());
        signed.extend_from_slice(domain);
        signed.push(0x1f);
        signed.extend_from_slice(body);
        self.ed25519
            .verify_strict(&signed, &Signature::from_bytes(&ed))
            .map_err(|_| CheckpointError::BadSignature)?;
        self.mldsa
            .verify(&signed, domain, &mldsa)
            .map_err(|_| CheckpointError::BadSignature)
    }

    fn shares_a_half_with(&self, other: &AuthorityKey) -> bool {
        self.ed25519 == other.ed25519 || self.mldsa.to_bytes() == other.mldsa.to_bytes()
    }
}

/// La autoridad de una cadena: una sola clave (versión 2) o k de n (versión 3).
#[derive(Clone)]
pub struct Authority {
    keys: Vec<AuthorityKey>,
    threshold: usize,
    cosigned: bool,
}

impl Authority {
    pub fn single(key: AuthorityKey) -> Self {
        Self {
            keys: vec![key],
            threshold: 1,
            cosigned: false,
        }
    }

    /// `threshold` de `keys`, sin una mitad repetida entre dos claves.
    pub fn cosigned(keys: Vec<AuthorityKey>, threshold: usize) -> Option<Self> {
        let distinct = keys
            .iter()
            .enumerate()
            .all(|(i, key)| keys[..i].iter().all(|other| !other.shares_a_half_with(key)));
        let fits = (1..=keys.len()).contains(&threshold) && keys.len() <= MAX_COSIGNERS;
        (distinct && fits).then_some(Self {
            keys,
            threshold,
            cosigned: true,
        })
    }

    /// La compilada para `chain_id`: la testnet v2 con su clave, la v3 con su
    /// conjunto de k de n.
    pub fn for_chain(chain_id: &str) -> Option<Self> {
        let first =
            AuthorityKey::from_hex(QUANTIC_TESTNET_AUTHORITY, QUANTIC_TESTNET_AUTHORITY_MLDSA)?;
        match chain_id {
            QUANTIC_PQ_TESTNET_V2_CHAIN_ID => Some(Self::single(first)),
            QUANTIC_PQ_TESTNET_CHAIN_ID => {
                let mut keys = vec![first];
                for (ed, mldsa) in QUANTIC_TESTNET_COSIGNERS {
                    keys.push(AuthorityKey::from_hex(ed, mldsa)?);
                }
                Self::cosigned(keys, QUANTIC_TESTNET_THRESHOLD)
            }
            _ => None,
        }
    }

    pub fn is_cosigned(&self) -> bool {
        self.cosigned
    }

    pub fn threshold(&self) -> usize {
        self.threshold
    }

    pub fn keys(&self) -> &[AuthorityKey] {
        &self.keys
    }
}

fn checkpoint_body(chain_id: &str, height: u64, block_hash: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(chain_id.len() + block_hash.len() + 10);
    body.extend_from_slice(chain_id.as_bytes());
    body.push(0x1f);
    body.extend_from_slice(&height.to_le_bytes());
    body.push(0x1f);
    body.extend_from_slice(block_hash.as_bytes());
    body
}

fn is_block_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

const MLDSA_HEX_LEN: usize = ML_DSA_65_SIGNATURE_BYTES * 2;

impl SignedCheckpoint {
    pub fn from_json(text: &str) -> Result<Self, CheckpointError> {
        serde_json::from_str(text).map_err(|_| CheckpointError::Malformed("json"))
    }

    /// La forma que exige el nodo a un checkpoint híbrido (versión 2) o de k
    /// de n (versión 3).
    pub fn validate_shape(&self) -> Result<(), CheckpointError> {
        match self.version {
            SIGNED_CHECKPOINT_HYBRID_VERSION => {
                if !self.cosignatures.is_empty() {
                    return Err(CheckpointError::Malformed("cosignatures"));
                }
                match &self.mldsa_signature {
                    Some(mldsa) if mldsa.len() == MLDSA_HEX_LEN => {}
                    _ => return Err(CheckpointError::Malformed("mldsa_signature")),
                }
                if self.signature.len() != 128 {
                    return Err(CheckpointError::Malformed("signature"));
                }
            }
            SIGNED_CHECKPOINT_COSIGNED_VERSION => {
                if !self.signature.is_empty() {
                    return Err(CheckpointError::Malformed("signature"));
                }
                if self.mldsa_signature.is_some() {
                    return Err(CheckpointError::Malformed("mldsa_signature"));
                }
                if self.cosignatures.is_empty()
                    || self.cosignatures.len() > MAX_COSIGNERS
                    || !self.cosignatures.windows(2).all(|pair| pair[0].signer < pair[1].signer)
                {
                    return Err(CheckpointError::Malformed("cosignatures"));
                }
                if self
                    .cosignatures
                    .iter()
                    .any(|c| c.ed25519.len() != 128 || c.mldsa.len() != MLDSA_HEX_LEN)
                {
                    return Err(CheckpointError::Malformed("cosignature"));
                }
            }
            other => return Err(CheckpointError::Version(other)),
        }
        if self.chain_id.trim().is_empty() || self.chain_id.len() > 128 {
            return Err(CheckpointError::Malformed("chain_id"));
        }
        if self.height == 0 {
            return Err(CheckpointError::Malformed("height"));
        }
        if !is_block_hash(&self.block_hash) {
            return Err(CheckpointError::Malformed("block_hash"));
        }
        Ok(())
    }

    /// La versión 2 con una sola clave: Ed25519 estricta y ML-DSA-65 con el
    /// dominio como contexto, sobre el mismo mensaje.
    pub fn verify(&self, authority: &AuthorityKey) -> Result<(), CheckpointError> {
        self.validate_shape()?;
        if self.version != SIGNED_CHECKPOINT_HYBRID_VERSION {
            return Err(CheckpointError::Version(self.version));
        }
        authority.verify_parts(
            SIGNING_DOMAIN_V2,
            &checkpoint_body(&self.chain_id, self.height, &self.block_hash),
            &self.signature,
            self.mldsa_signature.as_deref().unwrap_or_default(),
        )
    }

    /// Con la autoridad entera: la versión 2 si es de una clave; la 3 si es de
    /// k de n, donde cada firma tiene que ser de una de sus claves y valer, y
    /// tiene que haber al menos `threshold`.
    pub fn verify_with(&self, authority: &Authority) -> Result<(), CheckpointError> {
        if !authority.cosigned {
            return self.verify(&authority.keys[0]);
        }
        self.validate_shape()?;
        if self.version != SIGNED_CHECKPOINT_COSIGNED_VERSION {
            return Err(CheckpointError::Version(self.version));
        }
        let body = checkpoint_body(&self.chain_id, self.height, &self.block_hash);
        for cosignature in &self.cosignatures {
            let key = authority
                .keys
                .get(usize::from(cosignature.signer))
                .ok_or(CheckpointError::Malformed("signer"))?;
            key.verify_parts(SIGNING_DOMAIN_V3, &body, &cosignature.ed25519, &cosignature.mldsa)?;
        }
        let have = self.cosignatures.len();
        if have < authority.threshold {
            return Err(CheckpointError::Incomplete {
                have,
                need: authority.threshold,
            });
        }
        Ok(())
    }

    /// Verifica contra la autoridad de la cadena que el checkpoint declara.
    /// Quien lo usa tiene que comprobar además que sea la cadena que sigue.
    pub fn verify_for_declared_chain(&self) -> Result<(), CheckpointError> {
        let authority = Authority::for_chain(&self.chain_id)
            .ok_or_else(|| CheckpointError::NoAuthority(self.chain_id.clone()))?;
        self.verify_with(&authority)
    }
}
