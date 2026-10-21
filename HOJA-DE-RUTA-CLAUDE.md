# Zoka Quantic: hoja de ruta para continuar con Claude

Actualizada: **17 de octubre de 2026**, Argentina (Claude).
Proyecto: `C:\Users\mderramus\ZOKA`.

## Punto de partida

**21/10: decisiones del usuario.** Prioridad: terminar la cadena Zoka Quantic
completa (arquitectura) para lanzar testnet y mainnet, y después ZSilent; ZKAPriv
queda para más adelante.

- **ZSilent pasa a ser solo de Quantic** (reemplaza a Zenith; Zenith queda en las
  versiones anteriores).
- **Parámetros aprobados:** prefijos `zq1` / `zqtest1` / `zqregtest1`, memo de 512
  bytes, comisión de 0,0001 ZKA por acción con mínimo de 2 acciones.
- **La mainnet sale solo cuando todo sea postcuántico** (elección "Todo
  postcuántico antes"). Hoy Quantic es postcuántica en privacidad, no en el
  gasto: RedPallas, Halo 2 y los compromisos de Orchard caen con Shor. La testnet
  puede salir antes con Orchard. Inventario y atacante simulado:
  [QUANTIC-INVENTARIO-CRIPTOGRAFICO.md](docs/QUANTIC-INVENTARIO-CRIPTOGRAFICO.md);
  pool nuevo (STARK con Plonky3 + Poseidon2, a prototipar y medir, también en el
  teléfono de 32 bits): [QUANTIC-POOL-POSTCUANTICO.md](docs/QUANTIC-POOL-POSTCUANTICO.md).
- **Rama `quantic-cadena-completa`:** perfil de mainnet, autoridad híbrida
  obligatoria, nodo con pila grande, y el minero de Quantic con control en caliente
  para ZSilent (`--no-mine` + `/mining/start|stop`, `/quantic/mining`, `/stats`,
  `mine-status` con datos de Quantic).

**16 y 17/10: desarrollo post-cuántico en modo autónomo** (pedido del usuario:
"enfocate en el desarrollo post quantic"; "continúa ... cuando no sepas qué hacer
seguí el roadmap"). **Nada está pusheado ni publicado ni instalado.** Ninguna
red cambió: todo es código en ramas, probado en local.

- **Nodo, rama `etapa5-quantic-nodo`** (apilada sobre `etapa6-firmas-hibridas` y
  `etapa5-ironwood`): perfil Zoka Quantic completo detrás de la feature `quantic`
  (bloques, mempool, minería, P2P, API `/quantic/*`, `zoka quantic-wallet`),
  probado en una red local de tres nodos con reorganización
  ([perfil](docs/ETAPA5-perfil-quantic-nodo.md)); dificultad ASERT con trabajo
  real (7.2, [doc](docs/ETAPA7-dificultad-quantic.md)); protocolo P2P de wallets
  livianas `/zoka-quantic/light/1` con presupuesto por par; estado guardable de la
  wallet (biblioteca); motor del teléfono `src/mobile_quantic.rs` con red Quantic
  en `mobile_net` ([API](docs/QUANTIC-MOTOR-MOVIL.md)); especificación en inglés
  para la revisión externa ([7.7](docs/REVIEW-zoka-quantic.md)).
  Pruebas: `cargo test` (921+85+2) y `cargo test --features quantic`
  (962+86+2+3) en verde con la ASERT; después, las de red y de Quantic de cada
  cambio. La biblioteca: 34 unitarias y 7 de punta a punta con Halo 2 real.
- **ZKAPriv, rama `quantic-motor`:** puente JNI del motor (`native/src/quantic.rs`,
  feature `quantic`), compila para ARMv7. No toca Kotlin. Los `.so` de `jniLibs`
  modificados sin commitear **no son de este trabajo** (venían de antes): no
  pisarlos.
- **Web, rama `whitepaper-ironwood` de `zokaweb`:** whitepaper y litepaper 2.0 y
  `SectionOverlay.tsx` reescritos con lo implementado (Ironwood, X-Wing, sin
  ceremonia, comisiones públicas, checkpoints del equipo declarados). PDFs
  regenerados y revisados. **Sin publicar: lo revisa el usuario.**
- **libp2p 0.53 → 0.57.1 (7.1), hecho:** rama `libp2p-057` (encima de
  `etapa5-quantic-nodo`; worktree `c:\tmp\zoka-libp2p`). `cargo audit` pasó de 16
  vulnerabilidades a 0. Todas las pruebas en verde. Interoperabilidad probada en
  una red Quantic regtest local: un nodo 0.53 y uno 0.57 se sincronizan en los dos
  sentidos y un envío que entra por el 0.57 lo mina el 0.53.
- **Arreglo que afecta a Zenith** (commit `53bdcfc5`): un nodo completo cuyo único
  par es un teléfono dejaba de minar mientras le pedía bloques que el teléfono no
  tiene (más de un minuto por vez). Ahora el teléfono contesta al toque y el nodo
  no insiste. Lo encontró `examples/probar_motor_quantic`, que prueba el motor del
  teléfono de punta a punta contra un nodo real (41 s); también encontró que la
  prueba de Halo 2 desbordaba una pila de 1 MB (ahora corre en un hilo propio).

**Para contarle al usuario (decisiones suyas):**

1. Zenith tiene dos debilidades de consenso que Quantic no hereda: su prueba de
   trabajo no cubre el cuerpo del bloque, y elige la cadena por la suma de los
   bits y no por el trabajo. Los checkpoints de la PC las mitigan. ¿Se corrigen
   con un fork o queda como testnet protegida? Opciones y recomendación en
   [ZENITH-debilidades-consenso.md](docs/ZENITH-debilidades-consenso.md).
2. Revisar y aprobar la web (`whitepaper-ironwood`) antes de publicarla; declara
   los checkpoints del equipo y que las comisiones son públicas.
3. Prefijos de dirección (`zq1`, `zqtest1`, `zqregtest1`), tamaño del memo (512)
   y comisión (0,0001 ZKA por acción).
4. Interfaz de Quantic en ZKAPriv y ZSilent (el motor está; las pantallas no).
   Opciones y recomendación en
   [PROPUESTA-quantic-en-las-apps.md](docs/PROPUESTA-quantic-en-las-apps.md).
5. Pasos suyos para abrir la testnet de Quantic: crear la wallet del equipo
   (`zoka quantic-wallet new --network quantic-testnet --name equipo`) y pasar la
   dirección; crear la mitad ML-DSA de la clave de la autoridad
   (`zoka checkpoint-keygen --mldsa`).
6. Pushear las ramas a GitHub y GitLab cuando lo apruebe.
7. Una release de testnet con el arreglo del teléfono que frena al nodo y con
   libp2p 0.57: binario nuevo en la PC y `.so` nuevo en ZKAPriv.

**15/10: primera prueba real de `zkq1`** (el 25.630 se minó el 14/10 a las 23:25,
sin errores).

- **ZSilent → teléfono: OK.** Tx `81cede9a…42fc`, dos salidas v2 (`output_version`
  2, `kem_ciphertext` de 1.120 bytes), confirmada en el bloque 25.780; ZKAPriv la
  encontró en su revisión siguiente (100 ZKA, el monto que puso el usuario).
- **Teléfono → ZSilent: falló dos veces antes de armar la prueba**, con "el
  sondeo de altura … fue inconcluso … Timeout while waiting for a response". El
  teléfono llega al nodo solo por Tor (semillas `.onion` fijas en
  `Constants.ZOKA_P2P_SEEDS`), abre una conexión por consulta (72 en una hora) y
  parte de los pedidos quedan sin respuesta: a las 08:43 abandonó un lote que el
  nodo sirvió recién entre las 08:44 y las 08:45. Coincide con la revisión de
  fondo, que repite 12 cuerpos que nadie tiene. Pendiente: reintentar; si sigue,
  reproducirlo con `probar_cliente_movil` y arreglarlo en ZKAPriv (reintentar el
  sondeo, no competir con la revisión de fondo).
- **Los QR de las `zkq1` son ilegibles** en las dos apps (1.715 caracteres): hay
  que rediseñarlos.
- Sin QR, las direcciones se pasaron por adb: la del teléfono, con "Copy" y
  "Paste" en Enviar y leyendo el campo con `uiautomator dump` (ZKAPriv bloquea
  las capturas); la de ZSilent, escrita con `input text` en tramos de 40 con
  pausas (en tramos largos el teclado pierde caracteres) y verificada contra el
  original.
- No se leyeron las claves que el usuario dejó en el portapapeles: se pisaron con
  la `zkq1` del teléfono.

**14/10, 22:43: el bloque 25.570 (bundles canónicos) se minó a las 18:28 sin
errores en el log; la punta iba en 25.626, a 18 bits y ~6 min por bloque, así
que el 25.630 (`zkq1`) llegaba esa misma noche.** Checkpoint firmado en 25.608.
**Para el 15/10, a pedido del usuario:** confirmar que pasó el 25.630 y hacer los
envíos reales a `zkq1` ZSilent → teléfono y teléfono → ZSilent (las dos apps
abiertas).

**14/10, más tarde: ZSilent 2.1.3, arreglos del explorador** (rama
`explorador-interno` de zsilent-core; el nodo sigue en 2.1.2, mismo `zoka.exe`).
Publicada con aprobación del usuario (release `v2.1.3` de zsilent-core, MSI
`3ce744b0…` verificado por dentro y bajado de GitHub; `main` al día en GitHub y
GitLab) e instalada en la PC (respaldo `ZSilent Core.respaldo-20261014-v2.1.2`).
El usuario dudaba de que la altura fuera la correcta: lo era (checkpoint firmado
en 25.458 con el mismo hash que la cadena local; suministro 25.465 × 12 ZKA =
305.580, lo que mostraba la pantalla; el "1 peer" era el teléfono con ZKAPriv).
Lo que fallaba: el panel listaba alturas calculadas sin datos, y los botones
"Explorer ↗" abrían `zokascan.cc`, muerto desde julio (sus rutas `/rpc` y `/rpc2`
devuelven la página, sin datos, y dice "v1.6.1"). Ahora: últimos 8 bloques reales
que se abren con un clic, búsqueda de transacciones privadas con su bloque
(`GET /private/tx/:hash` da `status` y `block_height`) y los botones abren el
explorador interno. `prepare-release.ps1` deja que la app vaya adelante del nodo
solo en el último número. **Ojo:** la API del nodo corta a 1.200 lecturas por
minuto por cliente y bloquea hasta 64 s a quien se pasa (ZSilent comparte el
cliente 127.0.0.1): no recorrer la cadena bloque por bloque desde la PC.

**14/10: SALIÓ LA 2.1.2, alturas adelantadas a pedido del usuario: bundles
canónicos desde 25.570 y v2 / `zkq1` desde 25.630** (antes 30.284 y 31.004: once
días de minado con la PC sola a 17 bits). Cliente mínimo 2.1.2: la 2.1.0 y la 2.1.1
quedan fuera de la cadena desde 25.570 y sus releases avisan "Superseded".
Publicada en `github.com/zoka-network` (`v2.1.2` de nodo, ZSilent y ZKAPriv; `main`
de los tres en esa versión) y espejada en GitLab:

- Nodo (`9b129cd7`, `pq-experimental`): `zoka.exe` `74c95b7d…`, `zokahd.exe`
  `0914c854…`, zip `f3398397…`.
- ZSilent: MSI `7ce4451f…` (verificado por dentro: mismo `zoka.exe`, Tor 0.4.9.14
  `8c38513b…`). ZKAPriv: versionCode 15, APK `502c69bd…`, mismo certificado
  `061376f8…`. Lo publicado se bajó de GitHub y coincide byte a byte.
- **Instalado:** PC con ZSilent 2.1.2 (respaldo
  `ZSilent Core.respaldo-20261014-v2.1.1`; lo instaló Claude con el OK explícito
  del usuario) y teléfono con ZKAPriv 2.1.2 (`adb install -r`, conserva la wallet).
- Ese día, a pedido del usuario y fuera de Zoka: se desactivaron las cuatro tareas
  "BOTX …" del bot de trading, que prendían WSL al iniciar sesión (para Zoka no
  hace falta WSL), y se corrió `Desktop\optimizar-windows.ps1` como administrador:
  telemetría, mapas, fax, informe de errores, impresora, ubicación e indexado de
  búsqueda deshabilitados; el estado anterior quedó en
  `Desktop\optimizar-windows-antes.json` y se vuelve atrás con `-Revertir`.
- **Falta:** que la red llegue a 25.570 y 25.630 con la PC minando, ver que la
  autoridad siga firmando checkpoints, los envíos `zkq1` reales entre ZSilent y
  ZKAPriv y el anuncio (texto de la 2.1.2 en
  [docs/ANUNCIO-zenith-testnet-BORRADOR.md](docs/ANUNCIO-zenith-testnet-BORRADOR.md)).

**Después, el mismo 13/10: 2.1.1** (nodo y ZSilent, `main` de los tres repos ya en
la versión publicada). La autoridad de checkpoints mina aunque no tenga pares: Zenith
estaba quieta porque ninguna otra semilla respondía y ZSilent pide
`--min-mine-peers 1`. `zoka.exe` `6c6d8722…`, MSI `e86b1d03…`; instalada en la PC
(respaldo `ZSilent Core.respaldo-20261013-v2.1.0`). A las 21:58 la red volvió a
avanzar (25.245, 25.247…), dificultad 17 bits, unos 720 H/s: al 30.284 faltan entre
4 y 7 días de minado continuo.

**13/10/2026, a pedido del usuario: SALIÓ LA 2.1.0 DE TESTNET.** Publicada en
`github.com/zoka-network` (y espejada en GitLab), probada antes en una red local:

- Nodo `v2.1.0` (rama `release-2.1.0`, código de `b542a83f`, `pq-experimental`):
  `zoka.exe` `0ac64cfb…`, `zokahd.exe` `6ec21042…`. Fork en Zenith: **bundles
  canónicos desde 30.284 y salidas v2 / `zkq1` desde 31.004** (punta del día:
  25.244, la red estaba quieta). `PROTOCOL_FREEZE_ID` sin cambios.
- ZSilent `v2.1.0`: MSI `a8d3814b…` (verificado por dentro: mismo `zoka.exe`, Tor
  0.4.9.14). ZKAPriv `v2.1.0` (la `v1.1.0` ya existía de 2025): APK `e9176c93…`,
  mismo certificado `061376f8…`. Release `trusted-setup-mainnet-v1` con el `DIGEST`.
- **Instalado:** PC con ZSilent 2.1.0 (respaldo entero en
  `%LOCALAPPDATA%\ZSilent Core.respaldo-20261013-v2.0.2`) y teléfono con ZKAPriv
  2.1.0 (abre y carga el motor). **Falta que el usuario abra ZSilent y desbloquee**
  para que el nodo 2.1.0 corra y siga firmando checkpoints.
- **Falta:** el anuncio (texto listo en
  [docs/ANUNCIO-zenith-testnet-BORRADOR.md](docs/ANUNCIO-zenith-testnet-BORRADOR.md),
  lo publica el usuario), alguien que mine para llegar a 30.284, binario Linux
  (WSL no arrancó) y, después de 31.004, los envíos reales `zkq1` entre las apps.
  `main` sigue sin tocar: la versión está en las ramas `release-2.1.0`.

**La web y los papers están publicados. La base criptográfica experimental está
integrada al nodo detrás de la feature `pq-experimental`, apagada por defecto
(rama `postcuantica-integracion-nodo`), y Zenith se presenta como testnet en la
rama `zenith-testnet` de nodo, ZSilent y ZKAPriv, sin mover datos. Nada de eso
está publicado como release. La red actual no tiene activada privacidad
postcuántica.**

**12/10, a pedido del usuario:** `fullnode --sync-only` arreglado (rama
`arreglo-sync-only`); ZSilent, ZKAPriv y el nodo se presentan como **"Zoka
Quantic · Testnet"** y ZSilent tomó el estilo de zoka.network (ramas
`quantic-interfaz`); **etapa 3 implementada en el nodo y probada en una red
local, sin activar** (rama `etapa3-direcciones-hibridas`): consenso de la
salida v2 en todas las builds; envío, recepción, gasto y CLI de `zkq1` con
`pq-experimental`; la v2 exige también los bundles canónicos. Ver la etapa 3.

**13/10: la etapa 3 quedó cerrada en código, sin activar.** Motor móvil con
`pq-experimental` en los dos ABIs (+172 KB / +104 KB), ZKAPriv y ZSilent reconocen,
muestran y envían a la `zkq1` **solo cuando la red fija la altura** (hoy nula: no se
ve nada), y la comisión del minero viaja en un sobre v2 que **no cambia el
consenso**. Ramas `etapa3-apps-zkq1` (ZKAPriv `3ee8956`, `f2f8953`; ZSilent
`a55423f`) y `etapa3-direcciones-hibridas` (nodo). Nada instalado ni publicado.

**13/10, más tarde: el usuario delegó sus decisiones ("tomalas tú, menos la
VPS")** y pidió que **su PC sea el nodo autoritario** de Zenith y de Quantic.

- Decisiones tomadas y su criterio: [docs/DECISIONES-2026-10-13.md](docs/DECISIONES-2026-10-13.md).
  El ciphertext queda como arreglo JSON; los bundles canónicos y la v2 se activan
  en Zenith con la 2.1.0 (punta + una semana; la v2, un día después); las pruebas
  reales de `zkq1` entre apps van después de activar.
- **Nodo autoritario:** rama `checkpoints-autoritativos` (`9bcb6ad9`, sobre
  `etapa3-direcciones-hibridas`). Diseño en
  [docs/CHECKPOINTS-AUTORITATIVOS.md](docs/CHECKPOINTS-AUTORITATIVOS.md). La clave de
  Zenith está en la PC (`%APPDATA%\Zoka\.zoka\mainnet\checkpoint-authority.key`,
  pública `bf5c9215…7bd4`, compilada). **Es la única copia: el usuario tiene que
  respaldarla fuera de la PC.** Los demás nodos obedecen recién con un binario que
  la traiga (2.1.0). Suite: 904 + 81 + 1, sin fallos.
- VPS: paquete listo en `deploy/vps/` (`ed304d43`); no se contrata ni se arranca
  nada hasta que el usuario consiga los servidores.

**13/10, tarde: el usuario dijo "sigue tú solo".** Hecho, en ramas nuevas apiladas
sobre `checkpoints-autoritativos`:

- **Notas gastadas de ZSilent** (rama `arreglo-notas-gastadas`, `88e59f4a`): causa
  más probable hallada leyendo el código, no reproducida en vivo. Varios caminos
  guardaban una copia de wallets leída sin el candado y borraban la reserva de un
  envío hecho en el medio; el caso frecuente era el cierre de cada tanda de minado.
  Ahora se lee con el candado tomado (`with_fresh_private_wallets`). Falta verlo en
  ZSilent minando con el binario nuevo.
- **Checkpoints** (rama `checkpoints-correccion-manual`, `f94b624f`):
  `checkpoint-sign` para corregir una rama a mano, y el firmador no firma mientras
  su cadena no contenga el último checkpoint (si no, devolvía la red a la rama
  corregida).
- **Versiones:** `NETWORK_MIN_CLIENT_VERSION` = manifiesto (2.0.1); la prueba
  heredada de la 1.9.0 ahora comprueba coherencia y la suite pasa sin saltear nada.
- **Tor 0.4.9.14** (bundle 15.0.25 del 12/10): firma y SHA-256 verificados, fijado
  en zsilent-core rama `tor-0.4.9.14` (`9096d40`) y en
  `docs/zsilent-managed-tor-onion-seed.md`.
- **2.1.0:** procedimiento en [docs/RELEASE-2.1.0-testnet.md](docs/RELEASE-2.1.0-testnet.md)
  (el `PROTOCOL_FREEZE_ID` no cambia; las alturas se fijan el día de publicar) y
  sección 2.1.0 en el borrador de anuncio.
- **Etapa de gasto sin key images** (rama `etapa5-investigacion`): construcción
  elegida, **confirmada por el usuario**, **Ironwood** (el circuito corregido de Orchard sobre Halo 2,
  sin trusted setup), con el cifrado de notas X-Wing. Medido en el teléfono: 20 s
  por envío, 40 s para armar la clave al abrir, 111 MB. Ver
  [docs/ETAPA5-gasto-sin-key-images.md](docs/ETAPA5-gasto-sin-key-images.md).
- **Instalar en la PC:** el clasificador de permisos frenó dos veces reemplazar el
  `zoka.exe` instalado. Queda `scripts/instalar-en-pc.ps1`, que corre el usuario
  con ZSilent cerrado: instala el nodo de `target/release` y Tor 0.4.9.14, con
  respaldos y verificación de hashes y firma.

**Siguiente paso natural:** que el usuario corra `scripts/instalar-en-pc.ps1` y
respalde la clave de autoridad; después la 2.1.0 según su procedimiento (publicar
y anunciar, solo con su aprobación). Luego la etapa 4 (gastar sin key images), que
es diseño.

Las ramas del nodo están apiladas, cada una sobre la anterior:
`postcuantica-integracion-nodo` → `zenith-testnet` → `arreglo-sync-only` →
`quantic-interfaz` → `etapa3-direcciones-hibridas`. La última contiene todo.
En ZSilent y ZKAPriv: `zenith-testnet` → `quantic-interfaz`.

### Instalado en los equipos del usuario (12/10, con su autorización)

- **PC:** ZSilent desde `quantic-interfaz` (`21dac3e`) con `zoka.exe`/`zokahd.exe`
  release de `etapa3-direcciones-hibridas` (`38534d0c`, features por defecto, sin
  `pq-experimental`; `zoka.exe` SHA-256 `109B94FB…`), en
  `%LOCALAPPDATA%\ZSilent Core`. Copia completa de lo anterior en
  `%LOCALAPPDATA%\ZSilent Core.respaldo-20261012`; el `zoka.exe` previo también en
  `app\resources\bin\zoka.exe.bak-20261012`. Nota con los detalles:
  `INSTALADO-20261012.txt` en la carpeta de instalación. No se tocó `%APPDATA%\Zoka`;
  el binario nuevo abre la base real (altura 25.204 al instalar).
- **Teléfono (SM-A135M):** ZKAPriv 1.0.1 release (`com.zoka.zkapriv`, certificado
  `061376f8…`) desde `quantic-interfaz` (`8b60551`), con los motores recompilados
  contra `38534d0c`. Era la primera instalación (no había ninguna). Abre y muestra
  "Zoka Quantic · Testnet". La app usa `FLAG_SECURE`: no admite capturas de
  pantalla, a propósito.
- La versión declarada sigue siendo 2.0.2 (ZSilent y nodo) y 1.0.1 (ZKAPriv): no se
  cambió el número; alinear versiones es la etapa 7.4.
- **13/10, 14:08, el usuario corrió `scripts/instalar-en-pc.ps1`:** `zoka.exe`
  `1797905D…` y `zokahd.exe` `F50C4E41…` (código de `e142e349`, features por defecto;
  respaldos `*.bak-20261013`) y Tor 0.4.9.14 (respaldo
  `ZSilent Core\backup-tor-0.4.9.13-20261013`). Con ZSilent abierto el nodo firmó su
  primer checkpoint real: altura 25.238 (`a60526f5…`), la punta menos 6. **La red está
  quieta en 25.244 desde el 12/10: nadie mina.**
- **Respaldo completo de `%APPDATA%\Zoka` en `D:\Respaldo-Zoka-20261013`** (pedido
  del usuario; 2.505 archivos, verificado por tamaño y por hash de la clave de
  autoridad y de `iroh_key`). Incluye la clave de autoridad, las claves de la onion de
  la seed (`tor\zoka-p2p`), `iroh_key` y las wallets cifradas. Es un disco interno:
  sigue faltando una copia de la clave en un pendrive desconectado.

Segunda sesión, 11 y 12/10 (Claude): se cerró la integración de la etapa 1 y se
hizo la etapa 2 (Zenith como testnet) en ramas de los tres repos. El detalle de
lo hecho, lo probado y lo que falta está en cada etapa de abajo.

Este archivo es el punto de entrada del traspaso. El plan detallado y su historial
están en [docs/HOJA-DE-RUTA-mainnet-postcuantica.md](docs/HOJA-DE-RUTA-mainnet-postcuantica.md).
Ese documento conserva propuestas antiguas: no interpretar afirmaciones o diseños
propuestos como propiedades ya demostradas. Contrastar con el código, los resultados
de pruebas y las decisiones más recientes del usuario.

## Decisiones del usuario que hay que conservar

- La red actual, **Zenith**, pasa a ser la testnet. **Zoka Quantic** será una red nueva.
- Quantic arranca sin reclamo ni migración de saldos de Zenith.
- Premine del equipo: **5.000.000 ZKA**, gastable desde el génesis, dentro del máximo
  de **23.000.000 ZKA**. La minería emite los **18.000.000** restantes.
- Emisión acordada: 12 ZKA durante 1.051.200 bloques y 6 ZKA durante otros 897.600.
  Después termina el subsidio; la minería puede continuar con comisiones.
- El P2P Exchange se retira del nodo, ZSilent y la documentación. Preservar ese trabajo.
- Los cambios postcuánticos se implementan y prueban primero en la testnet.
- **VPS en espera:** el usuario avisará cuándo continuar con cuentas e infraestructura.
- Instalar en sus equipos solo con su indicación. **12/10: lo autorizó e instalé**
  (ver "Instalado en los equipos del usuario").
- Home: **ZOKA QUANTIC en una sola línea**, **Private by design. más destacado debajo**,
  esfera pequeña, fondo negro y acentos rosas. Esta fue la corrección final del usuario.
- **12/10:** ZSilent "ya no es más Zenith, pasa a ser Quantic": las dos apps muestran
  **"Zoka Quantic · Testnet"** (la mainnet Quantic todavía no existe) y ZSilent copia
  el estilo de la web (acento rosa `#F2ABD0`, ZOKA **QUANTIC**, "Private by design.").
- **12/10, etapa 3:** **D1 = ML-KEM-768 vía X-Wing** y **D2 = direcciones largas**
  (unos 1.715 caracteres, con QR y copiar/pegar).

## Lo terminado y dónde está

| Entregable | Estado y ubicación |
|---|---|
| Home y navegación | Publicados en [zoka.network](https://zoka.network); comprobados en escritorio y móvil |
| Whitepaper y litepaper v2 | Revisados y publicados como diseño de Quantic en desarrollo |
| Papers históricos v1 | Conservados con aviso inicial de correcciones; cuatro PDF publicados en total |
| Primitivas experimentales | [crates/zoka-pq](crates/zoka-pq/README.md): ML-KEM-768, X-Wing y ML-DSA-65 |
| Integración al nodo | Feature `pq-experimental` (apagada por defecto), `zoka::pq`; iroh-base 1.0.3 y curve25519-dalek 5.0.0 |
| Zenith como testnet | Rama `zenith-testnet` en los tres repos; ver [etapa 2](docs/ETAPA2-zenith-testnet.md) |
| Interfaz Zoka Quantic | Ramas `quantic-interfaz`; capturas en `zsilent-core/docs/capturas-quantic` |
| Arreglo de `--sync-only` | Rama `arreglo-sync-only` (nodo) |
| Etapa 3 (sin activar) | Consenso v2, envío/recepción/CLI y prueba en red local: [especificación](docs/ETAPA3-direcciones-hibridas.md) |
| Pruebas criptográficas | 13/13 en Windows, Linux x64, Linux ARM64 (qemu) y el teléfono ARMv7, con el lock alineado al nodo |
| Pruebas del nodo | Ver [resultados](docs/RESULTADOS-postcuantica-etapa1.md#integración-al-nodo) por plataforma |
| Ejecución pendiente | Android ARM64 (no hay dispositivo de 64 bits) y hardware ARM64 real |
| Benchmarks | Windows, Linux x64 y teléfono ARMv7 (repetido el 12/10 con el lock alineado); no equivalen a un escaneo completo de wallet |

Referencias Git verificadas al preparar este archivo:

- **Nodo:** rama `postcuantica-base-criptografica`, commit `1be41b57`.
  Publicada en GitHub y GitLab como rama de desarrollo; no se integró a `main`.
- **Nodo, etapa 1:** rama `postcuantica-integracion-nodo` (sobre `1be41b57`),
  commits `9c8fca0f` (integración) y `df74753c` (documentación).
- **Nodo, etapa 2:** rama `zenith-testnet` (sobre la anterior); ver etapa 2 abajo.
- La rama `postcuantica-integracion-nodo` (nodo) y las `zenith-testnet` de los
  tres repos se publicaron en GitHub (`zoka-network`) y GitLab. Ninguna se integró
  a `main`.
- **Web:** repositorio independiente `zokaweb`, rama `cerrar-web-quantic`.
  Commits `d4e0af6` (papers), `3c03184` (home y navegación) y `46e9afa`
  (título en una línea, privacidad más visible y esfera reducida).
  Publicados en `main` y en la rama en GitHub y GitLab.
- **Despliegue web:** Cloudflare Pages, proyecto `zokaweb`, carga directa.
  El push de Git por sí solo no despliega. Última URL inmutable:
  <https://b9b102ec.zokaweb.pages.dev>.

## Orden de trabajo pendiente

### 1. Integrar la base criptográfica al nodo

- [x] Revisar la rama y el estado Git de cada repositorio antes de modificarlo.
- [x] Resolver el conflicto: bastó subir `iroh-base` de 1.0.2 a 1.0.3, que solo
  acepta las versiones estables; `curve25519-dalek` pasa a 5.0.0. Diff de fuentes
  revisado: nada de lo que cambió está activo en el nodo.
- [x] `zeroize`: el crate acepta `^1.8.2` y el nodo resuelve 1.9.0. Los dos
  lockfiles quedaron alineados paquete por paquete.
- [x] Pruebas de compatibilidad: vector RFC 8032 para la identidad iroh y prueba
  cruzada con el binario 2.0.2 instalado: snapshot por iroh en las dos
  direcciones (349.675.478 bytes, 25.204 bloques) y wallets de un binario
  visibles en el otro. Contra la red real por Tor (12/10): el nodo nuevo y el
  2.0.2, en paralelo, se conectaron a seed3 y sincronizaron los mismos bloques
  (25.204 → 25.244). seed1 (PC del usuario) y seed2 no respondían.
- [x] **Falla previa encontrada y corregida (12/10, rama `arreglo-sync-only`,
  `1acd4c68`):** `fullnode --sync-only` no arrancaba ni en el 2.0.2 instalado
  (`invalid team member address`: sin wallet la dirección quedaba vacía y
  `TeamMiningRuntime::new` la rechazaba). Ahora ese modo usa un runtime de
  team-mining observador (`without_local_member`) con almacén en memoria: no
  actúa como miembro ni toca el `team.json`. ZSilent usa ese modo para bajar
  bloques antes del login. Suite: 884 + 79 + 3; probado con el binario en una
  carpeta temporal (llega a ready y no crea wallets). Falta un release.
- [x] Integrar `crates/zoka-pq` detrás de `pq-experimental`, apagada por defecto.
- [x] Pruebas del nodo en Windows y Linux x64 (880 + 76 + 3 en ambos); `check` para
  Android ARM64/ARMv7; build release cruzada a Linux ARM64 y subconjunto de
  pruebas bajo qemu (PQ, iroh, snapshot, economics, PoW RandomX, transacciones).
- [ ] Android ARM64: no hay dispositivo de 64 bits; el teléfono solo corre ARMv7.
  Linux ARM64 se ejecutó emulado con qemu-user, no en hardware real.

**Cierre:** ver [resultados](docs/RESULTADOS-postcuantica-etapa1.md#integración-al-nodo).
La única prueba del nodo que fallaba (`recovery_manifest_matches_the_compiled_1_9_consensus_policy`)
exigía la versión 1.9.0 con el crate en 2.0.2 desde agosto. Desde el 13/10 se llama
`network_manifest_matches_the_compiled_policy` y comprueba coherencia en vez de
fijar la versión: ya no hace falta `--skip`.

### 2. Preparar Zenith como testnet y definir activación

Hecha el 12/10 en la rama `zenith-testnet` de los tres repos. Detalle, decisión y
procedimiento: [docs/ETAPA2-zenith-testnet.md](docs/ETAPA2-zenith-testnet.md).

- [x] Presentar Zenith como testnet (el JSON suma `network_role`/`network_name`).
  Desde el 12/10 (ramas `quantic-interfaz`) el nombre visible es la marca nueva:
  "Zoka Quantic · Testnet" en ZSilent y ZKAPriv, "Zoka Quantic testnet" en CLI y
  API; ZSilent tomó además el estilo de zoka.network. ZKAPriv avisa que sus ZKA
  son de prueba.
- [x] Chain ID histórico conservado; `zoka-testnet-v1` retirada (nunca operó).
- [x] Directorios: **decisión de no mover nada.** Zenith sigue en `.zoka/mainnet`
  con la etiqueta interna `mainnet`, que las bases guardan en
  `chain:genesis:network`. `--network testnet|zenith|mainnet` abren Zenith. Mover
  los datos haría que un cliente viejo y uno nuevo vieran wallets distintas, y en
  la PC del usuario ya hay un `.zoka/testnet` de la red retirada con wallets
  propias, que no se toca.
- [x] Marco de activación: `economics::PqFeature` con alturas `None` en todas las
  builds. **Todavía no hay una altura acordada.**
- [ ] Anuncio: borrador en [docs/ANUNCIO-zenith-testnet-BORRADOR.md](docs/ANUNCIO-zenith-testnet-BORRADOR.md);
  publicarlo solo con aprobación del usuario y clientes nuevos publicados.
- [ ] Releases de nodo, ZSilent y ZKAPriv con estos cambios (no se generaron).

**Cierre:** compatibilidad probada con el binario 2.0.2 instalado (wallets visibles
en los dos sentidos, snapshot por iroh en los dos sentidos). Procedimiento de
cambio de cliente: reemplazar el binario, sin mover datos; volver atrás es
reinstalar el anterior. Suite del nodo en Windows: 883 + 79 + 3 OK (falla solo la
prueba heredada de la versión 1.9.0). ZSilent 62/62; ZKAPriv compila.

Ramas y commits de la etapa 2:

- **Nodo:** `zenith-testnet` sobre `postcuantica-integracion-nodo`: `357d2089`
  (presentación y retiro de la testnet vieja), `dab312af` (marco de activación),
  `b5414f5f` (ayuda de zokahd) y el de documentación.
- **ZSilent (`zsilent-core`):** `zenith-testnet` sobre `sacar-exchange` (`8c09b71`),
  commit `b86b1c5`. En la copia de trabajo de `zsilent-core` (rama `sacar-exchange`)
  había un cambio **sin commitear** en `ZSilentAppState.kt` de las 14:20 del 11/10 que
  borraba 1.195 líneas. **Resuelto el 12/10:** era la versión del primer release
  (0.1.0, `06c895d`, 19/05) regrabada con BOM, sin nada nuevo. Quedó guardada en
  `stash@{0}` de `zsilent-core` y el archivo volvió al último commit.
- **ZKAPriv (`zkapriv`):** `zenith-testnet` sobre `seguridad-octubre-2026` (`5e5752f`),
  commit `dc7f448`.

### 3. Direcciones, cifrado y escaneo de wallets

Rama `etapa3-direcciones-hibridas`: `986e98b5` (módulo híbrido), `38534d0c`
(costo en el teléfono), `4c5064f3` (consenso de la salida v2), `b814468b`
(armar, reconocer y gastar con `pq-experimental`), `4ea05c73` (CLI) y `6f3a1b57`
(prueba en red local: v2 atada a los bundles canónicos y mensaje P2P).
Especificación de trabajo: [docs/ETAPA3-direcciones-hibridas.md](docs/ETAPA3-direcciones-hibridas.md).
Decisiones del usuario del 12/10: probar en una red **local** de la PC (no en
la testnet pública), el coinbase sigue público (v1) y las apps muestran la
`zkq1` solo cuando la red la active. Nada de esto se instaló: la PC sigue con
el `zoka.exe` de `38534d0c`.

- [x] Especificar versiones, prefijos, checksum, tamaños máximos y recuperación de
  claves: dirección `zkq1`, versión 2, ~1.715 caracteres; misma clave de gasto que
  `zka1` y clave X-Wing derivada de la misma raíz (la frase recupera las dos).
- [x] Perfil híbrido y derivación contextual para salidas e importes
  (`src/private_wallet/hybrid.rs`, 8 pruebas). **Faltan** comisión del minero y
  coinbase (abiertos en la especificación: hoy el coinbase publica la dirección).
- [x] Conectar las salidas v2 al armado y la validación de transacciones, con el
  rechazo antes de la altura y el hash extendido solo para v2. El hash de las
  v1 quedó fijado con el código anterior. **La v2 exige además los bundles
  canónicos:** sin ellos los cuerpos no son de consenso y un nodo sin la
  activación aceptaba el bloque (medido en la red local).
- [x] Wallets guardadas y CLI (`wallet-receive --hybrid`, `tx-send --to zkq1…`).
  El vuelto sigue al pago (pago v2 ⇒ vuelto v2).
- [x] ZSilent y motor móvil (13/10): `.so` de los dos ABIs con `pq-experimental`,
  JNI `hybridWalletInfo`/`scanHybridOutputs`, escaneo v2 en lote en ZKAPriv, zkq1 y
  QR en las dos apps solo con altura, envío a zkq1 con la misma condición. Detalle
  en la sección "Clientes" de la especificación.
- [x] Comisión del minero (13/10): sobre v2 con ciphertext X-Wing en el campo
  efímero, etiqueta `zoka-hybrid-fee-claim-v2`; sin cambio de consenso (la regla de
  bloques solo exige campos no vacíos, y hay prueba). Sale v2 solo con la feature,
  la altura fijada y la wallet del minero local.
- [x] Prueba en red local de tres nodos (dos corridas): la v2 espera en el
  mempool hasta la altura, entra con sus cuerpos, el destinatario la ve y la
  gasta, los saldos cierran exactos y un nodo sin la activación rechaza el
  bloque. Hallazgos ajenos a la v2 (notas ya gastadas en la wallet del CLI,
  "Archived: 0" desde el nodo minero, chisme P2P muerto): en la especificación.
- [x] Medir escaneo en el teléfono de 32 bits: v2 3,8 ms por salida contra 1,5 ms de v1
  (`examples/hybrid_scan_bench.rs`, tres pasadas; ver la especificación). Faltan
  memoria, batería y tamaño real de los datos de la cadena.
- [x] D1/D2 decididos por el usuario: ML-KEM-768 vía X-Wing y direcciones largas.
- [ ] Diseñar y revisar cualquier etiqueta de vista antes de incorporarla: una etiqueta
  basada solamente en ECDH clásico puede filtrar pertenencia frente a un adversario cuántico.
- [ ] Probar envíos ZSilent → ZKAPriv y ZKAPriv → ZSilent, recuperación y resincronización.

**Referencia medida:** ML-KEM desencapsula en unos 1,11 ms y X-Wing en 3,41–3,52 ms
en el teléfono ARMv7. X-Wing incluye expansión interna de claves. No son tiempos
de escaneo completo ni una comparación aislada del coste del combinador.

### 4. Circuito de privacidad y eliminación de key images

**13/10: no se diseña un circuito propio.** Construcción elegida y confirmada por
el usuario: Ironwood, mediante el crate `orchard`, con el cifrado de notas X-Wing; quita además
el trusted setup de los gastos. Criterio, medición y próximos pasos en
[docs/ETAPA5-gasto-sin-key-images.md](docs/ETAPA5-gasto-sin-key-images.md). Las tareas
de abajo se cumplen adoptando ese circuito, salvo la revisión externa.

- [ ] Diseñar un circuito que demuestre propiedad de cada nota, pertenencia al árbol,
  derivación correcta de nullifiers y conservación de valor.
- [ ] Vincular la prueba al contenido de la transacción para impedir reutilización
  con otras salidas. Probar todas las entradas activas.
- [ ] Mantener compromisos consumidos, claves consumidas y caminos de inclusión
  como testigos privados en el diseño nuevo.
- [ ] Añadir pruebas adversariales de doble gasto, propiedad, pertenencia y emisión.
- [ ] Retirar CLSAG/key images únicamente cuando la construcción sustituta esté completa
  y revisada. Borrar campos del JSON no soluciona el problema criptográfico.
- [ ] Conseguir revisión externa y ejecutar una ceremonia multiparte con los circuitos finales.

**Hallazgos que no hay que perder:**

- El formato actual publica `input_commitments`, `input_public_keys` y caminos de
  inclusión; permite identificar entradas consumidas sin computación cuántica.
- El nullifier del wallet usa SHA-256 con dominio y la clave de gasto original.
  No confundirlo con el helper BLAKE3 de `src/transaction/nullifier.rs`.
- El `proof_nullifier` de Groth16 usa una relación sintética de 64 bits
  `secret * index + blind`; no demuestra la derivación SHA-256 del wallet.
- Resolver el logaritmo discreto de la clave pública de gasto no da directamente
  la preimagen de la clave de gasto. No repetir la afirmación contraria.

### 5. Snapshots, clientes y pendientes entre repositorios

- [ ] Implementar snapshots con Ed25519 + ML-DSA-65 y verificación obligatoria de ambos.
- [ ] Completar los motores móviles `.so` para los dos ABIs y generar APK revisables.
- [x] Corregir el problema de ZSilent que ofrece notas ya gastadas (13/10, rama
  `arreglo-notas-gastadas`). **Falta probarlo** en ZSilent minando con el binario nuevo.
- [ ] Revisar y sincronizar las ramas de seguridad/Tor/retiro del exchange de los demás
  repositorios, conservando los cambios previos y resolviendo divergencias sin force push.
- [ ] Rehospedar los archivos de trusted setup todavía referenciados en la cuenta antigua,
  verificar hashes y actualizar manifiestos y clientes de forma coherente.
- [ ] Publicar releases verificables en la nueva cuenta `zoka-network` cuando estén listos.

Los pendientes heredados y sus ramas están enumerados en las etapas 0, 0b y 0d
del plan detallado. Su estado debe comprobarse por repositorio; publicar la web
no significa haber publicado clientes nuevos.

### 6. Validación integral y preparación del génesis

- [ ] Resolver las actualizaciones mayores pendientes de red y sus avisos de seguridad.
- [ ] Ajustar dificultad inicial y verificar el tiempo objetivo de bloque con pruebas.
- [ ] Implementar y probar premine, suministro máximo, fronteras de emisión y comisiones.
- [ ] Fijar la dirección pública del equipo para el génesis; todavía no está definida aquí.
- [ ] Alinear versiones de crate, cliente mínimo, manifiestos y formatos de protocolo.
  13/10: cliente mínimo del código = manifiesto (2.0.1) y la prueba heredada de la
  1.9.0 comprueba coherencia; el salto a 2.1.0 va con la versión (ver
  [docs/RELEASE-2.1.0-testnet.md](docs/RELEASE-2.1.0-testnet.md)).
- [ ] Ejecutar pruebas integrales en la testnet y revisión externa de cifrado/circuitos.
- [ ] Revisar los papers contra lo efectivamente implementado, después de cerrar el diseño.

### 7. Lanzamiento de Quantic

- [ ] Fijar chain ID nuevo, génesis reproducible y parámetros finales.
- [ ] Activar desde el bloque 0 de Quantic el diseño que haya superado las validaciones.
- [ ] Preparar clientes, seeds, recuperación, explorador y documentación de operación.
- [ ] Explicar públicamente el premine y que los saldos de Zenith no se trasladan.
- [ ] Verificar los criterios anteriores antes de anunciar una mainnet operativa.

**Infraestructura VPS:** continúa en espera de la indicación del usuario. Tor se
conserva; un VPS no lo sustituye. No iniciar cuentas, servidores ni minería en la nube
como parte de este traspaso.

## Archivos para retomar

1. [Plan detallado e historial](docs/HOJA-DE-RUTA-mainnet-postcuantica.md).
2. [Diagnóstico de código y circuito](docs/REVISION-postcuantica-etapa1.md).
3. [Resultados y límites de validación](docs/RESULTADOS-postcuantica-etapa1.md).
4. [Benchmarks completos](docs/benchmarks/2026-10-11-pq.json).
5. [API experimental y límites](crates/zoka-pq/README.md).
6. [Script de validación de plataformas](scripts/check-pq-platforms.ps1).
7. [Procedimiento de papers](zokaweb/docs/papers.md).

Comandos existentes para verificar el crate independiente:

```powershell
cargo test --manifest-path crates/zoka-pq/Cargo.toml --locked
cargo clippy --manifest-path crates/zoka-pq/Cargo.toml --locked --all-targets -- -D warnings
cargo fmt --manifest-path crates/zoka-pq/Cargo.toml --check
./scripts/check-pq-platforms.ps1 -RunAndroidBench -Iterations 100 -AndroidRuns 3
```

El último comando requiere un dispositivo Android ARMv7 autorizado y las herramientas
del SDK/NDK. Usa archivos temporales de benchmark; no instala una aplicación.

## Cómo dejar el siguiente traspaso

- Actualizar este archivo al cerrar cada etapa: commit, pruebas, resultado y pendientes.
- Conservar los documentos/archivos ajenos al trabajo. Al preparar esta hoja había
  archivos sin seguimiento: `Zoka Network Web Applications.docx`, `apelacion-github.md`,
  `discord-bot/` y `zoka-guard-vault/`.
- No guardar tokens, semillas, claves privadas ni credenciales en la documentación.
- Separar siempre: diseño propuesto, código integrado, pruebas ejecutadas y activación real.
- No presentar el módulo experimental como privacidad postcuántica ya activa en la red.
