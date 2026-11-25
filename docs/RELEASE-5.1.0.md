# Nodo, minero y ZSilent 5.1.0: envíos rápidos y prueba v4 (actualización de red)

Preparación del 25/11/2026. **Primera actualización de red de la testnet v3: la
cadena sigue, no se reinicia.** Detalle técnico en [PRUEBA-V4.md](PRUEBA-V4.md).

## Qué trae

- **Billetera de la PC sobre el motor liviano** (el de ZKAPriv y la web): el
  saldo y el historial leen solo los bloques nuevos. 0,2 s por consulta en vez
  de ~30 s; la primera vez lee la cadena entera una sola vez (~20 s). La carpeta
  `wallet-mirror` (1 GB) deja de usarse y se puede borrar.
- **ZSilent sin cola:** una sola lectura de saldo a la vez y solo cuando entra
  un bloque; el envío pasa primero. Un envío dejó de esperar minutos.
- **Prueba de pago v4** (circuito angosto): 0,74 MB en vez de 2,59 MB (2 notas)
  y 0,79 MB en vez de 7,85 MB (8 notas); probar 2-3 veces más rápido y
  verificar un 25-40 % más rápido. Mismas notas, mismo árbol, mismas
  direcciones y la misma seguridad estimada (128 bits clásicos).
- **Activación en el bloque 20.000 de la testnet:** desde ahí los pagos llevan
  la prueba v4; los de la v3 valen hasta el 20.099. Las billeteras eligen la
  versión solas.

## Antes del bloque 20.000 tienen que actualizar

- Las tres autoridades de checkpoints (PC, VPS 1 y VPS 2) y las semillas.
- El faucet (VPS 1).
- Mineros y usuarios de ZSilent, ZKAPriv y la billetera web.

Un nodo que no actualice rechaza los bloques con pagos v4 y queda en otra rama.
ZKAPriv 4.0.0 sigue mostrando el saldo (no verifica pruebas), pero desde el
bloque 20.100 sus pagos v3 ya no entran.

**Fecha:** el 25/11 a las 11:30 la red iba por el bloque 9.834. A 2 minutos por
bloque, el 20.000 llega cerca del **9/12**. Si la publicación no llega a tiempo,
se cambia `TESTNET_PROOF_V4_HEIGHT` y se recompila todo antes de publicar.

## Verificación

- [x] `zoka-pq-pool`: todas las pruebas, más las 7 del circuito v4 y la de la
  regla de activación.
- [x] `quantic-pq-wallet`: punta a punta contra la API HTTP de un nodo de
  regtest (saldo, historial, pago reservado, minado y recibido, estado dañado).
- [x] Todo `quantic::` del nodo (104), con el ensayo de la activación en
  regtest: el pago después del bloque 8 sale con v4, el nodo lo toma y se mina.
- [x] Billetera web: `npm test` (38, 1 saltada como antes) y `npm run build`
  con el WASM nuevo.
- [x] `.so` de ZKAPriv (arm64-v8a y armeabi-v7a) con los dominios de la v3 y la
  v4; APK 4.0.1 (código 21) firmado con el mismo certificado de la 4.0.0
  (`061376f8…`, comprobado con `apksigner`).
- [x] zoka.network: `npm test` (17) y `npm run build` con las descargas nuevas.
- [ ] La v4 medida en el teléfono de 32 bits (el teléfono no estaba conectado).
- [ ] Publicar, instalar en la PC y en los VPS, faucet, Discord (usuario).

## Ramas (subidas a GitHub y GitLab el 25/11; ninguna en `main`)

| Repo | Rama | Commit | Qué |
| --- | --- | --- | --- |
| ZOKA | `prueba-v4` (contiene `billetera-motor-liviano`) | `d77aa6c7` + documentos | nodo y minero 5.1.0 |
| zsilent-core | `billetera-sin-cola` | `5e0ee8b` | ZSilent 5.1.0 |
| zkapriv | `red-v4` | `a3ec58f` | ZKAPriv 4.0.1 |
| zokawallet | `red-v4` | `66827f2` | WASM con la v4 |
| zokaweb | `descargas-5.1.0` | `7bc75bc` | descargas 5.1.0 / 4.0.1 y el mapa |

Las cinco salen de su `main` actual: se integran por avance rápido.

## Archivos (preparados, sin publicar): `target/release-5.1.0/`

