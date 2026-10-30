# Minero CLI de Zoka Quantic Testnet v2

Miná la **Zoka Quantic Testnet v2** desde una terminal, sin la wallet de
escritorio. Este paquete es el nodo de Zoka (`zoka`, el mismo que ZSilent 4.0
usa por dentro) con unos scripts que crean una wallet, arrancan el nodo para
minar con XMRig y muestran tu saldo.

> **¿Preferís una aplicación?** Usá **ZSilent 4.0**, la wallet oficial de
> escritorio para Windows: el mismo nodo y la misma red, con botones en vez de
> comandos.

- **Monedas de prueba, sin valor.** La testnet v2 reemplaza a la v1 (la de
  ZSilent 3.0): las wallets y las monedas de la v1 no pasan a la v2.
- **Privada y postcuántica.** Las recompensas y los pagos son notas blindadas,
  con pruebas STARK y cifrado X-Wing: la cadena no muestra qué dirección las
  recibe.
- **Sin puertos abiertos.** El nodo llega a los demás por QUIC, con los relays
  de n0. No hay que tocar el router ni instalar Tor.

## Qué trae

| Archivo | Para qué |
| --- | --- |
| `zoka` / `zoka.exe` | El nodo y la wallet de Zoka Quantic (versión 4.0.2) |
| `zoka-miner.ps1` / `zoka-miner.sh` | Un script para cada paso de abajo |
| `*.cmd` (Windows) | Accesos de doble clic a ese script |
| `SHA256SUMS.txt` | Los hashes de todos los archivos del paquete |

**XMRig no viene incluido** (ver el paso 3). Todo lo que escribe el minero (la
cadena, la identidad del nodo, las wallets) queda en una carpeta `data`, al lado
de los scripts.

## Requisitos

- Un procesador de 64 bits y **4 GB de RAM o más**: XMRig usa unos 2,5 GB.
- **Windows 10 u 11** (x64), o **Linux x86-64** con glibc 2.31 o más nueva
  (Ubuntu 20.04 o Debian 11 en adelante).
- Conexión a internet. No hay que abrir nada en el router.

## 1. Verificá la descarga

Compará el SHA-256 del archivo con el que está publicado al lado, en el release:

```powershell
# Windows (PowerShell)
Get-FileHash .\zoka-cli-miner-4.0.2-windows-x64.zip -Algorithm SHA256
```

```bash
# Linux
sha256sum zoka-cli-miner-4.0.2-linux-x64.tar.gz
```

Si no coinciden, no lo ejecutes: bajalo de nuevo del release oficial.

## 2. Creá tu wallet

- **Windows:** descomprimí el paquete donde quieras (por ejemplo, en Documentos)
  y hacé doble clic en **`1-create-wallet.cmd`**.
- **Linux:** `tar xzf zoka-cli-miner-4.0.2-linux-x64.tar.gz`, entrá a la carpeta
  con `cd` y ejecutá `./zoka-miner.sh create-wallet`.

Elegí una contraseña (8 caracteres como mínimo). Después vas a ver **24
palabras**: **anotalas en papel y guardalas bien.** Son la única forma de
recuperar la wallet, se muestran una sola vez, y quien las tenga puede gastar
tus monedas.

También te aparece tu **dirección corta** (57 caracteres, empieza con
`zqtest2`). Es la que compartís para que te paguen.

¿Ya tenés una wallet de la testnet v2, por ejemplo de ZSilent 4.0? Usá
**`restore-wallet.cmd`** o `./zoka-miner.sh restore-wallet` con tus 24 palabras.

## 3. Bajá XMRig

