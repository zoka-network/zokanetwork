# Billetera web (wallet.zoka.network)

Estado al 4/11/2026. Rama `billetera-web` del nodo (W1–W4) y repo nuevo
`zokawallet` (al lado de `zokaexplorer`, ignorado por este repo).

## Qué es

Una billetera de Zoka Quantic Testnet v2 que corre entera en el navegador:
React + Vite + TypeScript, sin otras dependencias de ejecución que React. Lo que
toca las claves corre en un Web Worker con el WebAssembly de este repo. El mismo
Rust lo usan el nodo y el teléfono:

| Crate | Qué aporta |
| --- | --- |
| `crates/zoka-pq-pool` | `LightWallet`: páginas `/scan2`, saldo, historial, plan y prueba STARK del pago, reserva |
| `crates/zoka-light` (W1) | Checkpoint híbrido (Ed25519 + ML-DSA-65) y estampilla Argon2id de la dirección corta, sin RocksDB ni iroh |
| `crates/zoka-pq-wallet-wasm` (W2, W4) | `WebWallet` y funciones para JS: frase, direcciones, clave de vista, `deriveKey`, `qrMatrix`, `shortOf` |

La frase es la misma de ZSilent y ZKAPriv: 24 palabras BIP-39, cuya entropía es
la semilla, y la cuenta sale de `Account::from_seed(semilla, 2, 0)`.

## Qué ve el nodo

La billetera solo habla con `api.zokascan.cc` (seed2, `--public-api`):

- `GET /status`, `GET /scan2?from=H&count=100` y `GET /checkpoint`. Las páginas
  son iguales para cualquiera que las pida: el nodo no sabe qué billetera lee.
- `GET /address/<corta>`: la billetera comprueba que la completa que recibe
  corresponda a la corta (`matchesShort`).
- `POST /transactions` (W3): el pago ya probado. El nodo lo toma como uno de un
  par y lo reparte por P2P.
- `POST /address` (W3): el registro de la corta, con la estampilla que el
  navegador ya encontró. El nodo público la verifica, pero nunca la busca.

Cupos de las escrituras en la API pública (W3):

| | Por cliente (`CF-Connecting-IP`, IPv6 por /64) | En total |
| --- | --- | --- |
| Pagos | 3 seguidos, y después 1 cada 20 s | 6 seguidos, y después 1 cada 3 s |
| Registros | 2 seguidos, y después 1 por minuto | 6 seguidos, y después 1 cada 5 s |

Las escrituras tienen 2 permisos propios y 60 s de plazo (un pago real pesa
1,94 MB: bloque 4079). Una subida lenta no ocupa los permisos de lectura. CORS
permite `POST` con `Content-Type` y expone `Retry-After`.

## Qué guarda el navegador

La bóveda está en IndexedDB. La frase, el nombre y el estado leído de cada
billetera se sellan con AES-256-GCM. La clave sale de la contraseña con Argon2id
(64 MiB, 3 pasadas, sal al azar de 16 bytes) y no se puede extraer de WebCrypto.
Los datos asociados de cada blob indican su billetera y su función, así que un
blob no se puede pasar a otra billetera. Una sola contraseña abre todas. Sin
movimiento durante 10 minutos, la billetera se bloquea, salvo en medio de un
pago. Cambiar la contraseña vuelve a sellar todo en una sola escritura.

`public/_headers` define los encabezados que manda Cloudflare Pages:

- CSP `default-src 'none'`, con scripts propios y `'wasm-unsafe-eval'`;
  `connect-src` permite solo el sitio y `https://api.zokascan.cc`;
- `frame-ancestors 'none'`, HSTS y `Referrer-Policy: no-referrer`;
- la cámara queda permitida solo para el sitio, para leer QR con
  `BarcodeDetector`.

Cada armado publica `/SHA256SUMS.txt`, y `src/wasm/SOURCE.txt` registra el
commit y los hashes del WASM copiado.

