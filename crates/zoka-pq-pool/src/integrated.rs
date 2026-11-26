//! Direcciones integradas: la dirección corta de una billetera más un código de
//! cliente (`tag`, 8 bytes), para que un exchange reciba en una sola billetera
//! y sepa de qué cliente es cada depósito.
//!
//! Quien paga a una dirección integrada resuelve la dirección completa como con
//! una corta (la trae un nodo y se comprueba contra el resumen) y escribe el
//! código en el memo, cifrado como todo el memo: `zoka-tag:<número>`, y si
//! quiere, un memo propio en la línea siguiente. Como el código es parte de la
//! dirección, nadie se olvida de ponerlo. Cambia el formato de las direcciones,
//! no el consenso: el pago es un pago común.
//!
//! Las direcciones integradas de una misma billetera llevan el mismo resumen,
//! así que cualquiera ve que son del mismo dueño, como las integradas de Monero:
//! están pensadas para exchanges, no para ocultar a sus clientes entre sí.

use crate::{
    model::check_network,
    wallet::{digest, ShortAddress, MEMO_BYTES},
    Error,
};

/// Cómo empieza el memo de un pago a una dirección integrada.
pub const TAG_MEMO_PREFIX: &str = "zoka-tag:";
const VERSION: u8 = 1;
const CHECK_DOMAIN: &[u8; 16] = b"ZokaPQIntegr2___";

