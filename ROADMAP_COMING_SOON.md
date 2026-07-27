# ZSilent Core — Roadmap: Explorer, P2P Exchange, Mining Pool

Última actualización: 2026-07-26
Complementa a `ROADMAP.md` (Fases A–D, wallet/core). Cubre las tres secciones que aparecieron en
el sidebar de ZSilent en v1.9.1 como "Coming Soon" (`ui/screens/Screens.kt`) y que hoy no tienen
funcionalidad real detrás.

Orden de ejecución: **Etapa 0 (limpieza) → Explorer → P2P Exchange → Mining Pool**.

---

## Decisiones cerradas (no reabrir)

### Transversales

| Tema | Decisión |
| --- | --- |
| Código muerto | Borrar el stack de UI paralelo **antes** de empezar (Etapa 0). |
| Tema | Un solo tema oficial: el verde pastel actual. Borrar `DarkScheme` lila y el parámetro `darkTheme` muerto. |
| Idioma | Toda la UI nueva en inglés, igual que el resto de la app. |
| Versionado | Release conjunto nodo+ZSilent al cerrar cada etapa, probados juntos. |
| Rollout | Cada sección sale detrás de un flag opt-in en Settings, marcada como experimental. |

### Explorer

| Tema | Decisión |
| --- | --- |
| Conectividad | 100% local — solo RPC del nodo propio. Nunca `zokascan.cc` ni terceros. |
| Base de código | Reutilizar la pantalla real existente y re-skinearla. No reescribir. |
| Alcance | Completo: bloques + transacciones + búsqueda unificada. |
| Nodo apagado / sincronizando | Estado explícito y honesto, nunca datos silenciosamente desactualizados. |

### P2P Exchange

| Tema | Decisión |
| --- | --- |
| Par | Solo ZKA → USDT, oferta unidireccional. Sin árbitro humano. |
| Red USDT | Tron (TRC-20). Proveedor TronGrid con fallback configurado. |
| Descubrimiento | Gossip P2P nativo, extendiendo `ZokaMessage`. No polling RPC. |
| Privacidad | La consulta a la API de Tron **siempre** sale por Tor. |
| Quién verifica | Solo el vendedor (es quien tiene la clave para liberar). |
| Vendedor offline | La oferta solo es visible/tomable con el nodo del vendedor online. |
| Liberación | Automática al detectar pago verificado. Sin botón manual. |
| Incumplimiento | Se registra y se comparte por gossip, con prueba de pago adjunta. |
| Monto incorrecto | De menos no libera; de más libera igual. |
| Persistencia | Ofertas y reservas persistidas en disco. |
| Anti-spam | Monto mínimo por oferta + tope de ofertas abiertas por address. |
| Comisión | Ninguna. El 100% del USDT va al vendedor. |
| Tope de monto | Sin tope en v1 (riesgo asumido, §2.8). |
| Expiración | 45 min desde que se toma la oferta (configurable). |
| Cancelación | El vendedor puede cancelar una oferta abierta aún no tomada. |
| Testing | Mainnet con montos mínimos. Sin fase de testnet. |
| Consentimiento | Pantalla de riesgos con aceptación explícita, una sola vez. |

### Mining Pool

| Tema | Decisión |
| --- | --- |
| Modelo | "Team mining" opt-in, equipos auto-organizados. Sin pool oficial de Zoka. |
| Custodia | **Ninguna.** No hay wallet de equipo: el coinbase rota entre las addresses de los miembros. |
| Reparto | Por turnos según shares aportadas — el coinbase mismo es el pago. |
| Consenso | Sin cambios. El coinbase sigue teniendo un receptor único. |
| Bloque hijackeado | No se puede impedir a nivel protocolo. Se detecta y se expulsa al miembro. |
| Coordinador caído | Coordinador de respaldo toma el rol automáticamente. |
| Dificultad de shares | Automática, ajustada al hashrate de cada minero. |
| Comisión | Ninguna. |
| Tamaño de equipo | Sin límite duro en v1. |

---

## Por qué este orden

