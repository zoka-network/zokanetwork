# Nodo, minero y ZSilent 5.1.1, ZKAPriv 4.0.2: relays propios y kit para exchanges

**Armada el 26/11/2026, sin publicar.** No es una actualización de red: la 5.1.1
funciona con los nodos 5.1.0, y la cadena y el consenso no cambian.

## Qué trae

- **Relays propios.** relay1.zoka.network (VPS 1) y relay2.zoka.network
  (VPS 2), con iroh-relay 1.3.0 en el 443 y certificados de Let's Encrypt;
  los de n0 quedan de respaldo (`src/network/relays.rs`, preset `ZokaN0`). La
  búsqueda de direcciones sigue por n0. Probado en vivo: dos nodos se
  encontraron por cada relay.
- **Direcciones integradas** (`zqtesti2…`, `zqi2…` en la mainnet): la corta de
  una billetera más un código de cliente de 8 bytes; quien paga escribe
  `zoka-tag:<código>` al principio del memo cifrado. Las pagan la CLI
  (`send --to`), ZSilent, ZKAPriv y la billetera web. Sin cambio de consenso.
- **Servidor de billetera para exchanges:** `zoka quantic-pq-wallet-rpc`,
  JSON-RPC 2.0 en 127.0.0.1 con token, retiros idempotentes por
  `request_id`, depósitos por código y firmeza por checkpoints. Guía:
  [EXCHANGE-INTEGRATION.md](EXCHANGE-INTEGRATION.md).
- **Motor liviano:** varios pagos pendientes a la vez, el código de cliente y
  el txid de cada nota, y qué notas son vuelto. Formato de estado 4: después de
  actualizar, cada billetera vuelve a leer la cadena una vez (la CLI, la web y
  ZKAPriv lo hacen solas).
- **CLI:** `integrated-address [--tag N]` y `parse-address --address TEXTO`.
- **Releases firmadas** desde la 5.1.0: cada carpeta lleva
  `SHA256SUMS.txt.sig` (clave `61c2afa5…`).

## Ramas

| Repo | Rama | Commit | Qué |
| --- | --- | --- | --- |
| ZOKA | `kit-exchanges` (incluye `relays-propios`) | `6ef0da2d` | nodo 5.1.1, kit, relays, guía del minero |
| zsilent-core | `kit-exchanges` (worktree `C:\tmp\zoka-wt\zsilent-sin-cola`) | `d863f8b` | ZSilent 5.1.1, paga integradas |
| zkapriv | `kit-exchanges` | `5310f51` + las `.so` | ZKAPriv 4.0.2, paga integradas |
| zokawallet | `integradas` | `32f4771` | paga integradas; WASM de ZOKA `e2cc03a1` |
| zokaweb | `descargas-5.1.1` | `eed594e` | descargas, relays, roadmap y «For exchanges» |
| ZOKA | `revision-externa` | `8c253ee2` | `SECURITY.md` y `docs/EXTERNAL-REVIEW.md` (no sale con la release) |
| ZOKA | `diseno-pools` | `8aae6e66` | diseño de pools para aprobar (no sale con la release) |

Todas salen de su `main` actual y se integran por avance rápido.

## Archivos: `target/release-5.1.1/`

| Archivo | SHA-256 |
| --- | --- |
| `node/zoka-cli-miner-5.1.1-windows-x64.zip` | `e7e5d5537f57083b1b4a18cf2c13a3aa78bfbf7dc003fa10c8127feb897a0a05` |
| `node/zoka-cli-miner-5.1.1-linux-x64.tar.gz` | pendiente |
| `node/zoka-5.1.1-linux-x64` (glibc 2.30+) | pendiente |
| `zsilent/ZSilent-Core-5.1.1.msi` (Tor 0.4.9.14) | `6b8bbe74de9a99c1009e3995e7bb90a67fc97d1e8a9ea1ce9f5bf3a26aa9180c` |
| `zkapriv/ZKAPriv-4.0.2.apk` | pendiente |
| `zoka.exe` (dentro del MSI y del ZIP) | `4e16ab553d3be64b04edb844aa5576d864b738c60ce6406bfb1ff85fe2bebced` |
| `zokahd.exe` | `af7fb383ed39d926397b6cac26386c916e4a402990db29a5f1e229ac9ac7e80a` |

