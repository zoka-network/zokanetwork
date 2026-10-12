# Etapa 3: direcciones y salidas híbridas (especificación de trabajo)

Estado: **experimental, sin activar.** Rama `etapa3-direcciones-hibridas`.
Implementación de referencia: `src/private_wallet/hybrid.rs`, detrás de la
feature `pq-experimental`. Las reglas de consenso de la salida v2 ya están en
el nodo (todas las builds) y rechazan toda salida v2 mientras la activación
siga en `None`. Con `pq-experimental`, el nodo y el CLI ya arman, reconocen y
gastan salidas v2 (ver "Envío y recepción"). ZSilent y ZKAPriv todavía no
saben nada de esto.

Decisiones tomadas por el usuario el 12/10/2026: **D1 = ML-KEM-768** (a través
de X-Wing) y **D2 = direcciones largas** (unos 1.715 caracteres, con QR y
copiar/pegar).

## Qué protege y qué no

| Pieza | Hoy (v1) | v2 híbrida |
|---|---|---|
| Reconocer qué salidas son de una dirección | ECDH Ristretto: roto por una computadora cuántica, también hacia atrás | X-Wing (X25519 + ML-KEM-768): hay que romper los dos |
| Leer el monto de una salida | Derivado del mismo ECDH | Derivado del secreto X-Wing |
| Autorizar el gasto (clave de un solo uso, CLSAG) | Ristretto | **Igual que hoy.** Es la etapa 5 |
| Grafo de gastos | Las entradas se ven en la cadena | **Igual que hoy.** Es la etapa 5 |
| Coinbase | Publica la dirección del minero en claro | Sin decidir (ver abiertos) |

La v2 sola **no** hace a la red "privada frente a una computadora cuántica":
cierra la parte de reconocimiento y montos. El grafo de gastos y la
autorización siguen siendo clásicos hasta la etapa 5.

## Claves

Todo sale de la raíz de la wallet: los 16 o 32 bytes de entropía de la frase
(la misma "scan key" que usa la dirección `zka1`).

- **Clave de gasto:** la de siempre, `derive_spending_key_from_recovery_seed`.
  Una misma wallet gasta salidas v1 y v2, y la frase de siempre recupera ambas.
- **Clave de escaneo X-Wing:** semilla de 32 bytes =
  `SHA-256("zoka-hybrid-scan-seed-v2" ‖ 0x1f ‖ raíz)`, expandida por X-Wing.

En el código, la clave de escaneo (`HybridScanKey`) está separada de la de
gasto: sale de la "scan key" guardada de la wallet, que en una wallet creada
desde una frase **es** la raíz. Así también tienen dirección v2 las wallets de
claves al azar o rotadas (`HybridWalletKeys::from_wallet_keys`, con su clave
de gasto real) y las de solo lectura, que reconocen y leen montos sin la
clave de gasto. La dirección v2 de una wallet toma la clave de gasto pública
de su `zka1`.

Una frase de **12 palabras** tiene 128 bits de entropía: frente a una búsqueda
con Grover eso equivale a la categoría 1 del NIST (como AES-128), por debajo de
la categoría 3 de ML-KEM-768. **Para Quantic se recomiendan 24 palabras**
(256 bits); el CLI ya acepta `--seed-words 24`.

## Dirección v2

```text
"zkq1" ‖ Base58( versión(1) ‖ gasto(32) ‖ x-wing(1216) ‖ checksum(4) )
checksum = primeros 4 bytes de SHA-256("zoka-hybrid-address-v2" ‖ 0x1f ‖ versión ‖ gasto ‖ x-wing)
```

- Versión `0x02`. Prefijo `zkq1` (Zoka Quantic); no se confunde con `zka1`.
- 1.253 bytes, unos 1.715 caracteres. Se rechaza sin decodificar todo lo que
  pase de 1.800.
- Al decodificar se valida, en este orden: largo, prefijo, Base58, largo
  exacto, checksum, versión, que la clave de gasto sea un punto Ristretto
  válido y distinto de la identidad, y que la clave X-Wing tenga formato
  canónico (incluye el chequeo de módulo de ML-KEM). Cada falla tiene su error
  y ninguno incluye la dirección.

## Salida v2

Cambia un campo y se agregan dos respecto de la v1:

