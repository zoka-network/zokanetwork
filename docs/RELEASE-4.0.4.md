# Nodo y minero CLI 4.0.4

Preparación del 3/11/2026. Estado: **publicado y verificado**.

La testnet v2 conserva su cadena y sus reglas de consenso. Esta versión agrega
seed3 y las direcciones IP fijas de seed2 y seed3 al manifiesto incorporado en
el nodo. Las dos semillas funcionan en VPS; la PC del equipo se conecta cuando
su dueño la abre. Los dos VPS comparten proveedor y centro de datos.

- Las semillas con IP pública pueden fijar el puerto UDP con `--bind`.
  `scripts/vps/semilla-vps.sh` prepara un servidor como semilla sin minería.
- El minero CLI vuelve a levantar sus procesos si se cortan y ofrece arranque
  opcional con Windows o systemd de usuario en Linux.
- El motor móvil obtiene páginas por P2P, aprende pares y comprueba las páginas
  de escaneo contra sus cabeceras y el checkpoint híbrido de la testnet.
- Tor sigue siendo opcional; seed2 conserva su identidad y su dirección .onion.

Los paquetes del minero no incluyen XMRig. ZSilent 4.0.6 incorpora este nodo y
el manifiesto actualizado. ZKAPriv 3.0 tiene su propia validación y publicación:
el soporte móvil del nodo no significa que el APK ya esté publicado.

## Verificación requerida

- [x] Compilar Windows desde la fuente actual con `randomx,quantic-pq`.
- [x] Compilar Linux en el VPS con Debian 11 y comprobar el límite de glibc.
- [x] Ejecutar las pruebas del nodo STARK y del pool privado: 55 + 22 pasan.
- [x] Probar los dos paquetes con datos temporales, sin minería.
- [x] Verificar el nodo y el manifiesto dentro del MSI de ZSilent 4.0.6.
- [x] Publicar y descargar otra vez los archivos para verificar sus hashes.
- [x] Actualizar y comprobar los enlaces de zoka.network.

## Resultados del 3/11

- Windows: `cargo build --release --offline --locked --features randomx,quantic-pq --bins`.
  El ZIP extraído pasa todos sus hashes; `status` y `node-id` funcionan en una
  ruta con espacios; los identificadores se conservan entre corridas. El
  ejecutable contiene seed3 y las dos IP fijas.
- Linux: compilado en Debian 11 dentro del VPS 2, sin WSL (24 min 31 s).
  El binario requiere glibc 2.30. Se corrigió el empaquetador para aceptar
  `Cargo.toml` con finales CRLF; no cambió la fuente del ejecutable.
  En el VPS 1, el paquete extraído verifica todos sus hashes y conserva su
  identidad. Un nodo nuevo llegó al bloque 3958 en 453 s, con dos pares
  salientes, `relay: false`, sin Stratum ni minería, y checkpoint 3923
  seguido. Datos de prueba: `/tmp/zoka404-smoke-ts9jlxos`.
- Nodo STARK: 55 pruebas pasan, una prueba con Tor externo queda ignorada.
  Pool privado: 22 pruebas pasan. Registro local: `target/release404-tests.log`.
- ZSilent: 84 pruebas sin fallas. Se habilitaron la prueba v2 contra un nodo
  real (3,1 s) y la de Tor contra la testnet (47,8 s, al menos 50 bloques).
  La prueba heredada de la wallet v1, condicionada a un RPC externo, no se
  ejecutó. MSI extraído por `msiexec /a`: coinciden nodo, zokahd, manifiesto de
  tres semillas y Tor con `release-manifest.json`. No se instaló en la PC.
- Web: siete pruebas y armado de producción aprobados.

## Publicación

- [Nodo/minero v4.0.4](https://github.com/zoka-network/zokanetwork/releases/tag/v4.0.4):
  etiqueta anotada en `50588833`, Windows, Linux y `SHA256SUMS.txt`.
- [ZSilent v4.0.6](https://github.com/zoka-network/zsilent-core/releases/tag/v4.0.6):
  etiqueta anotada en `162743d`, MSI, manifiesto y `SHA256SUMS.txt`.
- Todos los archivos se descargaron de GitHub y coinciden con los originales.
  Las versiones 4.0.3 del nodo y 4.0.5 de ZSilent conservan sus archivos y
  ahora indican que fueron reemplazadas.
- Web `89bef6a`, desplegada en `https://b328a0cf.zokaweb.pages.dev` y
  zoka.network. JS/CSS públicos verificados por SHA-256; las tres descargas
  responden HTTP 200. Código y etiquetas en GitHub y GitLab.

| Archivo | SHA-256 |
| --- | --- |
| `zoka.exe` | `1351c8e00af1d8cdeb95432345e7e4e09dda162061a461cb3756f5b70454cc87` |
| `zoka-cli-miner-4.0.4-windows-x64.zip` | `879a95f20c67add9356a5ecd79ad70c17345df7e30a93b194e1703730e888d59` |
| `zoka-cli-miner-4.0.4-linux-x64.tar.gz` | `94373ee629ae9aa4492ae35ffecd1130e284e4120934b8a3c135953d9fced645` |
| `ZSilent-Core-4.0.6.msi` | `8e55e5438b2be5b609d8cdf2826e8293b0b9c66493d40d99c100cde43b06a763` |