- **Etapa 0** primero: hay dos stacks de UI paralelos en el repo. Limpiar antes evita editar el
  archivo equivocado durante las tres etapas siguientes.
- **Explorer**: la mayoría del backend del nodo ya existe y la pantalla ya está maquetada. Menor
  esfuerzo y riesgo — sirve para asentar el patrón de trabajo y el ciclo de release conjunto.
- **P2P Exchange**: le da liquidez real a la red. Es la de mayor riesgo porque hay fondos en
  juego, por eso lleva un gate de seguridad obligatorio (§2.8) antes de salir de "Coming Soon".
- **Mining Pool**: no depende de las otras dos, no requiere cambio de consenso, y tras la decisión
  de coinbase rotativo tampoco tiene custodia — puede esperar sin bloquear nada.

Cada subsección numerada está escrita para poder entregarse sola a una sesión de desarrollo.

---

## Etapa 0 — Limpieza previa ✅ COMPLETADA

Rama `etapa0-limpieza-ui` (3 commits). `compileKotlin` y `test` en verde. **Sin mergear a
`main`** — pendiente de tu revisión.

### 0.1 Borrar el stack muerto ✅

31 archivos eliminados vía `git rm` (reversible). El cluster muerto resultó ser más grande
de lo estimado: no solo `design/` + `app/` + `ui/screens/dashboard/`, sino también
`ui/screens/onboarding/`, `ui/screens/welcome/` y los 8 componentes `ui/components/Z*.kt`.

Verificado cerrado antes de borrar: `app/ZSilentApp.kt` era el **único** punto de entrada al
cluster y nadie lo importaba; los componentes `Z*` solo se referencian entre sí; ningún test
lo toca; el stack vivo tiene su propio QR y su propio sidebar (inline en `ui/ZSilentApp.kt`,
que es lo que parcheó `b697e0f`).

- [x] `design/` completo — sistema de diseño paralelo sin usar.
- [x] `app/` completo (`ZSilentApp.kt`, `AppState.kt`, `Navigation.kt`).
- [x] `ui/screens/dashboard/`, `ui/screens/onboarding/`, `ui/screens/welcome/`.
- [x] `ui/components/Z{Button,Card,QrCard,SeedWordChip,Sidebar,TextField,TopBar,WarningBox}.kt`.
- [x] Compila y los tests pasan.

### 0.2 Consolidar el tema ✅

- [x] Eliminado el parámetro `darkTheme` de `ZSilentTheme` (lo ignoraba).
- [x] Borrado `DarkScheme` (lila `#C084FC`) y sus ~30 constantes huérfanas — 5 ya no las
      usaba ni el propio `DarkScheme`.
- [x] `LightScheme` → **`ZenithScheme`** (el nombre viejo describía como "Light" una paleta
      casi negra).
- [x] Corregido el call site en `Main.kt`.
- [x] Eliminado `toggleTheme()`, que ninguna UI llamaba.
- [x] **No** se tocó el campo persistido `darkTheme` de `PersistedSetup`/`AppModels`: se
      escribe en los archivos de setup de los usuarios, así que sacarlo es un cambio de
      serialización, no una limpieza. Queda como campo vestigial inofensivo.

### 0.3 Resolver la duplicación de `ExplorerScreen` ✅

- [x] Eliminado el placeholder sin argumentos de `Screens.kt`.
- [x] Router → `ExplorerScreen(uiState, appState)`.
- [x] Quitado el import huérfano `Icons.Outlined.Explore`.
- [ ] **Pendiente:** smoke test visual con la app corriendo (compila, pero no se verificó en
      pantalla). El Explorer ya muestra datos reales, aunque su botón "Open" sigue sin hacer
      nada hasta la Etapa 1.

---

## Etapa 1 — Explorer ✅ COMPLETADA Y VERIFICADA

Mergeada y pusheada a `main`. Tests en verde.

**Verificado contra nodo real (2026-07-26, mainnet altura 12943):** los modelos coinciden
campo por campo. Genesis efectivamente sirve `difficulty_bits: null` — el tipo nullable era
necesario, no defensivo, porque `0` es lo primero que cualquiera busca. Altura fuera de rango
da 404 → "no encontrado"; `/p2p-exchange/offers` da 404 en el binario viejo → "Node too old"
en vez de afirmar que no hay ofertas.

