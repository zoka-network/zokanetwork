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

- [ ] Compilar Windows desde la fuente actual con `randomx,quantic-pq`.
- [ ] Compilar Linux en el VPS con Debian 11 y comprobar el límite de glibc.
- [ ] Ejecutar las pruebas del nodo STARK y del pool privado.
- [ ] Probar los dos paquetes con datos temporales, sin minería.
- [ ] Verificar el nodo y el manifiesto dentro del MSI de ZSilent 4.0.6.
- [ ] Publicar y descargar otra vez los archivos para verificar sus hashes.
- [ ] Actualizar y comprobar los enlaces de zoka.network.
