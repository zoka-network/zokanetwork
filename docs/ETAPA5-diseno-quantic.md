# Etapa 5: las transacciones de Zoka Quantic (Ironwood + X-Wing)

16/10/2026. **Implementado como biblioteca en
[`crates/zoka-quantic`](../crates/zoka-quantic/README.md) (rama
`etapa5-ironwood`), sin integrar al nodo ni a las apps.** Nada de esto existe en
Zenith. Sigue la decisión del 13/10 de
[ETAPA5-gasto-sin-key-images.md](ETAPA5-gasto-sin-key-images.md) y queda sujeto
a la revisión externa del punto 7.7.

## En una línea

Cada transacción de Quantic es un bundle de **Ironwood** (el circuito corregido de
Orchard, Halo 2, sin trusted setup) armado con el crate `orchard` 0.16 **sin
tocar el circuito**, con el cifrado de notas de Orchard (Diffie-Hellman sobre
Pallas) reemplazado por un sobre **X-Wing** (X25519 + ML-KEM-768).

## Qué cambia respecto de Zenith

| | Zenith (testnet) | Quantic |
|---|---|---|
| Gasto | CLSAG + key images; publica compromiso y clave de la nota gastada | Prueba Halo 2: publica solo el nullifier y la raíz del árbol |
| Grafo de gastos | Público (relevado el 11/10) | Oculto: la prueba demuestra la pertenencia al árbol sin decir cuál nota |
| Trusted setup | Groth16 de una sola persona | Ninguno |
| Montos | Pedersen + Bulletproofs | Dentro de la prueba (compromiso de valor + firma de balance) |
| Cifrado hacia el destinatario | ECDH Ristretto (v1) o X-Wing (v2, `zkq1`) | X-Wing siempre |
| Comisión | Sobre cifrado para el minero | Pública (el balance de valor del bundle), fija por acción |
| Coinbase | Publica la dirección del minero | Nota privada; solo el monto es público |
| Tamaño de un envío de 2 acciones | 59–346 KB | **11.351 bytes** |

## Claves

Todo sale de la raíz de la wallet (la entropía de la frase; 24 palabras
recomendadas, ver etapa 3):

```text
seed       = BLAKE2b-512("ZokaQuanticSeed_", raíz)                semilla ZIP 32
sk         = clave de gasto Orchard en m/32'/coin_type'/cuenta'
xwing_seed = BLAKE2b-256("ZokaQ_XWingSeed_", sk)                  semilla de la clave X-Wing
```

- `coin_type` = `0x5a4b` en mainnet y 1 en testnet y regtest (convención SLIP-44):
  la misma frase da wallets distintas en mainnet y en las redes de prueba.
- Las claves de Orchard (autorización de gasto, clave de nullifier, claves de
  vista, direcciones diversificadas) son exactamente las del crate.
- La clave X-Wing cumple el papel de clave de vista entrante: quien la tiene ve
  los pagos; sin ella nadie los distingue.
- **Una dirección por wallet** (índice 0): la clave X-Wing va en la dirección,
  así que direcciones diversificadas con la misma clave no serían desvinculables.
  El vuelto va a la dirección interna de Orchard, que nunca se muestra.
- Clave de vista exportable: red ‖ clave de vista completa de Orchard (96) ‖
  semilla X-Wing (32) = 129 bytes. Es secreta: muestra toda la historia.

## Dirección

```text
contenido = versión 0x01 ‖ dirección Orchard (43) ‖ clave X-Wing (1.216)
checksum  = 4 primeros bytes de BLAKE2b-256("ZokaQ_AddrCheck_", prefijo ‖ contenido)
dirección = prefijo ‖ Base58( F4Jumble( contenido ‖ checksum ) )
```

- Prefijos **propuestos**: `zq1` (mainnet), `zqtest1`, `zqregtest1`. Ninguno
  empieza como `zka1` ni `zkq1`, para no confundirse con las de Zenith. **A
  confirmar por el usuario.**