Cada subcarpeta tiene su `SHA256SUMS.txt` y su `SHA256SUMS.txt.sig`. El MSI
se armó con `-TorBundleDir third_party\tor\windows-x86_64-0.4.9.14`: la
carpeta por defecto todavía tiene la 0.4.9.13 y el script la rechaza.

## Verificación

- Pool: 56 pruebas sin fallas (integradas, varios pendientes, vector fijo).
- Nodo: las 7 de `stark_wallet`, con el ensayo de punta a punta del exchange
  (depósito con código, retiro pagado una sola vez aunque se repita el pedido).
- Billetera web: 40 pruebas (1 saltada como antes) y `npm run build`.
- ZSilent: `IntegratedAddressRecognitionTest`.
- Motor de ZKAPriv: 5 pruebas en la PC; Kotlin compila.
- zoka.network: 17 pruebas y `npm run build`.
- El MSI abierto por extracción administrativa: `zoka.exe`, `zokahd.exe` y
  `tor.exe` son los de la tabla y el manifiesto.

## Lo que falta antes de publicar

1. **Prueba en el teléfono (usuario):** instalar la 4.0.2 encima de la 4.0.1 y
   pagar desde ZKAPriv a una dirección integrada del exchange de prueba del
   VPS 2. Yo controlo por el servidor RPC que el depósito llegue con su código
   y devuelvo un retiro a ZKAPriv.
2. **OK del usuario** para integrar, etiquetar y publicar.

## Pasos para publicar (con el OK)

1. Las ramas a `main` por avance rápido; etiquetas `v5.1.1` en ZOKA y
   zsilent-core, `v4.0.2` en zkapriv; en GitHub y en GitLab (443).
2. Releases en GitHub con los archivos de cada carpeta, `SHA256SUMS.txt`,
   `SHA256SUMS.txt.sig` y las notas de abajo. Latest las nuevas; 5.1.0 y
   4.0.1 pasan a Superseded.
3. VPS 1 y 2: `scripts\vps-actualizar-binario.ps1 -Binary
   target\release-5.1.1\node\zoka-5.1.1-linux-x64 -Version 5.1.1`.
4. Billetera web: `wrangler pages deploy dist` en zokawallet.
5. zoka.network: vista local, OK y despliegue (después de las releases).
6. ZSilent 5.1.1 en la PC (usuario, con ZSilent cerrada).
7. Discord (usuario): `discord-bot/notice.release-5.1.1.json`.

## Notas para GitHub

### zokanetwork `v5.1.1`: «Zoka Quantic Testnet v3 — nodo y minero 5.1.1 (relays propios y kit para exchanges)»

````text
Zoka Quantic Testnet v3: nodo y minero CLI 5.1.1. **No es una actualización de red:** funciona con los nodos 5.1.0 y la cadena sigue como está. Actualizar es recomendable, no obligatorio.

**Qué cambia**

- **Relays propios:** cuando dos nodos no se pueden conectar directo, se encuentran por relay1.zoka.network y relay2.zoka.network, con los relays de n0 de respaldo. El tráfico sigue cifrado de punta a punta con TLS híbrido postcuántico.
- **Direcciones integradas** (`zqtesti2…`): la dirección de un exchange con un código de cliente adentro, que viaja cifrado en el memo. `quantic-pq-wallet send --to` las acepta; `integrated-address` las crea y `parse-address` dice qué es cada dirección.
- **Servidor de billetera para exchanges:** `zoka quantic-pq-wallet-rpc`, JSON-RPC 2.0 con depósitos por código de cliente, retiros que nunca se pagan dos veces y firmeza por checkpoints. Guía: docs/EXCHANGE-INTEGRATION.md.
- **Varios pagos pendientes a la vez** en la billetera de la línea de comandos.