En la testnet v2 el nodo no mina solo: mina [XMRig](https://xmrig.com), el
minero de RandomX (`rx/0`, el algoritmo de Monero), a través del servidor
Stratum del nodo. Cada bloque que encuentra paga entero a tu wallet: es minería
individual a través de tu nodo, no un pool compartido.

1. Bajá XMRig solo de sus [releases oficiales](https://github.com/xmrig/xmrig/releases)
   (`xmrig-<versión>-windows-x64.zip`, o el `linux-static-x64` en Linux) y
   verificá su SHA-256 con el `SHA256SUMS` que publican al lado.
2. Descomprimilo **dentro de la carpeta de este paquete**. La carpeta que trae
   el archivo (`xmrig-<versión>`) sirve tal cual. También podés indicar dónde
   está: `zoka-miner.ps1 mine -Xmrig C:\ruta\xmrig.exe` o
   `./zoka-miner.sh mine --xmrig /ruta/xmrig`.

Los antivirus, Windows Defender incluido, marcan a los mineros de CPU como
XMRig y los frenan.

## 4. Miná

- **Windows:** doble clic en **`2-start-mining.cmd`**.
- **Linux:** `./zoka-miner.sh mine`

El script arranca el nodo, que se conecta a la red y baja la cadena. Después
publica tu dirección corta, así cualquiera puede pagarte con ella, y arranca
XMRig. XMRig usa todos los hilos del procesador menos uno. Para cambiarlo:
`zoka-miner.ps1 mine -Threads 2` (Windows) o `./zoka-miner.sh mine --threads 2`
(Linux). **Ctrl+C** detiene los dos.

Si el script no encuentra XMRig, deja el nodo esperando y te muestra el comando
para arrancarlo vos en otra ventana:

```text
xmrig -o 127.0.0.1:3333 -u miner -p x -a rx/0
```

Cada bloque paga **12 ZKA** a tu wallet. Se pueden gastar después de **60
bloques**, unas dos horas al ritmo previsto de un bloque cada dos minutos.

## 5. Mirá tu saldo

Con el minero andando, abrí otra ventana y ejecutá **`check-balance.cmd`**
(Windows) o `./zoka-miner.sh balance` (Linux). Te pide la contraseña de la
wallet. Con el nodo apagado, te muestra el saldo de la última vez que anduvo.

Otros comandos: **`show-address.cmd`** / `./zoka-miner.sh address` muestra tu
dirección, y **`status.cmd`** / `./zoka-miner.sh status` muestra la altura, los
pares y los mineros conectados.

## 6. Tu saldo desde cualquier lado: la clave de vista

**`show-view-key.cmd`** (Windows) o `./zoka-miner.sh view-key` (Linux) muestra la
**clave de vista** de tu wallet (`zqtestview2…`). Pegala en
[zokascan.cc/wallet](https://zokascan.cc/wallet): el explorador baja las salidas
cifradas de la cadena y las abre adentro de tu navegador, así que te muestra el
saldo y cada pago sin que la clave salga de la página.

- **No puede gastar**: para gastar hacen falta tus 24 palabras. Una wallet de
  Zoka Quantic no tiene una clave de gasto aparte.
- Quien la tenga ve todos los pagos de la wallet, los de antes y los que vengan.
  Compartila solo como compartirías un resumen del banco: no se puede revocar.
  Si se filtra, pasá tus monedas a una wallet nueva.

## Ayudá a la red: dejá un nodo prendido

Hoy la testnet v2 depende de un solo nodo semilla, el del equipo. Cada nodo que
queda prendido todo el día hace más firme la red, y un nodo que nunca se apaga
puede ser una **semilla**: uno de los nodos que vienen dentro de Zoka para que
los nodos nuevos encuentren la red.

- Dejá corriendo **`2-start-mining.cmd`** / `./zoka-miner.sh mine`. Sin XMRig
  el nodo igual corre y sirve la cadena; con XMRig, además mina.
- Para ofrecerlo como semilla, ejecutá **`show-node-id.cmd`** /
  `./zoka-miner.sh node-id` y mandale los dos identificadores al equipo de Zoka
  por Discord. Son públicos: nunca dan acceso a tu wallet ni a tu nodo. El
  equipo los agrega al documento de la red en la próxima versión.
- La IP de una semilla la ven los nodos que se conectan a ella, como la de
  cualquier nodo. Conservá la carpeta `node`: los identificadores viven ahí, y
  una carpeta nueva es una identidad nueva.

## Tus datos y tus copias de seguridad

- Todo está en **`data\quantic-testnet-v2`**, al lado de los scripts:
  - `node`: la cadena y la identidad del nodo;
  - `wallets`: tus wallets, cifradas con su contraseña;
  - `wallet-mirror`: la copia de la cadena que leen las wallets.
- **No copies la carpeta `node` a otra computadora.** Dos nodos con la misma
  identidad se pisan en la red. Cada instalación crea la suya.
- Tus **24 palabras** son la copia de seguridad. Con ellas podés restaurar la
  wallet en este paquete o en ZSilent 4.0, en cualquier computadora.
- También se puede hacer una copia cifrada en un archivo:
  `zoka quantic-pq-wallet backup --network testnet --dir data\quantic-testnet-v2 --name miner --out <archivo>`

## Puertos

| Puerto | Para qué |
| --- | --- |
| `127.0.0.1:9180` | La API local del nodo (la usan los scripts) |
| `127.0.0.1:3333` | El Stratum para XMRig (`-StratumPort` / `--stratum-port` lo cambia) |

Los dos escuchan solo en esta computadora. Hacia afuera el nodo usa QUIC (UDP)
y los relays de n0, sin puertos abiertos.

La API no choca con la de ZSilent 4.0. El Stratum sí usa el mismo puerto que el
de ZSilent: si tenés el de ZSilent encendido, elegí otro con
`zoka-miner.ps1 mine -StratumPort 3334` o `./zoka-miner.sh mine --stratum-port 3334`.
De todas formas, no mines con los dos a la vez: se reparten la misma CPU.

## Si algo no anda

- **"A node is already running":** otra ventana tiene el minero andando. Cerrala
  primero.
- **"The node stopped" o el nodo no arranca:** el detalle está en
  `data\quantic-testnet-v2\node-errors.log`.
- **XMRig no conecta:** mirá que el puerto del Stratum no lo esté usando otro
  programa (por ejemplo ZSilent) y que el nodo siga andando (`status`).
- **XMRig se cierra apenas arranca:** suele ser el antivirus. El script te
  muestra con qué código salió.
- **Windows advierte sobre el programa:** los binarios todavía no están firmados
  digitalmente. Verificá el SHA-256 (paso 1): si coincide con el del release, el
  archivo es el oficial.

## Usar los comandos a mano

Los scripts solo llaman a `zoka`. Para usarlo a mano, pasá
`--network testnet --dir data/quantic-testnet-v2`. Por ejemplo, para enviar,
con el nodo andando:

```text
zoka quantic-pq-wallet send --network testnet --dir data/quantic-testnet-v2 --name miner --to <dirección> --amount 5 --password-env ZOKA_CLI_PASSWORD --peer data/quantic-testnet-v2/node/peer.json --api http://127.0.0.1:9180
```

`zoka quantic-pq-wallet help` lista los comandos de la wallet, y
`zoka quantic-pq-node help`, los del nodo. La contraseña siempre va por una
variable de entorno (`--password-env`), nunca como argumento.
