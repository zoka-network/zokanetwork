# Lanzamiento de la testnet v2

**Lanzada el 29/10/2026** (rama `quantic-v2-genesis`). El registro de lo que
pasó está al final, en «Cómo salió».

Decisiones del usuario (28/10): la testnet v2 reemplaza a la v1; el génesis se
crea cuando estén el estado compacto y la red pública (ya están); el usuario
crea la wallet del equipo; la PC sigue como nodo autoritario.

## Qué fija el lanzamiento

Dos cosas que quedan compiladas en el binario:

1. **El bloque 1**, que trae el premine de 5.000.000 ZKA:
   `QUANTIC_PQ_TESTNET_GENESIS` en `src/economics.rs` pasa de `None` a su
   hash. Desde ahí los nodos solo aceptan ese bloque 1.
2. **La semilla:** `docs/quantic-testnet-v2-network.v1.json` pasa a
   `"status": "active"` y lista la PC como `seed1`, fijada por su endpoint id
   y la huella de su clave ML-DSA. Un nodo nuevo la encuentra por los relays
   de n0, sin puertos abiertos.

## Por qué el bloque 1 se mina en el momento

La ASERT se ancla en la hora del bloque 1. Si después del bloque 1 nadie mina,
la dificultad baja: se divide por dos por cada hora de atraso. Cuando se
vuelve a minar sale una ráfaga de bloques, que es lo que le pasó a la v1. Por
eso el bloque 1 se mina en el momento del lanzamiento, y desde ahí se mina sin
parar.

Al arrancar también hay bloques de más, por el otro lado. Los 14 bits iniciales
(`TESTNET_INITIAL_DIFFICULTY_BITS`, compartido con la v1) le dan a XMRig de la
PC un bloque cada 15 a 25 s: unos 680 H/s con 2 hilos y unos 1.100 con 3 o 4.
La ASERT llega a 120 s por bloque (unos 17 bits) en hora y media o dos. Son
unos 200 a 300 bloques de más, entre 2.400 y 3.600 ZKA, menos del 0,1 % del
premine. Subir el valor inicial solo para la v2 sería una constante nueva de
consenso. Se decide antes del bloque 1.

## Antes de empezar

- **ZSilent 3.0.5 cerrada**, o con su Stratum apagado: los dos usan el puerto
  3333.
- **El binario previo**, sin el génesis fijado:
  `cargo build --release --features randomx,quantic-pq --bins`.
- **XMRig** (6.25.0, el oficial) y el permiso de Defender (ThreatID
  2147978346). Los pone y los saca el usuario.

## Pasos

Los pasos 1 a 3 se pueden hacer de dos maneras.

**Opción A (recomendada): desde ZSilent 4.0.** Claude abre la versión de
desarrollo (`gradlew run` en `zsilent-core`, rama `quantic-v2`) con el OK del
usuario.

1. El onboarding crea la wallet del equipo; el usuario la llama `equipo` y
   anota la frase.
2. ZSilent arranca el nodo con las mismas rutas que el guion del paso 2.
3. Se enciende el Stratum en su sección. ZSilent publica la corta de la wallet
   y XMRig entra con ella (`-u <corta>`).

Es además la prueba de la app entera en la PC. El paso 7 se reduce a instalar
el MSI encima.

**Opción B: por la línea de comandos**, como siguen los pasos 1 a 3. En el
paso 7 el onboarding restaura `equipo` con su frase, bajo otro nombre, porque
`equipo` ya existe en la carpeta.

### 1. La wallet del equipo (la crea el usuario)

```powershell
$Zoka = "C:\Users\mderramus\ZOKA\target\release\zoka.exe"
$V2 = "$env:APPDATA\Zoka\.zoka\quantic-testnet-v2"
$env:ZSILENT_WALLET_PASSWORD = "<contraseña>"
& $Zoka quantic-pq-wallet new --name equipo --network testnet --dir $V2 --password-env ZSILENT_WALLET_PASSWORD --json
Remove-Item Env:ZSILENT_WALLET_PASSWORD
```

Imprime **una sola vez** la frase de 24 palabras: se anota fuera de la PC. El
`address` (la completa) es el que cobra el premine. La wallet queda en
`$V2\wallets\equipo.json`, donde la encuentra ZSilent 4.0.

### 2. El nodo semilla de la PC

```powershell
.\scripts\semilla-testnet-v2.ps1 -MineTo "<address completa de equipo>"
```

Lo arranca con las rutas de ZSilent 4.0:

- la carpeta es `$V2\node`;
- la identidad se cifra con `node.identity.secret`, 32 bytes al azar que el
  guion crea la primera vez y nunca imprime;
- usa el Stratum en 127.0.0.1:3333;
- si la carpeta de la v1 tiene las dos claves de la autoridad, pasa
  `--authority`.

Cuando se instale ZSilent 4.0, sigue el mismo nodo, con la misma identidad.

### 3. XMRig (lo arranca el usuario)

```powershell
xmrig.exe -o 127.0.0.1:3333 -u equipo -p x -a rx/0 -t 3
```

`equipo` no es una dirección, así que cobra la de `-MineTo`, el premine
incluido. Desde el bloque 1, **se mina sin parar**.

### 4. Fijar el génesis y la semilla (Claude)

Con el bloque 1 minado:

```powershell
Invoke-RestMethod http://127.0.0.1:9140/blocks/1/summary
Get-Content "$V2\node\peer.json"
```

