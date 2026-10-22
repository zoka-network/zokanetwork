# Lanzamiento de Zoka Quantic: testnet y mainnet

21/10/2026 (revisado el 22/10). Qué hace falta, en orden, para abrir cada red. Las claves privadas y
las frases **nunca** van a este repositorio ni a ningún documento: solo sus
partes públicas.

**Decisiones del usuario (21/10):**

- **Dos redes, las dos desde cero: Zoka Quantic Testnet y Zoka Quantic
  Mainnet.** Zenith se retiró: nada de su cadena, sus saldos ni sus direcciones
  pasa a Quantic, y una build con `quantic` no la abre (`--network zenith` y sus
  comandos de wallet responden qué usar en su lugar).
- **La testnet arranca ya**, con el protocolo de hoy, y **se reinicia desde
  cero** (con direcciones nuevas) cuando esté el pool privado 100 %
  postcuántico. Sus monedas no valen nada.
- **La mainnet sale solo con el protocolo completo** (ver
  [QUANTIC-POOL-POSTCUANTICO.md](QUANTIC-POOL-POSTCUANTICO.md)).

## Lo que ya está

Nodo, rama `quantic-cadena-completa` (versión 3.0.0):

- Las redes de Quantic con perfil propio: mainnet (`zoka-quantic-v1`, P2P
  28380, API 3300) y testnet (`zoka-quantic-testnet-v1`, 28180, 3100), más
  regtest (28280, 3200) solo para pruebas locales. No comparten protocolo,
  tópicos, carpetas ni manifiesto entre sí.
- Transacciones Ironwood + X-Wing, consenso, mempool, minería con RandomX y
  control en caliente, dificultad ASERT (16 bits de arranque en mainnet, 14 en
  testnet), wallets livianas por P2P, snapshots que rearman el estado.
- Autoridad de checkpoints **híbrida obligatoria** (Ed25519 + ML-DSA-65).
- TLS híbrido X25519MLKEM768 en las conexiones de iroh.
- `zoka quantic-wallet` completa (alta, frase, saldo, historial, comisión,
  envío con memo, contraseña, respaldo).
- libp2p 0.57, `cargo audit` sin vulnerabilidades conocidas.

ZSilent 3.0, rama `quantic-solo` de `zsilent-core`: solo Quantic, probado contra
un nodo real.

Qué es postcuántico hoy y qué no:
[QUANTIC-INVENTARIO-CRIPTOGRAFICO.md](QUANTIC-INVENTARIO-CRIPTOGRAFICO.md).

## Testnet

**Pasos del usuario, en la PC** (con el `zoka.exe` nuevo: hasta que se instale,
`C:\Users\mderramus\ZOKA\target\release\zoka.exe`):

1. Wallet del equipo de la testnet (recibe el premine de prueba):

   ```text
   zoka quantic-wallet new --network quantic-testnet --name equipo --password-stdin
   ```

   Anotar la frase de 24 palabras en papel, fuera de la PC. Pasarle a Claude la
   **dirección** (`zqtest1…`), que es pública.
