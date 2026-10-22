# Inventario criptográfico de Zoka Quantic

21/10/2026. Paso 1 del plan para cerrar todo el circuito postcuántico antes de
la mainnet (decisión del 21/10: la mainnet espera a que no quede nada clásico en
el gasto). La testnet puede salir antes con lo que hay.

La pregunta que contesta cada fila: **si mañana existe una computadora cuántica
que calcula logaritmos discretos y factoriza (algoritmo de Shor), ¿qué gana el
atacante?** Grover solo divide por dos los bits de seguridad de hashes y
cifrados simétricos; con 256 bits quedan 128, que alcanzan.

No hace falta una computadora cuántica para comprobar nada de esto: alcanza con
(1) que cada pieza dependa de un problema que ningún algoritmo cuántico conocido
resuelve rápido (redes, hashes) y (2) pruebas que le den al atacante lo que le
daría Shor (ver «Atacante cuántico simulado»).

## Lo que usa una red de Quantic

| Pieza | Biblioteca | Dónde | Problema | Con Shor | Estado |
| --- | --- | --- | --- | --- | --- |
| Sobre de cada nota | `x-wing` (X25519 + ML-KEM-768), ChaCha20-Poly1305, BLAKE2b | `crates/zoka-quantic/src/note_encryption.rs` | Redes (ML-KEM) | Nada: hace falta romper las dos mitades | **Cerrado** |
| Recuperación del emisor (`out`) | ChaCha20-Poly1305 con `ock` = BLAKE2b(ovk ‖ cv ‖ cmx ‖ kem) | `note_encryption.rs` | Simétrico | Nada sin la `ovk` | **Cerrado** |
| Checkpoints y snapshots | `ed25519-dalek` + `ml-dsa` (ML-DSA-65) | `src/checkpoint_authority.rs`, `src/hybrid_signature.rs`, `src/snapshot.rs` | Redes (ML-DSA) | Nada: Quantic exige las dos firmas (`authority_policy`) | **Cerrado** |
| Prueba de trabajo | `randomx-rs`, SHA-3 | `src/mining/pow.rs` | Hash | Grover: a lo sumo minar más rápido, no romper | **Cerrado** |
| Claves desde la frase | BIP-39 (PBKDF2-HMAC-SHA512), BLAKE2b | `crates/zoka-quantic/src/keys.rs` | Hash | Nada | **Cerrado** |
| Archivo de la wallet | Argon2 + AES-256-GCM | `src/quantic/wallet_file.rs` | Simétrico | Nada | **Cerrado** |
| Txid, checksums de dirección, árbol de bloques | BLAKE2b, BLAKE3, SHA-2 | varios | Hash | Nada | **Cerrado** |
| **Firma del gasto** | `reddsa` (RedPallas) | dentro de `orchard` | Logaritmo discreto (Pallas) | Firmar gastos ajenos, cambiar una tx del mempool | **Abierto** |
| **Pruebas del gasto** | `halo2_proofs` (IPA sobre Pasta) | dentro de `orchard` | Logaritmo discreto | Pruebas falsas: gastar o crear notas sin tenerlas | **Abierto** |
| **Compromisos de montos** | Pedersen (`cv`) y firma de balance | dentro de `orchard` | Logaritmo discreto | Crear monedas sin que se note (el balance deja de atar) | **Abierto** |
| **Compromisos de notas y árbol** | Sinsemilla | dentro de `orchard` | Logaritmo discreto | Abrir un compromiso a otra nota | **Abierto** |
| Direcciones | Orchard (`pk_d = [ivk]·g_d`) + clave X-Wing | `crates/zoka-quantic/src/address.rs` | Logaritmo discreto en la parte Orchard | Sacar la `ivk` de una dirección; no alcanza para leer notas, que van con X-Wing | Abierto en parte |
| Transporte P2P | `libp2p-noise` (`snow`, X25519), identidad Ed25519 | `src/network/p2p.rs` | Logaritmo discreto | Descifrar tráfico grabado: qué nodo mandó primero cada tx (metadatos, no fondos). Suplantar un nodo, sin tocar el consenso | **Abierto** |
| Snapshots por iroh | QUIC (`noq`) con `rustls` + `ring` y el grupo híbrido X25519MLKEM768 propio | `src/network/pq_tls.rs` | Redes (ML-KEM) + logaritmo discreto | Nada mientras ML-KEM aguante: el secreto de la sesión lleva las dos mitades | **Cerrado** en las redes de Quantic (Zenith sigue con X25519) |
| Tor (Arti) | ntor (X25519) | `zkapriv`, ZSilent | Logaritmo discreto | Desanonimizar circuitos grabados | Depende del proyecto Tor |

