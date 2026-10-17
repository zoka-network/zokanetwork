# Hoja de ruta: la red actual pasa a testnet, y mainnet nueva post-cuántica

Creada: 2026-10-11. Actualizada: 2026-10-12. Web y documentos publicados;
base criptográfica experimental integrada al nodo detrás de `pq-experimental`
(etapa 1) y Zenith presentada como testnet en ramas de desarrollo (etapa 2), sin
release. La red todavía no es post-cuántica.

---

## LEEME PRIMERO

**16/10/2026:** etapa 5 implementada como biblioteca en `crates/zoka-quantic`
(Ironwood del crate `orchard` 0.16 sin tocar el circuito, cifrado de notas X-Wing,
txid verificable y bloques compactos para la wallet liviana; diseño en
[ETAPA5-diseno-quantic.md](ETAPA5-diseno-quantic.md)). Etapa 6 en el nodo:
checkpoints y snapshots con firma híbrida Ed25519 + ML-DSA-65. Ninguna de las dos
cambia Zenith. Falta integrar Quantic al nodo y a las apps.

**15/10/2026:** la 2.1.2 activó en Zenith los bundles canónicos (25.570) y las
`zkq1` (25.630). El usuario pidió una wallet móvil que no tarde minutos y que
funcione con cualquier nodo: el plan, con el estado de cada etapa pendiente de este
documento, está en [HOJA-DE-RUTA-wallet-liviana.md](HOJA-DE-RUTA-wallet-liviana.md).

Lo que decidió el usuario el 2026-10-11:

1. **La red actual (Zenith) pasa a ser la testnet.** Nadie pagó dinero por ZKA, así
   que la mainnet nueva arranca de cero: **no hay reclamo ni migración de saldos**.
2. **El P2P Exchange sale entero**: de ZSilent, del nodo y de la documentación, ya
   (no espera a la mainnet).
3. **VPS gratis** (con tarjeta solo para verificar la cuenta) para el explorador y para
   ZKAPriv, **además** de Tor, no en su lugar. **En espera: el usuario avisa cuándo.**
4. **Lo post-cuántico se implementa ahora** y se prueba en la testnet antes de lanzar
   la mainnet.
5. **La mainnet nueva se llama Zoka Quantic.**
6. **Premine de 5.000.000 ZKA para el equipo, gastable desde el bloque 0**, dentro del
   tope de 23.000.000. La minería emite los 18.000.000 restantes: 12 ZKA por bloque
   durante 1.051.200 bloques (12.614.400 ZKA) y 6 ZKA durante 897.600 bloques más.
   Con bloques de 120 s, la emisión termina a los ~7,4 años.
7. **El whitepaper se reescribe ya** con el enfoque nuevo (versión 2.0, Zoka Quantic).

Reglas de trabajo para este plan:

- Nada cambia el consenso de la red actual sin una **altura de activación** explícita.
- Una rama por etapa, un commit por subetapa, compilar y correr las pruebas al cerrar
  cada etapa (acuerdo de trabajo de julio).
- **No exagerar lo post-cuántico** en ningún texto público. Ver "Qué se puede prometer".

---

## Qué protege cada pieza hoy

Relevado en el código el 2026-10-11. Lo que importa es la última columna: si una
computadora cuántica futura puede romperlo **hacia atrás**, con la cadena que cualquiera
puede guardar hoy.