| Archivo | SHA-256 |
| --- | --- |
| `node/zoka-cli-miner-5.1.0-windows-x64.zip` | `d3cb3b2477edbc2f39f4c8485c895cec1cfdba2b4a065e46228bb3ec31b6fd4e` |
| `node/zoka-cli-miner-5.1.0-linux-x64.tar.gz` | `23e376d55a874b2c736c141a516264c2a6c347df1a7dd2aa1e67c36b831950ed` |
| `node/zoka-5.1.0-linux-x64` (glibc 2.30+) | `4497894998e91cc29f2237ffe062cdee628606c85548b89b13429b8251a4ba65` |
| `zsilent/ZSilent-Core-5.1.0.msi` (Tor 0.4.9.14) | `5dab751d0dedfde84d617dc69c88127bf1eec970759173e0bec78c1cba7b64c6` |
| `zkapriv/ZKAPriv-4.0.1.apk` | `7479ee067c7d5d2be357a24449b839eb4e475a2ada1898df45c0ba666ab7ee1c` |
| `zoka.exe` (dentro del MSI y del ZIP) | `afe0489da387a740fb28d3626b932dfb7a4849e25c1b2f4538b356776ab366d4` |
| `zokahd.exe` | `ec9e673efaaebcbed9a1579b88f5f15cbbf0f458f76f414f71efc35a3b5c0206` |
| `zoka-linux-x64` (para el script de los VPS; = `node/zoka-5.1.0-linux-x64`) | `4497894998e91cc29f2237ffe062cdee628606c85548b89b13429b8251a4ba65` |

Cada subcarpeta tiene su `SHA256SUMS.txt` (comprobado con `sha256sum -c`);
`zsilent/` también lleva `release-manifest.json`. El MSI se llama con guion
(`ZSilent-Core-…`) porque así lo enlaza zoka.network: GitHub cambia los
espacios por puntos.

Los binarios del nodo salen de `d77aa6c7` (el mismo código que `4d30d5fd`, más
el `Cargo.lock`); el MSI, de zsilent-core `5e0ee8b`; el APK, de zkapriv
`a3ec58f`.

## Pasos para publicar (con el OK del usuario)

1. **Integrar y etiquetar** (lo puedo hacer yo con el OK): las cinco ramas a
   `main` por avance rápido; etiquetas `v5.1.0` en ZOKA y zsilent-core, y
   `v4.0.1` en zkapriv; en GitHub y en GitLab (443).
2. **Releases en GitHub** con los archivos de cada subcarpeta, su
   `SHA256SUMS.txt` y las notas de abajo: zokanetwork `v5.1.0`, zsilent-core
   `v5.1.0` (más `release-manifest.json`) y zkapriv `v4.0.1`. Marcarlas Latest
   y pasar 5.0.1 / 4.0.0 a Superseded, como la vez anterior.
3. **VPS 1 y VPS 2:** `scripts\vps-actualizar-binario.ps1` con
   `target/release-5.1.0/zoka-linux-x64` (guarda el 5.0.1 como respaldo). El
   faucet usa `--api` del nodo local: el motor nuevo lee `/scan2` de ahí, y
   `--peer` se ignora.
4. **ZSilent 5.1.0 en la PC** (con ZSilent cerrada).
5. **Billetera web:** `wrangler pages deploy dist` en zokawallet.
6. **zoka.network:** después de las releases (si no, las descargas dan 404).
   Vista local con `npm run preview` en zokaweb y, con el OK, el despliegue
   de siempre (`--branch=main`, cuenta de Zoka).
7. **Discord:** `discord-bot/notice.upgrade-5.1.0.json` con `send-notice.js`
   (vista previa primero).
8. Todo antes del bloque 20.000 (~9/12).

## Notas para GitHub

### zokanetwork `v5.1.0`: «Zoka Quantic Testnet v3 — nodo y minero 5.1.0 (actualización de red en el bloque 20.000)»

````text
Zoka Quantic Testnet v3: nodo y minero CLI 5.1.0. **Es la primera actualización de red de la testnet v3: la cadena sigue, no se reinicia nada.** Saldos, frases de recuperación y direcciones quedan como están.

**Qué cambia**

- **Prueba de pago v4** (circuito angosto): 0,74 MB en vez de 2,59 MB para un pago de 2 notas y 0,79 MB en vez de 7,85 MB para uno de 8. Probar es 2 a 3 veces más rápido y verificar hasta un 40 % más rápido. Las mismas notas, el mismo árbol, las mismas direcciones y la misma seguridad estimada.
- **Activación en el bloque 20.000:** desde ahí los pagos llevan la prueba v4; los pagos con la v3 valen hasta el bloque 20.099. Las billeteras eligen la versión solas.
- **Billetera de la línea de comandos sobre el motor liviano** (el de ZKAPriv y la billetera web): `quantic-pq-wallet` lee solo los bloques nuevos. El saldo tarda 0,2 s en vez de ~30 s; la primera vez lee la cadena una sola vez (~20 s). La carpeta `wallet-mirror` ya no se usa y se puede borrar.

