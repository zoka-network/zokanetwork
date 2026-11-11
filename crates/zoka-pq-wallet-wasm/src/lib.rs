//! La billetera web de Zoka Quantic, dentro del navegador.
//!
//! La página baja las páginas públicas `/scan2` y el checkpoint firmado de
//! `api.zokascan.cc` y se los pasa a un [`WebWallet`]. Acá se verifica cada
//! página contra su cabecera y lo leído contra el checkpoint (Ed25519 +
//! ML-DSA-65), se calcula el saldo y se prueba cada pago (STARK). La frase y
//! las claves quedan en esta memoria; el servidor solo ve pedidos de páginas
//! iguales para todos y el pago ya probado. Es el flujo del teléfono
//! (`zkapriv/native/src/stark.rs`) con HTTPS en vez de P2P.
#![forbid(unsafe_code)]

use bip39::{Language, Mnemonic};
use serde_json::{json, Value};
use wasm_bindgen::prelude::*;
use zeroize::Zeroizing;
use zoka_light::{checkpoint::SignedCheckpoint, directory::AddressRecord};
use zoka_pq_pool::{
    light::{LightWallet, Movement},
    scan::ScanPage2,
    transaction::Transaction,
    wallet::{Account, Address, ShortAddress},
};

fn js(error: impl std::fmt::Display) -> JsError {
    JsError::new(&error.to_string())
}

/// La cadena de cada red que una billetera web sigue: 2 es la testnet v2.
fn chain_id(network: u8) -> Option<&'static str> {
    match network {
        2 => Some(zoka_light::checkpoint::QUANTIC_PQ_TESTNET_CHAIN_ID),
        _ => None,
    }
}

/// Las redes que abre: la testnet v2 y regtest (3, la cadena local de las
/// pruebas, sin checkpoints firmados).
fn supported(network: u8) -> bool {
    chain_id(network).is_some() || network == 3
}

/// La semilla de 24 palabras BIP-39 en inglés: sus 256 bits de entropía, como
/// en ZSilent y en ZKAPriv (`Account::from_seed(semilla, red, 0)`).
fn seed_of(phrase: &str) -> Result<Zeroizing<[u8; 32]>, &'static str> {
    let normalized = phrase
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mnemonic = Mnemonic::parse_in_normalized(Language::English, &normalized)
        .map_err(|_| "those are not valid recovery words")?;
    let entropy = Zeroizing::new(mnemonic.to_entropy());
    if entropy.len() != 32 {
        return Err("Zoka Quantic recovery phrases have 24 words");
    }
    let mut seed = Zeroizing::new([0u8; 32]);
    seed.copy_from_slice(&entropy);
    Ok(seed)
}

/// 24 palabras nuevas, de 256 bits del generador del navegador.
#[wasm_bindgen(js_name = newPhrase)]
pub fn new_phrase() -> Result<String, JsError> {
    let mut entropy = Zeroizing::new([0u8; 32]);
    getrandom::fill(entropy.as_mut()).map_err(js)?;
    let mnemonic = Mnemonic::from_entropy_in(Language::English, entropy.as_ref()).map_err(js)?;
    Ok(mnemonic.to_string())
}

#[wasm_bindgen(js_name = validPhrase)]
pub fn valid_phrase(phrase: &str) -> bool {
    seed_of(phrase).is_ok()
}

/// `"full"`, `"short"` o `""` si no es una dirección de `network`.
#[wasm_bindgen(js_name = addressKind)]
pub fn address_kind(text: &str, network: u8) -> String {
    let text = text.trim();
    if Address::decode(text, network).is_ok() {
        "full".into()
    } else if ShortAddress::decode(text, network).is_ok() {
        "short".into()
    } else {
        String::new()
    }
}