| Pieza | Dónde | Qué hace una computadora cuántica | ¿Hacia atrás? |
|---|---|---|---|
| Direcciones `zka1` (clave de escaneo + clave de gasto, Ristretto) y salidas ocultas | `canonical_payment_address`, `derive_stealth_payload` en `src/private_wallet/mod.rs` | Quien conozca una dirección puede reconocer todas las salidas que recibió | **Sí** |
| Monto cifrado para el destinatario | `encrypt_private_note_payload_v2` (clave derivada del intercambio de claves) | Quien conozca la dirección puede leer los montos | **Sí** |
| Comisión cifrada para el minero | `PrivateFeeClaim` (`ephemeral_pubkey` + `ciphertext`) | Igual que el monto | **Sí** |
| Key images + firma en anillo CLSAG | `RingSignatureBackend::ClsagV1`, `verify_ring_signature_with_backend` | Recalcula la key image de cada salida y ve cuál gastó cada transacción | **Sí** |
| Nullifiers de las notas privadas | `derive_note_nullifier`, `src/private_wallet/mod.rs`: SHA-256 de dominio, `spending_key`, identificador de nota y compromiso | Resolver el logaritmo discreto de la clave pública revela `reduce(SHA256(spending_key))`, no directamente la preimagen `spending_key`. No se ha demostrado recuperación del nullifier por esa vía | No demostrado por esa vía; el grafo ya se filtra por los campos de entrada |
| Compromisos Pedersen + Bulletproofs | `src/zk/range_proofs.rs` | El monto queda oculto igual (ocultamiento perfecto), pero se podrían crear monedas sin que se note | No, solo hacia adelante |
| Groth16 sobre BLS12-381 | `src/zk/groth16_backend.rs` | La prueba no revela nada (conocimiento cero perfecto), pero se podrían falsificar pruebas | No, solo hacia adelante |
| Ed25519 | Firma de snapshots (`src/snapshot.rs`), identidades de red | **No protege fondos.** Se podría falsificar un snapshot de arranque | No |
| Prueba de trabajo (RandomX), SHA-2/3, BLAKE3, AES-256-GCM, Argon2 | — | Resisten | — |
| Transporte (libp2p Noise, iroh, Tor) | — | Tráfico grabado hoy se podría descifrar, pero casi todo lo que viaja ya es público | Poco valor |

Los bloques no llevan firma: el `seal` del encabezado es el hash de la prueba de trabajo
(`src/mining/block_validator.rs`). Las transacciones son todas privadas; el tipo
`Transaction` público queda prácticamente para el coinbase.

**Aclaración sobre los nullifiers (revisión del 2026-10-11):**
`Nullifier::from_commitment` en `src/transaction/nullifier.rs` ofrece una función
BLAKE3, pero no es la derivación usada por los envíos del wallet privado. Estos usan
`derive_note_nullifier`, con SHA-256 y la clave de gasto original. Además,
`proof_nullifier` es otro campo: un escalar de 64 bits construido para la relación
`secret * index + blind` del circuito Groth16 actual. Esa relación no demuestra que
los `input_nullifiers` públicos provengan de las notas consumidas y de su dueño.
La Etapa 5 exige ligar esas propiedades dentro de un circuito nuevo antes de retirar
CLSAG. Ver [revisión técnica de la Etapa 1](REVISION-postcuantica-etapa1.md).

### El principio que ordena todo

**Lo que protege la privacidad tiene que ser post-cuántico desde el génesis**, porque se
rompe hacia atrás: alguien puede guardar la cadena hoy y leerla en quince años.
**Lo que protege la validez** (que nadie cree monedas ni gaste lo ajeno) se rompe solo
hacia adelante, así que se puede migrar con un fork el día que haga falta, siempre que
los formatos indiquen qué esquema usan.

### Corrección de textos públicos (publicada el 11/10)

Whitepaper/litepaper v2 y la web ya explican que la privacidad post-cuántica es
un objetivo en desarrollo. Los documentos v1 conservan su texto histórico con
un aviso inicial fechado que corrige sus promesas sobre el grafo y la privacidad
cuántica. Los cuatro PDF se regeneraron y publicaron. Las direcciones, key images
y campos públicos de Zenith no se volvieron privados por esta corrección editorial.

### Qué se puede prometer al terminar

"Privacidad post-cuántica: direcciones, montos y el grafo de gastos siguen ocultos aunque
mañana exista una computadora cuántica." **No** "a prueba de computación cuántica": la
validez (Groth16, Bulletproofs, Pedersen) sigue siendo clásica hasta que haya un
reemplazo maduro (pruebas basadas en hashes, tipo STARK).

---

## Etapas, en el orden en que se ejecutan

### Etapa 0 — Cerrar la actualización de seguridad en curso

- [x] Tor 0.4.9.13 (Expert Bundle 15.0.24) instalado en ZSilent; guía, script y CI
      actualizados. Respaldo de 0.4.9.12 en `ZSilent Core\backup-tor-0.4.9.12-20261011`.
- [x] Dependencias del nodo (rustls, anyhow, chacha20, spin); 995 + 76 pruebas en
      verde; `zoka.exe` instalado en ZSilent (SHA256 `7DBCF418…`, respaldo
      `zoka.exe.bak-20261011`).