Esa corrida encontró dos errores de fixture que **ningún test unitario podía detectar**
(ambos decodifican igual): `reward_zka_display` es `"12"` pelado, no `"12.00000000 ZKA"`, y
la recompensa son 12_000_000 atoms, no 1_200_000_000. Ya corregidos.

### 1.1 Extender `ZokaRpcClient` ✅

- [x] `blockByHeight(height)` → `GET /blocks/:height`
- [x] `blockByHash(hash)` → `GET /blocks/hash/:hash`
- [x] `blockTransactions(height)` → `GET /blocks/:height/transactions`
- [x] `mempoolTransaction(hash)` → `GET /mempool/tx/:hash`
- [x] `latestBlock()` → `GET /blocks/latest`
- [x] Modelos `BlockSummary`, `BlockTransactions`, `MempoolTransaction`.
- [x] Helper `getOrNull` — 404 devuelve null, fallo de transporte lanza excepción.
- [ ] ~~`transactionByHash(hash)` → `GET /transactions/:hash`~~ **descartado:** ese endpoint
      solo mira el mempool, así que duplicaría `mempoolTransaction`. Ver 1.3.

### 1.2 Conectar el lookup de bloque ✅

- [x] "Open" consulta el nodo de verdad (antes era `onClick = {}`).
- [x] Detalle: hash, padre, state root, timestamp, nonce, dificultad, tamaño, recompensa,
      conteos de tx públicas/privadas/nullifiers.
- [x] Estado de carga durante la consulta.

### 1.3 Detalle de transacción ⚠️ PARCIAL — límite del nodo

**El nodo no tiene índice `tx_hash → bloque`.** Verificado en `src/storage/` y `src/state/`:
no existe. `GET /transactions/:hash` solo consulta el mempool y devuelve 404 con "Check
confirmed blocks" si la tx ya está minada.

Consecuencia: **una transacción confirmada no se puede buscar por hash**, ni desde el Explorer
ni desde ningún cliente. Solo se la ve si ya sabés en qué bloque está.

- [x] Detalle de tx pendiente (hash, timestamp, estado).
- [x] Lista de hashes de tx dentro de un bloque.
- [x] La UI explica el límite en vez de decir "no encontrado" a secas, que sugeriría que la
      transacción nunca existió.
- [ ] **Diferido:** construir el índice en el nodo. Es trabajo de storage en Rust con backfill
      de la cadena existente y crecimiento por bloque — merece su propia etapa, no un apéndice
      apurado de esta.

### 1.4 Búsqueda unificada ✅

- [x] Un input: dígitos → altura; resto → hash de bloque, luego tx pendiente.
- [x] Estado vacío que distingue "no existe" de "todavía no sincronizado".

### 1.5 Estados de nodo apagado / sincronizando ✅

- [x] Nodo apagado: aviso explícito y buscador deshabilitado.
- [x] Sincronizando: avisa que los bloques sobre el tip local no aparecerán todavía.
- [x] Nunca se muestran datos desactualizados sin aclararlo.

### 1.6 QA y cierre de etapa

- [x] Flag opt-in en Settings → Experimental, apagado por defecto. La sección se filtra del
      sidebar (no se muestra deshabilitada) y el router cae al Dashboard si se apaga estando
      dentro.
- [x] **Riesgo cubierto:** `PersistedSetup` se construye desde cero en 7 sitios en vez de
      copiarse, así que un campo que algún sitio olvide pasar vuelve a su default en silencio.
      Los 7 llevan `explorerEnabled` y `SetupStoreExplorerFlagTest` fija el round-trip y el
      camino de upgrade (archivo viejo sin el campo → flag apagado).
- [x] `ExplorerModelDecodingTest` fija los modelos contra los payloads de los serializadores
      del nodo, incluido genesis con `difficulty_bits: null`.
- [x] Verificado por lectura de código: el Explorer solo usa `appState.localRpc`. Ninguna
      llamada sale del nodo propio.