2. Clave de la autoridad de la testnet (las dos mitades):

   ```text
   zoka checkpoint-keygen --network quantic-testnet
   ```

   Quedan en `%APPDATA%\Zoka\.zoka\quantic-testnet\`. Respaldar los dos archivos
   (`checkpoint-authority.key` y `checkpoint-authority-mldsa.key`) fuera de la
   PC. Pasarle a Claude las dos **claves públicas** que imprime.

**Pasos de Claude:**

3. Génesis: `zoka quantic-genesis --network quantic-testnet --premine-to <dirección>
   --out docs/quantic-testnet-genesis.v1.json` (RandomX a 14 bits). Lleva la
   hora en que se crea y la dificultad ASERT se ancla ahí: si hasta el primer
   bloque minado pasan horas, los primeros salen al piso de 12 bits hasta
   alcanzar el horario de 120 s. En la testnet no rompe nada; en la mainnet el
   génesis se crea justo antes de lanzar.
4. Fijar en `src/economics.rs`: `QUANTIC_TESTNET_GENESIS` (hash del bloque y
   dirección), y las dos públicas de la autoridad en `CHECKPOINT_AUTHORITY_KEYS`
   y `CHECKPOINT_AUTHORITY_MLDSA_KEYS` para `zoka-quantic-testnet-v1`; el
   manifiesto de la testnet pasa de `pre-genesis` a `active`. Los pasos 3 y 4
   los hace `python scripts/lanzar-testnet-quantic.py <zoka.exe> <dirección>
   <pública Ed25519> <pública ML-DSA>` (solo datos públicos; revisa todo antes
   de escribir).
5. Compilar los dos binarios (`zoka` y `zokahd` tienen que decir 3.0.x), correr
   las pruebas y un ensayo local: un segundo nodo importa el génesis embebido,
   sincroniza y recibe un checkpoint híbrido (firmado con una clave de ensayo,
   nunca con la real). La wallet real del equipo la revisa el usuario en ZSilent.

   **Ensayo general del 22/10, con wallet y autoridad descartables:** pasaron
   los pasos 3 a 5 completos. El nodo importó el génesis del binario y minó
   encima (la dificultad bajó de 14 a 13 bits por los 20 minutos entre génesis y
   primer bloque), un segundo nodo lo siguió con el suministro exacto (premine +
   12 ZKA por bloque), la wallet "equipo" vio los 5.000.000 ZKA gastables desde
   el primer bloque y el checkpoint híbrido (versión 2, con ML-DSA) pasó.

   ```text
   cargo build --release --features randomx,quantic --bins
   ```

6. Armar el instalador de ZSilent 3.0 con ese binario, desde la raíz del nodo.
   Primero la revisión, después el paquete. La carpeta de Tor va explícita: la
   que el script usa por omisión (`windows-x86_64`) es la 15.0.24 y el script
   exige la 15.0.25 (Tor 0.4.9.14).

   ```text
   powershell -ExecutionPolicy Bypass -File .\zsilent-core\scripts\prepare-release.ps1 -TorBundleDir .\third_party\tor\windows-x86_64-0.4.9.14
   powershell -ExecutionPolicy Bypass -File .\zsilent-core\scripts\prepare-release.ps1 -TorBundleDir .\third_party\tor\windows-x86_64-0.4.9.14 -Package
   ```

   El setup de Groth16 de Zenith (12 MB) sigue en el instalador a propósito: un
   nodo de Quantic no lo usa, pero si quedara algún camino de Zenith sin cubrir,
   con el setup verifica en vez de colgarse.

**Pasos del usuario, otra vez:**

7. Instalar el binario y ZSilent 3.0 (con su aprobación explícita, como
   siempre). La PC arranca el nodo de la testnet minando: es la primera semilla.
8. La semilla ya está en el manifiesto (22/10): ZSilent usa una sola clave de
   servicio onion y una sola identidad de iroh para todas las redes, así que la
   PC publica la misma onion que tenía en Zenith
   (`d5pk3nhesk5wlvz4f5unbhcozxgtj7u6mhagauyprjrxndzpwbaoeiid`), ahora en el
   puerto 28180, y el mismo EndpointId de iroh (`ca7721e2…`). El manifiesto viaja
   embebido en el binario y dentro del instalador, así que el instalador que se
   publique ya la lleva. Falta comprobar, con la PC andando, que un segundo nodo
   la alcanza por la onion.
9. Pushear y publicar el release.

**Cuando esté el pool postcuántico:** génesis nuevo, testnet nueva desde cero
(otro chain id), wallets nuevas.

## Mainnet

Lo mismo con `--network quantic-mainnet`, más:

- **Antes del génesis:** el pool privado 100 % postcuántico terminado, la
  revisión externa ([REVIEW-zoka-quantic.md](REVIEW-zoka-quantic.md)) y la
  testnet con ese pool andando sin problemas un tiempo razonable.
- Una wallet del equipo **nueva y distinta** de la de la testnet, para los
  5.000.000 ZKA del premine. Su frase se guarda con el máximo cuidado: es la
  plata del equipo.
- Una clave de autoridad **propia de la mainnet**, distinta de la de la testnet.
- La dificultad inicial (16 bits) se revisa con lo que muestre la testnet.
- El anuncio: por qué este relanzamiento es distinto, el premine y los
  checkpoints del equipo declarados de frente.

## Lo que no se puede cambiar después del génesis sin un fork

Prefijos de dirección, memo, comisión convencional (es política, pero las wallets
la asumen), emisión, maduración, ventana de anclas, formato de transacción y de
bloque, dificultad (ASERT, vida media) y la regla de autoridad híbrida.
