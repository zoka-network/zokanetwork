# Minero CLI de Zoka Quantic (testnet)

Miná la **Zoka Quantic Testnet** desde una terminal, sin la wallet de
escritorio. Este paquete es el nodo de Zoka (`zoka`, el mismo que ZSilent usa
por dentro) con unos scripts que crean una wallet, minan y muestran tu saldo.

> **¿Preferís una aplicación?** Usá **ZSilent**, la wallet oficial de
> escritorio: el mismo nodo y la misma red, con botones en vez de comandos.

- **Monedas de prueba, sin valor.** La testnet vuelve a empezar de cero cuando
  esté listo el pool privado totalmente postcuántico.
- **Privado de entrada.** Las recompensas son notas blindadas: la cadena no
  muestra qué dirección las recibió.
- **Sin puertos abiertos.** El nodo llega a la red por Tor, que viene incluido
  (Windows) o se instala desde tu distribución (Linux).

## Qué trae

| Archivo | Para qué |
| --- | --- |
| `zoka` / `zoka.exe` | El nodo y la wallet de Zoka Quantic (versión 3.0.4) |
| `zoka-miner.ps1` / `zoka-miner.sh` | Un script para cada paso de abajo |
| `*.cmd` (Windows) | Accesos de doble clic a ese script |
| `tor/` (Windows) | Tor 0.4.9.14, del Tor Expert Bundle 15.0.25 oficial |
| `SHA256SUMS.txt` | Los hashes de todos los archivos del paquete |

Todo lo que escribe el minero (la cadena, la wallet, Tor) queda en una carpeta
`data`, al lado de los scripts.

## Requisitos

- Un procesador de 64 bits y **4 GB de RAM o más** (los datos de RandomX ocupan
  unos 2 GB).
- **Windows 10 u 11** (x64), o **Linux x86-64** con `tor` instalado
  (`sudo apt install tor` en Debian o Ubuntu).
- Conexión a internet. No hay que abrir nada en el router.

## 1. Verificá la descarga

Compará el SHA-256 del archivo con el que está publicado al lado, en el release:

```powershell
# Windows (PowerShell)
Get-FileHash .\zoka-cli-miner-3.0.4-windows-x64.zip -Algorithm SHA256
```

```bash
# Linux
sha256sum zoka-cli-miner-3.0.4-linux-x64.tar.gz
```

Si no coinciden, no lo ejecutes: bajalo de nuevo del release oficial.

## 2. Creá tu wallet

- **Windows:** descomprimí el paquete donde quieras (por ejemplo, en Documentos)
  y hacé doble clic en **`1-create-wallet.cmd`**.
- **Linux:** `tar xzf zoka-cli-miner-3.0.4-linux-x64.tar.gz`, entrá a la carpeta
  con `cd` y ejecutá `./zoka-miner.sh create-wallet`.

Elegí una contraseña (8 caracteres como mínimo). Después vas a ver **24
palabras**: **anotalas en papel y guardalas bien.** Son la única forma de
recuperar la wallet, se muestran una sola vez, y quien las tenga puede gastar
tus monedas.

También te aparece tu **dirección** (57 caracteres, empieza con `zqtest1`). Es
la que compartís para que te paguen.

¿Ya tenés una wallet, por ejemplo de ZSilent? Usá **`restore-wallet.cmd`** o
`./zoka-miner.sh restore-wallet` con tus 24 palabras.

## 3. Miná

- **Windows:** doble clic en **`2-start-mining.cmd`**.
- **Linux:** `./zoka-miner.sh mine`

El script arranca Tor y después el nodo. El nodo baja la cadena, espera a estar
conectado con al menos otro nodo (para que tus bloques lleguen a la red) y
empieza a minar. Usa todos los hilos del procesador menos uno; para cambiarlo,
`zoka-miner.ps1 mine -Threads 2` (Windows) o `./zoka-miner.sh mine --threads 2`
(Linux). Para parar, **Ctrl+C**.

Cada bloque paga **12 ZKA** a tu wallet. Se pueden gastar después de **60
bloques**, unas dos horas al ritmo previsto de un bloque cada dos minutos.