- [x] npm: Zveil y bot sin alertas; sitios de 39 a 16 (las que quedan exigen versión mayor).
- [ ] Motor móvil: `.so` de los dos ABIs, commit, APK (instalar solo cuando el usuario diga).
- [x] Tor 0.4.9.14: llegó en el Expert Bundle 15.0.25 (12/10). Firma y SHA-256
      verificados el 13/10 y fijado en zsilent-core (rama `tor-0.4.9.14`) y en la
      guía. **Falta instalarlo** en la PC con `scripts/instalar-en-pc.ps1`.
- [ ] Push de las ramas `seguridad-octubre-2026` (ZOKA, zkapriv, zokaweb, zokaexplorer,
      zoka-guard-vault) y `tor-0.4.9.13` (zsilent-core).

### Etapa 0b — Sacar el P2P Exchange entero ✅ (2026-10-11)

Pedido del usuario el 2026-10-11: "sacalo todo". **Hecho** en las ramas
`sacar-exchange` de ZOKA (nodo + docs), zsilent-core y zokaweb. Nodo: 879 + 76
pruebas en verde; ZSilent: 59. Lo que quedó resuelto de cada punto:

- Nada del exchange estaba en bloques (las ofertas iban por gossip). Los mensajes
  de red son JSON por nombre de variante: un nodo viejo que mande ofertas da un
  error de decodificación limpio.
- Las notas reservadas por ofertas abiertas se liberan al arrancar
  (`release_retired_exchange_reservations`): se reconocen por no tener una tx
  detrás en el historial de la wallet. **No** por el largo: el hash de una tx
  privada y el id de una oferta tienen los dos 32 hex.
- Los setups viejos de ZSilent siguen cargando (el lector ignora claves
  desconocidas; hay prueba).

Lo que sigue es el plan original, como registro.

0b.1 Relevar antes de borrar: qué toca `src/p2p_exchange/` (~2.900 líneas) en el resto
     del nodo (API, CLI, gossip, notas reservadas de la wallet) y si algo del exchange
     quedó en bloques de la cadena actual. Si hay datos en bloques, la validación de
     esos bloques viejos tiene que seguir funcionando en la testnet.
0b.2 Nodo: módulo, rutas de la API, comandos del CLI, mensajes de red y pruebas.
     Las notas marcadas `reserved` por ofertas viejas tienen que volver a quedar
     gastables.
0b.3 ZSilent: pantallas, navegación y llamadas a la API.
0b.4 Documentación y sitio: todo lo que mencione el exchange o Tron.

### Etapa 0d — Cuenta nueva de GitHub y web con Zoka Quantic (2026-10-11)

Pedido del usuario: pasar todo a `github.com/zoka-network` (cuenta de usuario, creada el
11/09), subir todos los repos y que la web presente a Quantic como red principal.

- Hecho local: referencias a la cuenta suspendida cambiadas en los 6 repos (nombres de
  repo iguales a GitLab); `publish-explorer-data.yml` borrado de `main`; web con Quantic
  como principal (en desarrollo) y Zenith como testnet; litepaper 2.0; PDF de whitepaper y
  litepaper con `zokaweb/scripts/build-papers.py`; descargas apuntan a GitLab.
- **Acceso resuelto:** `gh` autenticado como zoka-network; repos nodo y web existentes,
  Actions desactivado en ambos. El token ofrecido en el portapapeles no fue necesario.
- **Web publicada:** rama `cerrar-web-quantic`, commits `d4e0af6` (papers), `3c03184`
  (home, accesibilidad y enlaces) y `46e9afa` (ZOKA QUANTIC en una línea,
  privacidad más destacada y esfera reducida, según la corrección del usuario).
  Push de la web a `main` y rama en GitHub y GitLab.
  Cloudflare Pages: `https://b9b102ec.zokaweb.pages.dev`,
  producción `https://zoka.network`. Verificados menú, cinco secciones y PDFs en
  Chromium con anchos 1680, 390 y 320. Build/TypeScript/4 tests correctos; lint sin
  errores, 7 avisos heredados de Fast Refresh.
- La sincronización de las ramas de los demás repos sigue siendo una tarea separada;
  no confundir la publicación de la web con la publicación de clientes nuevos.
- Pendiente: las URLs del trusted setup en `docs/*-network.v1.json` siguen apuntando a
  releases de la cuenta vieja (se compilan en el nodo; cambiarlas exige rehospedar los
  archivos y sacar binario nuevo). Los releases con binarios no existen en GitHub nuevo.

