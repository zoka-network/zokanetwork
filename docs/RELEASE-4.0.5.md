# Nodo y minero CLI 4.0.5

Preparación del 3/11/2026, con el OK del usuario para publicar. Estado:
**armada y verificada; falta publicar** (las releases, las semillas y los
despliegues piden una confirmación explícita, ver abajo).

La testnet v2 conserva su cadena y sus reglas de consenso. Esta versión arregla
la demora de los pagos del 3/11 (un pago esperó 11 bloques) y deja buscar un
pago por su id. Detalle en [MEMPOOL-ENTRE-PARES.md](MEMPOOL-ENTRE-PARES.md).

- El nodo que llama también pide el mempool del par (`MEMPOOL`, `GET_TX`,
  anunciados en `Head.mempool`) y baja los pagos que le faltan. Un minero detrás
  de NAT ya no depende de que una semilla logre llamarlo. Con un nodo anterior
  no se usan: ese nodo cierra la conexión ante un pedido desconocido.
- `GET /transactions/<txid>` en la API local y en la pública: pendiente en el
  mempool o minado en uno de los últimos 720 bloques.

**Los mineros tienen que actualizar** para recibir los pagos al instante: el
arreglo funciona en el nodo que llama. ZSilent 4.0.7 incorpora este nodo.

## Verificación

- [x] Pruebas del nodo STARK en Windows: 57 pasan, una con Tor externo ignorada.
- [x] Compilar Windows con `randomx,quantic-pq` (6 min 11 s) y armar el ZIP.
- [x] Compilar Linux en el VPS 2 con Debian 11 (6 min) y armar el tar.gz.
  Requiere glibc 2.30.
- [x] Probar los dos paquetes con datos temporales, sin minería. Windows:
  hashes, `status`, identidad estable en una ruta con espacios y semillas
  incorporadas. Linux (VPS 1): hashes, versión e identidad; sincronizó desde
  cero hasta el bloque 4048 en 538 s contra las semillas 4.0.4, con 2 pares
  salientes y 2 entrantes, `relay: false`, sin Stratum, checkpoint 4022 seguido.
- [x] ZSilent 4.0.7: 89 pruebas sin fallas. MSI extraído con `msiexec /a`:
  `zoka.exe`, `zokahd.exe`, `tor.exe` y el manifiesto de red coinciden con
  `release-manifest.json`.
- [x] `main` y etiquetas anotadas en GitHub y GitLab: nodo `v4.0.5` =
  `7ff77929`, ZSilent `v4.0.7` = `3f1e55f`.
- [ ] Crear las releases y descargar otra vez los archivos para verificar sus
  hashes. Marcar 4.0.4 y 4.0.6 como reemplazadas.
- [ ] Actualizar seed3 y después seed2 (con el explorador) y comprobar que
  siguen la cadena. Guardar el binario anterior como `zoka-4.0.4`.
- [ ] Desplegar ZokaScan (`explorer-quantic-v2` = `056eea8`, `dist/` armado) y
  zoka.network (rama `descargas-4.0.7` = `8793fa7`, `dist/` armado).
- [ ] Anuncio de Discord: [ANUNCIO-4.0.5-BORRADOR.md](ANUNCIO-4.0.5-BORRADOR.md).

El 3/11 a la noche el clasificador de permisos frenó `gh release create`
(superficie pública) y la escritura en los VPS: hace falta la confirmación
explícita del usuario para esos pasos.

| Archivo | SHA-256 |
| --- | --- |
| `zoka.exe` | `63a7099666c481636d3a3675746add0a9fb325638ba1c4b078f5c2fe16b2c78e` |
| `zokahd.exe` | `e483e007db3ae5879a793a929de42adc921994f9065d70fc81638eaa99853fdd` |
| `zoka-cli-miner-4.0.5-windows-x64.zip` | `9471b510752c55f0b4a8afb82db46ed91068b21dfb7b9485f3de9dda809f4e3d` |
| `zoka-cli-miner-4.0.5-linux-x64.tar.gz` | `7b37031fdca5567375f9f0d31743b14dc82c0e4374e796ebb072bd84038dfa1c` |
| `ZSilent-Core-4.0.7.msi` | `f66ee0192e87f7e0539043223db8aa6304629d0983ac204c713c7f3e12ec3c93` |

Archivos para subir: `target/cli-miner/4.0.5/publicar/` y
`target/zsilent-4.0.7/build/release/4.0.7/publicar/`, cada uno con su
`SHA256SUMS.txt`. Las notas de cada release están al lado, en
`notas-release.md`.