/// Si la dirección completa `full` (la que devuelve `/address/<corta>`) es la
/// de la corta `short`: un servidor no puede cambiar el destinatario.
#[wasm_bindgen(js_name = matchesShort)]
pub fn matches_short(full: &str, short: &str, network: u8) -> bool {
    let (Ok(full), Ok(short)) = (
        Address::decode(full.trim(), network),
        ShortAddress::decode(short.trim(), network),
    ) else {
        return false;
    };
    full.short_address()
        .map(|own| own.encode() == short.encode())
        .unwrap_or(false)
}

/// La corta de una dirección completa de `network`, para mostrar a quién se
/// paga; `""` si no lo es.
#[wasm_bindgen(js_name = shortOf)]
pub fn short_of(full: &str, network: u8) -> String {
    Address::decode(full.trim(), network)
        .ok()
        .and_then(|full| full.short().ok())
        .unwrap_or_default()
}

/// Lo que acepta [`derive_key`]: desde el piso de OWASP para Argon2id
/// (19 MiB, 2 pasadas) hasta lo que un teléfono todavía aguanta.
const MIN_KDF_KIB: u32 = 19 * 1024;
const MAX_KDF_KIB: u32 = 256 * 1024;

/// La clave AES-256 que cifra la frase y el estado en el navegador: Argon2id
/// de la contraseña con `salt` (al azar, de cada bóveda), `memory_kib` KiB y
/// `iterations` pasadas, en un carril. Con 64 MiB y 3 pasadas tarda alrededor
/// de un segundo: llamarla en el Worker.
#[wasm_bindgen(js_name = deriveKey)]
pub fn derive_key(
    password: &str,
    salt: &[u8],
    memory_kib: u32,
    iterations: u32,
) -> Result<Vec<u8>, JsError> {
    derive(password, salt, memory_kib, iterations).map_err(JsError::new)
}

fn derive(password: &str, salt: &[u8], memory_kib: u32, iterations: u32) -> Result<Vec<u8>, &'static str> {
    if password.is_empty() {
        return Err("the password is empty");
    }
    if salt.len() < 16 {
        return Err("the salt has fewer than 16 bytes");
    }
    if !(MIN_KDF_KIB..=MAX_KDF_KIB).contains(&memory_kib) || !(2..=10).contains(&iterations) {
        return Err("unsupported key derivation parameters");
    }
    let params = argon2::Params::new(memory_kib, iterations, 1, Some(32))
        .map_err(|_| "unsupported key derivation parameters")?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = vec![0u8; 32];
    argon
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|_| "the key could not be derived")?;
    Ok(key)
}

/// El QR de `text` con corrección M, como el de ZKAPriv: `[ancho, módulos…]`,
/// un byte por módulo (1 = oscuro), fila por fila y sin el margen.
#[wasm_bindgen(js_name = qrMatrix)]
pub fn qr_matrix(text: &str) -> Result<Vec<u8>, JsError> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M)
        .map_err(js)?;
    let width = code.width();
    let mut out = Vec::with_capacity(1 + width * width);
    out.push(u8::try_from(width).map_err(js)?);
    out.extend(
        code.to_colors()
            .into_iter()
            .map(|color| u8::from(color == qrcode::Color::Dark)),
    );
    Ok(out)
}

#[wasm_bindgen]
pub struct WebWallet {
    network: u8,
    seed: Zeroizing<[u8; 32]>,
    /// Las direcciones que lee, las cuentas `0..` de la semilla: la 0 es la
    /// principal (y la del vuelto), las demás se suman con
    /// [`WebWallet::add_address`].
    accounts: Vec<Account>,
    wallet: LightWallet,
    last: Option<Transaction>,
}

fn movement(m: &Movement, tip: u32) -> Value {
    let kind = if m.coinbase {
        "mined"
    } else if m.spent > 0 {
        "sent"
    } else {
        "received"
    };
    let amount = if m.spent > 0 { m.paid() } else { m.received };
    json!({
        "txid": hex::encode(m.txid),
        "height": m.height,
        "timestamp": m.timestamp,
        "kind": kind,
        "amount_atoms": amount,
        "fee_atoms": m.fee,
        "memo": m.memo,
        "confirmations": tip.saturating_sub(m.height) + 1,
    })
}