- **El resumen:** su `reward_atoms` tiene que ser 5000000000000, y
  `quantic-pq-wallet balance --name equipo` tiene que mostrar el premine.
- **El hash** del resumen va a `QUANTIC_PQ_TESTNET_GENESIS`.
- **De `peer.json`** salen `endpoint_addr.id` y `ml_dsa_key_id`, para la
  semilla del manifiesto:

  ```json
  {"label": "seed1", "operator": "Zoka Network", "endpoint_id": "<id>",
   "ml_dsa_key_id": "<huella>", "status": "active",
   "note": "La PC del equipo: semilla y autoridad de checkpoints."}
  ```

Después van las pruebas y un commit.

### 5. Los binarios de la versión

- El nodo para Windows, y para Linux en el Debian 11 de WSL.
- ZSilent 4.0: `prepare-release.ps1` y el MSI.

Se verifican los hashes de cada uno.

### 6. Un nodo nuevo, solo con el manifiesto

En otra carpeta, o mejor en otra máquina: `quantic-pq-node serve --network
testnet` sin `--peer`. Tiene que encontrar `seed1` por los relays, bajar el
bloque 1 con el hash fijado y seguir la cadena.

### 7. La PC pasa a ZSilent 4.0 (lo instala el usuario)

Con el nodo detenido se instala el MSI, que reemplaza a la 3.0.5. ZSilent
sigue el mismo nodo, con la misma identidad y la misma cadena.

- **Con la opción A**, el setup y la wallet ya están.
- **Con la opción B**, el onboarding es nuevo y se restaura `equipo` con su
  frase.

XMRig se detiene solo lo que tarda el cambio.

### 8. Publicar (con el OK del usuario)

Las versiones en GitHub y GitLab, la web zoka.network y el aviso en Discord.

## Lo que no se hace

- **Reiniciar la cadena después de publicar:** el hash del bloque 1 ya está en
  los binarios de todos.
- **Correr dos nodos con la misma identidad** (la carpeta `node` copiada a otra
  PC).
- **Firmar checkpoints de la testnet fuera de la cadena real:** en un ensayo,
  `-Authority none`. Un checkpoint de otra cadena, firmado con las claves de
  verdad, frenaría a los nodos de la real.

## Si algo sale mal antes de publicar

Mientras el hash no está en un binario publicado, se puede empezar de nuevo:

1. se detiene el nodo y se renombra `$V2\node\db`, sin borrarlo;
2. se vuelve al paso 2.

La identidad y la wallet del equipo se conservan.

## Cómo salió

29/10/2026, por la opción A (ZSilent 4.0 de desarrollo en la PC).

- **La wallet del equipo** la creó el usuario en el onboarding con el nombre
  `zsilent`, no `equipo`: `$V2\wallets\zsilent.json`.
- **XMRig** (6.25.0, 3 hilos) entró al Stratum a las 08:35:10. El **bloque 1**
  salió a las 08:35:20, con hora de cabecera 1793273705 (08:35:05 -03:00), el
  hash `69249437d8c35b7e4bf28ec524491fe18384b9869670fadff546fef29148fdb2` y
  los 5.000.000 ZKA del premine. Quedó fijado en el commit `85cc0c19`, junto
  con `seed1` (endpoint `e3bf05e2…c7eb70c4`, clave `2a29953c…a822f9`).
- **Ritmo de arranque:** un bloque cada 20 s más o menos durante la primera
  media hora, como se esperaba con los 14 bits iniciales. El Stratum contó
  algunos bloques de más: son soluciones dobles del mismo trabajo, que quedan
  como ramas laterales. Con la dificultad mínima, uno de cada cuatro shares de
  XMRig es un bloque.
- **El paso 6** pasó con el binario del génesis: un nodo en una carpeta nueva,
  sin `--peer`, encontró `seed1` por las semillas compiladas, bajó la cadena
  con el bloque 1 fijado, la siguió y aceptó los checkpoints 2 y 33 que firmó
  la PC.

Lo que apareció en el camino, todo arreglado antes de armar la versión:

1. ZSilent pasaba `--mine-to` sin el Stratum, y en la testnet el nodo v2 así
   no arranca. El usuario encendió el Stratum; el código quedó arreglado en
   `zsilent-core` `7b8547f`.
2. La app de desarrollo buscaba el manifiesto por el nombre de la red y
   encontraba el de Zenith (`testnet-network.v1.json`), con la API en el
   puerto 3000. Por eso el nodo de la PC escucha en el 3000 hasta que se
   instale el MSI. Ahora busca `quantic-testnet-v2-network.v1.json`
   (`7b8547f`).
3. Cada comando de la wallet que encontraba bloques nuevos armaba los 2 GB de
   RandomX en modo rápido: unos 50 s con todos los núcleos, cada dos minutos
   en ZSilent, compitiendo con XMRig. Los comandos v2 (`quantic-pq`,
   `quantic-pq-node` y `quantic-pq-wallet`) ahora usan el modo liviano (256 MB,
   los mismos hashes): ningún comando v2 mina con RandomX. La consulta baja a
   medio segundo y el nodo, de 2,3 GB a unos 270 MB. `ZOKA_RANDOMX_MODE=full`
   lo fuerza.
4. RocksDB guardaba 1000 logs de información por base, y la copia de la wallet
   se abre en cada comando. Ahora guarda 5.

Con eso el nodo pasó a **4.0.0** y el minero CLI se rehízo para la v2: sin Tor,
con XMRig por el Stratum.
