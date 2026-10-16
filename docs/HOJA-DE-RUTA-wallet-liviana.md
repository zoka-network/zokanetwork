# Plan: wallet liviana con cualquier nodo, y lo que falta de lo post-cuántico

15/10/2026. Pedido del usuario: que el teléfono deje de tardar minutos en cargar el
saldo y en preparar un envío, usando "mi nodo o cualquier nodo conectado". Y que el
plan no se olvide de lo post-cuántico pendiente (parte B).

## Por qué hoy tarda tanto

Medido el 15/10 con ZKAPriv 2.1.2/2.1.3 contra la PC (único nodo de Zenith):

- Para saber qué notas son suyas y cuáles gastó, el teléfono baja **cada
  transacción privada entera** de la cadena: 267, de las que 12 no las tiene ningún
  nodo. Una pesa entre 59 KB y 346 KB porque lleva pruebas, anillo y pruebas de
  rango: unos **25 MB por Tor**.
- Los cuerpos no se guardan en el teléfono, así que después de cada reinicio de la
  app el envío vuelve a bajarlos. A las 17:56 el envío bajó 7 en 9 minutos.
- Cada consulta abre una conexión Tor nueva (72 en una hora).
- La prueba del envío **no** es el cuello de botella: 4 a 14 s en el teléfono de
  pruebas (32 bits).

Parches ya hechos en ZKAPriv 2.1.3 (rama `envio-sin-cuerpos-perdidos`, sin
publicar): el envío saltea los cuerpos que todos los pares dicen no tener, reintenta
la consulta de altura y trae los cuerpos en lote. Ayudan, pero no cambian el
volumen: el problema es de diseño.

## Parte A — Wallet liviana

### Idea

Cualquier nodo le manda al teléfono **solo los datos públicos chicos** de cada
transacción privada (el "resumen"): las salidas cifradas y las marcas de gasto. El
teléfono decide con su clave cuáles son suyas, sin decírselo a nadie. Solo de esas
pocas pide el cuerpo entero, lo verifica contra su hash y recién ahí las cuenta.
Es el modelo de las wallets livianas de Zcash (bloques compactos).

| | Hoy | Con resúmenes |
|---|---|---|
| Datos por transacción | 59–346 KB | ~0,3 KB por salida v1, ~1,4 KB por salida v2 |
| Primer saldo de Zenith | ~25 MB por Tor | ~100–300 KB |
| Qué aprende el nodo | Nada (baja todo) | Nada (todos bajan lo mismo) |

### Qué lleva el resumen

De `PrivateTransaction`, lo que usa `apply_private_transaction_for_wallet_inner`:

- `tx_hash`, `nullifier`, `timestamp`;
- marcas de gasto: `input_nullifiers`, `input_key_images`, `input_commitments`;
- por salida: `output_id`, `asset_tag`, `output_version`, `stealth_address`,
  `ephemeral_pubkey`, `kem_ciphertext` (v2), `commitment`, `one_time_public_key`,
  `encrypted_amount`.

Queda afuera todo lo que solo sirve para validar o gastar: `zk_proof`,
`ring_members`, `ring_signature`, `range_proof`, `fee_range_proof`,
`input_inclusion_proofs`, `proof_*`, `commitment_blind`.

### Seguridad con un nodo que miente

El resumen no se puede verificar contra el `tx_hash` (el hash cubre el cuerpo
entero), así que un nodo malicioso podría:

1. **Inventar un pago:** un resumen falso con una salida a tu dirección (es pública).
   Defensa: una salida propia se cuenta recién después de bajar el cuerpo entero,
   comprobar su hash canónico y que ese hash esté en `private_tx_hashes` de un bloque
   (los bloques sí están atados a sus hashes y a los checkpoints firmados).
2. **Inventar un gasto** de una nota tuya (para esconderte saldo). Defensa: igual que
   arriba; todo resumen que toque la wallet se confirma con el cuerpo entero.
3. **Esconder un pago**, omitiendo la transacción o quitando la salida del resumen.
   La omisión se detecta: los bloques dicen qué hashes existen y falta su resumen.
   La salida quitada no se detecta con un solo nodo; se mitiga preguntándole a dos
   nodos cuando haya más de uno. En Quantic se cierra del todo (ver A5).

Ninguno de los tres permite gastar plata que no existe: el nodo que valida rechaza
el envío.

### La prueba del envío