/// `zqi2`, `zqtesti2` o `zqregtesti2`: no empieza como las otras formas
/// (`zq2`, `zqtest2`), así que una billetera vieja la rechaza en vez de leerla
/// mal.
pub fn integrated_prefix(network: u8) -> &'static str {
    match network {
        1 => "zqi2",
        2 => "zqtesti2",
        _ => "zqregtesti2",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IntegratedAddress {
    pub short: ShortAddress,
    pub tag: u64,
}

impl IntegratedAddress {
    pub fn new(short: ShortAddress, tag: u64) -> Self {
        Self { short, tag }
    }

    pub fn network(&self) -> u8 {
        self.short.network
    }

    pub fn encode(&self) -> String {
        let prefix = integrated_prefix(self.short.network);
        let mut data = vec![VERSION];
        data.extend(self.short.digest);
        data.extend(self.tag.to_be_bytes());
        let check = digest(CHECK_DOMAIN, &[prefix.as_bytes(), &data]);
        data.extend(&check[..4]);
        format!("{prefix}{}", bs58::encode(data).into_string())
    }

    pub fn decode(text: &str, network: u8) -> Result<Self, Error> {
        check_network(network)?;
        let prefix = integrated_prefix(network);
        let body = text.strip_prefix(prefix).ok_or(Error::Network)?;
        if body.len() > 80 {
            return Err(Error::Encoding);
        }
        let raw = bs58::decode(body).into_vec().map_err(|_| Error::Encoding)?;
        if raw.len() != 1 + 32 + 8 + 4 || raw[0] != VERSION {
            return Err(Error::Encoding);
        }
        let check = digest(CHECK_DOMAIN, &[prefix.as_bytes(), &raw[..41]]);
        if raw[41..] != check[..4] {
            return Err(Error::Encoding);
        }
        Ok(Self {
            short: ShortAddress {
                network,
                digest: raw[1..33].try_into().map_err(|_| Error::Encoding)?,
            },
            tag: u64::from_be_bytes(raw[33..41].try_into().map_err(|_| Error::Encoding)?),
        })
    }
}

/// El memo de un pago a una dirección integrada: el código y, si hay, el memo
/// de quien paga en la línea siguiente. Falla si no entra en el memo.
pub fn tag_memo(tag: u64, note: &str) -> Result<String, Error> {
    if note.contains('\0') {
        return Err(Error::Ledger("a memo cannot contain a NUL character"));
    }
    let text = if note.is_empty() {
        format!("{TAG_MEMO_PREFIX}{tag}")
    } else {
        format!("{TAG_MEMO_PREFIX}{tag}\n{note}")
    };
    if text.len() > MEMO_BYTES {
        return Err(Error::Ledger("the memo is longer than 512 bytes"));
    }
    Ok(text)
}

/// El código de un memo que escribió [`tag_memo`], si lo tiene. Solo la forma
/// canónica: dígitos sin ceros a la izquierda, que entren en 64 bits.
pub fn memo_tag(memo: &str) -> Option<u64> {
    let rest = memo.strip_prefix(TAG_MEMO_PREFIX)?;
    let digits = rest.split('\n').next().unwrap_or_default();
    if digits.is_empty()
        || !digits.bytes().all(|b| b.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
    {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wallet::{Account, Address};

    #[test]
    fn an_integrated_address_round_trips_on_every_network_and_is_not_a_plain_one() {
        for network in [1u8, 2, 3] {
            let account = Account::from_seed(&[21; 32], network, 0).unwrap();
            let full = account.address();
            let short = full.short_address().unwrap();
            for tag in [0, 1, 42, u64::MAX] {
                let text = IntegratedAddress::new(short, tag).encode();
                assert!(text.starts_with(integrated_prefix(network)));
                assert!(text.len() < 90, "{} characters", text.len());
                let back = IntegratedAddress::decode(&text, network).unwrap();
                assert_eq!(back, IntegratedAddress::new(short, tag));
                assert!(back.short.matches(&full));
                // The plain decoders refuse it instead of misreading it.
                assert!(ShortAddress::decode(&text, network).is_err());
                assert!(Address::decode(&text, network).is_err());
            }
            let text = IntegratedAddress::new(short, 7).encode();
            let other = if network == 2 { 1 } else { 2 };
            assert!(IntegratedAddress::decode(&text, other).is_err());
            assert!(IntegratedAddress::decode(&short.encode(), network).is_err());
        }
    }

    #[test]
    fn a_changed_character_or_tag_breaks_the_checksum() {
        let account = Account::from_seed(&[22; 32], 2, 0).unwrap();
        let short = account.address().short_address().unwrap();
        let text = IntegratedAddress::new(short, 123_456).encode();
        let prefix = integrated_prefix(2);
        let body: Vec<char> = text[prefix.len()..].chars().collect();
        for at in [0, body.len() / 2, body.len() - 1] {
            let mut changed = body.clone();
            changed[at] = if changed[at] == 'a' { 'b' } else { 'a' };
            let changed: String = prefix.chars().chain(changed).collect();
            assert!(IntegratedAddress::decode(&changed, 2).is_err(), "position {at}");
        }
    }

    #[test]
    fn the_tag_travels_in_the_memo_with_or_without_a_note() {
        assert_eq!(tag_memo(42, "").unwrap(), "zoka-tag:42");
        assert_eq!(tag_memo(42, "invoice 7").unwrap(), "zoka-tag:42\ninvoice 7");
        assert_eq!(memo_tag("zoka-tag:42"), Some(42));
        assert_eq!(memo_tag("zoka-tag:42\ninvoice 7"), Some(42));
        assert_eq!(memo_tag(&tag_memo(u64::MAX, "x").unwrap()), Some(u64::MAX));
        assert_eq!(memo_tag("zoka-tag:0"), Some(0));
        for not_a_tag in [
            "",
            "hello",
            "zoka-tag:",
            "zoka-tag:007",
            "zoka-tag:-1",
            "zoka-tag:1a",
            "zoka-tag:18446744073709551616",
            " zoka-tag:1",
            "ZOKA-TAG:1",
        ] {
            assert_eq!(memo_tag(not_a_tag), None, "{not_a_tag:?}");
        }
        assert!(tag_memo(1, &"x".repeat(MEMO_BYTES)).is_err());
        assert!(tag_memo(1, "a\0b").is_err());
    }
}

#[cfg(test)]
mod vector {
    use super::*;

    /// The fixed vector every app checks against: the testnet short address
    /// of test seed 2 (the one ZSilent's tests use) with tag 42.
    pub const SHORT: &str = "zqtest22m1iDEYKpaemBFZMVj2k1SAzEV9BGL1uFXRZVABUrvAYuHQRbj";
    pub const TAG_42: &str = "zqtesti23jYjdoHfKWvz5DDUPwYYXACN7eAWX8qdEVQRw5uAGAeCskj7rGtZtrKFb16E2";

    #[test]
    fn the_shared_vector_stays_the_same() {
        let short = ShortAddress::decode(SHORT, 2).unwrap();
        assert_eq!(IntegratedAddress::new(short, 42).encode(), TAG_42);
        let back = IntegratedAddress::decode(TAG_42, 2).unwrap();
        assert_eq!((back.short, back.tag), (short, 42));
    }
}