| Campo | v1 | v2 |
|---|---|---|
| `ephemeral_pubkey` | 32 bytes (Ristretto) | vacío |
| `kem_ciphertext` | — | 1.120 bytes (X-Wing) |
| `output_version` | — (implícito 1) | 2 |
| `one_time_public_key`, `commitment`, `amount_commitment`, `range_proof`, `encrypted_amount` | igual | igual |
| `stealth_address` | `stlth_…` | `stlth2_…` |

Una salida v2 pesa unos 1.090 bytes más que una v1.

Derivación, con `ss` = secreto compartido X-Wing (32 bytes):

```text
ctx          = SHA-256("zoka-hybrid-output-ctx-v2" ‖ 0x1f ‖ ciphertext ‖ 0x1f ‖ compromiso)
desplazamiento = H_escalar("zoka-hybrid-output-key-v2", ss, ctx)
P (un solo uso) = B (gasto) + desplazamiento · G
ciego del monto = H_escalar("zoka-hybrid-amount-blind-v2", ss, ctx)
secreto de gasto = b + desplazamiento
```

`H_escalar` es el `hash_to_scalar` de siempre (SHA-512 con separador `0x1f`,
reducido módulo el orden del grupo). `ctx` liga el secreto al ciphertext y al
compromiso: no sirve mezclar el ciphertext de una salida con otra.

## Escaneo

Por cada salida v2: decodificar el ciphertext; desencapsular (si X-Wing lo
rechaza por no contributivo, la salida no es propia); calcular `P'` y
compararlo en tiempo constante con `one_time_public_key`. Si coincide, la
salida es propia y se derivan el ciego del monto y el secreto de gasto. Como
ML-KEM usa rechazo implícito, una salida ajena da un secreto distinto y la
comparación falla; no hay oráculo de error.

**Costo medido (12/10/2026)** con `examples/hybrid_scan_bench.rs`, release,
mediana de 100 iteraciones. En el teléfono, rango de tres pasadas:

| Operación | Windows (i5-6400) | Teléfono ARMv7 (SM-A135M) |
|---|---:|---:|
| Escanear una salida v1 (lo que hace hoy la wallet) | 89 µs | 1,52 ms |
| Escanear una salida v2 propia | 262 µs | 3,78–3,80 ms |
| Escanear una salida v2 ajena | 263 µs | 3,80–3,82 ms |
| Armar una salida v2 (quien envía) | 199 µs | 2,66–2,69 ms |
| Codificar la dirección v2 | 2,22 ms | 7,14–7,15 ms |
| Decodificar y validar la dirección v2 | 0,80 ms | 4,04 ms |

Escanear una salida v2 cuesta unas **2,5 veces** lo de una v1 en el teléfono
(+2,3 ms por salida). Como referencia aritmética: 10.000 salidas tardan
~15 s en v1 y ~38 s en v2; 100.000 salidas, ~2,5 min y ~6,3 min. Propias y
ajenas cuestan lo mismo, así que el tiempo no revela cuáles son de la wallet.
El escaneo incremental (bloques nuevos) no se nota; una resincronización
completa en el teléfono sí. Incluye reexpandir la clave X-Wing en cada salida
(la versión 0.1.1 no expone una clave expandida): si una versión futura lo
permite, se ahorra parte. No se midieron memoria ni batería. Evidencia:
[benchmarks/2026-10-12-hybrid-scan.json](benchmarks/2026-10-12-hybrid-scan.json).

Con estos números **D1/D2 quedan cerrados como decidió el usuario**: el costo
de ML-KEM-768 vía X-Wing es manejable en el teléfono de 32 bits sin etiquetas
de vista.

**Sin view tags.** Una etiqueta de vista derivada solo de ECDH clásico le
permitiría a una computadora cuántica descartar o confirmar pertenencia sin
romper ML-KEM. Si el escaneo resulta demasiado lento, una etiqueta tiene que
derivarse del secreto híbrido y revisarse antes de incorporarla.

## Envío y recepción (con `pq-experimental`)

- **Destinatario.** `canonical_send_recipient` acepta `zka1` como siempre y
  `zkq1`, que se decodifica y valida entera antes de armar nada. El coinbase
  usa `canonical_payment_address`, que solo acepta `zka1`.