- [ ] **Pendiente — verificación contra nodo real.** Todo lo anterior es build + tests
      unitarios. Nunca se ejecutó contra un nodo vivo: falta probar genesis (altura 0), último
      bloque, tx en mempool, hash inexistente, altura fuera de rango, nodo apagado y nodo a
      media sincronización. Requiere levantar `zoka fullnode`.
- [ ] **Pendiente — smoke test visual.** No se levantó la app; no está verificado en pantalla.
- [ ] Release conjunto nodo+ZSilent (requiere aprobación explícita).

---

## Etapa 2 — P2P Exchange (ZKA → USDT)

### 2.0 Verificar tolerancia del protocolo P2P ✅ RESUELTO

**Veredicto: es seguro agregar la variante.** Medido, no asumido — rama `etapa2-p2p-exchange`
en el repo ZOKA, commit `796c086b`, 3 tests en verde.

- [x] `ZokaMessage` se serializa en **JSON** (`serde_json`), así que una variante desconocida
      es un error de deserialización común y corriente, no un panic.
- [x] Los **6** sitios de `from_bytes` en `p2p.rs` tratan el `Err` igual: `warn!` y seguir. El
      mensaje se descarta, **el peer se conserva**.
- [x] Nada marca al emisor como incompatible: eso solo ocurre vía
      `mark_peer_protocol_version`, que lee el string de protocolo de *identify* y nunca mira
      el payload.
- [x] **No hay peer scoring** configurado en gossipsub (`with_peer_score` ausente), así que
      tampoco hay penalización de reputación.

**Dos hallazgos que el plan no anticipaba:**

1. **Existe un protocol freeze.** `PROTOCOL_FREEZE_ID = "/zoka/1.9.0-mainnet-recovery"`
   (`p2p.rs:74`), y `ZOKA_PROTOCOL_FREEZE_ENFORCE` **por defecto es true**: los binarios
   publicados desconectan peers con un id de protocolo distinto. Agregar la variante **no**
   requiere tocar ese id — y no hay que tocarlo, porque bumpearlo particiona la red. Si alguna
   vez hace falta, existe el mecanismo de grace window (`=0`) usado en rollouts previos.

2. **Tópico separado, decidido por otra razón.** Como los nodos viejos descartan la variante
   igual, un tópico aparte no cambia quién ve las ofertas (solo nodos nuevos, en ambos casos).
   Pero por el tópico `general` **cada oferta emitiría un `warn!` en todos los nodos viejos de
   la red** — spam de logs proporcional al volumen de ofertas. Con un tópico propio los nodos
   viejos ni se suscriben y no ven nada. → **Usar tópico separado.**

> **Estado al 2026-07-26 (fin de sesión).** Rama `etapa2-p2p-exchange` (repo ZOKA), pusheada.
> **66 tests de `p2p_exchange::` en verde**, más `network::message` (20) y `network::p2p` (39)
> sin regresión. UI y flag del Explorer están en `main` de zsilent-core, también pusheado.
>
> **Hecho:** 2.0 (compat P2P), 2.1 (modelo `Offer`), 2.2 (gossip + tópico propio +
> `signing_preimage`), 2.3 (`select_notes` + adaptador de wallet), 2.4 (reglas de pago Tron),
> 2.5 (store persistente + reconciliación), endpoints RPC (`/p2p-exchange/*`), 2.8 (pantalla
> de riesgos) y 2.9 (UI de las 3 secciones).
>
> **Pendiente — sobre todo integración:**
>
> 1. Enchufar `select_notes` a `create_exchange_offer` y marcar las notas reservadas en la
>    wallet. Hoy las ofertas se crean con `reserved_note_ids` vacío.
> 2. Cliente HTTP que consuma `tron::trc20_transfers_path` **por el proxy SOCKS** y aplique
>    `transfer_settles_offer`. Las reglas están; falta el transporte.
> 3. §2.6 presencia del vendedor, §2.7 liberación + reputación.
> 4. Pase visual sobre las 3 secciones (acordado para el cierre de la etapa).
>
> **Deudas anotadas, no olvidadas:**
>
> - La firma de gossipsub autentica el **nodo**, no la **wallet**: alguien puede publicar una
>   oferta con la `zka1` de otro. Aceptado para v1, `signing_preimage()` listo para atarlo.
> - **Nada se verificó nunca contra un nodo real.** Todo es compilación + tests unitarios.
>   Desbloquea: crear la wallet del nodo (`zoka node --password <la tuya>`).
> - El binario del nodo en `target/release` es del 23/7 y **no** tiene los endpoints nuevos;
>   por eso la UI de P2P dice "Node too old" hasta recompilar.
>
> Nota: `cargo test --lib` completo (860+ tests) excede 10 min; usar filtros
> (`cargo test --lib p2p_exchange::`).

