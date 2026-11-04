# Nodo 4.0.6: la API pública para la billetera web

Preparación del 4/11/2026, con el OK del usuario para publicar.

Es la 4.0.5 más lo que necesita la billetera web (`wallet.zoka.network`).
Detalle en [BILLETERA-WEB.md](BILLETERA-WEB.md). La cadena, las reglas de
consenso y el P2P no cambian.

- `POST /transactions` en la API pública (`--public-api`): recibe un pago ya
  probado y lo reparte por P2P, como si llegara de un par.
- `POST /address` en la API pública: registra una dirección corta con su
  estampilla Argon2id. El nodo la verifica; nunca la busca él.
- Cupos por cliente (`CF-Connecting-IP`, IPv6 por /64) y globales, dos permisos
  de escritura aparte de los de lectura, 60 s de plazo, y CORS con `POST` y
  `Content-Type`.
- Crates nuevos para las billeteras livianas: `zoka-light` (checkpoint híbrido
  y estampilla, sin RocksDB ni iroh) y `zoka-pq-wallet-wasm` (la billetera en
  WebAssembly).

**Es una release de servidor.** Solo cambia algo en un nodo que corre con
`--public-api` (seed2, detrás de `api.zokascan.cc`). Los mineros y ZSilent
siguen con la 4.0.5 y ZSilent 4.0.7; quien ya actualizó no tiene que volver a
hacerlo. Por eso no se marca como "Latest" y lleva solo el binario de Linux que
corren las semillas.

## Verificación

- [x] Pruebas del nodo en la rama `billetera-web`: todo `quantic::` (96), con
  la prueba nueva de límites, CORS, estampilla y permisos de la API pública.
- [x] Un pago probado en WebAssembly lo acepta la API pública y se mina
  (regtest, 4/11).
- [ ] Compilar Linux en el VPS 2 con Debian 11, desde el código de la etiqueta.
- [ ] Instalar en seed3 y después en seed2, guardar el binario anterior como
  `zoka-4.0.4`, y comprobar que siguen la cadena.
- [ ] `api.zokascan.cc`: versión 4.0.6, preflight de `POST /address` con
  `Content-Type` permitido.
- [ ] Crear la release (sin "Latest") y bajar otra vez los archivos para
  verificar sus hashes.