impl WebWallet {
    fn primary(&self) -> &Account {
        &self.accounts[0]
    }

    fn at(&self, index: u32) -> Result<&Account, JsError> {
        self.accounts
            .get(index as usize)
            .ok_or_else(|| JsError::new("this wallet has no such address"))
    }
}

#[wasm_bindgen]
impl WebWallet {
    /// `state`: lo que devolvió [`WebWallet::encode_state`] la última vez, o
    /// vacío para leer la cadena desde el bloque 1.
    #[wasm_bindgen(constructor)]
    pub fn new(phrase: &str, network: u8, state: &[u8]) -> Result<WebWallet, JsError> {
        if !supported(network) {
            return Err(JsError::new("this network has no web wallet"));
        }
        let seed = seed_of(phrase).map_err(JsError::new)?;
        let wallet = if state.is_empty() {
            LightWallet::new(network, 1).map_err(js)?
        } else {
            let wallet = LightWallet::decode(state)
                .map_err(|_| JsError::new("the saved wallet state is unreadable"))?;
            if wallet.network() != network {
                return Err(JsError::new("the saved wallet state is of another network"));
            }
            wallet
        };
        let accounts = (0..wallet.addresses())
            .map(|index| Account::from_seed(&seed, network, index))
            .collect::<Result<Vec<_>, _>>()
            .map_err(js)?;
        Ok(Self {
            network,
            seed,
            accounts,
            wallet,
            last: None,
        })
    }

    /// Cuántas direcciones lee: la principal y las que se sumaron.
    pub fn addresses(&self) -> u32 {
        self.wallet.addresses()
    }

    /// Una dirección más para dar a otra persona: quienes pagan a dos
    /// direcciones distintas no pueden saber que pagaron a la misma billetera.
    /// Hasta ocho; cada una es un descifrado más por salida leída. Lee los
    /// pagos a la nueva desde el próximo bloque, así que hay que crearla antes
    /// de darla. Devuelve su número.
    #[wasm_bindgen(js_name = addAddress)]
    pub fn add_address(&mut self) -> Result<u32, JsError> {
        let index = self.wallet.addresses();
        let account = Account::from_seed(&self.seed, self.network, index).map_err(js)?;
        self.wallet.set_addresses(index + 1).map_err(js)?;
        self.accounts.push(account);
        Ok(index)
    }

    /// La dirección completa número `index` (0 es la principal).
    #[wasm_bindgen(js_name = addressAt)]
    pub fn address_at(&self, index: u32) -> Result<String, JsError> {
        self.at(index)?.address().encode().map_err(js)
    }

    /// La dirección corta número `index`.
    #[wasm_bindgen(js_name = shortAddressAt)]
    pub fn short_address_at(&self, index: u32) -> Result<String, JsError> {
        self.at(index)?.address().short().map_err(js)
    }

    /// La dirección completa (unos 1.760 caracteres).
    pub fn address(&self) -> Result<String, JsError> {
        self.primary().address().encode().map_err(js)
    }

    /// La dirección corta: la que se comparte y va en el QR.
    #[wasm_bindgen(js_name = shortAddress)]
    pub fn short_address(&self) -> Result<String, JsError> {
        self.primary().address().short().map_err(js)
    }

    /// Muestra el saldo de la dirección principal a quien la tenga, sin poder
    /// gastar. Cada dirección tiene la suya.
    #[wasm_bindgen(js_name = viewKey)]
    pub fn view_key(&self) -> Result<String, JsError> {
        Ok(self.primary().view_key().map_err(js)?.encode())
    }

