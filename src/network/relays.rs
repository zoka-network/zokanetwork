//! Los relays de iroh que usan los nodos de Zoka: los de n0 y los propios.
//!
//! Un relay deja que dos nodos detrás de un router se encuentren: cada uno se
//! conecta hacia afuera, el relay pasa los datos (cifrados de punta a punta)
//! y, casi siempre, los ayuda a abrir una conexión directa. Desde 5.1.1 hay
//! uno en cada VPS de Zoka, además de los públicos de n0: cada nodo elige el
//! más rápido, y si los VPS se caen siguen los de n0.
//!
//! La búsqueda de direcciones (de un identificador de nodo a su dirección)
//! sigue siendo la de n0.

use iroh::endpoint::{
    presets::{self, Preset},
    Builder,
};

/// Uno en cada VPS. Con punto final, como los de n0: nombres absolutos, sin
/// dominio de búsqueda.
pub const ZOKA_RELAYS: [&str; 2] = [
    "https://relay1.zoka.network.",
    "https://relay2.zoka.network.",
];

/// Los relays de n0 más los de Zoka.
pub fn zoka_relay_mode() -> iroh::RelayMode {
    let map = iroh::defaults::prod::default_relay_map();
    map.extend(&iroh::RelayMap::from_iter(
        ZOKA_RELAYS
            .iter()
            .filter_map(|url| url.parse::<iroh::RelayUrl>().ok()),
    ));
    iroh::RelayMode::Custom(map)
}

/// El preset de n0 (búsqueda de direcciones y relays) con los relays de Zoka
/// sumados a los suyos.
#[derive(Debug, Clone, Copy, Default)]
pub struct ZokaN0;

impl Preset for ZokaN0 {
    fn apply(self, builder: Builder) -> Builder {
        presets::N0.apply(builder).relay_mode(zoka_relay_mode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoka_relays_join_the_n0_ones() {
        let map = zoka_relay_mode().relay_map();
        let n0 = iroh::defaults::prod::default_relay_map();
        assert_eq!(map.len(), n0.len() + ZOKA_RELAYS.len());
        for url in ZOKA_RELAYS {
            assert!(map.contains(&url.parse().unwrap()), "{url} is missing");
        }
        for url in n0.urls::<Vec<_>>() {
            assert!(map.contains(&url), "{url} of n0 is missing");
        }
    }

    /// Dos endpoints que solo conocen uno de nuestros relays (ni direcciones ni
    /// búsqueda) se encuentran a través de él y se pasan datos. Usa los relays
    /// en vivo: `cargo test ... relays -- --ignored`.
    #[tokio::test]
    #[ignore = "uses the live Zoka relays over the internet"]
    async fn two_endpoints_meet_through_each_zoka_relay() {
        use iroh::{endpoint::presets, Endpoint, EndpointAddr, RelayMode};
        use std::time::Duration;
        const ALPN: &[u8] = b"zoka/relay-check/1";
        for url in ZOKA_RELAYS {
            let relay: iroh::RelayUrl = url.parse().unwrap();
            let mode = RelayMode::custom([relay.clone()]);
            let server = Endpoint::builder(presets::Minimal)
                .relay_mode(mode.clone())
                .alpns(vec![ALPN.to_vec()])
                .bind()
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(30), server.online())
                .await
                .unwrap_or_else(|_| panic!("the server never reached {url}"));
            let client = Endpoint::builder(presets::Minimal)
                .relay_mode(mode)
                .bind()
                .await
                .unwrap();
            let accepting = server.clone();
            let echo = tokio::spawn(async move {
                let connection = accepting.accept().await.unwrap().await.unwrap();
                let (mut send, mut recv) = connection.accept_bi().await.unwrap();
                let got = recv.read_to_end(64).await.unwrap();
                send.write_all(&got).await.unwrap();
                send.finish().unwrap();
                connection.closed().await;
            });
            let address = EndpointAddr::new(server.id()).with_relay_url(relay.clone());
            let connection = tokio::time::timeout(Duration::from_secs(30), client.connect(address, ALPN))
                .await
                .unwrap_or_else(|_| panic!("no connection through {url}"))
                .unwrap();
            let (mut send, mut recv) = connection.open_bi().await.unwrap();
            send.write_all(b"hola desde zoka").await.unwrap();
            send.finish().unwrap();
            assert_eq!(recv.read_to_end(64).await.unwrap(), b"hola desde zoka");
            connection.close(0u32.into(), b"done");
            echo.abort();
            client.close().await;
            server.close().await;
        }
    }
}