- **Armado.** Hacia una `zkq1`, `derive_stealth_payload` produce una salida
  v2 (`hybrid_output_material`): ciphertext X-Wing, clave efímera vacía,
  etiqueta `stlth2_`. El monto viaja cifrado igual que en v1, con el ciego
  derivado del secreto X-Wing.
- **El vuelto sigue al pago.** Si el pago va a una `zkq1`, el vuelto va a la
  `zkq1` propia: una salida v1 al lado de una v2 delataría cuál de las dos es
  el vuelto. Un envío a `zka1` sigue siendo v1 entero.
- **Recepción y gasto.** El descifrado de la wallet, el gasto de notas y el
  escaneo con clave de vista tienen una rama v2 que desencapsula X-Wing. La
  clave de vista se usa junto con la `zka1`: la clave de gasto pública es la
  misma, así que ve también los pagos a la `zkq1`.
- **Notas guardadas.** Una nota v2 guarda `output_version` y `kem_ciphertext`
  para poder gastarse; las v1 no cambian.
- **Builds sin `pq-experimental`** (por ejemplo, el motor de ZKAPriv hoy): no
  pueden enviar a `zkq1` (error claro), no reconocen salidas v2 (las saltean)
  y validan el consenso igual que las demás.

### CLI

- `zoka wallet-receive --address <zka1> --hybrid [--json]` muestra también la
  dirección v2 (JSON: `hybrid_address`, `hybrid_activation_height`). Si la
  activación no está fijada lo avisa: nadie puede pagarle todavía y la que se
  comparte es la `zka1`.
- `zoka tx-send --to <zkq1…>` (y la consola guiada) aceptan una `zkq1` solo si
  la activación está fijada (en el binario o por
  `ZOKA_PQ_HYBRID_ADDRESSES_ACTIVATION_HEIGHT`) y la build tiene la feature;
  si no, lo rechazan antes de reservar notas.
- Si la `zkq1` es de una wallet local, el CLI la reconoce como destinataria
  (para el control de montos y el de aislamiento entre wallets).

## Activación y consenso

Implementado en `src/private_wallet/mod.rs` (`validate_private_output_format`)
y presente en **todas** las builds: validar una salida v2 no requiere X-Wing,
solo revisar forma y largos. Un nodo sin `pq-experimental` valida igual que
uno con ella.

- Reglas de formato de cada salida:
  - v1 (o sin `output_version`, que vale 1): `kem_ciphertext` vacío.
  - v2: solo si `PqFeature::HybridAddresses` está activa; ciphertext de
    exactamente 1.120 bytes; `ephemeral_pubkey` vacío; `stealth_address` con
    prefijo `stlth2_`.
  - Cualquier otra versión se rechaza.
- **Los cuerpos tienen que ser datos de consenso.** En Zenith los bloques
  comprometen hashes y nulificadores de las transacciones privadas, pero los
  cuerpos viajan como adjuntos y solo se validan desde la altura de los
  bundles canónicos (`ZOKA_PRIVATE_TX_CANONICAL_BUNDLE_ACTIVATION_HEIGHT`, hoy
  `None`). Una regla sobre salidas es una regla sobre cuerpos: sin bundles
  canónicos, un nodo sin la activación v2 acepta igual el bloque (medido en la
  red local, corrida 1). Por eso la salida v2 exige **las dos alturas** y vale
  desde la mayor (`economics::pq_hybrid_outputs_activation_height`), igual que
  el perfil `v2_i1`.
- **Dónde se aplica la altura.** Al validar los cuerpos de un bloque (y al
  armar un bloque candidato) se usa la altura exacta del bloque. En el
  mempool, la API y el P2P no hay altura: una salida v2 pasa solo si las dos
  activaciones ya están fijadas (en el binario o por las variables de
  entorno), y el bloque la vuelve a validar con su altura. Hoy las dos son
  `None`: toda salida v2 se rechaza.
- **Hash.** En una salida v2 el hash de la transacción agrega
  `"zoka-output-v2" ‖ versión ‖ ciphertext ‖ 0x1f` después de la clave
  efímera. En v1 recorre exactamente los mismos bytes que antes: la prueba
  `una_salida_v1_conserva_su_json_y_su_hash` fija el valor
  `f12314d4d35a13a1bc62371840bbae93`, calculado con el código anterior a la
  etapa 3 (commit `2b245d1e`).