    /// Solo lo recibido, sin saldo ni gastos. Se deriva de la misma cuenta:
    /// no hay que configurar ni migrar la billetera.
    #[wasm_bindgen(js_name = incomingViewKey)]
    pub fn incoming_view_key(&self) -> Result<String, JsError> {
        Ok(self.primary().incoming_view_key().map_err(js)?.encode())
    }

    /// La altura del próximo bloque que necesita: pedir `/scan2?from=<next>`.
    pub fn next(&self) -> u32 {
        self.wallet.next()
    }

    #[wasm_bindgen(js_name = inSync)]
    pub fn in_sync(&self) -> bool {
        self.wallet.in_sync()
    }

    /// Lee una página de `/scan2` (tiene que empezar en [`WebWallet::next`]);
    /// devuelve la punta que tenía el servidor. Cada bloque se compara con su
    /// cabecera: una página que esconde o inventa una salida se rechaza.
    #[wasm_bindgen(js_name = addPage)]
    pub fn add_page(&mut self, bytes: &[u8]) -> Result<u32, JsError> {
        let page = ScanPage2::decode(bytes)
            .map_err(|_| JsError::new("the server sent an unreadable page"))?;
        self.wallet.add_page_for(&self.accounts, &page).map_err(js)?;
        Ok(page.tip)
    }

    /// Verifica la firma del checkpoint (el JSON de `/checkpoint`) y confirma
    /// lo leído contra él. `true` si esa altura ya se leyó.
    #[wasm_bindgen(js_name = confirmCheckpoint)]
    pub fn confirm_checkpoint(&mut self, json: &str) -> Result<bool, JsError> {
        let checkpoint = SignedCheckpoint::from_json(json).map_err(js)?;
        if Some(checkpoint.chain_id.as_str()) != chain_id(self.network) {
            return Err(JsError::new("a checkpoint of another chain"));
        }
        checkpoint.verify_for_declared_chain().map_err(js)?;
        let height = u32::try_from(checkpoint.height).map_err(js)?;
        self.wallet
            .confirm_checkpoint(height, &checkpoint.block_hash)
            .map_err(js)
    }

    /// Saldo, punta, verificación, pago pendiente e historial, en JSON
    /// (montos en átomos: 1 ZKA = 1.000.000).
    pub fn status(&self) -> String {
        let balance = self.wallet.balance();
        let history: Vec<Value> = self
            .wallet
            .history()
            .iter()
            .rev()
            .take(500)
            .map(|m| movement(m, balance.height))
            .collect();
        json!({
            "height": balance.height,
            "tip": balance.tip,
            "next": self.wallet.next(),
            "in_sync": self.wallet.in_sync(),
            "spendable_atoms": balance.spendable_atoms,
            "immature_atoms": balance.immature_atoms,
            "reserved_atoms": balance.reserved_atoms,
            "max_sendable_atoms": balance.max_sendable_atoms,
            "notes": balance.notes,
            "verified": balance.verified,
            "pending": self.wallet.pending().map(|p| json!({
                "txid": hex::encode(p.txid),
                "expiry": p.expiry,
            })),
            "history": history,
        })
        .to_string()
    }

    /// La comisión de un pago de `amount_atoms`, o por qué no alcanza.
    pub fn quote(&self, amount_atoms: u64) -> String {
        match self.wallet.quote(amount_atoms) {
            Ok((fee, inputs)) => json!({"enough": true, "fee_atoms": fee, "inputs": inputs}),
            Err(reason) => json!({"enough": false, "reason": reason.to_string()}),
        }
        .to_string()
    }