Después de actualizar, la billetera vuelve a leer la cadena una vez (~20 s).

Descargá el ZIP de Windows o el tar.gz de Linux y comprobá el archivo con `SHA256SUMS.txt` y su firma `SHA256SUMS.txt.sig` (docs/release-verification.md; huella de la clave `61c2afa5 67d79ad5 c43a89c5 82656f55 ffc888a5 4ac06ddc 8445dece 63ed5c99`). XMRig se descarga por separado con `2-get-xmrig.cmd`. El paquete de Linux funciona con glibc 2.30 o posterior. Para la billetera de escritorio, usá ZSilent 5.1.1.

**SHA-256**

```
e7e5d5537f57083b1b4a18cf2c13a3aa78bfbf7dc003fa10c8127feb897a0a05  zoka-cli-miner-5.1.1-windows-x64.zip
(pendiente)  zoka-cli-miner-5.1.1-linux-x64.tar.gz
(pendiente)  zoka-5.1.1-linux-x64
```

Compilado de `6ef0da2d`.
````

### zsilent-core `v5.1.1`: «ZSilent 5.1.1»

````text
ZSilent 5.1.1 para **Zoka Quantic Testnet v3**, con el nodo Zoka 5.1.1. No es una actualización de red: tu billetera, tus direcciones y tu saldo quedan como están.

**Qué cambia**

- **Pagos a exchanges:** ZSilent reconoce las direcciones integradas (`zqtesti2…`), la dirección de un exchange con tu código de cliente adentro, y lo manda cifrado en el memo. La nota que escribís pierde los bytes del código.
- **Relays propios:** tu nodo encuentra a los demás por relay1.zoka.network y relay2.zoka.network, con los de n0 de respaldo.

Después de instalarla, la billetera vuelve a leer la cadena una vez (~20 s).

Cerrá ZSilent antes de instalar `ZSilent-Core-5.1.1.msi`. Se instala encima de la 5.1.0. Verificá su SHA-256 con `SHA256SUMS.txt` y su firma `SHA256SUMS.txt.sig`; `release-manifest.json` identifica los binarios y el manifiesto de red incluidos.

La minería requiere XMRig.

**SHA-256**

```
6b8bbe74de9a99c1009e3995e7bb90a67fc97d1e8a9ea1ce9f5bf3a26aa9180c  ZSilent-Core-5.1.1.msi
```

Incluye `zoka.exe` `4e16ab553d3be64b04edb844aa5576d864b738c60ce6406bfb1ff85fe2bebced` y `zokahd.exe` `af7fb383ed39d926397b6cac26386c916e4a402990db29a5f1e229ac9ac7e80a` (nodo 5.1.1, `6ef0da2d`) y Tor 0.4.9.14. ZSilent compilado de `d863f8b`.
````

### zkapriv `v4.0.2`: «ZKAPriv 4.0.2 — pay exchange addresses»

````text
ZKAPriv 4.0.2 for Android — **Zoka Quantic Testnet v3**. Not a network upgrade: your wallet, recovery words, addresses and balance stay as they are.

- **Pay exchange addresses:** ZKAPriv recognises integrated addresses (`zqtesti2…`), an exchange's address with your customer code inside, and sends the code encrypted in the memo. Your own note loses the bytes the code takes.
- **Zoka's own relays:** the phone reaches the network through relay1.zoka.network and relay2.zoka.network, with n0's relays as a fallback.
- Engines for arm64-v8a and armeabi-v7a. Android 8 or later.

After updating, ZKAPriv reads the chain once more.

**Updating from 4.0.1:** install it over the old app; it has the same package and signing certificate. Check the APK against `SHA256SUMS.txt` and its signature `SHA256SUMS.txt.sig`. Signing certificate SHA-256: `061376f8c4e3bc54a01ccfa7279124568cfdffe4c23f8625b42f673eddc64532`.

**SHA-256**

```
(pendiente)  ZKAPriv-4.0.2.apk
```

Built from `5310f51` and the engines from ZOKA `6ef0da2d`.
````
