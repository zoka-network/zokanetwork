# Etapa 2: Zenith pasa a ser la testnet

Rama `zenith-testnet` en el nodo, ZSilent (`zsilent-core`) y ZKAPriv (`zkapriv`).
Implementado el 11/10/2026. No hay altura de activación acordada para nada
post-cuántico: Zenith valida exactamente igual que antes.

## Decisión: se cambia el nombre, no los datos

La hoja de ruta proponía mover `.zoka/mainnet` a `.zoka/testnet`. Al revisarlo
contra el código y contra una instalación real se decidió **no mover nada**:

- Las bases guardan la etiqueta `mainnet` (o el chain id) en
  `chain:genesis:network`, y el nodo rechaza una base cuya marca no coincide.
- ZSilent, los scripts de minería y el propio nodo, cuando se relanza, pasan
  `--network mainnet`. Los clientes viejos no conocen otra palabra.
- Si los datos se movieran, un cliente viejo y uno nuevo instalados en la misma
  PC verían wallets distintas: el viejo seguiría escribiendo en `.zoka/mainnet`.
- En una instalación real ya existe `.zoka/testnet` con base y wallets de la
  vieja `zoka-testnet-v1`. Copiar encima habría mezclado dos cadenas.
- Zoka Quantic va a tener su propio perfil y sus propias rutas, que ningún
  cliente viejo conoce. No necesita liberar `.zoka/mainnet`.

Qué queda igual: chain id `zoka-mainnet-zenith-v1`, `.zoka/mainnet`,
`docs/mainnet-network.v1.json`, `docs/genesis.mainnet.v1.json`, modo de minería
`pow-mainnet`, la etiqueta interna `mainnet` y el campo `network` del JSON.

Qué cambia:

| Dónde | Antes | Ahora |
|---|---|---|
| CLI `--network` | `mainnet` → Zenith, `testnet` → zoka-testnet-v1 | `testnet`, `zenith` y `mainnet` → Zenith |
| `zoka-testnet-v1` | se podía abrir | retirada: error claro; `.zoka/testnet` no se lee ni se toca |
| Textos del CLI | "mainnet" | "Zenith testnet" |
| JSON del CLI y `/health`, `/ready` | `network: mainnet` | igual, más `network_role: testnet` y `network_name: Zenith testnet` |
| ZSilent | "Zenith mainnet" | "Zenith testnet", con color de testnet; ya no empaqueta el manifiesto de zoka-testnet-v1 |
| ZKAPriv | "Zoka Network · MainNet" | "Zoka Network · Zenith testnet" y aviso en los términos |

## Procedimiento para actualizar un cliente

1. Cerrar ZSilent o detener el nodo.
2. Reemplazar el binario (o instalar el ZSilent nuevo). No hace falta mover,
   copiar ni borrar ninguna carpeta.
3. Comprobar que se ven las mismas wallets y la misma altura que antes:
   `zoka wallets --network testnet` y `zoka height --network testnet`.
   `--network mainnet` da el mismo resultado.
4. Con el nodo andando, `GET /health` devuelve `network: mainnet`,
   `network_role: testnet` y `network_name: Zenith testnet`.

Volver atrás es reinstalar el binario anterior: los datos no cambiaron de lugar
ni de formato.

La carpeta `.zoka/testnet` de quien haya probado `zoka-testnet-v1` queda
intacta. El software no la borra; si alguien quiere eliminarla, que la respalde
antes.

## Marco de activación post-cuántica

`economics::PqFeature` nombra lo que se va a probar en Zenith antes de Quantic:

| Función | Etapa | Altura compilada | Variable para redes locales |
|---|---|---|---|
| `pq-hybrid-addresses` | 3 | `None` | `ZOKA_PQ_HYBRID_ADDRESSES_ACTIVATION_HEIGHT` |
| `pq-spend-circuit` | 5 | `None` | `ZOKA_PQ_SPEND_CIRCUIT_ACTIVATION_HEIGHT` |
| `pq-hybrid-snapshot-signatures` | 6 | `None` | `ZOKA_PQ_HYBRID_SNAPSHOT_SIGNATURES_ACTIVATION_HEIGHT` |

Reglas:

- Una altura para Zenith se fija en la constante de `economics.rs`, se publica
  en un release y se anuncia con margen. Hoy no hay ninguna acordada.
- La variable de entorno sirve solo para ensayar en redes locales. Un nodo de
  la red pública que la use se separa de los demás.
- Los ganchos de validación se agregan con cada función, en su etapa. Hoy
  ningún camino de consenso consulta este marco.

## Pruebas

| Qué | Resultado |
|---|---|
| Pruebas nuevas del nodo: nombres de red, rutas históricas, cadena retirada, bases marcadas con `mainnet` o con el chain id, presentación y marco PQ | OK en Windows y bajo qemu-user (Linux aarch64) |
| Suite completa del nodo en Windows con `pq-experimental` | 883 lib (6 ignoradas), 79 bin y 3 PQ OK. Falla la misma prueba heredada que en la etapa 1 (`recovery_manifest_matches_the_compiled_1_9_consensus_policy`, versión 1.9.0 contra 2.0.2) |
| ZSilent: `gradlew test` | 62/62 (59 previas y 3 nuevas) |
| ZKAPriv: `:core` y `:ui` `compileDebugKotlin` | OK; no se generó APK |
| Wallet creada por el binario 2.0.2 instalado, listada por el nuevo | Misma dirección y desbloqueada con `--network mainnet`, `testnet` y `zenith`; el viejo la sigue viendo; no se crea `.zoka/testnet` |
| `fullnode` nuevo sobre una copia de la base real | Arranca con `network: mainnet`, `network_role: testnet`, `network_name: Zenith testnet`; sirve snapshot por iroh a un cliente 2.0.2 |

Las pruebas con binarios usaron carpetas temporales (`ZOKA_HOME`) y copias de
la base; no se tocó `%APPDATA%\Zoka`. El binario nuevo (release, features por
defecto, SHA-256 `CED0D8C6…`) se compiló desde `c1f0f44b`, antes de rebasar la
rama sobre la documentación de la etapa 1; el código (`src`, `Cargo.*`, `tests`)
es idéntico al de `b5414f5f`. El viejo es el 2.0.2 instalado en ZSilent
(`7DBCF418…`). Resultados de iroh y de la red real en
[RESULTADOS-postcuantica-etapa1.md](RESULTADOS-postcuantica-etapa1.md#compatibilidad-con-la-red).

## Pendiente

- Publicar el anuncio ([borrador](ANUNCIO-zenith-testnet-BORRADOR.md)) cuando
  el usuario lo apruebe y haya clientes nuevos publicados.
- Releases de nodo, ZSilent y ZKAPriv con estos cambios (no se generaron).
- La rama de ZSilent se armó sobre `sacar-exchange` (8c09b71). En la copia de
  trabajo de `zsilent-core` había un cambio sin commitear en
  `ZSilentAppState.kt` (-1195 líneas) que no se tocó ni se incluyó.