- Unos 1.730 caracteres (1.264 bytes). Se lee con el QR animado en partes que ya
  usan ZSilent y ZKAPriv.
- **F4Jumble** (ZIP 316, crate `f4jumble`) hace que cada carácter dependa de todos
  los bytes. Sin él, alguien podría dejar la parte Orchard de una dirección real
  (que da los primeros caracteres) y poner su propia clave X-Wing: quien compara
  solo el principio pagaría una nota que su dueño nunca encuentra.
- El checksum cubre el prefijo: una dirección de testnet no se decodifica como
  de mainnet.

## Cifrado de notas

Por cada acción, en lugar del cifrado de Orchard:

```text
eseed     = BLAKE2b-512("ZokaQ_KemEncSeed", rseed ‖ rho ‖ cmx)
(kem, ss) = X-Wing.Encaps(clave X-Wing del destinatario, eseed)        1.120 bytes
clave     = BLAKE2b-256("ZokaQ_NoteEncKey", ss ‖ kem ‖ cmx)
enc       = ChaCha20-Poly1305(clave, nonce 0, texto)                    580 bytes
texto     = 0x03 ‖ diversificador (11) ‖ valor (8) ‖ rseed (32) ‖ memo (512)
ock       = BLAKE2b-256("ZokaQ_OutCiphKey", ovk ‖ cv_net ‖ cmx ‖ kem)
out       = ChaCha20-Poly1305(ock, nonce 0, dirección Orchard ‖ ss)     91 bytes
```

- El texto es el de las notas V3 de Ironwood (las "recuperables" del ZIP 2005),
  sin cambios.
- **Comprobaciones al recibir** (equivalen a la de `epk = [esk] g_d` de Orchard):
  los campos descifrados tienen que reconstruir el compromiso publicado `cmx`, y
  volver a encapsular con el `eseed` de esa nota tiene que dar exactamente el
  ciphertext publicado. Un envío armado de otra forma se descarta.
- El encapsulado usa la semilla derivada de `rseed`, que es secreta y única por
  nota (la advertencia del crate `x-wing` sobre aleatoriedad repetida o pública
  no aplica). `zoka-pq` lo expone como `encapsulate_derand`, probado contra los
  vectores oficiales de X-Wing.
- Las salidas de relleno (Orchard rellena a 2 acciones) van a una clave X-Wing
  descartable: no se distinguen de las reales.
- `out` permite al emisor recuperar lo que mandó con su `ovk` (es simétrico: no
  lo rompe una computadora cuántica). Sin `ovk` es ruido.
- **Salida pública** (`ovk` de ceros): cualquiera abre `out` y comprueba quién
  recibió cuánto. Se usa para el premine. El nullifier sigue necesitando la clave
  del dueño, así que no se puede seguir su gasto.

## Transacción

| Campo | Bytes |
|---|---:|
| Versión (1), red, tipo (envío / coinbase), altura | 4 + 1 + 1 + 4 |
| Cantidad de acciones (1 a 64) | 2 |
| Por acción: `cv_net`, nullifier, `rk`, `cmx` | 4 × 32 |
| Por acción: sobre X-Wing (`kem`, `enc`, `out`) | 1.120 + 580 + 91 |
| Flags, balance de valor, ancla | 1 + 8 + 32 |
| Prueba Halo 2 (largo fijo: 2.720 + 2.272 por acción) | 4 + 7.264 con 2 acciones |
| Firmas de gasto (una por acción) y firma de balance | 64 por acción + 64 |

- **Altura:** en un coinbase, la del bloque; en un envío, la de vencimiento (0: no vence).
- Decodificación estricta: largos exactos, codificaciones canónicas, `rk` distinto
  de la identidad, prueba del largo exacto y nada sobrante.
- La red va dentro de la transacción: lo firmado para una red no vale en otra.

## Id de la transacción (A5 de la wallet liviana)

