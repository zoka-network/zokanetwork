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
/// Dos de tres: la PC, el VPS 1 y el VPS 2 (11/11/2026).
pub const QUANTIC_TESTNET_THRESHOLD: usize = 2;
pub const QUANTIC_TESTNET_COSIGNERS: &[(&str, &str)] = &[
    (
        "177c6fdf6c4f992dd958d9b38f66bd7df5d26b91829e4493369352a53e060ebb",
        "48243310bebb927908d07f394aa42dba1b73c4384246ac7eb9c21c0447823bf91571e6e645e813e49e4d9e6dabdad625af461c9fa914f28a9eb9dd67c99c78d622daa0ec9788da573d1ac47baf0908c99593e0deac3789711b0e8c6b1671a52ee90270be07f5267540bff2841da2653253768264a1a0d23aa2c75b99b44176c65498e454305e28021bb3ea3fb1290660d3e294591e4f5ac6fb462d780a5b4c8c3a6e42663026083017912923330f616ad9df611a5cba588255a43c511a5ac8189d486ad42baefee96a901863454ce325336fab1a96947b1d6853b01dfa92e386a8ef197dcfa62e5fef782a8817bcc0583ea10be22bc73b0dfabc90f6bea129ff5c11aa8b2010984bcede7de57c222f5326408d9de8246574c47b3cc58272f00f3f0c83fd2778f62e1ed50a214b8c6744c4b87a9ce612a5fa9e6fb7eca109eebc4953508d2cd65fdc87e8104ec72d1dbce028ee171cab2ae90f183780eee54fbd0e0b07170017eea5afefa7bd6a6118e7696315cbe68d50001fbf43acb3e524ad9d3487303bf791aeac8b9203b0aaf72211c7afcca7e851b47f0316f383ebd54e574ddb90b0ca41c386c78dbabf44efcae3284701281dda64955d4fa1e021d6a54349809ad7555f669efddb0c4d83c670e728f8d5456a77e3aefdecd6247800629ebd5ec2a3eaa2d978dc83657d602ffc8fcce98c838fedc4d6fa4945ee99a01425ff8a43c20dcc9f9866febb177ae5ed7fb9c076095173f63ab18ee9c88ea33202644d9b768b7081d6c8f7c55f93c724c7cb57bc8667a7d83ad3540ee8fd9db64c3aaee3b980a2fb4456cf7cd82c610ddd859d496ce71f30377019bce1cbbe85c10ed31a7136aeca6ca9093ad62077c54d762ba2160054b368fb5496d7a70b82da5fdf6c6a3e681881d8c84dd9663f59d280fd84398cf218d24a133f264ed2edb864cdcf17f384232556d5d4dc6acbf595b2f427f0a4042d72390c4018884053d920ab0d3a15d986bb6b15c678502d4386e27bbfcb9fc1899ee2aadba3072d418c54f634ec5c703bb58ba369204a005669fec91093acb625a8fcd0ef0a263a67f6c67d7daa0f9439d81cc4080e0a814be283ec55af3a0e48d0602d1b266fde37ccff2934b317873a5b7616f0dbc8bbfdeda06d357719e7997474a77aba3fe8b0f4b065c765c2afcdd95fe5f87f25f844a565ce4e4e292595e4842daa0e0f8930ad11d08f1334c2156c3066a2608e72d272371f2d878fbdb0fbd43bfb74cd944823344c92e9333734f02a7924409131086bcec5139060fb9938ee44e653f522b8a134371624da6f441cca0311f4a2f309cfaf00eac6d284dbe060314c9f4acc61bc0886509555c3bcc35993acf8b0ac0fb3dc05822f67d0376eb05b18f56fc92ec58f116a9d0353fa210c5721c8531b5531aa4af06fa4aba74d944b74d0c6e6af5eb79a733c9daf534077686364f790aa103790d0ca60904a58b0bf7d4014b7aa16ef8343d3f54b98a39c6b9d6d459d15732d66de98863913fdd4e9ce602eb1b52189f987dc16dcbfbff0a95c6b4957909a7a6c68471fcd3a8c7d56a51751f3fc70e9f107b28897feb1f4612f7fcb5bed23462e2b7f3473c79babc9e715b0a43affd80402da9d14bfc3e5a4576bcfef831ffc6c12c2c3816ec3f3c4eda696a7902805be3ffd717873beff091a0aab485478109eaef96bc61b55d028e22f27dc2eb1b240c4792d76ba625bce21ea345f6139ff5a4f191b0bd72aa7a9a7ffcd52c68ef1d82c5681dc83c1f3c745102dc23d5e2644f626b79bb9f56af444452f36ed906a6340df4ac6b5c769d6d6932aac130ef2eafd1e3f0ba9a2e267af9dd4f9f1ca6f6404b7d27fffe35e4845a944e5ec81578bb00b49197e0b6293db27a58443a3b4b4d1f2a297a40592f009d8cb3099371d3a813c01e4d77147f5e865248cd296a080db838ed8ea229779fe2d51555bbbc942786641f962588d37681e3456d29cfa962ddb049f3c3e0af7335467e630674bd04a5960038cde5257dafbcd470e8a8eed27db7f9362a72eb51cafefd68892026844f3c843de5149e89d036b852638aea56f2edd946ad2b06d901651fb82b6dcc520e616c331749e4462fc311de2a116b54c3a4a4f991a49eee36483ddfb7e666ddeaddfc7dbad9e172fa1201122d0a094c0a3e587ddc5cc242ab4ebf590e56bf88bcca5e5265b2d2afdda01ef4608a28ae3180173994085430e766190015c23f2005b7aae8c04f9a3de0ae148f7c405700d1d7ea399f2983a2bc882de730e7f5287c44d1592cca920759aabcebf06f401e158f2225f6a894bded98afed165ee1e6f50268f20d817e885ace1beb481658418abead4ee98e0244ab71f8ca1846d56db0df802e6467cf80f17fa8ae74d39d918a69c58582e09c32ec1b9122cf874af0f7bdbfa31d436579bcae3a248913ab6bdce3d6126be11023135455bf2daf4ff3c58b033eea4c8fc2e91a330b01d592067be783165c23b9eb8ee69e050e327901b02cadbc843511f114a00874da8b085c163619387d8800010f710a3f9cdd5c8b4546415dfcc6943df05b43ed397ff113eaa5e87ad75a9b61890e4f95ddb2d0693cfae395e94a67295d8da3b66971ce2044443fbc54146dfa9db3c1d8e5bd4c0958a09e0b39b310b85fea300d5563bde75fdf4e4b213eeb70f2fc835fdbaf474b7bda84fdf50feee693bf356e9443c0a13efd990e827dd0537114ed6e765a276d737e8cc03fc79f04279a6cf8e",
    ),
    (
        "6efd08393ad1e218de657e4321819c4c16e79e3c11ab944bfd340272e2f2a4ad",
        "7654a51700e74261af346d2ba481f489551b2a0af7a9187c3ab80aefb0fa1721c5a2f7e66bdf09d41e61d6e8ff2e0ae9f359917fde85494fb1c3dafecb63375cbaad85a53d0ff12568f25b0ce9c9322bffd32567530ad6826ffee7a65d5b4049ef8a77a1427f5d7d67f9789834a31efbaa4da467fca50fe770be3f7f8f2742280a773f595ac27f83c1b2d259111926f0a52068211a9a54efb7ad32cd423e007fa37ad75aa75c654057007ca9706bfd114167a439ffb496f86c03cd5b3e8c2e1497c98fe58c60d98fa59639079ba5bb666b914d5e2c61f37e960a5b4386f11c09124a86de709ee326e3575ff2f7010e0ae056386320361f37976fac92dfa47deb3dd85d1f7994050107edcc441d26024fdd1fd99a9a4b2eebfe103100e2fcd7e0226f5dc98a13d5523fe08ffe28ca0ebcb3dab81d671d4669bd33f99fa9aeadfe3e807046dc7f54bebeeaa7e35b07aa00dbe9b67f6e0309801ba55a0bb1f39b10a30331c48d4ee89ee785b4fe067d2ed79473ce56c0d1df3e860c9e704ade7c9b974dc6c8cf409a44019d176be60b80290f8b21937b61980b38b1b6a6d7b7074d85e176a7438e68a46a72dec2edbd4ca52a547ef688471717afd8e98bf74e6ba9e6d3e364620012f21f9748efa67307545dae6383abac5048b37ae00c620e6182eba9539216b771cf4710dab36e69566dc9212c1d808609a1359f1b4a309c2f604d1d656fa7fc650f5d570966deaf571cc03677864b518955468610b9c44627b21aa920f3ff33cc1d1210166124ceb4d198dafead39a36d09954c1d195dab1000ab4fe8b32f448dc6da08647e12ee3b78141b34a9fba61a808f8fc78b563367eb40deb947ab48e36abcb5e7074f20a239194c7346e7b8e79a3b536741f831d0034c74f328eee0ca82c519909ca2f3db144f68091cc16bf3caf4c5e082a367ec1afc999c33bd6d8f5b739486d60204f337ab3c5a68f6bb2434aedb86319fdd75428c377a26c56fc5c7cb70a1116f577d21a701f2a72654e3e38feee3be66810f9c8e023749519ea0999ee19e5bfdce56694511e2763c08bb3212293b1d4e884198a2337930c11afed252fb2154d6d9cd1ebc349280bbcbf988fdfe4f7757ab1dec26cf063ba989b93835e8996d35be1ffcc0f9992e4481af9df36f8ae6ce30aebd1971643da5a526a2f538912b00dd10e9d5fb480375ca420c1dc5ff76cd4a899e051513a9c0144de0ea311567549ad6318fd9245aa9cf08cfc90d05411a030b09eb133cc8d9823e1d43ef1e32dcf62997f156268e86ca5962b28ed07c574ade20887d1b81c9bf3a8878c250e0a8b06f03d143f294bcfdb63947622cf17954e945ee26f2955fd5be54b00711b4fea67078b0811a03c81de1d5f30cff3483af1fbb68a32c4f74ed84ffeb2ee946368ca3eccb4aeccb20b0c4e7bb43e7a80d89c1739acd93ef4ed34d294d77554e81e31361dcae03abc46a2fc033965b5bfb734b0bcf9b08d39bacda8248512d1ce7671160cdf81559cbd3921b9ff2f354d6d2e88dfd70cae2c92c47c038b72494f15e8d5af9f77ac62d89eb7926778d4969096370e9f56ae6df979ef9b69649e88385bbf23e37475e6a195cf9039ffe3bc9e65673b7be925ba0069aa2ba21942e3cb80fec008808dcf1dc0cce1ca7262921362dba5a93b39dc608d067b37289264cb85f4476c10ef136a083e7a210c2abf2b139fdb32e55d304792f6a70790d002ea5d7ab52d06d803551502fd82f1ac0a1c398f2329560907fa634651f73fd87449b826b8f4fe40a49f686fb49d60acf24a256350d1972dbff1fda6396e5b3e37a1ec249331a612e7faf111261e0e8094a278d8a5c0dbd1b80983913ec811fa608abfe78eab6b884482071b447b2bc102032d241079fbf4e7f74c9bd607b06df6cf67803dd810141955333b6ce1e4325e4f36cd931182f35552b38482c80901640cb1df5da884af546c7d4ae61a52feb081c375be4b57f4727af457db3bada6043df64f7ee542cff45663dabf72e151aabd073d82a4ebdb654d669d30a51b33e5e4157dc6588ade33c0fdc7732cf966064b48fe9e1acdf67bfbeb53924ebe3ce3ecf74432525623da617685e12aabbbf9d099eb739880ec1fae20c6c5853aa286d80abad8ca124c889955920805da08bca4435e9ef81c16e807dce4b1da90237154c4a7e0ae8f53ce4d0360cc67041d683c3ae5c8c5c7fec687b8c67a5056613de48667af68a882686cf2bd5cc1ece18d9a345e09bd18b2724b93f9cfee04656d6c1be77071eba68bfa5eb217f6e88ee8b329462ba3fed6557e766e8665a74c6691cab2e42388a4f5e6b382238cbec3f2cdd5c2b67de960d16628b5aa6a1f8e220fd7c42ced59f8d24c9f85d28ded2287e4bf2c97afed305f792cd8970fb65786c105f5ce6580d1f4ae7bf50ac7c87f7ec4f8dd0992e4af9c901d826569567586a52441cf4e4bb3e62ff17c73b54e08a1a421d497700fa2fda2ec3f7342f4e550c66cd414bf460e65dccc6e3f57c8e18e49a1ac3760b836375b4253d4cb4df9194bbf33055c5f16e60d5ef27efcdb8305c673555b32b0fc97d706afe0577f0707cc7fb70859122d1da935cd12805c0a1e832fe2fda872c758e140aaf9bbafcb966d76e5a28ce796fefe99ac489e2525f6b1e06def76a7f0abcd2fb9addabec40950d7ed659a9dd428b913f2416375c1ab5a8c6bf0ed0039d84aff2bb070ce88649f4e4c4fad70a2637e1a785e6dc20e799462256",
    ),
];
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