### Etapa 0c — Whitepaper 2.0 (Zoka Quantic) — publicado como diseño, 11/10

Revisados whitepaper (12 páginas) y litepaper (4), publicados como diseño de una
red todavía no lanzada. Declaran premine de 5M dentro de 23M, emisión minera de
18M, continuidad de minería por comisiones, falta de migración de saldos y los
problemas del grafo de Zenith. Distinguen implementación, investigación y planes.
Corregidos X-Wing/X25519, view tags, alcance de Groth16 y costos pendientes.
Los v1 (11 y 5 páginas) incluyen aviso histórico; extracción, símbolos, títulos,
aritmética y páginas comprobados. Generador: `zokaweb/scripts/build-papers.py`;
procedimiento: `zokaweb/docs/papers.md`. La revisión final contra el protocolo
implementado sigue en la etapa 7.8.

### Etapa 1 — Base post-cuántica

Implementación experimental en `crates/zoka-pq`, separada del nodo: ML-KEM-768,
X-Wing y ML-DSA-65, parsers estrictos, zeroization y Debug redactado, 13 pruebas
con vectores independientes NIST/X-Wing y casos negativos. CLI `pq-bench` mide
expansión, encapsulado, desencapsulado, firma y verificación por separado.
Revisión: `docs/REVISION-postcuantica-etapa1.md`; resultados por plataforma en
`docs/RESULTADOS-postcuantica-etapa1.md`: check y biblioteca release en cinco
targets; 13/13 tests ejecutados en Windows y Android ARMv7; benchmarks en ambos.
Linux no se ejecutó; Android ARM64 se enlazó pero no se ejecutó. En el teléfono,
ML-KEM desencapsula en ~1,11 ms y X-Wing en ~3,41–3,52 ms (medianas). Falta el
escáner completo para cerrar D1/D2. Clippy y formato correctos.

**Integrada al nodo (11/10, rama `postcuantica-integracion-nodo`):** el choque
de versiones se resolvió subiendo `iroh-base` de 1.0.2 a 1.0.3, que solo acepta
las versiones estables de la curva; `curve25519-dalek` pasa de 5.0.0-rc.0 a
5.0.0 sin cambios activos en el nodo (diff de fuentes revisado). El crate entra
como dependencia opcional detrás de `pq-experimental`, apagada por defecto, y no
cambia el consenso ni el formato de direcciones/bloques de Zenith. No hubo cambio
de wallet ni activación. Resultados por plataforma en
`docs/RESULTADOS-postcuantica-etapa1.md#integración-al-nodo`.

1.1 Base experimental fijada: `ml-kem` 0.3.2, `ml-dsa` 0.1.1 y `x-wing` 0.1.1 de
    RustCrypto, con lockfile independiente y vectores de procedencia registrada.
    No tienen auditoría conocida. Se usa el combinador de la implementación
    especificada; no se introdujo uno propio. Integrada al nodo el 11/10 detrás
    de `pq-experimental` (ver arriba).
1.2 Que compile en los cinco destinos: Windows x86_64, Linux x86_64, **Linux aarch64**
    (para el VPS de Oracle), Android arm64-v8a y **armeabi-v7a** (el teléfono del usuario).
1.3 **Medir en el teléfono de 32 bits** el desencapsulado ML-KEM: el escaneo prueba
    cada salida de la cadena, así que este número decide si hacen falta etiquetas de
    vista para descartar rápido.
1.4 Con esas mediciones, cerrar D1 y D2.

### Etapa 2 — La red actual pasa a ser la testnet ✅ (2026-10-12, sin release)

**Hecha en la rama `zenith-testnet` de nodo, zsilent-core y zkapriv**
(ver [ETAPA2-zenith-testnet.md](ETAPA2-zenith-testnet.md)). Cambio respecto del
plan de abajo: **2.2 no mueve datos.** Las bases guardan la etiqueta `mainnet` en
`chain:genesis:network`, ZSilent y los scripts pasan `--network mainnet`, y en una
instalación real ya existe `.zoka/testnet` de la red retirada. Se cambió lo que se
muestra ("Zenith testnet") y `--network testnet|zenith|mainnet` abren Zenith en
`.zoka/mainnet`. 2.3 quedó como `economics::PqFeature`, sin alturas. 2.5 es un
borrador sin publicar.