### Decisiones cerradas 2026-07-26 (segunda ronda)

| Tema | Decisión |
| --- | --- |
| Clave de firma | **Confiar en la firma de gossipsub** para v1. libp2p ya firma cada mensaje con la identidad del nodo (`MessageAuthenticity::Signed`). **Límite aceptado:** autentica el *nodo*, no la *wallet* — impide que un peer falsifique mensajes ajenos en tránsito, pero **no** impide que alguien publique una oferta con la `zka1` de otro. El campo `signature` y `signing_preimage()` quedan listos para cuando se ate a la wallet. |
| Fondos insuficientes | **Rechazar la oferta.** Si no hay notas suficientes para reservar, la creación falla. Ninguna oferta llega a la red sin respaldo. |
| Id de oferta | **128 bits aleatorios** (`new_offer_id()`). El id entra en el preimagen de firma, así que una colisión sería un vector de falsificación, no una molestia. |
| Endpoint Tron | **TronGrid público por defecto + override en Settings** (URL y API key opcional). Sin fricción inicial, con salida si rate-limitea. |

### 2.1 Modelo de datos `Offer` ✅

Implementado en `src/p2p_exchange/mod.rs`. Decisiones que conviene no revisitar:

- **Enteros, nunca floats.** ZKA en atoms, USDT en micro-USDT (6 decimales, TRC-20). Un float
  redondearía precios en silencio y —peor— haría que dos nodos discrepen sobre si un pago
  coincide, que es el único juicio del flujo sin humano que lo apele.
- La tolerancia del 0.5% compara con multiplicación cruzada en vez de dividir primero, para que
  no se redondee a cero en ofertas chicas.
- `validate()` es lo que corre un nodo receptor antes de retransmitir, así que rechaza ofertas
  auto-inconsistentes (una `Open` que ya nombra comprador, una `Claimed` sin deadline).
- La dirección TRON se valida por forma (34 chars, empieza en `T`, base58). Detecta direcciones
  ETH pegadas por error, no una bien formada pero equivocada — en ese caso el pago nunca llega
  y la oferta expira.

### 2.1-bis Modelo original (referencia)

- [ ] Campos: `id`, `seller_address`, `zka_amount`, `usdt_amount`, `usdt_receive_address`
      (TRC-20), `status` (`open`/`claimed`/`payment_detected`/`released`/`expired`/`cancelled`),
      `created_at`, `claim_expires_at`, `claimed_by`, `reserved_note_ids`, `signature`.
- [ ] Sin campos de comisión: `usdt_amount` es íntegro para el vendedor.
- [ ] Serializable para viajar por el canal P2P — definirlo pensando en eso desde el inicio.
- [ ] Persistido en disco (§2.5), no solo en memoria.

### 2.2 Gossip de ofertas ✅

Tópico propio `zoka/offers/v1` (`OFFERS_TOPIC_NAME`), variantes `OfferAnnounce` y
`OfferStatusUpdate` en `ZokaMessage`. Los updates llevan **solo id + estado nuevo**, nunca el
cuerpo de la oferta: reenviarlo dejaría que un peer altere montos o dirección de cobro
disfrazado de cambio de estado. `validate_inbound_message` rechaza: announce con oferta ya
tomada, claim sin comprador o sin deadline, y cualquier update que reabra una oferta (un
"reopen" resucitaría una oferta cuyos ZKA ya se gastaron).

