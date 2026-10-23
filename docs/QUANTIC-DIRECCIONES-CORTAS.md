# Direcciones cortas de Zoka Quantic

22/10/2026. Decisión del usuario: **diseñar direcciones cortas**, y con ellas
volver a un QR normal ("al acortar las direcciones quizás podamos mantener los
QR").

**Estado al 23/10/2026:** construido con la opción A (directorio en los nodos):
formato, directorio, `quantic-wallet` y ZSilent. Ese día el usuario decidió
además:

- **Solo la corta a la vista** («¿no podemos solo tener la corta?»). La completa
  sigue existiendo por dentro, porque lleva las claves con las que se cifra el
  pago, pero ZSilent no la muestra. Si una wallet la pide, se copia desde
  Ajustes > Wallets (ZSilent 3.0.4).
- **Publicación automática, «sola y siempre»**: ZSilent publica la corta de cada
  wallet que el nodo local no conoce, sin botón. Reemplaza al «con permiso
  explícito al crear la wallet» del plan de abajo; el permiso es esta decisión.
- **QR estándar**: el de puntos redondos les costaba a los teléfonos. Ahora tiene
  módulos cuadrados, las esquinas de la norma, 4 módulos de margen, corrección M
  (33 módulos para una corta) y cada módulo en píxeles enteros.

## El problema

Una dirección de Quantic mide hoy **1.734 caracteres** (`zqtest1` + Base58 de
1.264 bytes, `crates/zoka-quantic/src/address.rs`):

| Parte | Bytes | Para qué |
| --- | ---: | --- |
| Versión | 1 | `0x01` |
| Receptor Orchard | 43 | a quién va la nota |
| Clave X-Wing | 1.216 | cifra la nota para que solo el dueño la lea, también frente a una computadora cuántica (ML-KEM-768: 1.184 + X25519: 32) |
| Checksum | 4 | errores de tipeo, y la red: cubre el prefijo |

El 96 % es la clave X-Wing, y no hay un KEM postcuántico seguro mucho más chico:
ML-KEM-512 tiene 800 bytes con menos margen, y los otros candidatos son más
grandes. El pool postcuántico
([QUANTIC-POOL-POSTCUANTICO.md](QUANTIC-POOL-POSTCUANTICO.md)) cambia el
receptor pero mantiene X-Wing: el problema no se va solo.

Lo que cuesta hoy:

- **QR:** un solo QR de 1.734 caracteres tiene 141 × 141 módulos, y el 15/10 la
  cámara del teléfono no lo leía desde la pantalla. Por eso ZSilent muestra un
  QR animado de 6 partes, que solo ZKAPriv sabría juntar y que hoy ninguna app
  lee para Quantic.
- **Compartir:** no entra en un SMS, se corta en algunos chats y no se puede
  verificar a ojo.

## La idea

Una **dirección corta es un resumen criptográfico de la dirección completa**.
Quien paga busca la completa en la red y comprueba que el resumen coincida. Como
el resumen ata la dirección entera, el que responde no puede cambiar la clave:
lo peor que puede hacer es no responder.

```text
resumen = BLAKE2b-256("ZokaQ_ShortAddr", prefijo ‖ payload completo)
corta   = prefijo ‖ Base58( versión 0x02 ‖ resumen (32) ‖ checksum (4) )
```

Son 37 bytes: unos **58 caracteres** con el prefijo (`zqtest1…`, `zq1…`), el largo
de una dirección de Bitcoin. No hace falta F4Jumble: cada carácter ya depende de
toda la dirección completa a través del resumen.

- **Seguridad del resumen:** encontrar otra dirección completa con el mismo
  resumen (segunda preimagen) cuesta 2^256 operaciones, o 2^128 con Grover. Una
  colisión (unos 2^85 operaciones cuánticas) solo le sirve a quien controla las
  dos direcciones, no a un atacante contra un tercero.
- **La completa sigue valiendo** (versión `0x01`): lo que hoy la acepta la sigue
  aceptando. ZSilent mostraría la corta por omisión y la completa en "ver
  dirección completa".
- **QR:** 58 caracteres entran en un QR de versión 3 o 4 (29 a 33 módulos), con
  corrección Q o H. Se lee al instante, de lejos y con cámaras malas. El QR
  animado deja de hacer falta para recibir. Opcional: un URI
  `zoka:<corta>?amount=…&memo=…`.

## Dónde se busca la dirección completa

Es la decisión de fondo.

| | A. Directorio en los nodos | B. En la cadena | C. En el nodo del que recibe |
| --- | --- | --- | --- |
| Consenso | No lo toca | Lo cambia | No lo toca |
| Disponibilidad | Se pide a varios nodos; ninguno está obligado | Como cualquier bloque | Solo si el que recibe está prendido |
| Antispam | Sello de trabajo (RandomX) por dirección | La comisión | — |
| Primera vez | Gratis (cuesta el sello) | Hace falta tener ZKA para publicar | — |
| Peso | 1,3 KB por dirección en cada nodo completo | 1,3 KB por dirección en la cadena | — |
| Teléfono | El sello tarda en un teléfono de 32 bits: lo publica el nodo de su ZSilent | Paga una tx | — |

**A. Directorio en los nodos (sin consenso).** La wallet publica su dirección
completa con un sello de trabajo que frena el spam. Cada nodo completo la guarda
y la reenvía por gossip, con tope por par. Las wallets la piden por la API o por
el protocolo liviano, por Tor, y la guardan en su libreta. Un millón de
direcciones son unos 1,3 GB por nodo.

**B. En la cadena.** Una transacción puede publicar direcciones completas, que
los nodos indexan. La disponibilidad queda garantizada, pero hay que cambiar el
consenso antes del génesis de la mainnet (o con un fork después). Y aparece el
problema del huevo y la gallina: quien no tiene monedas no puede publicar, salvo
que publique por él el que le paga o el equipo.

**C. En el nodo del que recibe** (la corta diría cómo contactarlo por iroh o por
onion): si está apagado no se le puede pagar. **Descartada.**

**Recomendación: A en la testnet**, con la dirección completa siempre de
respaldo, para medir disponibilidad y spam con uso real. **Decidir B antes del
génesis de la mainnet** según lo que muestre la testnet; si se elige B, tiene que
estar en el consenso de la mainnet desde su génesis.

## Privacidad

- Publicar la dirección completa no revela nada que no se comparta hoy. Lo nuevo
  es que queda en el directorio: cualquiera puede ver que esa dirección existe y
  desde cuándo. Quien no quiera eso comparte la completa y no la publica.
- Al buscar, el nodo consultado ve qué dirección corta se busca; por Tor no ve
  quién la busca. La libreta evita repetir la búsqueda.
- Direcciones diversificadas: cada una es otra dirección completa con otra
  corta, y los resúmenes no se relacionan entre sí. Publicar varias del mismo
  dueño solo las vincula por la hora y el nodo de publicación; se mitiga
  publicando por Tor y espaciado.

## Plan por etapas

1. **Formato** (`crates/zoka-quantic`): versión `0x02`, codificar, decodificar,
   resumen y vectores de prueba. Sin red.
2. **Directorio en el nodo:** almacenamiento, sello, gossip con tope por par,
   `GET /quantic/address/<corta>` y la misma consulta en el protocolo liviano.
   Pruebas de spam y de disponibilidad con varios nodos.
3. **`quantic-wallet`:** `address --short`, `publish-address`, y que `send --to`
   acepte una corta (la resuelve, la verifica y la guarda en la libreta).
4. **ZSilent:** muestra la corta con su QR fijo, publica la dirección al crear la
   wallet (con permiso explícito del usuario) y resuelve las cortas al enviar.
5. **ZKAPriv**, cuando tenga Quantic: lee el QR fijo.
6. **Medir en la testnet** y decidir A o B antes de la mainnet.

## Lo que no se puede cambiar después del génesis

Solo la opción B, porque es consenso. Con A el directorio es de los nodos, y el
formato de la corta es de las wallets: se pueden mejorar cuando haga falta.

## Preguntas abiertas

- ¿Cuánto trabajo pide el sello? Con 2^16 hashes de RandomX es más o menos un
  minuto en una PC y demasiado en un teléfono de 32 bits. Hay que medirlo con el
  minero real.
- ¿Cuántas direcciones guarda un nodo, y cuáles descarta si se llena (las que
  nadie pidió en mucho tiempo)?
- ¿Conviene que la corta lleve también el receptor Orchard (43 bytes más, unos
  117 caracteres)? Para pagar no hace falta, porque igual se busca la completa.