Plan original, como registro:

2.1 **El chain id no se puede renombrar.** El hash sintético del génesis sale de
    `MAINNET_CHAIN_ID` (`src/economics.rs`), así que cambiarlo rompe la cadena. Los
    clientes nuevos presentan `zoka-mainnet-zenith-v1` como testnet y retiran
    `zoka-testnet-v1`, que nunca estuvo operativa.
2.2 Directorios de datos (`.zoka/mainnet` → `.zoka/testnet`) en el CLI y en ZSilent,
    **sin perder wallets**: copiar, verificar y recién después cambiar la ruta.
2.3 Marco de activación por altura para las funciones post-cuánticas, como ya existe
    para `PRIVATE_TX_V2_I1_ACTIVATION_HEIGHT`.
2.4 Los clientes viejos van a seguir llamándola "mainnet". No rompe nada; el anuncio lo
    explica.
2.5 Anuncio: los ZKA actuales pasan a ser de prueba y la mainnet nueva arranca de cero.

### Etapa 3 — Direcciones y cifrado híbridos

3.1 Versión nueva de dirección: clave de escaneo + clave de gasto + clave pública
    ML-KEM, con su propio prefijo y checksum.
3.2 Un solo secreto compartido híbrido (intercambio de claves clásico + ML-KEM,
    combinados con un KDF que incluya el ciphertext y las claves públicas) para:
    la clave de un solo uso, la etiqueta de vista, el monto cifrado, la comisión
    cifrada del minero y el coinbase privado (`PrivateCoinbaseOutput`).
3.3 Cada salida lleva el ciphertext ML-KEM (1088 bytes con ML-KEM-768).
3.4 Escaneo en el nodo (ZSilent) y en el motor móvil (ZKAPriv).
3.5 Activación por altura en la testnet y prueba de punta a punta: envío ZSilent →
    ZKAPriv y ZKAPriv → ZSilent, en el teléfono real.

### Etapa 4 — VPS para la testnet (en paralelo con la 3) — EN ESPERA

**No arrancar hasta que el usuario avise** (pedido del 2026-10-11). Él crea las
cuentas; el resto lo armo yo.

4.1 **Oracle Cloud Always Free** (ARM, hasta 4 núcleos y 24 GB entre todas sus
    máquinas): nodo que **no mina**, seed, y generador de los datos del explorador.
    Oracle recupera las máquinas que ve ociosas: vigilarlo.
4.2 **Google Cloud e2-micro** (1 GB, solo regiones de EE.UU.): segundo seed. Medir la
    RAM antes; RandomX en modo liviano usa unos 256 MB.
4.3 Explorador: el nodo del VPS genera el JSON y lo sube a Cloudflare Pages con un
    cron, como `zoka.network`. Se deja de depender de GitHub Actions.
4.4 ZKAPriv: conexión directa a los VPS (iroh/QUIC) para sincronizar rápido, con Tor
    como modo privado. El VPS ve la IP del teléfono, no qué notas son suyas, porque el
    teléfono baja todos los bloques y escanea localmente.
4.5 Reglas: **nunca minar en un VPS gratis** (es lo que más prohíben sus términos y lo
    que terminó con GitHub); dos proveedores distintos; la red tiene que seguir andando
    si los dos desaparecen.

### Etapa 5 — Gastar sin key images

Objetivo: que una computadora cuántica futura no pueda reconstruir qué salida gastó
cada transacción. Es la etapa más grande y **arranca investigando**.

**13/10/2026: construcción elegida → Ironwood**, confirmada por el usuario (el circuito corregido de
Orchard, Halo 2, sin trusted setup), con el cifrado de notas X-Wing de la etapa 3.
Medido en el teléfono de pruebas: 20 s por envío y 40 s para armar la clave al abrir.
Criterio, opciones y fuentes en [ETAPA5-gasto-sin-key-images.md](ETAPA5-gasto-sin-key-images.md).
Reemplaza el plan de abajo de un circuito Groth16 propio (5.3 a 5.5).

