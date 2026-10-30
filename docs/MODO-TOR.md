# Modo Tor para el nodo v2: diseño y estado

Pedido del usuario (30/10/2026): integrar Tor como opción de privacidad. Este
documento es el plan y el registro. El trabajo de código va en la rama
`modo-tor`.

## Problema

El nodo v2 habla con sus pares por iroh: QUIC sobre UDP, con TLS híbrido
postcuántico (X25519MLKEM768) obligatorio, más una firma ML-DSA-65 del servidor
en cada conexión. Tor solo lleva flujos TCP, así que hoy un nodo no puede
salir por Tor. Pares y relays ven su IP.

## Diseño

**Tres capas.** Cada una se prueba sola.

1. **Transporte de flujos para iroh** (`src/network/stream_transport.rs`).
   Usa los transportes propios de iroh 1.3 (feature
   `unstable-custom-transports`). Los datagramas de QUIC viajan enmarcados en
   un flujo de bytes:
   - **marco:** `u16` big-endian con el largo, y los bytes;
   - **primer marco del que marca:** un saludo (`ZQSTREAM1` más su dirección).
   Así el que escucha sabe a quién responder por ese mismo flujo.
   - **Direcciones** `CustomAddr` con id `STREAM_TRANSPORT_ID`. Pueden ser
     **marcables** (`host:puerto`, que con Tor es `xxxx.onion:puerto`) o
     **fichas de cliente** (16 bytes al azar): un nodo que solo sale no
     publica dirección, y se le contesta por el flujo que abrió.
   - **Marcador** intercambiable: TCP (pruebas) o Tor (Arti).
   - Si una cola se llena, el paquete se descarta. QUIC lo retransmite, como
     con UDP.
2. **Marcador Tor.**
   - **El que sale:** Arti embebido (`arti-client`, como ZKAPriv) abre el
     flujo a la .onion.
   - **La semilla:** un servicio .onion (con `tor.exe` 0.4.9.14, el de
     `third_party/tor`, o Arti) que manda a un puerto TCP de loopback donde
     escucha el transporte.
   - El nodo del explorador, siempre prendido en la PC, es el candidato a
     primera semilla .onion.
3. **Opciones y app.**
   - `quantic-pq-node serve --tor`: preset con TLS híbrido, solo el transporte
     de flujos con el marcador Tor, sin IP, sin relays y sin publicar
     direcciones en n0. Pares: solo semillas con .onion.
   - `--stream-listen 127.0.0.1:PUERTO --stream-address xxxx.onion:PUERTO`:
     para la semilla.
   - Manifiesto: las semillas pueden traer una dirección .onion.
   - `NodeDescriptor::validate` acepta direcciones propias con
     `STREAM_TRANSPORT_ID`.
   - ZSilent: el interruptor «Tor» en Network privacy. Reinicia el nodo en
     Modo Tor y muestra si Tor ya conectó.

**Por qué así.** Arriba del transporte no cambia nada: el protocolo, las
identidades y el TLS postcuántico van adentro del túnel. Tor no es
postcuántico; el canal de iroh sí. QUIC sobre un flujo pierde algo (bloqueo de
cabeza de línea, doble control de congestión), pero el tráfico de un nodo es
chico.

**Costo para el usuario.**
- La sincronización y el envío son más lentos.
- Un bloque minado tarda unos segundos más en salir.
- El nodo no recibe conexiones entrantes.
- Depende de que haya semillas .onion.

## Etapas

- [x] **E1.** `stream_transport` con marcador TCP; prueba de eco entre dos
  endpoints iroh, solo con este transporte y TLS híbrido obligatorio. Pasó:
  200 KB de eco en ambos sentidos en 0,18 s (rama `modo-tor`). Del lado que
  escucha, cada flujo entrante recibe una dirección al azar (ignora lo que
  diga el que marca), así nadie desvía las respuestas de otro.
- [x] **E2.** Descriptor y manifiesto con dirección propia. Prueba de punta a
  punta en regtest: un nodo sincroniza bloques de otro solo por el
  transporte de flujos. Pasó: sincronizó 2 bloques y devolvió uno minado,
  sin UDP ni relays. `NodeDescriptor::validate` acepta solo direcciones
  marcables de este transporte; el manifiesto suma `stream_address`. Ojo:
  justo después de levantar, el endpoint todavía no reporta su dirección
  propia, así que el nodo tiene que agregarla al descriptor a mano (E4,
  `--stream-address`), como ya hace con las IP locales.