```text
encabezado = H("ZokaQ_TxHeader__", versión ‖ red ‖ tipo ‖ altura)
compacto   = H("ZokaQ_ActCompact", por acción: nullifier ‖ cmx ‖ kem ‖ enc[..52])
memos      = H("ZokaQ_ActMemos__", por acción: enc[52..])
resto      = H("ZokaQ_ActNonComp", por acción: cv_net ‖ rk ‖ out)
efectos    = H("ZokaQ_BundleEff_", compacto ‖ memos ‖ resto ‖ flags ‖ balance ‖ ancla)
txid       = H("ZokaQ_TxId______", encabezado ‖ efectos)
autoriz.   = H("ZokaQ_TxAuth____", prueba ‖ firmas de gasto ‖ firma de balance)
```

- Las firmas firman el txid, que no las incluye: re-aleatorizar una firma no
  cambia el id. El bloque compromete los txid y los digests de autorización (dos
  árboles de Merkle con la cantidad de hojas en la raíz).
- **Lo que esto cierra:** una wallet liviana recibe de cada acción solo la parte
  compacta (1.236 bytes) y, por transacción, los digests `memos` y `resto`;
  recalcula el txid y lo compara con el bloque. Un nodo ya no puede quitar ni
  cambiar una salida del resumen sin que se note: era el ataque 3 de la wallet
  liviana de Zenith.

## Reglas de consenso

- **Emisión:** premine de 5.000.000 ZKA en la altura 0; 12 ZKA por bloque del 1
  al 1.051.200 y 6 ZKA hasta el 1.948.800; después, nada. Suma exacta: 23.000.000.
- **Coinbase:** primera transacción del bloque, gastos deshabilitados, ancla del
  árbol vacío, acuña exactamente subsidio + comisiones del bloque.
- **Maduración:** las notas de un coinbase entran al árbol 60 bloques después
  (como la maduración de Zenith). Hasta entonces ninguna ancla las contiene, así
  que no se pueden gastar. El premine entra en el bloque 0: se puede gastar desde
  el bloque 1.
- **Premine:** el coinbase del génesis tiene una sola acción, que tiene que ser
  una salida pública y abrir con la dirección y el monto anunciados en los
  parámetros de la red (incluida la clave X-Wing).
- **Envíos:** gastos y salidas habilitados; balance de valor ≥ 0 (es la comisión;
  no hay otro destino para el valor); ancla = la raíz del árbol al final de algún
  bloque anterior; nullifiers nuevos y sin repetir en el bloque; sin vencer.
- **Límites:** 64 acciones por transacción y 512 por bloque (sin contar el coinbase).
- **Comisión convencional:** 0,0001 ZKA por acción, mínimo 2 acciones (estilo
  ZIP 317). La wallet paga exactamente eso, así la comisión no identifica a nadie.
  No es regla de consenso: un nodo la exigirá para retransmitir.
- Pruebas y firmas de todo el bloque se verifican en un solo lote.

## Qué protege frente a una computadora cuántica

- **Reconocer y leer notas:** X-Wing; hay que romper también ML-KEM-768. Aun
  conociendo la dirección, no se pueden encontrar ni leer sus notas (el ataque
  del ZIP 2005 con la dirección en la mano necesitaba el cifrado de curva).
- **Grafo de gastos y montos:** la transacción publica el nullifier, la raíz,
  `cmx`, un `rk` re-aleatorizado y un compromiso de valor que oculta
  perfectamente. Derivar el nullifier exige la clave de nullifier, que no está en
  la dirección. **Hipótesis para la revisión externa**, como en el documento de
  la decisión.
- **Validez: sigue siendo clásica** (Pallas, Halo 2, RedPallas). Una computadora
  cuántica podría falsificar pruebas y crear monedas. El camino de migración son
  las notas V3 recuperables del ZIP 2005, que Quantic usa desde el génesis.

## Medido (PC del usuario, i5-6400, 4 hilos)