**16/10/2026: implementada como biblioteca** (`crates/zoka-quantic`, rama
`etapa5-ironwood`): claves ZIP 32 + X-Wing, direcciones `zq1` con F4Jumble,
sobre X-Wing en lugar del cifrado de Orchard, formato binario y txid estilo
ZIP 244, armado por los roles PCZT, consenso de referencia (emisión exacta de
23M, premine público verificable, maduración del coinbase por inserción
diferida en el árbol, anclas, nullifiers) y wallet de referencia, también sobre
bloques compactos. Envío de 2 acciones: 11.351 bytes. 31 pruebas unitarias y 5
de punta a punta con pruebas Halo 2 reales. Diseño:
[ETAPA5-diseno-quantic.md](ETAPA5-diseno-quantic.md). Falta: perfil Quantic en
el nodo, motor del teléfono y ZSilent, medir en el teléfono, revisión externa.

**16 y 17/10/2026, en el nodo** (rama `etapa5-quantic-nodo`): perfil Quantic
(bloques, mempool, minería, P2P, API, `zoka quantic-wallet`) probado en una red
local de tres nodos con reorganización ([perfil](ETAPA5-perfil-quantic-nodo.md));
dificultad ASERT (7.2); protocolo P2P de wallets livianas
`/zoka-quantic/light/1` (info, bloques compactos y envíos, probado entre nodos
reales); estado guardable de la wallet en la biblioteca, y motor del teléfono
(`src/mobile_quantic.rs` + `mobile_net`) probado sobre una cadena regtest real.
Puente JNI en ZKAPriv, rama `quantic-motor` (feature `quantic`, sin tocar la
interfaz). API del motor: [QUANTIC-MOTOR-MOVIL.md](QUANTIC-MOTOR-MOVIL.md).
Falta: la interfaz de ZKAPriv y ZSilent (decisión del usuario), medir en el
teléfono el escaneo con X-Wing, revisión externa.

**Hallazgo del 2026-10-11, más grave que lo cuántico:** hoy **no hace falta una
computadora cuántica** para saber qué nota gasta cada transacción. `PrivateTransaction`
publica `input_commitments` ("Commitments of consumed notes") e `input_public_keys`
("One-time public keys of consumed notes"), y al armar un envío se llenan con el
compromiso y la clave de un solo uso de **las notas reales** (`selected_notes`, en
`src/private_wallet/mod.rs`). No hay ninguna serialización que los borre. Como cada
salida publica esos mismos dos valores, cualquiera puede unir cada gasto con la salida
que consume: el anillo CLSAG lista señuelos, pero la nota real va al lado en claro.
Los montos (Pedersen) y los destinatarios (direcciones ocultas) siguen ocultos; el
**grafo de transacciones**, no.

**Confirmado el 2026-10-11 sobre la cadena real** (copia de la base, 25.204 bloques):
las 254 tx privadas publican sus 834 notas gastadas (clave y compromiso); solo 254 de
esas claves están dentro del anillo, una por tx, y las demás ni siquiera tienen
señuelos; 559 se unen directo con la salida donde nacieron (ejemplo: la tx del bloque
297 gasta una nota nacida en el coinbase del bloque 18).

Consecuencia para el diseño de Zoka Quantic: la pertenencia de la nota gastada al
árbol de compromisos se prueba **dentro** del circuito, y la transacción publica solo
el nullifier y la raíz del árbol, nunca el compromiso ni la clave de la nota gastada.

5.1 Verificar si el circuito Groth16 actual ya prueba, por sí solo, la propiedad de la
    nota y que el nullifier está bien calculado, sin depender de la firma en anillo.
5.2 Clave de nullifier que **no** se pueda deducir de las claves públicas de la
    dirección (derivada del seed con una función pseudoaleatoria, no un punto de curva).
5.3 Mainnet nueva: Groth16 obligatorio en cada gasto, sin key images ni CLSAG.
5.4 Costo: una prueba en cada envío (unos 14 s en el teléfono con el perfil v1).
5.5 Cambia los circuitos, así que la ceremonia de la Etapa 7 tiene que ser posterior.

### Etapa 6 — Firmas híbridas de snapshots y checkpoints

**16/10/2026, rama `etapa6-firmas-hibridas`:** firma híbrida con las dos mitades
obligatorias (`src/hybrid_signature.rs`). Checkpoints de la autoridad versión 2
(ver [CHECKPOINTS-AUTORITATIVOS.md](CHECKPOINTS-AUTORITATIVOS.md)) y snapshots
con firma híbrida (`snapshot-export --mldsa-key-file`; un firmante híbrido en
`snapshot_signers` hace que solo valgan snapshots híbridos). **En Zenith no cambia
nada** hasta que el usuario cree la mitad ML-DSA de la clave de la autoridad
(`zoka checkpoint-keygen --mldsa`) y salga un binario con su pública compilada.

