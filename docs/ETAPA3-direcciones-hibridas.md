# Etapa 3: direcciones y salidas híbridas (especificación de trabajo)

Estado: **experimental, sin activar.** Rama `etapa3-direcciones-hibridas`.
Implementación de referencia: `src/private_wallet/hybrid.rs`, detrás de la
feature `pq-experimental`. Nada de esto está conectado a transacciones,
validación de bloques, wallets guardadas, ZSilent ni ZKAPriv todavía.

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

Una frase de **12 palabras** tiene 128 bits de entropía: frente a una búsqueda
con Grover eso equivale a la categoría 1 del NIST (como AES-128), por debajo de
la categoría 3 de ML-KEM-768. **Para Quantic se recomiendan 24 palabras**
(256 bits); el CLI ya acepta `--seed-words 24`.

## Dirección v2

```
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

```
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

**Costo:** un desencapsulado X-Wing por salida. En el teléfono ARMv7 del
usuario, ~3,4–3,5 ms (incluye reexpandir la clave en cada llamada: X-Wing
0.1.1 no expone una clave expandida). Medición del escaneo completo en el
teléfono: ver resultados abajo.

**Sin view tags.** Una etiqueta de vista derivada solo de ECDH clásico le
permitiría a una computadora cuántica descartar o confirmar pertenencia sin
romper ML-KEM. Si el escaneo resulta demasiado lento, una etiqueta tiene que
derivarse del secreto híbrido y revisarse antes de incorporarla.

## Activación y consenso (pendiente de implementar)

- Antes de la altura de `PqFeature::HybridAddresses` (hoy `None`), los nodos
  **rechazan** cualquier salida con `output_version = 2` o `kem_ciphertext` no
  vacío. Un nodo viejo no conoce esos campos y calcularía otro hash de
  transacción, así que activarla es un hard fork con altura anunciada.
- El hash de la transacción incluye `output_version` y `kem_ciphertext` solo en
  salidas v2: los hashes de las transacciones v1 históricas no cambian.
- En Zenith (testnet) conviven v1 y v2 después de la altura. En Quantic,
  solo v2 desde el bloque 0.

## Abiertos

1. **Coinbase:** hoy publica `recipient_address` y cifra el monto con una clave
   derivada de esa dirección pública. Decidir para Quantic si la dirección del
   minero sigue siendo pública (el team mining la usa) y, si no, usar salidas v2.
2. **Comisión del minero** (`PrivateFeeClaim`): misma construcción con su
   propia etiqueta de dominio (`zoka-hybrid-fee-claim-v2`).
3. Formato en wallets guardadas (archivos `.private.json`), comando del CLI
   para mostrar la dirección v2, QR en ZSilent y ZKAPriv.
4. Motor móvil: compilar con `pq-experimental` para los dos ABIs y medir el
   tamaño del `.so`.
5. Revisión externa de esta construcción antes de activarla en cualquier red.

## Pruebas actuales

`cargo test --features pq-experimental --lib private_wallet::hybrid`:
ida y vuelta de la dirección; determinismo desde la raíz; misma clave de gasto
que la dirección `zka1`; el destinatario reconoce su salida y deriva el mismo
ciego y el mismo secreto de un solo uso; otra wallet, otro compromiso,
ciphertext alterado (parte ML-KEM y parte X25519), truncado o clave de un solo
uso ajena no se reconocen; cada salida usa un encapsulado nuevo; errores
precisos para prefijo, Base58, largo, checksum, versión, clave de gasto
identidad y entradas demasiado largas; etiquetas de dominio distintas; `Debug`
sin secretos.
