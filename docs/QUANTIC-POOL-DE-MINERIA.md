# Minería con pools en Zoka Quantic

Primera versión: 22/10/2026, un borrador de diseño de un pool. Actualizado el
23/10/2026, después de que el usuario pidiera apuntar a los pools grandes y a
P2Pool, con opciones Stratum.

## Estado

| | Estado |
| --- | --- |
| PoW compatible con XMRig (consenso) | Hecho. Vale desde el génesis de la testnet del 23/10 (`b7d58bf4…`) |
| Stratum en el nodo (`--stratum host:puerto`) | Hecho y probado con XMRig. Es minería solo a través del nodo propio, sin custodia |
| ZSilent 3.0.2: sección Stratum | Hecho. Se retiró la sección Mining Pool de Zenith |
| RPC para el software de pools de Monero | Pendiente (ver «Pools grandes») |
| Pool sin custodia propio, estilo P2Pool | Pendiente (ver «Opciones») |

## El PoW de Quantic (v2)

`build_quantic_pow_input` en `src/mining/pow.rs` arma un blob de 76 bytes con la
forma del de Monero, que es lo que XMRig sabe recorrer:

| Bytes | Contenido |
| --- | --- |
| 0 | versión `0x02` |
| 1..7 | hora del bloque, u48 little endian |
| 7..39 | SHA-256 de `zoka-quantic-pow-v2-where`, chain_id, prev_hash y altura |
| 39..43 | nonce de 32 bits, little endian (el que cambia XMRig) |
| 43..75 | SHA-256 de `zoka-quantic-pow-v2-what`, merkle_root, state_root y extranonce |
| 75 | cero |

- El nonce de 64 bits del encabezado es `extranonce << 32 | nonce32`. Con el
  extranonce, cada conexión del Stratum recorre su propio espacio sin tocar el
  coinbase.
- El hash es RandomX `rx/0` estándar (`randomx-rs`). La semilla de la época es
  SHA-256 de 32 bytes, y la recibe el minero en `seed_hash`.
- El hash se compara con el objetivo como un número little endian de 256 bits,
  como en Monero. Por eso el objetivo de 8 bytes que entiende XMRig sirve tal
  cual.
- Regtest hashea con SHA3 para ir rápido: XMRig no puede minarla, y el Stratum
  se niega a arrancar ahí.

## El Stratum del nodo

`src/quantic/stratum.rs`. Habla el protocolo de pools de Monero (`login`, `job`,
`submit`, `keepalived`).

- **A quién paga.** Cada bloque le paga entero a la dirección del `login` del
  minero:
  - una dirección completa;
  - una corta, que el nodo resuelve con su directorio (tiene que estar
    publicada);
  - si el login no es una dirección (por ejemplo `x`, o el nombre de un
    equipo), a la de `--quantic-mine-to` del nodo.

  Un login que empieza como dirección de Quantic (`zq1`, `zqtest1`,
  `zqregtest1`) y no es de esta red, o está mal copiado, recibe un error. No se
  le paga en silencio a la del nodo.
- **Aciertos.** Son 16 veces más fáciles que un bloque, para ver que el minero
  trabaja. El nodo recalcula cada hash. Si no coincide con el del minero, lo
  rechaza: el formato no es el que el minero cree.
- **Plantillas.** Se rehacen cuando cambia la punta o cada 30 s. Puede haber
  hasta 16 direcciones de cobro distintas a la vez, porque cada una necesita su
  prueba Halo 2 del coinbase en cada plantilla.
- **Límites.** Hasta 256 conexiones a la vez y líneas de hasta 8 KiB.
- **Dónde escucha.** Por defecto, en `127.0.0.1`. Abrirlo a la red local es una
  decisión de quien corre el nodo.
- **API.** `GET /quantic/stratum` devuelve dónde escucha, los mineros
  conectados, los aciertos aceptados y rechazados, y los bloques encontrados.
- **ZSilent.** La sección **Stratum** lo enciende, por defecto solo para esta
  PC; una opción lo abre a la red local. ZSilent reinicia el nodo con
  `--stratum`, muestra el estado y arma la línea de XMRig. No trae ni lanza
  XMRig, y avisa que los antivirus lo tratan como amenaza.

```sh
xmrig -o 127.0.0.1:3333 -u <dirección corta publicada> -p x -a rx/0
```

### Lo que se probó (23/10)