- Envío de 2 acciones: 11.351 bytes; cada acción extra suma 4.255.
- Las cuatro pruebas de punta a punta (abajo), con pruebas Halo 2 reales, tardan
  unos 3 minutos corriendo en paralelo.
- En el teléfono (ARMv7) ya se midió el circuito el 13/10: 40 s la clave, 20 s
  por envío. Falta medir el sobre X-Wing dentro del escaneo (la etapa 3 midió
  3,8 ms por salida).

## Pruebas

`cargo test --manifest-path crates/zoka-quantic/Cargo.toml`:

- Claves: determinismo, redes y cuentas separadas, largo de la raíz, clave de
  vista ida y vuelta, `Debug` sin secretos.
- Dirección: ida y vuelta en cada red, ~1.730 caracteres, red equivocada,
  prefijo cambiado, cualquier carácter alterado, versión desconocida, cambiar la
  clave X-Wing cambia el comienzo.
- Cifrado: el destinatario encuentra su nota y nadie más; vuelto interno; versión
  compacta; un byte alterado en cualquier parte la oculta; un ciphertext que no
  es el de la nota se rechaza; recuperación del emisor solo con su `ovk`; salida
  pública verificable contra el anuncio (y no contra otra dirección ni con otra
  clave X-Wing); relleno que no abre nadie.
- **De punta a punta, con pruebas Halo 2 reales:** génesis con premine gastable
  de inmediato; el equipo paga a Alice con vuelto; el minero cobra subsidio +
  comisión y no puede gastar hasta madurar; Alice y el minero le pagan a Bob en
  el mismo bloque; repetir un envío se rechaza (nullifier gastado); Bob junta
  notas de dos emisores en un envío; lo que tienen las wallets es exactamente lo
  emitido.
- Formato: ida y vuelta byte a byte con el mismo txid; un bit cambiado en el
  sobre, el memo, `out` o la prueba ya no verifica; bytes de más, truncado.
- Consenso: génesis que paga a otro o sin salida pública, coinbase que acuña de
  más, minero que cobra un átomo de más, altura equivocada, bloque salteado,
  ancla desconocida, transacción de otra red, dos envíos con la misma nota en un
  bloque.
- **Wallet liviana:** la que escanea solo bloques compactos tiene el mismo saldo
  y el mismo árbol que la que ve los bloques enteros, baja menos de un tercio de
  los bytes y gasta con los testigos que armó; un bloque compacto al que le
  falta una transacción, con un byte cambiado en un ciphertext, con otra comisión
  o reordenado no coincide con la raíz del encabezado.

## Wallet liviana (A5)

`compact`: por acción viajan nullifier, `cmx`, ciphertext X-Wing y los 52
primeros bytes de `enc` (1.236 bytes); por transacción, los digests `memos` y
`resto`, flags, balance y ancla. Con eso la wallet:

1. recalcula cada txid y, con ellos, la raíz de txid del bloque, que compara con
   el encabezado (que ya validó por prueba de trabajo y checkpoints);
2. reconoce sus notas con el descifrado compacto (sin memo; si lo quiere, baja la
   transacción entera y comprueba su txid);
3. detecta sus gastos por los nullifiers y lleva el árbol y los testigos con los
   `cmx` de todas las acciones, con el mismo calendario de maduración que el nodo.

## Lo que falta

1. Integrar al nodo un perfil de red Quantic: bloques con encabezado propio
   (raíces de txid, de autorización y del árbol), base de datos del árbol y de
   los nullifiers, mempool, minería, P2P, API, servicio de bloques compactos.
   Zenith no cambia.
2. Motor del teléfono (JNI) y ZSilent sobre el mismo crate.
3. Medir en el teléfono el escaneo con X-Wing y el armado con la clave en segundo plano.
4. Decisiones del usuario: prefijos de dirección, tamaño del memo (512 como
   Zcash, o menos para achicar la cadena) y la comisión.
5. Revisión externa (7.7).