Falta: firmar los anuncios con la clave del vendedor (el campo `signature` existe pero todavía
no se verifica) y el TTL/rebroadcast.

### 2.3/2.5 Store persistente ✅

`src/p2p_exchange/store.rs`. Claim es compare-and-set (el segundo comprador recibe
`NotClaimable`). Escritura por archivo temporal + rename; archivo corrupto es error, no reset
silencioso. `reconcile()` al arranque barre ofertas vencidas durante la caída y **reporta notas
reservadas sin oferta viva detrás** — ese es el peor caso del feature: una nota reservada
huérfana es ingastable para siempre y el usuario ni siquiera ve por qué le falta saldo.

### 2.2-bis Gossip (checklist original)

- [ ] Mensajes `OfferAnnounce` (publicar) y `OfferStatusUpdate` (tomada/expirada/cancelada).
- [ ] Firmar los anuncios con la clave del vendedor: sin esto cualquiera publica ofertas ajenas
      o cancela las de otros.
- [ ] Anti-spam: no propagar ofertas sin ZKA efectivamente bloqueados.
- [ ] Monto mínimo por oferta + tope de ofertas abiertas simultáneas por address.
- [ ] TTL / rebroadcast: cuánto vive una oferta y qué pasa con las expiradas.
- [ ] Solo propagar ofertas de vendedores online (§2.6).

### 2.3 Lock de ZKA sobre notas privadas

- [ ] Reutilizar `reserved` / `reserved_by_tx` de `PrivateNote`
      (`src/private_wallet/mod.rs`, Fase A1 de `ROADMAP.md`). No construir un mecanismo nuevo.
- [ ] Al expirar/cancelar: `release_reserved_notes`.
- [ ] Al completarse: gastar las notas hacia la address del comprador.

### 2.4 Verificación de pago USDT vía Tor

- [ ] Cliente HTTP con Ktor CIO (ya es dependencia) configurado con **proxy SOCKS hacia el
      TorSupervisor existente**. La consulta a Tron nunca sale directo.
- [ ] Manejar Tor caído como estado explícito: sin Tor no hay verificación, y el usuario tiene
      que verlo — no fallar en silencio ni caer a conexión directa.
- [ ] TronGrid como proveedor primario, un fallback configurado (ej. TronScan).
- [ ] Solo el vendedor consulta. El comprador ve el estado por gossip.
- [ ] Matching: address de cobro + monto + timestamp posterior al claim. Nunca solo por monto.
- [ ] Pago de menos (fuera de tolerancia) → no libera, la oferta expira normal.
- [ ] Pago de más → libera igual.
- [ ] Tolerancia de redondeo ~0.5%, configurable.
- [ ] Ventana de expiración: 45 min desde el claim, configurable.

### 2.5 Persistencia y arranque

- [ ] Ofertas y reservas sobreviven a un reinicio del nodo.
- [ ] **Al arrancar, reconciliar:** toda nota marcada `reserved` sin una oferta viva que la
      respalde tiene que liberarse. Sin esto un reinicio en mal momento deja fondos trabados
      para siempre.
- [ ] Al reiniciar, reanunciar por gossip las ofertas todavía abiertas.

### 2.6 Presencia del vendedor

- [ ] Una oferta solo se propaga y es tomable mientras el nodo del vendedor está online.
- [ ] Al desconectarse: dejar de reanunciar y emitir `OfferStatusUpdate`.
- [ ] La UI del comprador tiene que reflejar que una oferta dejó de estar disponible.

### 2.7 Claim atómico y liberación

- [ ] Al tomar una oferta, marcarla `claimed` atómicamente: dos compradores no pueden tomarla
      a la vez.
- [ ] Liberación **automática** al detectar el pago verificado. No existe botón "liberar" que el
      vendedor pueda no apretar.
- [ ] Registro de incumplimiento: si un vendedor no libera pese al pago verificado, se registra.
- [ ] Denuncia compartida por gossip, firmada por el comprador y **con el hash de la tx USDT como
      prueba**. Sin prueba no se propaga.