**Actualizá antes del bloque 20.000.** Un nodo 5.0.x rechaza los bloques con pagos v4 y queda afuera de la cadena. Hasta ese bloque, los nodos 5.0.1 y 5.1.0 se hablan entre sí.

Descargá el ZIP de Windows o el tar.gz de Linux y comprobá el archivo con `SHA256SUMS.txt`. XMRig se descarga por separado. El paquete de Linux funciona con glibc 2.30 o posterior. Para la billetera de escritorio, usá ZSilent 5.1.0.

**SHA-256**

```
d3cb3b2477edbc2f39f4c8485c895cec1cfdba2b4a065e46228bb3ec31b6fd4e  zoka-cli-miner-5.1.0-windows-x64.zip
23e376d55a874b2c736c141a516264c2a6c347df1a7dd2aa1e67c36b831950ed  zoka-cli-miner-5.1.0-linux-x64.tar.gz
4497894998e91cc29f2237ffe062cdee628606c85548b89b13429b8251a4ba65  zoka-5.1.0-linux-x64
```

Compilado de `d77aa6c7`.
````

### zsilent-core `v5.1.0`: «ZSilent 5.1.0»

````text
ZSilent 5.1.0 para **Zoka Quantic Testnet v3**, con el nodo Zoka 5.1.0. Trae la primera actualización de red de la testnet v3, en el bloque 20.000: **la cadena sigue, y tus billeteras, tus direcciones y tu saldo quedan como están.**

**Qué cambia**

- **Envíos rápidos:** ZSilent ya no apila lecturas de saldo. Hace una a la vez, solo cuando entra un bloque, y el envío pasa primero. Un envío ya no espera minutos detrás de esas lecturas.
- **Saldo en 0,2 s:** la billetera usa el motor liviano de ZKAPriv y la billetera web y lee solo los bloques nuevos. La primera vez lee la cadena una sola vez (~20 s). La carpeta `wallet-mirror` ya no se usa y se puede borrar.
- **Prueba de pago v4** desde el bloque 20.000: 0,74 MB en vez de 2,59 MB (2 notas) y 0,79 MB en vez de 7,85 MB (8 notas), y 2 a 3 veces más rápida de armar. ZSilent elige la versión sola.

**Instalala antes del bloque 20.000:** con la 5.0.1, tu nodo queda afuera de la cadena después de ese bloque.

Cerrá ZSilent antes de instalar `ZSilent-Core-5.1.0.msi`. Se instala encima de la 5.0.1. Verificá su SHA-256 con `SHA256SUMS.txt`; `release-manifest.json` identifica los binarios y el manifiesto de red incluidos.

La minería requiere XMRig.

**SHA-256**

```
5dab751d0dedfde84d617dc69c88127bf1eec970759173e0bec78c1cba7b64c6  ZSilent-Core-5.1.0.msi
```

Incluye `zoka.exe` `afe0489da387a740fb28d3626b932dfb7a4849e25c1b2f4538b356776ab366d4` y `zokahd.exe` `ec9e673efaaebcbed9a1579b88f5f15cbbf0f458f76f414f71efc35a3b5c0206` (nodo 5.1.0, `d77aa6c7`). ZSilent compilado de `5e0ee8b`.
````

### zkapriv `v4.0.1`: «ZKAPriv 4.0.1 — network upgrade at block 20,000» (en inglés, como la 4.0.0)

````text
ZKAPriv 4.0.1 for Android — **Zoka Quantic Testnet v3**.

This update brings the first network upgrade of testnet v3, at block 20,000. **The chain continues:** your wallet, recovery words, addresses and balance stay as they are.

- **Proof v4:** from block 20,000 the phone builds each payment's proof on a narrow circuit: 0.74 MB instead of 2.59 MB for a payment of two notes, and 0.79 MB instead of 7.85 MB for eight notes. ZKAPriv picks the version by itself.
- Engines for arm64-v8a and armeabi-v7a. Android 8 or later.

**Update before block 20,000.** Payments with the old proof stay valid up to block 20,099; from block 20,100, ZKAPriv 4.0.0 still shows your balance but can no longer send.

**Updating from 4.0.0:** install it over the old app; it has the same package and signing certificate. Check the APK against `SHA256SUMS.txt`. Signing certificate SHA-256: `061376f8c4e3bc54a01ccfa7279124568cfdffe4c23f8625b42f673eddc64532`.

**SHA-256**

```
7479ee067c7d5d2be357a24449b839eb4e475a2ada1898df45c0ba666ab7ee1c  ZKAPriv-4.0.1.apk
```

Built from `a3ec58f`.
````