Lo que solo usa Zenith (`bulletproofs`, `merlin`, `bls12_381`/Groth16,
Ed25519 en las tx) no corre en una red de Quantic y queda fuera de este
inventario.

## Lo que sí resiste aunque el gasto esté abierto

- **Montos y destinatarios ya publicados.** Los compromisos de Pedersen y
  Sinsemilla ocultan de forma perfecta (con su aleatoriedad), y los sobres van
  con ML-KEM: grabar la cadena hoy no sirve para leerla mañana.
- **Quién gasta.** La clave de verificación de cada gasto (`rk`) se
  re-aleatoriza; con Shor se obtiene `ask + α`, que no une dos gastos entre sí.
  La privacidad de los nullifiers ante un atacante cuántico está pendiente de
  comprobar contra la especificación de Orchard y la ZIP 2005.

O sea: hoy Zoka Quantic es postcuántica en **privacidad**, no en **integridad**.
Lo abierto es poder robar y crear monedas, y eso es lo que tiene que cerrar el
pool nuevo antes de la mainnet.

## Cómo se cierra lo abierto

1. **Gasto, pruebas y compromisos** (todo lo de `orchard`): un pool nuevo con un
   sistema de pruebas basado en hashes (STARK, sin curvas elípticas), compromisos
   y árbol con un hash apto para pruebas, y la autorización del gasto **adentro
   de la prueba** (se prueba conocer la clave que deriva la dirección y la prueba
   queda atada al txid), así no hace falta una firma re-aleatorizable. El
   balance se prueba dentro del circuito, sin compromisos homomórficos. X-Wing
   sigue para los sobres. Hay que elegir biblioteca, medir el tamaño de las
   pruebas y el tiempo en el teléfono de 32 bits, y pasar por auditoría externa.
2. **Direcciones**: en el pool nuevo, solo hashes y la clave X-Wing; no queda nada
   del que sacar una clave de visión.
3. **Transporte**: iroh ya está cerrado (21/10, `src/network/pq_tls.rs`): el
   grupo X25519MLKEM768 sobre el proveedor de `ring`, primero en `kx_groups`, con
   X25519 detrás y la mitad clásica ofrecida aparte, así un par viejo sigue
   conectando. Probado por QUIC real: con los dos lados híbridos se negocia el
   grupo híbrido; con un lado clásico, la conexión sigue. Para libp2p no hay Noise postcuántico: evaluar QUIC con el
   mismo proveedor, o un handshake híbrido propio. Identidad de los nodos: al
   final, no toca fondos.
4. **Tor**: seguir lo que haga el proyecto Tor; no está en nuestras manos.

## Atacante cuántico simulado

Pruebas que le dan al atacante lo que obtendría con Shor —la clave privada de
cada clave pública de curva elíptica que ve— y comprueban qué puede hacer:

- Con la `ivk` de Orchard de una dirección (lo que se saca de `pk_d`), no puede
  descifrar los sobres ni los bloques compactos de esa dirección.
- Con la clave Ed25519 de la autoridad, no consigue que un nodo de Quantic
  acepte un checkpoint ni un snapshot (falta la mitad ML-DSA).
- En el pool nuevo: con todas las claves de curva que vea, no puede gastar, ni
  crear valor, ni cambiar una tx del mempool. Hoy esa prueba fallaría con Orchard:
  es justamente lo que falta.

Y un control del inventario: una prueba que falla si una parte marcada como
cerrada empieza a depender de criptografía clásica nueva.