- [ ] Advertir al comprador que denunciar expone públicamente su transacción USDT — es el precio
      de que la denuncia sea verificable.

### 2.8 Seguridad y consentimiento — gate obligatorio

No sale de "Coming Soon" sin esto.

- [ ] Pantalla de riesgos con aceptación explícita la primera vez que se entra a la sección
      (mismo patrón que `CustodyWarningScreen` del onboarding).
- [ ] Explicar en esa pantalla, en inglés claro: el lock es de software y no criptográfico; no
      hay árbitro ni reembolso; el vendedor podría incumplir; sin tope de monto.
- [ ] **Riesgo asumido:** v1 sale sin tope de monto por oferta. El peor caso de un vendedor
      malicioso (cliente modificado que gasta notas "reservadas" tras cobrar el USDT) no tiene
      techo. Si aparece el primer abuso real, poner un tope es la mitigación inmediata a mano.
- [ ] **Riesgo asumido:** se prueba directo en mainnet con montos mínimos, sin fase de testnet.
      Mantener los montos realmente bajos hasta que el flujo completo se haya ejecutado varias
      veces sin incidentes.
- [ ] Revisión de seguridad dedicada (código + flujo económico) antes de habilitar para usuarios
      reales. Sin tope de monto y sin testnet, este gate es la única barrera antes de fondos
      reales.

### 2.9 UI

- [ ] Reemplazar `P2PExchangeScreen()`.
- [ ] Publicar oferta: monto ZKA, monto USDT, address TRC-20 de cobro.
- [ ] Listado de ofertas disponibles + tomar oferta.
- [ ] Estado del intercambio: bloqueado → pago detectado → liberado / expirado.
- [ ] Estado de Tor visible (sin Tor no hay verificación).
- [ ] Flag opt-in en Settings + release conjunto.

---

## Etapa 3 — Mining Pool (team mining sin custodia)

**Sin wallet de equipo.** El coinbase rota entre las addresses de los miembros según las shares
aportadas: cada bloque candidato se arma pagando a quien le toca. El coinbase *es* el pago — no
hay tx de reparto ni fondos en manos de un coordinador.

Esto elimina el riesgo de custodia por completo, a cambio de uno distinto: un miembro puede
encontrar un bloque que le paga a otro y reescribirlo para cobrarlo él. Ver §3.4.

### 3.1 Modelo de equipo

- [ ] Estructura: `team_id`, miembros (address de cada uno), coordinador actual, coordinador de
      respaldo.
- [ ] Invitación por código de equipo. Sin infraestructura de identidad nueva.
- [ ] Sin límite duro de tamaño en v1.
- [ ] Confirmar que no se toca el consenso: `validate_coinbase_shape` /
      `validate_coinbase_amount` (`src/state/chain.rs`) siguen exigiendo un receptor único —
      la rotación respeta eso por diseño.

### 3.2 Protocolo de shares

- [ ] Endpoint nuevo para enviar soluciones parciales al coordinador. No hay nada reutilizable
      de `/mining/*` (es solo para minería solitaria).
- [ ] Dificultad de share **automática por minero**, ajustada a su hashrate para un ritmo parejo
      de envío. Un minero débil tiene que poder aportar; uno fuerte no debe saturar.
- [ ] Validar las shares recalculando contra el header candidato: un miembro no puede inflar su
      aporte con shares falsas.

### 3.3 Rotación del coinbase

- [ ] El coordinador arma cada template asignando el receptor según quién acumuló más shares sin
      cobrar todavía.
- [ ] Llevar la cuenta de lo aportado vs. lo efectivamente cobrado por cada miembro, para que la
      rotación converja a proporcional en el tiempo.
- [ ] Al confirmarse un bloque del equipo, descontar lo cobrado del saldo de turnos de ese
      miembro.
- [ ] Comunicar en la UI que el reparto es **estadístico**: a corto plazo es desparejo, converge
      con el tiempo.

### 3.4 Detección de bloques hijackeados

