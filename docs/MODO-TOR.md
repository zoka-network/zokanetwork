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

- [ ] **E1.** `stream_transport` con marcador TCP; prueba de eco entre dos
  endpoints iroh, solo con este transporte y TLS híbrido obligatorio.
- [ ] **E2.** Descriptor y manifiesto con dirección propia. Prueba de punta a
  punta en regtest: un nodo sincroniza bloques de otro solo por el
  transporte de flujos.
- [ ] **E3.** Marcador Tor con Arti (feature `tor`). Prueba con una .onion
  local (`tor.exe` con servicio oculto hacia el puerto del transporte).
- [ ] **E4.** Opciones `--tor`, `--stream-listen` y `--stream-address`. La
  semilla .onion en el nodo del explorador. Manifiesto.
- [ ] **E5.** ZSilent: interruptor, estado y textos. Release, con el OK del
  usuario.

## Registro

- 30/10: diseño. iroh 1.3.0 confirmado:
  - `Builder::add_custom_transport`, `clear_ip_transports`,
    `clear_relay_transports`;
  - `CustomTransport`, `CustomEndpoint` y `CustomSender` en
    `iroh::endpoint::transports`;
  - ejemplo en `examples/custom-transport.rs` y transporte de prueba en
    `src/test_utils/test_transport.rs` del crate.