- **JSON.** Una salida v1 se serializa sin los campos nuevos, byte por byte
  como antes; lo mismo las notas de las wallets guardadas. No hay formatos
  binarios (todo es JSON), así que no hay cambios de diseño de registro.
- **Hard fork.** Un nodo viejo ignora los campos desconocidos, calcula otro
  hash para una transacción con salidas v2 y la rechaza; con bundles
  canónicos activos también rechaza el bloque que la trae. Activarla exige
  una altura anunciada y que la red actualice antes.
- **P2P.** El mensaje de salida del P2P (`PrivateOutputMessage`) lleva los dos
  campos nuevos con la misma serialización: una v1 viaja byte por byte igual
  y una v2 no pierde nada.
- **Coinbase:** sigue siendo v1 (decisión del usuario del 12/10/2026). Su
  estructura no tiene los campos v2 y el minero sigue cobrando en `zka1`.
- En Zenith (testnet) conviven v1 y v2 después de la altura. En Quantic,
  solo v2 desde el bloque 0.

## Prueba en red local (12/10/2026)

Tres nodos en la PC del usuario, aislados: cada uno con su `ZOKA_HOME`, su
API (3311-3313) y su puerto P2P, un manifiesto sin semillas ni RPC públicas y
`--no-iroh-bootstrap`. Nada llegó a la testnet pública ni al nodo del usuario
(puerto 3000). Binario release con `pq-experimental` y dificultad de ensayo
(`--unsafe-rehearsal-difficulty-bits 1`). Las activaciones se fijaron por
variables de entorno; los binarios siguen con `None`.

| Nodo | Rol | Salida v2 desde | Bundles canónicos (corrida 2) |
|---|---|---|---|
| A | minero, wallet alice | 80 | 70 |
| B | wallet bob, no mina | 80 | 70 |
| C | `--sync-only` | (sin activar) | 70 |

**Corrida 1, sin bundles canónicos:**

- En la altura 67 alice le pagó 5 ZKA a la `zkq1` de bob (`ce85666a`): pago y
  vuelto v2, 26 KB de JSON. Esperó en el mempool de 68 a 79 y entró en el 80.
- El nodo de bob vio 5 ZKA; bob los gastó hacia la `zka1` de alice
  (`97abef87`, salidas v1, bloque 86). Saldos finales exactos: bob 3; alice
  1.065 = 89 × 12 − 5 + 2 (su vuelto v2 reconocido).
- **C aceptó el bloque 80:** sin bundles canónicos nadie valida los cuerpos.
  De ahí la corrección: la salida v2 exige las dos alturas.

**Corrida 2, bundles canónicos desde el 70 y binario corregido:**

- alice2 le pagó a la `zkq1` de bob2 5 ZKA (`a5d85da1`) y 1 ZKA (`906b9f14`)
  entre las alturas 63 y 67. El minero las salteó en cada bloque hasta el 79
  ("hybrid private outputs are not active at this height") y las incluyó en
  el 80 con sus cuerpos (bloque de 139 KB).