## 4. Mirá tu saldo

Con el minero andando, abrí otra ventana y ejecutá **`check-balance.cmd`**
(Windows) o `./zoka-miner.sh balance` (Linux). Te pide la contraseña de la
wallet.

Otros comandos: **`show-address.cmd`** / `./zoka-miner.sh address` muestra tu
dirección, y **`status.cmd`** / `./zoka-miner.sh status` muestra la altura y tus
bloques.

Mientras el nodo anda, tu dirección se publica sola en la red, así cualquiera
puede pagarte.

## Minar con XMRig (opcional)

Zoka Quantic usa RandomX (`rx/0`), el mismo algoritmo que Monero, así que
[XMRig](https://xmrig.com) puede minarla a través del servidor Stratum del nodo:

1. Ejecutá **`start-node-for-xmrig.cmd`** (Windows) o `./zoka-miner.sh xmrig-node`
   (Linux). El nodo abre un servidor Stratum en el puerto 3333 y no mina por su
   cuenta.
2. Arrancá XMRig con tu dirección:

   ```text
   xmrig -o 127.0.0.1:3333 -u <tu dirección> -p x -a rx/0
   ```

   Desde otra computadora de tu red local, usá la IP local de esta en lugar de
   `127.0.0.1`. Windows puede preguntar si `zoka.exe` puede aceptar conexiones:
   permitilo solo en redes privadas.

Cada minero entra con su propia dirección y se lleva la recompensa entera de
cada bloque que encuentra. Es minería individual a través de tu nodo, no un
pool compartido.

XMRig es un programa aparte y no viene incluido. Bajalo solo de sus releases
oficiales y verificá su SHA-256. Los antivirus, Windows Defender incluido,
marcan a los mineros de CPU como XMRig y los frenan. Una GPU también puede minar
RandomX, pero rinde menos y gasta más energía que una CPU.

## Tus datos y tus copias de seguridad

- Todo está en **`data`**, al lado de los scripts. No la borres si no tenés tus
  24 palabras.
- Tus **24 palabras** son la copia de seguridad. Con ellas podés restaurar la
  wallet en este paquete o en ZSilent, en cualquier computadora.
- También se puede hacer una copia cifrada en un archivo:
  `zoka quantic-wallet backup --network quantic-testnet --name miner --out <archivo>`
  (con `ZOKA_HOME` apuntando a la carpeta `data`).

## Puertos

| Puerto | Para qué |
| --- | --- |
| `127.0.0.1:3180` | La API local del nodo (la usan los scripts) |
| `127.0.0.1:28181` | El puerto P2P del nodo, solo local (los pares llegan por Tor) |
| `127.0.0.1:19150` | El puerto SOCKS de Tor |
| `0.0.0.0:3333` | El Stratum para XMRig, solo con `xmrig-node` |

No chocan con los de ZSilent, así que los dos pueden estar instalados en la
misma computadora. No mines con los dos a la vez: se reparten la misma CPU.

## Si algo no anda

- **"A node is already running":** otra ventana tiene el minero andando. Cerrala
  primero.
- **Tor no arranca:** revisá la conexión a internet y si hay un proxy o un
  firewall; el detalle está en `data/tor/tor.log`.
- **Nunca empieza a minar:** el nodo espera a estar conectado con otro nodo, por
  Tor. Dejalo unos minutos y mirá `status`.
- **Windows advierte sobre el programa:** los binarios todavía no están firmados
  digitalmente. Verificá el SHA-256 (paso 1): si coincide con el del release, el
  archivo es el oficial.

## Usar los comandos a mano

Los scripts solo llaman a `zoka`. Para usarlo a mano, apuntá `ZOKA_HOME` a la
carpeta `data` y pasá `--network quantic-testnet`. Por ejemplo, para enviar:

```text
zoka quantic-wallet send --network quantic-testnet --name miner --to <dirección> --amount 5 --password-env ZOKA_CLI_PASSWORD --rpc http://127.0.0.1:3180
```

`zoka --help` lista todos los comandos. La contraseña siempre va por una
variable de entorno (`--password-env`), nunca como argumento.
