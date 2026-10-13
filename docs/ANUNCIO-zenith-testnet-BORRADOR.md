# BORRADOR — Anuncio: Zenith pasa a ser la testnet

> Borrador preparado el 11/10/2026 para la etapa 2.5 de la hoja de ruta.
> **No publicar sin la aprobación del usuario.** Revisar fechas, enlaces y
> versiones de clientes antes de difundirlo. Hay versión en castellano y en inglés.

---

## Castellano

**Zenith, la red que funciona hoy, pasa a ser la testnet de Zoka.**

La red principal va a ser **Zoka Quantic**, una cadena nueva que todavía está en
desarrollo y no tiene fecha de lanzamiento. Mientras tanto, Zenith sigue
funcionando como hasta ahora y se usa para probar todo lo nuevo antes de
llevarlo a Quantic.

Qué cambia y qué no:

- **Los ZKA de Zenith son de prueba.** No tienen valor y **no se trasladan** a
  Zoka Quantic: no habrá reclamo ni migración de saldos.
- **La cadena es la misma.** Mismo historial, mismas wallets, mismos nodos. No
  hace falta reinstalar ni mover archivos. Los clientes viejos la siguen
  llamando "mainnet"; los nuevos la muestran como "Zoka Quantic · Testnet".
- **Se puede seguir minando y enviando** en Zenith, ahora con el fin de probar.
- **El P2P Exchange se retiró** del nodo, de ZSilent y de la documentación.

Zoka Quantic arranca de cero:

- Suministro máximo de **23.000.000 ZKA**. La minería emite **18.000.000**:
  12 ZKA por bloque durante 1.051.200 bloques y 6 ZKA durante 897.600 bloques
  más. Después la minería sigue cobrando comisiones.
- **Premine del equipo de 5.000.000 ZKA**, dentro de ese máximo y gastable desde
  el bloque génesis.
- El objetivo es que direcciones, montos y el grafo de gastos sigan ocultos aun
  frente a una computadora cuántica futura. **Hoy eso está en desarrollo y no
  está activo en ninguna red.** Las primitivas criptográficas se prueban primero
  en Zenith y van a necesitar revisión externa antes del lanzamiento.

Vamos a anunciar la fecha de Zoka Quantic recién cuando el diseño esté
implementado, probado en la testnet y revisado.

### Versión 2.1.0 (completar las alturas y la fecha al publicar)

**Zenith 2.1.0: hay que actualizar antes del bloque {ALTURA_BUNDLES}.**

- Desde el bloque {ALTURA_BUNDLES} cada bloque lleva adentro los cuerpos de sus
  transacciones privadas, y desde el {ALTURA_V2} la red acepta las direcciones
  postcuánticas `zkq1` (X-Wing: X25519 + ML-KEM-768) para probarlas. Es la
  primera prueba pública de esta parte de Quantic; la red sigue siendo de prueba.
- Un nodo que no actualice queda en otra rama desde esa altura.
- **Checkpoints del equipo.** El nodo del equipo firma cada 30 bloques un
  checkpoint ("en la altura H el bloque es X") y los nodos 2.1.0 no siguen
  ninguna rama que lo contradiga: evita forks. Es una confianza explícita en el
  equipo, que elige cuál de las cadenas válidas es la canónica, pero no puede
  crear monedas, gastar fondos ajenos ni volver válido un bloque inválido.
  Quantic va a usar el mismo mecanismo con otra clave.
- ZSilent ya no muestra un saldo inflado por notas que ya se habían gastado.

---

## English

**Zenith, the network running today, becomes the Zoka testnet.**

The main network will be **Zoka Quantic**, a new chain that is still in
development and has no launch date yet. Meanwhile Zenith keeps running as it
does today and is where everything new gets tested before it reaches Quantic.

What changes and what does not:

- **Zenith ZKA are test coins.** They have no value and **do not carry over** to
  Zoka Quantic: there is no claim and no balance migration.
- **It is the same chain.** Same history, same wallets, same nodes. Nothing to
  reinstall and no files to move. Older clients still call it "mainnet"; new
  ones show it as "Zoka Quantic · Testnet".
- **Mining and sending keep working** on Zenith, now for testing purposes.
- **The P2P Exchange has been removed** from the node, ZSilent and the docs.

Zoka Quantic starts from zero:

- Maximum supply of **23,000,000 ZKA**. Mining issues **18,000,000**: 12 ZKA per
  block for 1,051,200 blocks and 6 ZKA for another 897,600 blocks. After that,
  miners keep earning fees.
- **Team premine of 5,000,000 ZKA**, inside that maximum and spendable from the
  genesis block.
- The goal is for addresses, amounts and the spend graph to stay hidden even
  against a future quantum computer. **This is in development and not active on
  any network today.** The cryptographic building blocks are tested on Zenith
  first and will need external review before launch.

We will announce a Zoka Quantic date only once the design is implemented,
tested on the testnet and reviewed.

### Release 2.1.0 (fill in heights and date when publishing)

**Zenith 2.1.0: update before block {HEIGHT_BUNDLES}.**

- From block {HEIGHT_BUNDLES} every block carries the bodies of its private
  transactions, and from block {HEIGHT_V2} the network accepts post-quantum
  `zkq1` addresses (X-Wing: X25519 + ML-KEM-768) for testing. This is the first
  public test of this part of Quantic; the network is still a testnet.
- A node that does not update ends up on a different branch from that height.
- **Team checkpoints.** Every 30 blocks the team's node signs a checkpoint
  ("at height H the block is X") and 2.1.0 nodes do not follow any branch that
  contradicts it, which prevents forks. This is explicit trust in the team: it
  picks which valid chain is canonical, but it cannot create coins, spend
  anyone's funds or make an invalid block valid. Quantic will use the same
  mechanism with a different key.
- ZSilent no longer shows a balance inflated by notes that were already spent.