- B2 siguió la cadena. **C2 rechazó el bloque 80** ("invalid canonical
  private transaction bundle: … hybrid private outputs are not active at this
  height") y quedó en el 79.
- bob2 vio 6 ZKA y gastó 4 hacia la `zka1` de alice2 (`ad440c12`, bloque 87).
  Saldos finales exactos: bob2 2; alice2 1.138 = 95 × 12 − 5 − 1 + 4.

**Hallazgos que no son de la v2, anotados para después:**

- La wallet del CLI volvió a ofrecer una nota que ya gastaba una transacción
  pendiente (`fca318a9` contra `906b9f14`): la API respondió 409 y el minero
  eligió una sola. Es el problema conocido de las notas ya gastadas.
- En los primeros envíos desde el nodo minero el CLI informó "Archived private
  tx bodies: 0", aunque la transacción entró igual por la cola lateral; por un
  proxy de diagnóstico y desde el nodo B la publicación anduvo. La causa no
  quedó reproducida.
- El chisme del mensaje `PrivateTransaction` (y del mezclador) se descarta
  para toda transacción moderna: la validación de forma exige
  `recipient_commitment`, que el armador deja vacío a propósito, y al mensaje
  le falta `status`. Las transacciones viajan por anuncio + cuerpo crudo, que
  conserva los campos v2.

## Abiertos

1. **Coinbase:** hoy publica `recipient_address` y cifra el monto con una clave
   derivada de esa dirección pública. Decidir para Quantic si la dirección del
   minero sigue siendo pública (el team mining la usa) y, si no, usar salidas v2.
2. **Comisión del minero** (`PrivateFeeClaim`): misma construcción con su
   propia etiqueta de dominio (`zoka-hybrid-fee-claim-v2`).
3. ~~Formato en wallets guardadas y comando del CLI~~ (hecho: ver "Envío y
   recepción"). Falta mostrar la dirección v2 y su QR en ZSilent y ZKAPriv,
   solo cuando la red la active (decisión del usuario del 12/10/2026).
4. Motor móvil: compilar con `pq-experimental` para los dos ABIs y medir el
   tamaño del `.so`. Hasta entonces la app no reconoce pagos a su `zkq1`.
5. Revisión externa de esta construcción antes de activarla en cualquier red.
6. **Tamaño en JSON.** `kem_ciphertext` se serializa como arreglo de números
   (igual que `ephemeral_pubkey` y `range_proof`): unos 4 KB por salida v2 en
   el cuerpo JSON, frente a 1.120 bytes en binario. El hash usa los bytes, así
   que pasarlo a hex o base64 no cambiaría hashes, pero sí el formato de red:
   decidirlo antes de activar.
7. El envío por la API con semilla (el camino viejo de ZKAPriv) sigue
   aceptando solo `zka1`.

## Pruebas actuales

Medición: `cargo run --release --no-default-features --features pq-experimental --example hybrid_scan_bench -- --iterations 100`.

`cargo test --features pq-experimental --lib private_wallet::hybrid`:
ida y vuelta de la dirección; determinismo desde la raíz; misma clave de gasto
que la dirección `zka1`; el destinatario reconoce su salida y deriva el mismo
ciego y el mismo secreto de un solo uso; otra wallet, otro compromiso,
ciphertext alterado (parte ML-KEM y parte X25519), truncado o clave de un solo
uso ajena no se reconocen; cada salida usa un encapsulado nuevo; errores
precisos para prefijo, Base58, largo, checksum, versión, clave de gasto
identidad y entradas demasiado largas; etiquetas de dominio distintas; `Debug`
sin secretos.

Consenso, en todas las builds (`cargo test --lib -- salida_v1_conserva
hash_de_una_salida_v2 formato_de_las_salidas la_red_rechaza_salidas_v2`): el
JSON y el hash de una salida v1 quedan fijos; el hash de una v2 cubre versión
y ciphertext (bajarle la versión cambia el hash); reglas de formato por
versión; y una transacción real con una salida v2 bien formada se rechaza
antes de la activación, en el mempool y dentro de un bloque.

Envío y recepción (`cargo test --features pq-experimental --lib -- zkq1
private_wallet::hybrid`): de punta a punta sin red, alice le paga a la `zkq1`
de bob; pago y vuelto salen v2; la transacción es válida con la activación
fijada; bob reconoce y descifra 30, alice su vuelto de 70, carol nada; bob la
recibe, la nota guarda versión y ciphertext, y después la gasta hacia una
`zka1` en una transacción v1 válida. La clave de vista sola reconoce el pago
v2 y el vuelto v2. Una `zkq1` mal escrita se rechaza sin reservar notas, el
coinbase no acepta `zkq1`, y sin la feature el error lo dice. Una wallet de
claves sueltas tiene su propia dirección v2 y gasta sus salidas; la de una
frase da lo mismo por los dos caminos.

CLI (`cargo test --bin zoka -- recipient_accepts_zka1`): `--to` sigue
aceptando `zka1` igual que antes y rechaza `zkq1` con un mensaje claro
mientras la activación no esté fijada.

Activación y P2P (`cargo test --lib -- hybrid_outputs_need_canonical
el_mensaje_p2p_conserva`): la salida v2 exige también los bundles canónicos y
vale desde la mayor de las dos alturas; una salida v1 viaja por el mensaje
P2P igual que antes y una v2 no pierde versión ni ciphertext.
