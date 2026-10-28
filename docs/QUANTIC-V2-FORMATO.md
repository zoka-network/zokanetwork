# ZOKA QUANTIC STARK v2: formato de notas, coinbase y estado

28 de octubre de 2026. Rama `quantic-v2-formato-final`. Cierra tres decisiones
de formato que son consenso y por eso tenían que estar antes del génesis de la
testnet v2. Ninguna resigna privacidad del dueño ni de los montos de las
transferencias.

Código: [model.rs](../crates/zoka-pq-pool/src/model.rs),
[transfer.rs](../crates/zoka-pq-pool/src/transfer.rs) (AIR),
[transaction.rs](../crates/zoka-pq-pool/src/transaction.rs),
[ledger.rs](../crates/zoka-pq-pool/src/ledger.rs) y
[stark.rs](../src/quantic/stark.rs) (nodo).

## 1. La nota y su `rho`

```
Note   = { owner, rho, rcm, value }          owner = H(OWNER, clave de gasto)
hide   = H(HIDE, owner ‖ rcm)
cm     = H(NOTE, rho ‖ value ‖ hide)
nf     = H(NULLIFIER, clave de gasto ‖ rho)
```

`H` es Poseidon2 sobre KoalaBear (ancho 32), con dominio y red en el mensaje.

**Antes**, `rho` lo elegía al azar quien enviaba y el circuito no lo ataba a
nada. Un emisor malicioso podía mandar dos notas con el mismo `rho` al mismo
destinatario: mismo nullifier, y solo una se podía gastar (el ataque «Faerie
Gold», el que corrigió Zcash al pasar de Sprout a Sapling y Orchard).

**Ahora** `rho` nunca se elige:

| Nota | `rho` | Quién lo hace cumplir |
| --- | --- | --- |
| Salida `i` de una transferencia | `H(RHO, primer nullifier ‖ i)` | El AIR: el registro `rho` de cada salida es el resultado de ese hash |
| Salida `i` del coinbase de la altura `h` | `H(COINBASE_RHO, h ‖ i)` | El ledger, al recalcular el compromiso |

El primer nullifier de una transferencia se gasta una sola vez en la cadena y
las alturas no se repiten, así que dos notas de una misma cadena nunca
comparten `rho`, ni por lo tanto nullifier. Los dominios separan los dos casos.

Como `rho` se puede calcular desde datos públicos, ya no oculta nada: el
ocultamiento del dueño y del monto lo da `rcm`, 16 elementos al azar dentro de
`hide`. El nullifier sigue sin poder ligarse a su nota sin la clave de gasto.

El circuito exige al menos una entrada (el `rho` de las salidas sale del primer
nullifier). Costo: dos hashes más por nota. Medido: pago 1→2 de 1.933.755 bytes
(antes 1.868.731) en 4,7 s; forma máxima 8→8 de 8.705.190 bytes. El estimador
de Plonky3 sigue dando 128 bits clásicos para las dos formas.

## 2. El coinbase sin prueba

Antes cada coinbase llevaba una prueba STARK (0 entradas, 1 salida) de unos
947 KB: más de 1,8 MB guardados por bloque vacío, unos 250 GB por año solo en
recompensas. Ahora un coinbase publica, por salida:

```
Opening = { hide, value }
```

y el nodo comprueba `cm_i = H(NOTE, H(COINBASE_RHO, h ‖ i) ‖ value_i ‖ hide_i)` y
que la suma de los `value_i` sea lo acuñado (subsidio más comisiones). Los
`Opening` están dentro del txid. Un coinbase con prueba, sin openings o de otra
altura se rechaza. Ocupa menos de 4 KB, casi todo el sobre X-Wing para el
destinatario.

Qué se ve: el monto de cada salida (el total ya era público; con varias salidas,
un pool muestra el reparto) y `hide`, que sin `rcm` no revela al dueño. Una
dirección conocida no permite reconocer sus coinbases.

## 3. El estado compacto

Antes `state_root` comprometía la codificación entera del ledger (todas las
notas, todos los nullifiers, anclas y pendientes) y el nodo guardaba esa copia
en cada bloque: crecimiento cuadrático. Ahora:

```
Committed  = { red, altura, emisión, cantidad de notas, raíz del árbol, acumulador }
acumulador = blake3(acumulador anterior ‖ nullifiers del bloque, ordenados)
state_root = "quantic-v2:" ‖ blake3-xof-512("ZOKA QUANTIC STARK state v3", Committed)
```

Por bloque (de cualquier rama) el nodo guarda un `Record` de unos cientos de
bytes: `Committed`, la frontera del árbol (los nodos izquierdos que esperan
hermano, uno por bit de la cantidad de notas: misma raíz que el árbol completo),
el padre, las notas del coinbase que maduran después y los nullifiers del
bloque.

El ledger sobre un bloque cualquiera (`Branch`) se arma con esos registros:

- **Anclas:** las raíces de los últimos 100 registros de la rama.
- **Coinbase que madura:** el del registro de la altura `h − madurez` (60
  bloques; 2 en regtest).
- **¿Nullifier gastado en esta rama?** Sí si lo gastó un bloque de la rama que
  no es canónico, o si el índice canónico (`nullifier → altura`) lo tiene a la
  altura del punto de bifurcación o por debajo. Así una rama lateral puede
  gastar lo que la canónica gastó después de bifurcarse y ganar la
  reorganización, y nunca gastar dos veces lo mismo dentro de una rama. La
  promoción de rama en `chain.rs` vuelve a comprobarlo.

Las reglas están una sola vez en el crate (`LedgerState`, `apply`,
`check_transfer`) y las usan el `Ledger` completo y `Branch`. La wallet, que
necesita todas las notas para sus caminos de Merkle, reconstruye el árbol
completo con `replay` sobre los bloques canónicos, sin volver a verificar las
pruebas, y comprueba que la raíz coincida con la del registro.

Medido con XMRig 6.25.0 real contra un nodo de testnet v2 local: 5 bloques,
25 aciertos aceptados y ninguno rechazado, base de 119 KB (antes 7,4 MB para 4
bloques). Un acierto que es bloque se responde en unos 14 ms (antes unos 240) y
el trabajo nuevo sale en menos de un segundo (antes unos 2).

## Compatibilidad

Las bases, bloques, transacciones y sobres de antes de este cambio no se pueden
abrir con el formato nuevo: era una cadena local de pruebas, sin génesis
público. Las wallets v2 (`zoka-quantic-stark-wallet-v2`) no cambian: guardan la
semilla, no notas.

## Lo que queda abierto

- El tamaño de las pruebas de transferencia (1,9 MB por pago) y de los bloques
  llenos: parámetros FRI, agregación o recursión. Es rendimiento y también
  consenso (cambia el verificador).
- Los bloques se guardan en JSON con hex: el doble del binario.
- Revisión criptográfica externa de este formato.
