# Nodo y minero CLI 4.0.4

Preparación del 3/11/2026. Estado: **en verificación, sin publicar**.

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
- [ ] Compilar Linux en el VPS con Debian 11 y comprobar el límite de glibc.
- [x] Ejecutar las pruebas del nodo STARK y del pool privado: 55 + 22 pasan.
- [ ] Probar los dos paquetes con datos temporales, sin minería.
- [x] Verificar el nodo y el manifiesto dentro del MSI de ZSilent 4.0.6.
- [ ] Publicar y descargar otra vez los archivos para verificar sus hashes.
- [ ] Actualizar y comprobar los enlaces de zoka.network.

## Resultados del 3/11

- Windows: `cargo build --release --offline --locked --features randomx,quantic-pq --bins`.
  El ZIP extraído pasa todos sus hashes; `status` y `node-id` funcionan en una
  ruta con espacios; los identificadores se conservan entre corridas. El
  ejecutable contiene seed3 y las dos IP fijas.
- Nodo STARK: 55 pruebas pasan, una prueba con Tor externo queda ignorada.
  Pool privado: 22 pruebas pasan. Registro local: `target/release404-tests.log`.
- ZSilent: 84 pruebas sin fallas. Se habilitaron la prueba v2 contra un nodo
  real (3,1 s) y la de Tor contra la testnet (47,8 s, al menos 50 bloques).
  La prueba heredada de la wallet v1, condicionada a un RPC externo, no se
  ejecutó. MSI extraído por `msiexec /a`: coinciden nodo, zokahd, manifiesto de
  tres semillas y Tor con `release-manifest.json`. No se instaló en la PC.
- Web: siete pruebas y armado de producción aprobados.

| Archivo | SHA-256 |
| --- | --- |
| `zoka.exe` | `1351c8e00af1d8cdeb95432345e7e4e09dda162061a461cb3756f5b70454cc87` |
| `zoka-cli-miner-4.0.4-windows-x64.zip` | `879a95f20c67add9356a5ecd79ad70c17345df7e30a93b194e1703730e888d59` |
| `ZSilent-Core-4.0.6.msi` | `8e55e5438b2be5b609d8cdf2826e8293b0b9c66493d40d99c100cde43b06a763` |