    /// Arma y prueba (STARK, segundos) un pago a la dirección completa `to`,
    /// y reserva sus notas. Devuelve `{txid, amount_atoms, fee_atoms,
    /// expiry}`; los bytes para `POST /transactions`, con
    /// [`WebWallet::last_transaction`]. Si la red lo rechaza: [`WebWallet::release`].
    pub fn prove(&mut self, to: &str, amount_atoms: u64, memo: &str) -> Result<String, JsError> {
        let to = Address::decode(to.trim(), self.network)
            .map_err(|_| JsError::new("that is not a full address of this network"))?;
        let plan = self
            .wallet
            .plan_for(&self.accounts, to, amount_atoms, memo.as_bytes())
            .map_err(js)?;
        let (amount, fee, expiry) = (plan.amount(), plan.fee, plan.expiry);
        let tx = plan.prove_for(&self.accounts).map_err(js)?;
        self.wallet.reserve(&tx).map_err(js)?;
        let txid = hex::encode(tx.txid());
        self.last = Some(tx);
        Ok(json!({"txid": txid, "amount_atoms": amount, "fee_atoms": fee, "expiry": expiry}).to_string())
    }

    /// Los bytes del último pago probado, para `POST /transactions`.
    #[wasm_bindgen(js_name = lastTransaction)]
    pub fn last_transaction(&self) -> Result<Vec<u8>, JsError> {
        self.last
            .as_ref()
            .ok_or_else(|| JsError::new("no payment was proved"))?
            .encode()
            .map_err(js)
    }

    /// La red rechazó el pago: sus notas vuelven a estar disponibles.
    pub fn release(&mut self) {
        self.wallet.release();
        self.last = None;
    }

    /// El estado leído, para guardarlo (cifrado) y seguir desde ahí.
    #[wasm_bindgen(js_name = encodeState)]
    pub fn encode_state(&self) -> Result<Vec<u8>, JsError> {
        self.wallet.encode().map_err(js)
    }

    /// El registro de la dirección corta para `POST /address`, con su
    /// estampilla Argon2id (unos segundos: llamarla en un Worker).
    #[wasm_bindgen(js_name = addressRecord)]
    pub fn address_record(&self) -> Result<String, JsError> {
        let mut start = [0u8; 8];
        getrandom::fill(&mut start).map_err(js)?;
        let record = AddressRecord::new(&self.primary().address(), u64::from_le_bytes(start))
            .map_err(|e| JsError::new(&e))?;
        serde_json::to_string(&record).map_err(js)
    }