- [ ] Al confirmarse un bloque encontrado por el equipo, verificar que el receptor del coinbase
      sea el que el coordinador había asignado.
- [ ] Si no coincide: marcar al miembro y expulsarlo del equipo.
- [ ] Dejar claro en la UI, al unirse, que esto es detectable pero **no impedible** a nivel
      protocolo — un bloque válido que te paga a vos es válido para toda la red. El modelo asume
      equipos entre conocidos.

### 3.5 Coordinador de respaldo

- [ ] Designar un miembro de respaldo al formar el equipo.
- [ ] Si el coordinador se cae, el respaldo toma el rol automáticamente.
- [ ] Transferir el estado de shares al respaldo para que nadie pierda lo aportado.
- [ ] Sin riesgo de custodia agregado: no hay clave de equipo que compartir, solo el rol de
      coordinación.

### 3.6 UI

- [ ] Reemplazar `MiningPoolScreen()`.
- [ ] Toggle "team mining" (off por defecto).
- [ ] Código de invitación, miembros activos, hashrate combinado.
- [ ] Shares propias vs. del equipo, y turno estimado para cobrar.
- [ ] Reusar el lenguaje visual de las `StatusCard` de la minería solitaria.
- [ ] Flag opt-in en Settings + release conjunto.

---

## Apéndice — referencias técnicas verificadas

Endpoints ya montados en el nodo (`src/api/mod.rs`), reutilizables sin cambios:

```text
GET  /blocks/latest
GET  /blocks/:height
GET  /blocks/:height/raw
GET  /blocks/:height/transactions
GET  /blocks/hash/:hash
GET  /blocks/range/:from/:to
GET  /transactions/:hash
GET  /mempool/pending
GET  /mempool/tx/:hash
GET  /accounts/:address
GET  /accounts/:address/proof
GET  /network/miners
POST /network/miners/heartbeat
```

Piezas a reutilizar (no reinventar):

```text
src/private_wallet/mod.rs
  reserved: bool / reserved_by_tx: Option<String>   (PrivateNote)
  release_reserved_notes(tx_hash)

src/state/chain.rs
  COINBASE_SENDER = "zoka:coinbase:v1"
  validate_coinbase_shape()    — exige exactamente 1 coinbase, 1 receptor
  validate_coinbase_amount()   — exige monto == subsidio esperado
  economics::COINBASE_MATURITY_BLOCKS

src/network/message.rs
  enum ZokaMessage (línea 33) — punto de extensión para OfferAnnounce

src/network/p2p.rs
  gossipsub con MessageAuthenticity::Signed
  tópicos: general / pbft / stem / mixer

zsilent-core/.../core/tor/TorSupervisor.kt
  proxy SOCKS para las consultas a la API de Tron
```

Stack de UI **vivo** en zsilent-core:

```text
Main.kt
  → ui/ZSilentApp.kt          (router, línea 124 = Explorer)
  → ui/theme/Theme.kt         (LightScheme, verde pastel #A8E6B0)
  → ui/screens/Screens.kt     (2943 líneas, incluye los placeholders)
  → ui/screens/ExplorerScreen.kt  (real, 106 líneas, maqueta sin RPC)
  → ui/components/            (ScreenScaffold, Panel, StatusCard, ZCard…)
```

Stack **muerto** a borrar en Etapa 0:

```text
design/                      (Colors.kt, Shapes.kt, Spacing.kt, Typography.kt, ZSilentTheme.kt)
app/ZSilentApp.kt            (112 líneas — router paralelo sin usar)
ui/screens/dashboard/        (incluye un 3er ExplorerScreen de 27 líneas)
ui/theme/Theme.kt            → DarkScheme (paleta lila #C084FC, sin usar)
```

Dependencias disponibles (`build.gradle.kts`):

```text
io.ktor:ktor-client-cio:3.0.1          — HTTP, soporta proxy SOCKS para Tor
kotlinx-serialization-json:1.7.3       — modelos de respuesta RPC y API
kotlinx-coroutines-core:1.9.0
compose.material3 + materialIconsExtended
com.google.zxing                        — QR (ya usado en ReceiveScreen)
```