- [x] **E3.** Marcador Tor. **Pasó contra la red Tor real (30/10):** se conectó
  a una .onion recién creada a los 28 s (lo que tarda en publicarse) y a los
  39 s había sincronizado 2 bloques y entregado 1. **Cambio de diseño (30/10):** en escritorio va
  por SOCKS5 hacia `tor.exe` (`SocksDialer`, con `tokio-socks`), no con Arti.
  Así lo había decidido el proyecto: el `Cargo.toml` de ZKAPriv dice que Arti
  queda solo en el teléfono y que el nodo de escritorio habla Tor por un
  proxy externo. ZSilent conserva su `TorSupervisor`. El mismo `tor.exe` hace
  de cliente y aloja la .onion de la semilla. Prueba (ignorada por defecto,
  pide `ZOKA_TEST_TOR_EXE` y la red Tor): un nodo sincroniza por Tor desde una
  .onion local.
- [x] **E4a. Opciones del nodo (30/10).** `--tor 127.0.0.1:PUERTO`, que solo
  sale por el SOCKS de Tor:
  - sin UDP, sin relays y sin anunciarse;
  - una identidad de transporte nueva en cada arranque;
  - solo pares con .onion, y de ellos solo la dirección .onion;
  - no corre el bucle de pares aprendidos.
  `--stream-listen` y `--stream-address` son para la semilla: `AlsoStream`
  suma el transporte al de siempre y el descriptor declara la .onion. El
  handshake STARK tiene 90 s, en vez de 15, por la ruta de flujo (de los dos
  lados).
  **Probado** con `scripts/probar-modo-tor.ps1`: dos nodos reales en regtest y
  un `tor.exe` propio. El nodo en Modo Tor siguió a la semilla 6 minutos solo
  por Tor (a 2 bloques, con bloques cada 10 s).
  Hallazgos en el camino:
  - con 15 s, el primer handshake por Tor se agotaba;
  - con las IP de la semilla en el descriptor, iroh intentaba cerrar la única
    ruta ("failed closing path: LastOpenPath") y la sesión se trababa a los
    2,5 minutos.
- [x] **E4b. Semilla .onion de verdad (30/10, autorizado por el usuario).**
  - `%APPDATA%\Zoka\.zoka\tor-seed\` tiene `tor.exe` 0.4.9.14, un `torrc` sin
    SOCKS y la .onion `u2soz3q63237afuccfxf3jzytpljpbxpvyzt3jumrr45ukzowdakayqd.onion:9444`.
    Se abre sin ventana con `tor-seed-oculto.vbs`.
  - El nodo del explorador corre con `--stream-listen 127.0.0.1:9444` y
    `--stream-address`.
  - Es seed2 en el manifiesto.
  - Probado con `scripts/probar-modo-tor-testnet.ps1 -Seeds builtin`: un nodo
    nuevo bajó la testnet real (1097 bloques) en 6 minutos, solo por Tor.
  - Antes hubo que arreglar dos cosas:
    - la descarga, con pedidos en tubería y el atajo del ancestro común (por
      Tor pasó de 0,5 a 3 bloques/s);
    - una pausa de 7 minutos: la semilla registró "failed closing path:
      LastOpenPath" porque un circuito cortado dejaba a QUIC respondiendo a un
      flujo muerto. La solución fue la ficha estable en el saludo, con prueba
      de corte, más el corte por 60 s de silencio.
- [x] **E5. ZSilent 4.0.5.**
  - Interruptor "Reach the network only through Tor" en Settings > Network,
    apagado por defecto.
  - Tor cliente (solo SOCKS) desde `TorSupervisor`, y el nodo con `--tor`. Si
    Tor no arranca, el nodo tampoco.
  - Bloqueado en una PC con las claves de autoridad.
  - El MSI trae Tor otra vez.
  - Prueba `TorModeE2ETest`, contra la testnet real: altura 50 en 202 s.
  - El minero CLI 4.0.3 suma `-Tor` / `--tor`.

## Registro

- 30/10: diseño. iroh 1.3.0 confirmado:
  - `Builder::add_custom_transport`, `clear_ip_transports`,
    `clear_relay_transports`;
  - `CustomTransport`, `CustomEndpoint` y `CustomSender` en
    `iroh::endpoint::transports`;
  - ejemplo en `examples/custom-transport.rs` y transporte de prueba en
    `src/test_utils/test_transport.rs` del crate.
