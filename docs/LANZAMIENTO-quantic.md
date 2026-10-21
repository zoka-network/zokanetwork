# Lanzamiento de Zoka Quantic: testnet y mainnet

21/10/2026. Qué hace falta, en orden, para abrir cada red. Las claves privadas y
las frases **nunca** van a este repositorio ni a ningún documento: solo sus
partes públicas.

## Lo que ya está (rama `quantic-cadena-completa`)

- Las tres redes de Quantic con perfil propio: mainnet (`zoka-quantic-v1`, P2P
  28380, API 3300), testnet (`zoka-quantic-testnet-v1`, 28180, 3100) y regtest
  (28280, 3200). No comparten protocolo, tópicos, carpetas ni manifiesto con
  Zenith ni entre sí.
- Transacciones Ironwood + X-Wing, consenso, mempool, minería con RandomX,
  dificultad ASERT (16 bits de arranque en mainnet, 14 en testnet), wallets
  livianas por P2P, snapshots que rearman el estado.
- Autoridad de checkpoints **híbrida obligatoria**: una red de Quantic sin la
  mitad ML-DSA de la clave no tiene autoridad.
- libp2p 0.57, `cargo audit` sin vulnerabilidades conocidas.

## Testnet

**Pasos del usuario, en la PC:**

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
   PC, junto al respaldo de la clave de Zenith. Pasarle a Claude las dos
   **claves públicas** que imprime.

**Pasos de Claude:**

3. Génesis: `zoka quantic-genesis --network quantic-testnet --premine-to <dirección>
   --out docs/quantic-testnet-genesis.v1.json` (RandomX a 14 bits).
4. Fijar en `src/economics.rs`: `QUANTIC_TESTNET_GENESIS` (hash del bloque y
   dirección), y las dos públicas de la autoridad en `CHECKPOINT_AUTHORITY_KEYS`
   y `CHECKPOINT_AUTHORITY_MLDSA_KEYS` para `zoka-quantic-testnet-v1`.
5. Compilar con `--features randomx,pq-experimental,quantic`, correr las pruebas y
   un ensayo local: un segundo nodo importa el génesis embebido, sincroniza,
   recibe un checkpoint híbrido y la wallet del equipo ve el premine.
6. ZSilent de Quantic (ver [PROPUESTA-quantic-en-las-apps.md](PROPUESTA-quantic-en-las-apps.md)).

**Pasos del usuario, otra vez:**

7. Instalar el binario y ZSilent nuevos (con su aprobación explícita, como
   siempre). La PC arranca el nodo de la testnet minando: es la primera semilla.
8. Con la dirección onion de la PC (el mismo servicio de ZSilent, con un segundo
   `HiddenServicePort 28180`), Claude la agrega a `seed_nodes` del manifiesto de
   la testnet.
9. Pushear y publicar el release.

## Mainnet

Lo mismo con `--network quantic-mainnet`, más:

- **Antes del génesis:** la revisión externa de lo que hizo ZOKA
  ([REVIEW-zoka-quantic.md](REVIEW-zoka-quantic.md)) y la testnet andando sin
  problemas un tiempo razonable.
- Una wallet del equipo **nueva y distinta** de la de la testnet, para los
  5.000.000 ZKA del premine. Su frase se guarda con el máximo cuidado: es la
  plata del equipo.
- Una clave de autoridad **propia de la mainnet**, distinta de la de la testnet.
- La dificultad inicial (16 bits) se revisa con lo que muestre la testnet.
- El anuncio (8.4 del plan): por qué este relanzamiento es distinto, el premine
  y los checkpoints del equipo declarados de frente.

## Lo que no se puede cambiar después del génesis sin un fork

Prefijos de dirección, memo, comisión convencional (es política, pero las wallets
la asumen), emisión, maduración, ventana de anclas, formato de transacción y de
bloque, dificultad (ASERT, vida media) y la regla de autoridad híbrida.