## Pruebas

- Nodo (rama `billetera-web`):
  - `zoka_light_equivalence`: 4 pruebas.
  - `stark_api`: 10 pruebas, más 2 que se corren a mano (abajo). El pago de
    la billetera liviana entra por la API pública. Hay una prueba nueva de
    límites, CORS, estampilla y permisos. Todo `quantic::`: 96 pasan.
  - `zoka-light`: 3 pruebas.
  - WASM nativo: 4 pruebas.
- `zokawallet`: 30 pruebas.
  - Montos y enlaces de pago.
  - El núcleo, con el WASM real, una IndexedDB falsa y tres bloques reales de
    la testnet. Cubre crear, desbloquear, la contraseña equivocada, que no
    queden secretos en claro, publicar la corta una sola vez, varias
    billeteras, el cambio de contraseña, los blobs cambiados de lugar,
    resolver una corta falsa y el pago sin fondos.
  - El cliente de la API.
  - Las pantallas.
- `zokawallet/scripts/e2e-browser.mjs`: un Chrome sin ventana con perfil propio,
  manejado por DevTools, contra la testnet real y con los mismos encabezados que
  Cloudflare. Crea la billetera, lee 4.448 bloques en 37 s, recorre las
  pestañas, bloquea y desbloquea. Los pedidos salieron solo hacia `localhost` y
  `api.zokascan.cc`, ninguno llevó las palabras y no hubo violaciones de CSP.
  `POST /address` falla por CORS hasta que seed2 tenga W3.
- El pago probado en WebAssembly lo acepta el nodo (regtest). Pasos:
  1. `cargo test --release --features quantic-pq --lib web_wallet_regtest_fixture -- --ignored`
  2. Desde `crates/zoka-pq-wallet-wasm`:
     `wasm-pack build --release --target nodejs --out-dir pkg-node --no-pack`
     y después `node tests/regtest-payment.mjs`.
  3. `cargo test --release --features quantic-pq --lib a_payment_proved_in_webassembly -- --ignored`

  Resultado del 4/11: el WASM probó el pago en 7,55 s (Node, un hilo), de
  1.937.222 bytes; la reserva sobrevive al estado guardado y un segundo pago
  espera. El nodo lo tomó por `POST /transactions` público (202), lo minó en
  el bloque 4 y Bob leyó sus 7 ZKA con la nota.

## Lo que falta (todo pide un OK explícito)

1. Publicar el nodo 4.0.6, que es 4.0.5 más W3, y actualizar seed2. Sin eso,
   la web lee pero no puede publicar la corta ni pagar.
2. Crear los repos `zokawallet` en GitHub (`zoka-network`) y en GitLab
   (`zokanetwork`), y subirlos.
3. Crear el proyecto `zokawallet` en Cloudflare Pages con wrangler, con la
   cuenta de Zoka, y desplegar una prueba en `*.pages.dev`. Después, el usuario
   agrega el dominio `wallet.zoka.network` en el panel.
4. Hacer la prueba de punta a punta con dinero: recibir desde ZSilent, pagar a
   ZKAPriv y ver el bloque.
5. Agregar en zoka.network el enlace "Web wallet" y escribir el anuncio.

## Límites conocidos

- La primera lectura baja todas las páginas desde el bloque 1: con la cadena
  de hoy son 45 páginas y unos 40 s. El cupo de páginas del nodo público es
  global (1 por segundo, con ráfaga de 10), así que muchas billeteras nuevas a
  la vez se demoran entre ellas. Más adelante se puede cachear en Cloudflare
  las páginas viejas, que no cambian, o arrancar desde un estado firmado.
- Un pago probado que no llegó a la red queda en memoria: si se cierra la
  pestaña, sus notas siguen reservadas hasta que vence, 20 bloques después.
- `BarcodeDetector` existe en Chrome de Android, ChromeOS y macOS. En el resto
  de los navegadores no aparece el botón del QR y la dirección se pega.
