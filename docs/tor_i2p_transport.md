# Plan: Tor / I2P transporte real

## Estado actual

- [`NetworkPrivacyConfig`](../src/network/p2p.rs) ya acepta `tor_proxy` y
  `i2p_sam_endpoint` como strings opcionales.
- `enforce_anonymous_transport: true` hace que el nodo **rechace**
  transacciones privadas cuando el transporte es `Direct` o cuando los
  endpoints configurados no son alcanzables.
- Sin embargo, el dialer libp2p nunca enruta efectivamente el trÃ¡fico
  por Tor o I2P. El campo existe, el enforcement existe, pero la
  conexiÃ³n subyacente sigue siendo TCP directa. Es un placebo de red.

## Goal

Que dos nodos ZOKA se puedan conectar intercambiando tx privadas por un
circuito onion real, sin que un observador pasivo pueda correlacionar
IP emisor â†” IP receptor.

## Paths de diseÃ±o

### Path A â€” libp2p `Transport` sobre SOCKS5 (Tor)

Tor expone un SOCKS5 local (por defecto `127.0.0.1:9050`). libp2p no
tiene un transport SOCKS5 nativo, pero sÃ­ tiene `libp2p-tcp` con
`TcpConfig` configurable. Hay dos sub-opciones:

**A1 â€” Wrap TCP con `tokio-socks`**

- Usar [`tokio-socks`](https://crates.io/crates/tokio-socks) para
  envolver cada conexiÃ³n TCP entrante/saliente en un handshake SOCKS5.
- Implementar un `Transport` libp2p custom (`TorSocks5Transport`) que
  sustituya a `TcpConfig` cuando `tor_proxy` estÃ© configurado.
- Address format: `/onion3/<hash>.onion:9050` â†’ translate a SOCKS5
  target.

**A2 â€” arti (Tor nativo en Rust)**

- Usar [`arti`](https://crates.io/crates/arti-client) que embebe un
  cliente Tor completo en el proceso.
- MÃ¡s pesado (~+5MB binary, ~30s primera conexiÃ³n para consensus
  download) pero elimina la dependencia de un daemon externo.
- Mejor DX para usuarios finales, peor para nodos servidor que pueden
  compartir un daemon Tor con otros servicios.

**RecomendaciÃ³n:** A1 en producciÃ³n (reusa Tor system-wide), A2 como
opciÃ³n de conveniencia bajo feature flag `arti-embedded`.

### Path B â€” I2P sobre SAM v3

I2P expone el protocolo SAM (Simple Anonymous Messaging) en
`127.0.0.1:7656`. Hay [`i2p-rs`](https://crates.io/crates/i2p) que
implementa SAM v3.

- Bootstrap es mÃ¡s lento que Tor (~2 min primera vez).
- El address scheme es `<base32>.b32.i2p`, distinto a Tor.
- Menos adopciÃ³n pero resistencia a ataques diferentes (I2P no tiene
  exit nodes, solo servicios internos).

**RecomendaciÃ³n:** bajo feature flag `i2p-transport`, no en build por
defecto hasta que haya demanda.

## ImplementaciÃ³n mÃ­nima viable

1. Crear `src/network/tor_transport.rs` con un `Transport` libp2p custom
   que envuelva TCP en SOCKS5 via `tokio-socks`.
2. En `Node::build_swarm`, si `tor_proxy` estÃ¡ seteado, sustituir
   `TcpConfig` por `TorSocks5Transport`.
3. Los peer multiaddrs deben soportar `/onion3/<hash>:port` â€” libp2p ya
   tiene esto vÃ­a `Multiaddr::with_p2p`.
4. Tests E2E usando `tor --SocksPort 9050` lanzado desde `testcontainers`.

## Cosas que romper / cosas que no

- **No rompe** el wire format. Private txs ya viajan igual por el pipe.
- **No rompe** la CRS. No es cambio criptogrÃ¡fico.
- **Rompe** asunciones sobre latencia: conexiones Tor tardan ~1s extra.
  Hace falta subir timeouts de handshake PBFT y gossip.
- **Rompe** asunciones sobre NAT traversal: hidden services no necesitan
  STUN/UPnP pero sÃ­ descubrimiento por DHT â€” verificar que la DHT
  libp2p funciona sobre Tor (normalmente sÃ­, pero con mÃ¡s jitter).

## Riesgos

- **CorrelaciÃ³n de timing end-to-end**: Tor protege IP pero no patrones
  temporales. Si un atacante ve tanto el lado emisor como el receptor
  (ISP global), puede correlacionar bursts. MitigaciÃ³n: Dandelion++ ya
  presente + jitter adicional en el stem.
- **Exit-node malicioso** (solo Tor, no aplica a hidden services):
  un exit podrÃ­a modificar bytes. MitigaciÃ³n: **siempre** usar
  `/onion3/` hidden services para peer-to-peer, no clearnet via Tor
  exit. Documentar como requerimiento de despliegue.
- **DoS sobre hidden services**: circuitos Tor son recurso caro. Puede
  ser blanco de DoS. MitigaciÃ³n: rate-limit por peer ID + require
  proof-of-work tag en los primeros handshakes (fuera de scope).
- **Lentitud**: validadores PBFT necesitan consensus rÃ¡pido; Tor aÃ±ade
  ~1s por round-trip. Medir y decidir si PBFT se hace sobre Tor o solo
  el transport de tx privadas.

## Acceptance criteria

- [x] Un nodo con `tor_proxy` puede ejecutar `probe_anonymous_route` y forzar destinos `/onion3/` cuando `enforce_anonymous_transport=true`.
- [x] `connect_to_peer` integra preflight anonimo y rechaza targets directos en modo Tor/I2P enforced.
- [x] Integration test en `tests/tor_transport_test.rs` valida handshake SOCKS5 para destino onion (mock local).
- [ ] PBFT sigue alcanzando finality en < 30s sobre transport Tor.
- [ ] Docs de despliegue con ejemplo `torrc` mÃ­nimo.

## EstimaciÃ³n

- Path A1 solo: 1â€“2 semanas (1 ingeniero, dedicado).
- Path A1 + I2P: 2â€“3 semanas.
- Path A2 (arti): +1 semana por la learning curve del crate.

Pre-requisito: ninguno criptogrÃ¡fico. Es trabajo de red.