6.1 `src/snapshot.rs`: Ed25519 + ML-DSA-65, las dos obligatorias. ✅
6.1b Checkpoints de la autoridad con el mismo esquema. ✅
6.2 Opcional: firmar los releases con el mismo esquema.

### Etapa 7 — Lo que hay que cerrar antes del génesis

Después del génesis cualquier cambio de estos exige un fork.

7.1 libp2p a la versión mayor nueva (arrastra las alertas sin arreglo: hickory-proto,
    lru 0.12, core2, instant).
7.2 Emisión: los bloques salen demasiado rápido (dificultad trabada en el piso,
    ~2 bloques por minuto). Fijar tiempo de bloque y dificultad inicial.
    **16/10/2026, hecho para Quantic:** objetivo de 256 bits con ASERT (τ = 1 h),
    trabajo real para elegir la cadena, reglas de hora y 120 s por bloque. La
    revisión encontró que Zenith elige la cadena por la suma de bits (no por el
    trabajo) y que su prueba de trabajo no cubre el cuerpo del bloque: ver
    [ETAPA7-dificultad-quantic.md](ETAPA7-dificultad-quantic.md).
7.3 Premine en el génesis: 5.000.000 ZKA a una dirección del equipo, contado dentro
    del suministro emitido para que `block_reward_for_height` corte la minería en
    18.000.000. Monto y dirección públicos en el génesis, para que cualquiera lo
    verifique.
7.4 Alinear versiones: crate, `NETWORK_MIN_CLIENT_VERSION` y manifiesto no coinciden.
7.5 Bug de ZSilent que ofrece notas ya gastadas (saldo inflado).
7.6 **Ceremonia multiparte del trusted setup** con los circuitos finales. Si los
    parámetros los genera una sola persona, esa persona puede crear monedas.
7.7 Revisión externa, al menos de las etapas 3 y 5.
7.8 Revisar el whitepaper 2.0 contra lo que de verdad quedó implementado.

### Etapa 8 — Génesis y lanzamiento

8.1 Zoka Quantic: chain id nuevo (propuesta: `zoka-quantic-v1`) y génesis nuevo.
8.2 Todo lo post-cuántico activo desde el bloque 0, sin alturas de activación.
8.3 Seeds: VPS + onion + la PC del usuario.
8.4 Anuncio que explique por qué este relanzamiento es distinto del de Zenith (junio
    2026) y el compromiso de que no habrá más reinicios.

---

## Decisiones abiertas

| # | Decisión | Recomendación |
|---|---|---|
| D1 | ML-KEM-768 o ML-KEM-512 | 768: es la elección estándar (la usa TLS); 512 ahorra unos 400 bytes por dirección y 300 por salida |
| D2 | Largo de las direcciones: con 768 rondan los 1.700 caracteres (1.200 con 512) | Aceptarlo y apoyarse en QR y copiar/pegar; las alternativas exigen un directorio de claves, que filtra privacidad |
| D5 | Tiempo de bloque y dificultad inicial (la curva ya está decidida, ver LEEME) | Revisar con números en la Etapa 7 |

Decididas el 2026-10-11: nombre **Zoka Quantic**; premine de **5M para el equipo,
gastable de inmediato, dentro de los 23M**; el exchange **sale entero ya**; los VPS
**esperan** a que el usuario avise.

## Riesgos

- Es el **segundo relanzamiento** en el año: si no se explica bien, espanta a los que quedan.
- **Premine del 21,7 % sin bloqueo:** el whitepaper 1.0 prometía "sin premine". Hay que
  declararlo de frente (monto, dirección y que es gastable de inmediato); escondido o
  minimizado es lo que más rápido destruye la confianza en una moneda nueva.
- Librerías post-cuánticas **jóvenes y sin auditoría** conocida.
- **Tamaño:** cada salida suma más de 1 KB; pesa en la cadena y en los datos del teléfono.
- **Escaneo más lento** en el teléfono de 32 bits (se mide en la Etapa 1).
- Los VPS gratis pueden suspenderse sin aviso (precedente: GitHub, agosto 2026).
- La validez sigue siendo clásica: ver "Qué se puede prometer".