XMRig 6.26.0 oficial (hash del zip verificado) contra un nodo de testnet local y
aislado, con el génesis nuevo, entrando con la dirección corta de una wallet de
prueba:

- 6 aciertos aceptados a dificultad 4096 y ninguno rechazado: el hash que
  recalcula el nodo es el mismo que el de XMRig;
- 3 bloques de XMRig entraron en la cadena;
- la wallet cobró los 36 ZKA de esos 3 bloques (todavía sin madurar) por su
  dirección corta.

Ese mismo día, en la PC del usuario: XMRig 6.25.0 oficial (versión gcc,
verificada contra el `SHA256SUMS` del release) minó por la red local contra
ZSilent 3.0.3, con la dirección corta de la wallet del equipo. Encontró 7
bloques en minutos, con 0 aciertos rechazados.

Windows Defender cierra XMRig a los pocos segundos de arrancar
(`Trojan:Win64/HashvaultMiner.A`, ThreatID 2147978346). Le pasa a cualquier
minero de CPU; no es algo de Zoka. Una exclusión de carpeta no alcanza, porque
lo detecta por comportamiento. Para la prueba, el usuario permitió esa
detección puntual mientras duró.

## Pools grandes

- **Pools de Monero (nodejs-pool, cryptonote-nodejs-pool y parecidos).** Le
  piden al nodo `get_block_template`, con un espacio reservado para su
  extranonce, y `submit_block`. Arman el blob a minar con un módulo nativo que
  conoce el formato de cada moneda. Para que un operador sume Zoka hacen falta
  dos cosas:
  - un RPC del nodo con esos dos métodos, que devuelva el blob de 76 bytes, el
    extranonce y la semilla, y acepte el bloque resuelto;
  - el formato de Zoka en ese módulo, o que el pool tome el blob ya armado del
    nodo.

  Esos pools tienen custodia: cobran ellos y les pagan después a los mineros.
  Es decisión de cada operador. Zoka no diseña pools con custodia.
- **xmrig-proxy.** Junta muchos XMRig en una sola conexión de Stratum. Debería
  funcionar contra el nodo, pero falta probarlo.
- **P2Pool.** El de Monero es solo de Monero: su cadena de aciertos paga en el
  coinbase de Monero, con el formato de transacciones de Monero. Para Zoka
  habría que construir uno propio (opción C de abajo), no sumarse al existente.

## Opciones de un pool propio, sin custodia

**B. Equipos (lo de Zenith, llevado a Quantic).** Un coordinador reparte turnos
según el trabajo aportado, y cada bloque le paga entero al del turno. No cambia
el consenso, pero hace falta confiar en el coordinador. Ya no está en ZSilent:
se retiró junto con el resto del minado en equipo de Zenith, y no es la
prioridad.

**C. Cadena de aciertos estilo P2Pool.** Los mineros arman entre todos una
cadena de aciertos, y cada bloque real les paga en su coinbase a los que
aportaron en la última ventana. Nadie coordina ni tiene custodia.

- El consenso admite hasta 64 acciones en el coinbase (`MAX_ACTIONS_PER_TX`), y
  solo el génesis exige una (`check_coinbase` en
  `crates/zoka-quantic/src/state.rs`). Un coinbase de varias salidas **no
  necesita fork**.
- Su costo: cada salida es una acción de Orchard con su prueba Halo 2, y hay
  que rehacerla en cada cambio de plantilla. Con 2 acciones tarda del orden de
  un segundo; con 64, falta medirlo.
- Variante más barata: cada bloque les paga solo a los N con más saldo
  acumulado (por ejemplo, N = 8).

## Orden propuesto

1. **Hecho:** PoW v2 y Stratum en el nodo. Cualquiera con XMRig mina a través
   de su nodo, y un operador puede poner un nodo con `--stratum` para varios
   mineros, cada uno cobrando a su dirección.
2. **Siguiente:** el RPC para el software de pools de Monero
   (`get_block_template` y `submit_block`), para que un pool existente pueda
   sumar Zoka.
3. **Después de la mainnet, si hay muchos mineros:** la opción C, con N salidas,
   midiendo antes el costo de prueba del coinbase.

## Preguntas abiertas

- ¿Cuántas salidas aguanta un coinbase sin que la plantilla tarde demasiado en
  rehacerse?
- En la opción C, ¿cuál es la ventana de aciertos, y cómo se paga un bloque que
  cae cuando cambia la ventana?
