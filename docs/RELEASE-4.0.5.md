# Nodo y minero CLI 4.0.5

Preparación del 3/11/2026, con el OK del usuario para publicar. Estado: **en
preparación**.

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

## Verificación requerida

- [x] Pruebas del nodo STARK en Windows: 57 pasan, una con Tor externo ignorada.
- [ ] Compilar Windows con `randomx,quantic-pq` y armar el ZIP.
- [ ] Compilar Linux en el VPS con Debian 11 y armar el tar.gz.
- [ ] Probar los dos paquetes con datos temporales, sin minería.
- [ ] Verificar el nodo dentro del MSI de ZSilent 4.0.7.
- [ ] Publicar y descargar otra vez los archivos para verificar sus hashes.
- [ ] Actualizar seed2 y seed3 y comprobar que siguen la cadena.
- [ ] Actualizar los enlaces de zoka.network.