Se sigue haciendo **en el teléfono**. Para hacerla, un nodo tiene que ver monto,
destinatario y notas gastadas; con un nodo ajeno eso rompe la privacidad. Se
ofrecerá delegarla **solo en un nodo marcado como propio** (A4).

### Etapas

- **A1 — Nodo** (rama `wallet-liviana`): estructura `PrivateTxDigest`, protocolo
  P2P `/zoka/private-tx-digests/1` (pedido por hashes, servido desde la base como
  `private_tx_bodies_from_storage`, con tope por pedido y los que sobran informados
  como pendientes y no como faltantes), ruta HTTP `GET /private/tx/:hash/digest` para
  medir y depurar, y cliente en `mobile_net.rs`. Pruebas: el resumen no lleva campos
  pesados y detecta lo mismo que el cuerpo entero.
- **A2 — Motor móvil:** escaneo de saldo con resúmenes; cuerpo entero solo para lo
  que toca la wallet, verificado contra el hash y el bloque.
- **A3 — ZKAPriv:** saldo y envío con A2; guardar en el teléfono lo ya escaneado
  (notas propias y hasta qué altura), cifrado, para no empezar de cero; una sola
  conexión P2P abierta en vez de una por consulta; no volver a pedir en cada carga
  los cuerpos que ningún nodo tiene.
- **A4 — Opcional:** delegar la prueba al nodo propio (el teléfono firma la
  autorización de gasto, que nunca sale de él).
- **A5 — Quantic, antes del génesis:** formato compacto de Ironwood y un id de
  transacción que se compone de las partes (como el ZIP 244 de Zcash), para que el
  resumen se pueda verificar contra el id. Cierra el ataque 3. **Hecho el 16/10
  en `crates/zoka-quantic` (`compact`):** 1.236 bytes por acción, verificado
  contra la raíz de txid del encabezado.

Las A1 a A3 sirven también en Zenith; A1 exige que la PC corra un nodo con el
protocolo antes de que los teléfonos lo usen.

## Parte B — Lo que falta de lo post-cuántico

Detalle y criterio en [HOJA-DE-RUTA-mainnet-postcuantica.md](HOJA-DE-RUTA-mainnet-postcuantica.md).

| Etapa | Qué falta | Estado al 15/10 |
|---|---|---|
| 3.5 | Envío `zkq1` ZKAPriv → ZSilent en el teléfono real | ZSilent → teléfono OK (bloque 25.780); el otro sentido, trabado por la lentitud de arriba |
| 3 | QR de las `zkq1`: con 1.715 caracteres no los lee ninguna app | Rediseñar (QR partido, o pago por enlace corto que lleve la dirección completa) |
| 4 | VPS (nodo sin minar, seeds, explorador) | En espera del usuario; servidor Linux en casa como paso intermedio |
| 5 | Ironwood (Orchard corregido, Halo 2, sin trusted setup) en nodo y teléfono | Biblioteca hecha el 16/10 (`crates/zoka-quantic`, con A5); falta el perfil Quantic del nodo y las apps |
| 6 | Firmas híbridas Ed25519 + ML-DSA-65 en snapshots, y también en los checkpoints de la autoridad | Hecho el 16/10 en el nodo; falta que el usuario cree la mitad ML-DSA de la clave |
| 7.1 | libp2p a la versión mayor nueva | Sin empezar |
| 7.2 | Tiempo de bloque y dificultad: con la PC sola Zenith va a 16–18 bits y 3–6 min por bloque | Fijar con números antes del génesis |
| 7.3 | Premine de 5M en el génesis, público y verificable | Diseñado |
| 7.5 | ZSilent ofrecía notas gastadas | Arreglado el 13/10; falta verlo en uso |
| 7.6 | Ceremonia del trusted setup | Con Ironwood el gasto no la necesita; revisar qué circuitos Groth16 quedan |
| 7.7 | Revisión externa de las etapas 3 y 5 | Pendiente |
| 7.8 | Whitepaper 2.0 contra lo implementado (sumar la wallet liviana) | Pendiente |
| 8 | Génesis de Quantic, seeds, anuncio | Al final |

## Orden propuesto

1. A1 (nodo) y release de testnet con el protocolo.
2. A2 + A3 (teléfono): saldo y envío en segundos. Con eso se cierra la prueba 3.5.
3. QR de las `zkq1`.
4. Etapa 5 (Ironwood) junto con A5: es el formato de Quantic, conviene hacerlos juntos.
5. Etapa 6, después la 7 y la 8.
