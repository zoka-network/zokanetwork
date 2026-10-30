# BORRADOR — Anuncio: testnet v2, ZokaScan y la clave de vista

> Preparado el 30/10/2026. **No publicar sin la aprobación del usuario**: lo
> pega él en Discord. Cada versión entra en un mensaje de Discord (menos de
> 2.000 caracteres). Revisar versiones y enlaces antes de pegar.

---

## Castellano

**Zoka Quantic Testnet v2 está en marcha** 🟣

Desde el 29/10 corre la testnet v2: un pool privado postcuántico, con pruebas
STARK sobre Poseidon2, cifrado X-Wing y firmas ML-DSA-65. Son monedas de prueba,
sin valor. La testnet v1 se retiró, y sus monedas no pasan a la v2.

**Para usarla**
• ZSilent 4.0.4, la billetera para Windows: https://github.com/zoka-network/zsilent-core/releases/tag/v4.0.4
• El minero CLI 4.0.2, para Windows y Linux: https://github.com/zoka-network/zokanetwork/releases/tag/v4.0.2
Se mina con XMRig (RandomX) a través del Stratum de tu propio nodo. No hay que
abrir puertos.

**ZokaScan volvió**: https://zokascan.cc
Bloques, gráficos de la red (hashrate, dificultad, tiempo entre bloques) y una
API pública de solo lectura.

**Nuevo: tu saldo con la clave de vista**
En ZSilent: Settings → Security → *Show view key*. En el minero CLI:
`show-view-key.cmd`. La pegás en *My wallet* de ZokaScan y el navegador abre
tus notas ahí mismo: la clave no sale de la página.
La clave de vista no puede gastar (para eso hacen falta las 24 palabras), pero
quien la tenga ve todos tus pagos y no se puede revocar. Compartila como
compartirías un resumen del banco.

**Una ayuda**: hoy la red depende de un solo nodo semilla. Si podés dejar un
nodo prendido todo el día, avisá en el canal.

Verificá el SHA-256 de cada descarga con el que figura en su release.
Más información: https://zoka.network

---

## English

**Zoka Quantic Testnet v2 is live** 🟣

Since 29 October, testnet v2 has been running: a post-quantum private pool, with
STARK proofs over Poseidon2, X-Wing encryption and ML-DSA-65 signatures. Test
coins, no value. Testnet v1 is retired; its coins do not carry over.

**Get started**
• ZSilent 4.0.4, the Windows wallet: https://github.com/zoka-network/zsilent-core/releases/tag/v4.0.4
• CLI miner 4.0.2, for Windows and Linux: https://github.com/zoka-network/zokanetwork/releases/tag/v4.0.2
Mine with XMRig (RandomX) through your own node's Stratum. No ports to open.

**ZokaScan is back**: https://zokascan.cc
Blocks, network charts (hashrate, difficulty, block time) and a public,
read-only API.

**New: your balance by view key**
In ZSilent: Settings → Security → *Show view key*. In the CLI miner:
`show-view-key.cmd`. Paste it in ZokaScan's *My wallet*: your browser opens
your notes locally, and the key never leaves the page.
A view key cannot spend (that takes your 24 words), but whoever holds it sees
every payment, and it cannot be revoked. Share it as you would a bank
statement.

**You can help**: the network relies on a single seed node today. If you can
keep a node running around the clock, say so in the channel.

Check each download's SHA-256 against the one in its release.
More: https://zoka.network
