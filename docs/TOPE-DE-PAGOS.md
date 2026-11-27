# Tope de pagos por bloque: análisis y propuesta

27/11/2026. El usuario pidió analizar `MAX_BLOCK_TRANSACTIONS = 16`
(`crates/zoka-pq-pool/src/ledger.rs`) y proponer un número. Cambiarlo es
consenso: entra como una actualización de red a una altura, como la prueba v4.

## Los límites de hoy

| Límite | Valor | De dónde |
| --- | --- | --- |
| Pagos por bloque | 16 (más la coinbase) | `ledger.rs` |
| Bytes por bloque | 32 MiB | `ledger.rs` (`MAX_BLOCK_BYTES`) |
| Bytes por pago | 751.251 (2→2, bloque 10.642) a ~0,80 MB (8→2) | la prueba v4 es el 99 % |
| Verificar una prueba en la PC | 126 ms (2→2) a 199 ms (8→2) | `PRUEBA-V4.md` |
| Escaneo de una billetera liviana | ~6 KB por pago (dos salidas de ~3 KB) | `stark_api.rs` |
| Comisión | exacta: 0,0002, 0,0004 o 0,0008 ZKA | `conventional_fee` |
| Recompensa | 12 ZKA por bloque | `/supply` |

El tope de bytes ya limita por su cuenta a unos 44 pagos 2→2 (o 41 de 8→2)
por bloque. Los nodos guardan todas las pruebas: con unos 160 pagos desde el
12/11, la base del nodo de la PC ya ocupa 1,1 GB.

Uso real: unos 160 pagos en 15 días (~10 por día). La capacidad de hoy,
11.520 pagos por día, es unas mil veces eso.

## Lo que cuesta cada tope con los bloques llenos

| Tope | Pagos por día | Cadena por día | Por año | Verificar un bloque (PC) | Escaneo de un teléfono por día | Llenar todos los bloques cuesta |
| --- | --- | --- | --- | --- | --- | --- |
| **16** | 11.520 | 8,6 GB | 3,2 TB | 2,0 a 3,2 s | ~69 MB | 2,3 ZKA por día |
| 24 | 17.280 | 13 GB | 4,7 TB | 3,0 a 4,8 s | ~104 MB | 3,5 ZKA por día |
| 32 | 23.040 | 17 GB | 6,3 TB | 4,0 a 6,4 s | ~138 MB | 4,6 ZKA por día |
| 44 (tope de bytes) | 31.680 | 24 GB | 8,7 TB | 5,5 a 8,8 s | ~190 MB | 6,3 ZKA por día |

El tiempo de verificar sobra en todos los casos (un bloque cada 120 s). Lo
que no sobra es el disco, y es barato llenarlo. Con 16, un atacante llena
todos los bloques con 2,3 ZKA por día de comisiones, menos de un quinto de la
recompensa de **un** bloque, más unas 3 horas de CPU para las pruebas. A cambio,
cada nodo de la red guarda 8,6 GB más por día. Subir el tope multiplica ese
daño al mismo costo por pago.

## Propuesta

**Mantener 16 para el lanzamiento de la mainnet.** Sobra para el uso
esperado, y el problema de un tope más alto no es la velocidad sino el
almacenamiento barato de llenar.

Antes de subirlo, en este orden:

1. **Descartar las pruebas ya validadas debajo de un checkpoint**, con nodos
   de archivo que las guarden todas. Ya está planteado en `PRUEBA-V4.md`. Cada
   pago pasaría de ~0,75 MB a unos pocos KB en el disco de un nodo común.
2. **Una comisión que suba con la ocupación de los bloques.** Tiene que seguir
   siendo exacta (la misma para todos a una altura, para no distinguir a los
   pagadores): por ejemplo, el doble cuando los bloques anteriores vienen
   llenos. Así, llenar bloques cuesta en proporción al daño.
3. **Con 1 y 2 hechos, subir a 32** por una actualización de red. Deja margen
   dentro de los 32 MiB para pagos de 8 notas, y la verificación de un bloque
   lleno queda en unos 6 s en una PC.

Para los exchanges: 16 pagos por bloque, unos 11.500 por día para toda la red,
ampliable por una actualización de red.