    /// Igual que [`WebWallet::address_record`], para la dirección número
    /// `index`: así su corta también sirve.
    #[wasm_bindgen(js_name = addressRecordAt)]
    pub fn address_record_at(&self, index: u32) -> Result<String, JsError> {
        let mut start = [0u8; 8];
        getrandom::fill(&mut start).map_err(js)?;
        let record = AddressRecord::new(&self.at(index)?.address(), u64::from_le_bytes(start))
            .map_err(|e| JsError::new(&e))?;
        serde_json::to_string(&record).map_err(js)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // El vector de BIP-39 de 24 palabras con entropía 0x00...: la misma semilla
    // que usan ZSilent y ZKAPriv para esas palabras.
    const ZEROS: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

    /// Una dirección más de la misma semilla: distinta de la principal, la
    /// misma que deriva cualquier otra billetera con esa frase, y el estado
    /// guardado se acuerda de cuántas hay.
    #[test]
    fn a_web_wallet_adds_an_address_and_remembers_it() {
        let mut wallet = WebWallet::new(ZEROS, 3, &[]).unwrap();
        assert_eq!(wallet.addresses(), 1);
        let main = wallet.address().unwrap();
        assert_eq!(wallet.address_at(0).unwrap(), main);
        assert_eq!(wallet.add_address().unwrap(), 1);
        let second = wallet.address_at(1).unwrap();
        assert_ne!(second, main);
        let expected = Account::from_seed(&[0; 32], 3, 1).unwrap().address();
        assert_eq!(second, expected.encode().unwrap());
        assert_eq!(wallet.short_address_at(1).unwrap(), expected.short().unwrap());
        let state = wallet.encode_state().unwrap();
        let again = WebWallet::new(ZEROS, 3, &state).unwrap();
        assert_eq!(again.addresses(), 2);
        assert_eq!(again.address_at(1).unwrap(), second);
        assert_eq!(again.address().unwrap(), main);
    }

    #[test]
    fn the_phrase_gives_the_seed_and_address_of_the_other_wallets() {
        let seed = seed_of(ZEROS).unwrap();
        assert_eq!(*seed, [0u8; 32]);
        // Mayúsculas y espacios de más no cambian la billetera.
        let messy = format!("  {}  ", ZEROS.to_uppercase().replace(' ', "   "));
        assert_eq!(*seed_of(&messy).unwrap(), [0u8; 32]);
        let account = Account::from_seed(&seed, 2, 0).unwrap();
        let full = account.address().encode().unwrap();
        let short = account.address().short().unwrap();
        assert_eq!(address_kind(&full, 2), "full");
        assert_eq!(address_kind(&short, 2), "short");
        assert!(matches_short(&full, &short, 2));
        assert_eq!(short_of(&full, 2), short);
        assert_eq!(short_of(&short, 2), "");
    }

    #[test]
    fn bad_phrases_and_foreign_addresses_are_refused() {
        assert!(!valid_phrase("abandon abandon"));
        assert!(!valid_phrase(&ZEROS.replace(" art", " zoo")));
        // 12 palabras válidas en BIP-39, pero Quantic usa 24.
        assert!(!valid_phrase("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"));
        let other = Account::from_seed(&[1; 32], 2, 0).unwrap().address();
        let mine = Account::from_seed(&[0; 32], 2, 0).unwrap().address();
        assert!(!matches_short(&other.encode().unwrap(), &mine.short().unwrap(), 2));
        assert_eq!(address_kind("zqtest2nothing", 2), "");
    }

    #[test]
    fn the_password_key_depends_on_password_and_salt_and_refuses_weak_settings() {
        let salt = [7u8; 16];
        let key = derive("correct horse", &salt, MIN_KDF_KIB, 2).unwrap();
        assert_eq!(key.len(), 32);
        assert_eq!(key, derive("correct horse", &salt, MIN_KDF_KIB, 2).unwrap());
        assert_ne!(key, derive("correct horsf", &salt, MIN_KDF_KIB, 2).unwrap());
        assert_ne!(key, derive("correct horse", &[8u8; 16], MIN_KDF_KIB, 2).unwrap());
        assert_ne!(key, derive("correct horse", &salt, MIN_KDF_KIB, 3).unwrap());
        // Below OWASP's floor, a short salt or no password: refused.
        assert!(derive("correct horse", &salt, MIN_KDF_KIB - 1, 2).is_err());
        assert!(derive("correct horse", &salt, MIN_KDF_KIB, 1).is_err());
        assert!(derive("correct horse", &salt, MAX_KDF_KIB + 1, 2).is_err());
        assert!(derive("correct horse", &[7u8; 15], MIN_KDF_KIB, 2).is_err());
        assert!(derive("", &salt, MIN_KDF_KIB, 2).is_err());
    }

    #[test]
    fn the_qr_holds_the_short_address_with_its_finder_patterns() {
        let short = Account::from_seed(&[0; 32], 2, 0).unwrap().address().short().unwrap();
        let qr = qr_matrix(&short).unwrap();
        let width = qr[0] as usize;
        assert_eq!((width - 21) % 4, 0, "a QR is 21 + 4k modules wide");
        assert_eq!(qr.len(), 1 + width * width);
        let at = |row: usize, col: usize| qr[1 + row * width + col];
        // The three finder patterns: dark corners, a light ring, a dark core.
        for (row, col) in [(0, 0), (0, width - 7), (width - 7, 0)] {
            assert_eq!((at(row, col), at(row + 1, col + 1), at(row + 3, col + 3)), (1, 0, 1));
        }
        // And the light separator around the first one.
        assert!((0..8).all(|i| at(7, i) == 0 && at(i, 7) == 0));
    }
}
