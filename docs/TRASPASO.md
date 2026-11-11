# Traspaso: qué se está haciendo ahora

Registro vivo para que otro agente (Codex, Claude) siga donde quedó este. Cada
paso se marca al terminarlo, con su commit; lo que queda a medias dice en qué
punto exacto está. Lo histórico está en [HOJA-DE-RUTA-CLAUDE.md](../HOJA-DE-RUTA-CLAUDE.md).

## Estado actual al 10/11/2026: batería terminada y trabajo recuperado

El usuario pidió retomar a Claude, que se detuvo mientras corría el fuzzing
del nodo. La tarea vigente es la **revisión técnica sin cambios de consenso**
de la entrada del 10/11 en la hoja de ruta. La tarea del móvil del 3/11 que
figuraba arriba en este archivo quedó vieja: ZKAPriv 3.0.0 y la billetera web
se publicaron el 4/11, según esa hoja.

### Lo que quedó terminado

Rama `endurecimiento-sin-consenso`, sobre `main` (`c276021a`):

- `7a25c90f`: dependencias con seis avisos de seguridad corregidos.
- `f372d598`: clave de vista solo de entrantes, disponible en biblioteca y CLI.
- `f274b8e7`: validar el árbol de notas guardado antes de abrir la billetera.
- `720c863b`: lectura de claves de autoridad cifradas; **sin activar en la PC**.
- `1491bea`: fuzzing determinista del pool y nodo, seis objetivos de
  libFuzzer y protección frente a pánicos del verificador. Claude lo dejó
  escrito y probado; Codex recuperó los resultados y guardó el commit.

**La última prueba terminó a las 21:54:30 (Argentina). No queda una batería
corriendo.** Resultados de los registros de Claude, verificados al retomar:

| Batería | Resultado |
| --- | --- |
| Pool completo | 36 aprobadas, 0 fallas; 119,36 s |
| Nodo, selección de módulos de esta revisión | 201 aprobadas, 0 fallas; 3 ignoradas y 883 filtradas; 228,17 s |
| Fuzzing del nodo | 4 aprobadas, 0 fallas; 61,31 s |
| Fuzzing del pool, escala 20 | 9 aprobadas, 0 fallas; unas 500.000 entradas; 1.178,10 s |

Evidencia conservada en `target/endurecimiento-2026-11-10/`, con copias
comparadas por SHA-256. Alcance, limitaciones y comandos en
[FUZZING.md](FUZZING.md). La pasada extendida es anterior a la protección
del verificador; las pruebas finales del pool y nodo cubren el código final.
No hizo falta repetirlas para guardar el trabajo. No se probaron aplicaciones,
teléfonos ni la red pública en esta tanda.

### Cómo seguir

1. Correr los seis objetivos de **libFuzzer guiado por cobertura en Linux**:
   hasta ahora solo compilaron en Windows. La pasada determinista extendida
   no reemplaza ese trabajo. Preparar el VPS sin interferir con sus nodos y
   registrar límites, semilla, duración y resultados.
2. Integrar la clave de vista de entrantes a las aplicaciones (ZSilent,
   billetera web, ZKAPriv y ZokaScan) y validar cada una.
3. El usuario decide cómo ingresar la contraseña de las claves de autoridad;
   él cifra sus claves. No leerlas ni activar el cifrado por cuenta propia.
4. Integración a `main`, paquetes y publicación siguen pendientes. **No hubo
   release, deploy ni instalación de esta rama.** Publicar requiere su OK.

Los cambios de consenso y las otras mejoras de la revisión están enumerados
en la hoja de ruta; no se mezclan con este cierre.

## Registro histórico al 3/11/2026, retomado por Codex

Lo que sigue conserva el traspaso de ese día, incluidos pendientes que Claude
resolvió después. No es el estado actual; consultar arriba y la hoja de ruta
antes de volver a ejecutar sus pasos.

### ZKAPriv en curso (3/11, después de las 18 h)

El usuario pidió trabajar con el Samsung SM-A135M conectado (armeabi-v7a):
Home limpio con saldo/Quantic/privacidad; navegación inferior Home, Send,
Receive y Activity; cuadrícula animada durante la prueba, check rosa al
aceptar y error rojo; saldo actualizado con la app abierta. **Corrección
explícita del usuario: toda la interfaz de Zoka va en inglés**, aunque la
conversación y este traspaso sigan en castellano. Esa decisión reemplaza la
regla general de idioma para los textos del producto.

Se trabaja en `zkapriv`, rama `zkapriv-v2`, conservando y revisando los cambios
del 31/10. Respaldo previo en `target/zkapriv-3-review/previous-work.patch`
y `previous-native.tar.gz`. No se leyeron claves ni el almacén del teléfono.
La app vieja `com.zoka.zkapriv` (2.1.3, sin motor STARK) no fue reemplazada.
La variante `com.zoka.zkapriv.benchmark` ya estaba instalada desde el 30/10
y se actualizó con `adb install -r`, conservando sus datos. Ahí está abierta
la vista de diseño con datos ficticios; no se abrió el almacén. Esa variante
de octubre es la que hay que usar para las pruebas v2 con el usuario, no
pedirle restaurar en la app vieja. Falta instalar el APK con el motor nuevo;
el APK de diseño aún lleva `.so` previos y no sirve para probar envíos.

- Ya compilaron interfaz y 53 pruebas de red/UI, incluidas recuperación de
  sincronización y montos con coma decimal. Repetidas sin fallas a las 18:27
  (`gradle-final-ui.log`). Capturas comprobadas `home-final-dark.png` y
  `proof-final-light.png`: todo en inglés y barras del sistema acordes al tema.
  Solo queda recompilar un retoque de etiqueta en Receive y formato del código.
- Motor Android compilándose en VPS2: unidad `zkapriv3-native-build`, registro
  `/root/zkapriv3-native-build.log`, fuentes `/root/zkapriv3-src`. NDK r27d
  instalado y verificado. Script `native/build-android-linux.sh`. Ambos ABI
  deben completarse antes del release; no mezclar con los `.so` anteriores.
  **armeabi-v7a terminó a las 18:43**: `libzoka_mobile.so` SHA-256
  `3ed07e893e6e9d8f6ec70c3dc0c9ab52ccfc4e0348ce8ce42144a21ccc10c385`, copiado
  al repo y verificado; los tres símbolos JNI están exportados. ARM64 sigue
  compilando. APK debug para el Samsung en `gradle-native-v7a.log`.
  La copia del nodo incluye también los manifiestos públicos de `docs/`.
  El enlace LTO necesitó memoria adicional: swap temporal de 4 GiB en
  `/root/zkapriv3-build.swap` (no está en fstab), unidad limitada a 6 GiB RAM
  y 3 GiB swap. Al terminar ambos ABI: `swapoff` y borrar solo ese archivo
  temporal. El script mejorado local difiere del que está corriendo: copiarlo
  después de que termine la unidad, no modificar el archivo en ejecución.
- Falta validar el motor nuevo en el teléfono, firmar y comparar certificado,
  actualizar sin borrar datos y hacer pruebas reales con los envíos del
  usuario. No publicar el APK hasta completar esas pruebas.
- Prueba instrumentada lista: `StarkReadOnlyInstrumentedTest` crea un estado
  temporal aislado, consulta HTTPS/checkpoint y vuelve por P2P y Arti/Tor.
  No manda pagos ni lee el almacén del usuario. Ejecutar solo esa clase; las
  instrumentadas viejas esperan el servicio de la red v1, que ya no se empaqueta.
  Detalle de pruebas en `zkapriv/docs/VALIDACION-3.0.0.md`.

### Primera etapa terminada

El plan vigente está conservado en [PLAN-CIERRE-2026-11-03.md](PLAN-CIERRE-2026-11-03.md),
recuperado de `C:/Users/mderramus/.claude/plans/sprightly-swimming-papert.md`.
Orden acordado: nodo/minero 4.0.4 y ZSilent 4.0.6, ZKAPriv 3.0, vigilante y
mejoras de red/privacidad. El usuario autorizó publicar las versiones cuando
pasen las pruebas y los hashes coincidan; los anuncios de Discord se muestran
antes. No instalar ZSilent sin que lo pida.

- **Infraestructura:** seed2, explorador, túnel y .onion en `13.140.206.10`;
  seed3 y compilación Linux en `13.140.206.11`. SSH por 443 con la identidad
  `~/.ssh/zoka_vps` y `~/.ssh/known_hosts_zoka_vps`. No leer claves ni archivos
  de credenciales. Las claves de checkpoints siguen únicamente en la PC.
- **PC libre:** nada de WSL ni servicios/minería iniciados en segundo plano.
  Windows se compila acá por autorización del usuario; Linux y el motor
  Android en VPS. No levantar el explorador local: comparte identidad con seed2.
- **Etapa 1 cerrada y publicada (3/11, 17:44, Argentina):** nodo y minero
  `v4.0.4` = `50588833`, ZSilent `v4.0.6` = `162743d`; etiquetas y código en
  GitHub y GitLab. La rama del nodo se integró a `main` sin reescribir historia.
  Los releases anteriores quedaron marcados como reemplazados. Pruebas y
  hashes en [RELEASE-4.0.4.md](RELEASE-4.0.4.md).
- El ZIP anterior quedó conservado en
  `target/cli-miner/4.0.4-descartado-sin-seed3`.
- **Linux:** fuente archivada de `ced0cfce` en `/root/release404-src` del VPS 2;
  servicio temporal `zoka-release404-build`, registro `/root/release404-build.log`,
  salida `/root/release404-artifacts`. Compiló en Debian 11 aislado en 24 min
  31 s; requiere glibc 2.30. El servicio terminó con un error del empaquetador
  al leer CRLF: corregido en `6043b5e`, copiado al VPS y reempaquetado sin
  recompilar. No quedan compilaciones en marcha.
- **ZSilent:** worktree `target/zsilent-4.0.6`, ahora en `main` (`162743d`).
  El `ZSilentAppState.kt` modificado en la copia habitual queda intacto.
  MSI armado y extraído; 84 pruebas sin fallas, incluidas v2 y Tor real.
  Release público, con sus tres archivos descargados y verificados por hash.
  Archivos en `target/zsilent-4.0.6/build/release/4.0.6`; copia descargada en
  `target/release-verify/zsilent-4.0.6`. No se instaló.
- **Paquetes publicados:** ZIP y tar.gz en `target/cli-miner/4.0.4`; copia
  descargada en `target/release-verify/node-4.0.4`. Windows: hashes, semillas
  nuevas y comandos en una ruta con espacios. Linux: sincronizó desde cero
  hasta el bloque 3958 en 453 s, mismo hash que el explorador, dos pares,
  sin relay ni minería, checkpoint 3923 seguido. Pruebas: 55 del nodo y 22
  del pool aprobadas. Los procesos de prueba se cerraron.
- **Web publicada:** `zokaweb` `89bef6a`, en ambos remotos; despliegue
  `https://b328a0cf.zokaweb.pages.dev`. Versiones 4.0.6/4.0.4 y guía con las
  semillas permanentes. Siete pruebas y build aprobados; JS/CSS de zoka.network
  coinciden por SHA-256 y las tres descargas públicas responden HTTP 200.
- **Sigue ZKAPriv 3.0:** recuperado el informe del agente de Claude en
  `target/zkapriv-informe-claude-3nov.json`. Antes del APK faltan conectar la
  confirmación HTTPS del checkpoint, mostrar verificación/pares, revisar los
  cambios locales (incluido el `isActive` que no compila) y recompilar ambos
  ABI. El nodo en `main` ya incluye todo lo que necesita el motor móvil.
  La prueba de envíos la hace el usuario. Después siguen vigilante y etapas
  técnicas del plan; no se publicaron anuncios de Discord.

## Estado histórico al 30/10/2026, mediodía (Claude)

- **Testnet v2 en marcha.** Bloque 1 `69249437…`, cadena `zoka-quantic-testnet-v2`.
  La semilla es la PC del usuario: ZSilent 4.0.4 (nodo 4.0.2, API 127.0.0.1:9140,
  Stratum 3333) y XMRig minando a la wallet del equipo (`zsilent`).
- **Publicado:** nodo y minero CLI 4.0.2, ZSilent 4.0.4, zoka.network y zokascan.cc
  (con api.zokascan.cc). Todo en GitHub (`zoka-network/*`) y GitLab (`zokanetwork/*`).
- **Lo que corre en la PC y cómo se levanta** (siempre con `explorer.exe`):

  | Proceso | Lanzador | Comprobar |
  | --- | --- | --- |
  | ZSilent 4.0.4 | `%LOCALAPPDATA%\ZSilent Core\ZSilent Core.exe` (el usuario entra con su contraseña) | `http://127.0.0.1:9140/status` |
  | XMRig vigilado | `target\v2-genesis-2026-10-29\launch\xmrig-equipo-oculto.vbs` (sin ventana; se relanza solo a los 10 s; `detener-xmrig.cmd` lo para) | su `xmrig.log`, "accepted"; `vigilancia.log` |
  | Nodo del explorador | `%APPDATA%\Zoka\.zoka\explorer-node\explorer-node-oculto.vbs` (sin ventana) | `http://127.0.0.1:9181/status` |
  | Túnel de Cloudflare | `%APPDATA%\Zoka\.zoka\cloudflared\tunnel-zokascan-oculto.vbs` (sin ventana) | `https://api.zokascan.cc/status` (error 1033 = túnel caído) |

## Trabajo en curso (pedido del usuario: «avanzá con todo lo que puedas»)

Orden elegido. Lo público (deploy, release, anuncio) se deja listo y se
publica solo con el OK del usuario.

1. [x] **XMRig se relanza solo.** Se había cerrado dos veces junto con ZSilent,
   sin error en su registro ni en los eventos de Windows. ZSilent solo mata sus
   propios `zoka.exe`. Lo más probable es que se haya cerrado su ventana. Ahora
   corre sin ventana con un vigilante (`xmrig-vigilado.cmd`); probado
   matándolo: volvió a los 10 s. Los lanzadores viven en `target/`, fuera de
   git.
2. [x] **Anuncio para Discord** de la testnet v2, ZokaScan, la clave de vista
   y ZSilent 4.0.4: [docs/ANUNCIO-testnet-v2-zokascan-BORRADOR.md](ANUNCIO-testnet-v2-zokascan-BORRADOR.md),
   en castellano y en inglés, cada uno de menos de 2.000 caracteres. Lo pega
   el usuario.
3. [x] **Papers 2.6** (zokaweb `45170ee`, pusheado, **sin desplegar**): la
   clave de vista exportada (semilla de vista, clave de salida y dueño), el
   escaneo en el navegador, sin Tor (y el modo planificado) y la API pública.
   El litepaper ya no dice que ZokaScan está fuera de línea. PDF rearmados.
   Para publicar: `npm run build` y
   `npx wrangler pages deploy dist --project-name zokaweb --branch main`, con
   el OK del usuario.
4. [x] **Segunda semilla** (lo que se puede hacer sin un operador):
   - [docs/SEMILLAS.md](SEMILLAS.md): por qué, cómo se prueba un manifiesto
     con `--seeds` y cómo se fija y se publica;
   - minero CLI: comando `node-id` (`show-node-id.cmd`), que muestra los dos
     ids públicos; probado en Windows y en Linux (WSL), y da la misma
     identidad en cada corrida;
   - guías README/LEEME: «Help the network» / «Ayudá a la red».
   Sale en el próximo paquete CLI: necesita un nodo 4.0.3, porque el armado
   exige que `zoka.exe` sea la versión de `Cargo.toml`. **Falta** que alguien
   ofrezca un nodo siempre prendido; lo pide el anuncio.
5. [x] **Modo Tor (publicado el 30/10):** diseño en [docs/MODO-TOR.md](MODO-TOR.md), con sus etapas
   E1 a E5 y su registro. Código en la rama `modo-tor` (pusheada). Seguir por
   la primera etapa sin marcar.
   - E1 y E2 hechas (`644b3e9f`, `85e5be88`).
   - E3 hecha: `SocksDialer` (por `tor.exe`), probado contra la red Tor real
     (conexión a una .onion nueva en 28 s, 2 bloques y 1 entregado en 39 s).
     La prueba ignorada
     `a_node_syncs_through_tor_from_an_onion_seed`. Se corre con
     `ZOKA_TEST_TOR_EXE=.../third_party/tor/windows-x86_64-0.4.9.14/tor.exe`
     y `cargo test --release --features quantic-pq --lib tor_from_an_onion -- --ignored --nocapture`.
   - E4a hecha: opciones `--tor`, `--stream-listen` y `--stream-address`, probadas
     con `scripts/probar-modo-tor.ps1` (6 minutos siguiendo a la semilla solo por
     Tor). Siguen E4b (semilla .onion real y manifiesto, con OK del usuario) y
     E5 (ZSilent).
6. [x] **Caché de /scan en el borde** (desplegada el 30/10): una Pages Function en
   zokascan.cc, para que la PC no sirva cada página de escaneo.

## Autorizaciones del usuario (30/10, 14:00, antes de irse un rato)

Dadas por escrito en la conversación; valen para este trabajo:

1. **Nodo y minero CLI 4.0.3:** Modo Tor por CLI (`--tor`), semilla .onion en el
   manifiesto y `node-id`. **Publicar** (release, merge a `main`, web) cuando
   pase la prueba contra la testnet real por Tor.
2. **ZSilent 4.0.5:** interruptor "Tor" opcional y apagado por defecto en
   Network privacy, con `tor.exe` de vuelta en el MSI. **Armar y publicar**
   cuando esté probado.
3. **Instalar ZSilent nueva en la PC aunque el usuario no esté.** Se cierra
   ZSilent (semilla y Stratum) y la cadena queda frenada hasta que el usuario
   vuelva a entrar.
4. **Desplegar** los papers 2.6 en zoka.network y la caché de las páginas de
   `/scan` en zokascan.cc.

Tor es **opcional** (decisión del usuario): nunca obligatorio.

## Bitácora

- 30/10: se crean `AGENTS.md` y este archivo.
- 30/10, 13:58: semilla .onion en marcha. `tor-seed` (sin ventana) con la .onion
  `u2soz3q63237afuccfxf3jzytpljpbxpvyzt3jumrr45ukzowdakayqd.onion:9444`, y el nodo del
  explorador con el binario de `modo-tor`, `--stream-listen 127.0.0.1:9444` y
  `--stream-address`. Respaldo: `explorer-node\zoka-4.0.2.exe` y `explorer-node-4.0.2.cmd`.

- 30/10, tarde: **E4b, E5 y releases en marcha**:
  - **Nodo 4.0.3** (rama `modo-tor`, sin merge a `main` todavía):
    - seed2 con su .onion en el manifiesto;
    - descarga en tubería (16 pedidos) y atajo del ancestro común: por Tor
      pasó de 0,5 a 3 bloques/s;
    - ficha estable en el saludo, para que un circuito cortado no trabe la
      conexión;
    - corte por 60 s de silencio;
    - `.gitattributes` con `*.sh eol=lf`.
  - Prueba contra la testnet real: `scripts/probar-modo-tor-testnet.ps1 -Seeds builtin`.
  - **Minero CLI 4.0.3:** `-Tor` / `--tor` y guías. Paquete de Windows
    `c37a2ceb…`; el de Linux, compilando en WSL.
  - **ZSilent 4.0.5** (zsilent-core, rama `modo-tor`, `db9d6ca`):
    - interruptor de Tor opcional;
    - MSI con Tor, armándose en `target/zsilent-4.0.5`.
  - **Web** (zokaweb `613eb92`) y caché de `/scan` (zokaexplorer `a734c88`):
    commiteadas, sin desplegar.
  - **Falta:**
    - merge de las ramas `modo-tor` a `main`;
    - etiquetas, releases, despliegues e instalación de ZSilent 4.0.5 (todo
      autorizado);
    - actualizar la hoja de ruta.
- 30/10, 16:15: **todo lo autorizado está hecho**:
  - releases v4.0.3 (nodo y CLI) y v4.0.5 (ZSilent), con hashes
    verificados; la 4.0.2 y la 4.0.4 quedaron marcadas como reemplazadas;
  - zoka.network y zokascan.cc desplegados;
  - ZSilent 4.0.5 instalada en la PC; la cadena está frenada en 1110 hasta
    que el usuario entre.
  **Pendiente:**
  - el anuncio en Discord (lo pega el usuario);
  - una segunda semilla fuera de la PC;
  - ZKAPriv para la v2.
- 30/10, noche: **ZKAPriv 3.0** (rediseño visual y v2 STARK), en marcha. El
  usuario dejó la sesión en automático. Plan:
  `zkapriv/docs/PLAN-ZKAPRIV-3.md`, rama `zkapriv-v2` del repo zkapriv. Seguir
  por la primera etapa sin marcar de ese plan.
- 30/10 a 31/10: **ZKAPriv 3 avanzó en dos sesiones.** En la rama
  `zkapriv-v2` de zkapriv:
  - E0 (inventario) y E1 (diseño de ZSilent, ícono Z-Q);
  - E2: el motor v2 (`native/src/stark.rs`, compilado para los dos ABI);
  - las pantallas nuevas en `ui/quantic/`.

  El 31/10, otra sesión dejó **sin commitear** el envío entre la PC y el
  teléfono, que según el usuario ya funciona:
  - `native/src/lib.rs` inicializa el contexto de Android para el DNS de
    hickory; sin eso la app se cerraba;
  - cambios en `Cargo.toml`/`Cargo.lock` nativos, `QuanticWallet`,
    `QuanticVault`, `WalletViewModel`, `Receive` y el `.so` de
    armeabi-v7a.

  **Revisarlos con el usuario antes de commitearlos.**
- 3/11: **la red estuvo parada 4 h.**
  - Qué pasó: el Tor de la semilla .onion murió el 31/10 y XMRig el 1/11 a
    las 10:46. Un minero externo sostuvo la cadena hasta las 03:19 del 3/11.
  - Ya está arriba: XMRig vigilado y Tor relanzados.
  - Arranque automático (con el OK del usuario): cuatro accesos "Zoka - …" en
    `shell:startup` (explorador, túnel, Tor de la semilla, XMRig). Además,
    `tor-seed.cmd` ahora también es vigilado.
- 3/11: **4.0.4 en la rama `zkapriv-v2-motor` del nodo, sin publicar:**
  - `f7153fd8`: el minero CLI se vigila solo (XMRig y nodo) y tiene
    `autostart`, en Windows y Linux. Probado en Windows.
  - `63c4c6e6`: la billetera liviana verifica cada página contra la cabecera
    de su bloque (`zoka_pq_pool::commitment`, igual al consenso por prueba) y
    confirma la cadena contra el checkpoint híbrido (`verify_checkpoint`).
    Probado con el checkpoint real del bloque 3651; pasan 51 pruebas STARK.
  - `docs/SEMILLAS.md` al día: hoy las dos semillas están en la PC; tiene una
    guía para una semilla en un VPS.

  **Pendiente:**
  - el usuario consigue una semilla externa (el 3/11, a la tarde): meterla
    como seed3 en el manifiesto de esta misma 4.0.4;
  - compilar Linux (WSL: `MSYS_NO_PATHCONV=1 wsl -d Debian-D -u root -- bash
    /mnt/c/.../scripts/build-linux-bullseye.sh /mnt/c/Users/mderramus/ZOKA`);
  - armar los paquetes y publicar con OK;
  - instalar la 4.0.4 en las semillas de la PC. ZSilent necesita una 4.0.6
    con el nodo nuevo.
- 3/11: **análisis general** (red, privacidad, poscuántico).
  - La v2 ya es poscuántica en el gasto, las notas, el TLS del P2P (exige
    X25519MLKEM768) y los checkpoints híbridos.
  - Falta para la mainnet: un análisis de solidez cuántica de la prueba STARK
    y una auditoría (hoy la estimación es de 100 bits clásicos o más, y no es
    una auditoría PQ); firmar las releases con ML-DSA; y ocultar el origen
    del envío (sin Tor, la semilla ve la IP de quien manda).
- 3/11, 19:00–20:00 (Claude): **pago lento y saldo escondido en ZSilent.**
  Un pago de ZSilent a ZKAPriv tardó 11 bloques (4001 → 4012) y mientras tanto
  ZSilent mostraba 36.776 ZKA en vez de 5.019.799. Diagnóstico completo en
  `docs/MEMPOOL-ENTRE-PARES.md` de la rama del nodo `arreglo-mempool-entre-pares`
  (worktree `target/nodo-mempool`): el P2P solo empujaba transacciones y el
  único minero, detrás de NAT, dependía de que seed2 lograra llamarlo. Ahora el
  que llama también pide el mempool (`MEMPOOL`/`GET_TX`, anunciado en `Head`).
  ZSilent: rama `arreglo-reserva-y-nonce` de zsilent-core (worktree
  `target/zsilent-arreglo-reserva`): el total suma `reserved_atoms` y el nonce
  u64 ya no rompe "Latest blocks". Además, el nodo suma `GET /transactions/<txid>`
  (pendiente en el mempool o minado en los últimos 720 bloques) y el buscador de
  ZSilent y el de ZokaScan (rama `nonce-exacto`) lo usan: antes ningún pago ni hash
  de bloque se encontraba en la v2. Nada publicado ni instalado.
- 3/11, 20:00–21:00 (Claude): **4.0.5 / 4.0.7 armadas y Home de ZKAPriv.**
  - Nodo 4.0.5 (`v4.0.5` = `7ff77929`) y ZSilent 4.0.7 (`v4.0.7` = `3f1e55f`):
    `main` y etiquetas en GitHub y GitLab; paquetes, MSI y pruebas en
    `docs/RELEASE-4.0.5.md`. **Falta, con OK explícito del usuario** (el
    clasificador frenó `gh release create` y la escritura en los VPS): crear
    las releases, marcar 4.0.4/4.0.6 como reemplazadas, actualizar seed3 y
    seed2, desplegar ZokaScan (`dist/` armado) y zoka.network (rama
    `descargas-4.0.7`), y el anuncio `docs/ANUNCIO-4.0.5-BORRADOR.md`.
  - ZKAPriv (`zkapriv-v2`, sin commitear, encima de los cambios de Codex):
    Home nuevo pedido por el usuario. Barra Wallet · Send · Receive · Activity
    · Settings; Home limpio (ZKAPriv / ZOKA QUANTIC con el botón QR, saldo y
    tarjeta de red con altura/hashrate/sincronización; la actividad solo en su
    pestaña), sin botones de refrescar ni tarjeta "Quantic privacy"; Network &
    Tor como pantalla propia (desde la tarjeta o Settings); escáner con marco,
    línea, tocar para enfocar y linterna. Arreglado volver a Wallet: la barra
    recibía una referencia memorizada a selectTab con la ruta vieja; ahora se
    lee del NavController en cada toque. Respaldo previo de Codex:
    `target/zkapriv-3-review/codex-estado-20261103-2016.patch` y
    `codex-sin-seguimiento-20261103-2016.tar.gz`. Solo lo de Claude:
    `target/zkapriv-3-review/claude-home-20261103.patch`. Instalado en el
    Samsung (variante benchmark, datos conservados).
  - **Varias billeteras en ZKAPriv** (pedido del usuario, hecho el 3/11 a la
    noche, sin commitear): `QuanticVault` guarda una lista (`wallets`) y la
    activa (`active_wallet`). La primera es `main` y conserva las claves y la
    carpeta de antes (`phrase`, `birth`, `address_published`, `files/stark`):
    nada se migra ni se resincroniza. Las demás usan `phrase.<id>`,
    `birth.<id>`, `address_published.<id>`, `name.<id>` y `files/stark-<id>`.
    PIN, biometría y Modo Tor son de la app. `QuanticWallet.switchTo/addWallet/
    removeWallet` toman el candado `busy` (el mismo de sync y send); la
    publicación de la dirección guarda el id con el que empezó. UI: pastilla con
    el nombre arriba del saldo → pantalla Wallets (usar, renombrar, sacar,
    "Create new wallet" con respaldo + verificación + nombre, "Restore with 24
    words"). No se probó con una prueba instrumentada: escribiría en el almacén
    real del teléfono del usuario.
- 4/11, 09:00–10:40 (Claude): **billetera web armada y probada, sin publicar.**
  Detalle en `docs/BILLETERA-WEB.md` de la rama `billetera-web` del nodo
  (`05d46034`, en GitHub y GitLab).
  - Nodo: W1 `crates/zoka-light` (checkpoint híbrido y estampilla, puro); W2/W4
    `crates/zoka-pq-wallet-wasm`; W3 `POST /transactions` y `POST /address`
    en `--public-api`, con cupos por cliente (CF-Connecting-IP) y globales y
    permisos de escritura aparte. `quantic::`: 96 pruebas pasan.
  - Un pago probado en WebAssembly (7,55 s, 1.937.222 bytes) lo aceptó la API
    pública y lo minó un nodo de regtest (pruebas a mano, 3 pasos en el doc).
  - Repo nuevo `zokawallet` (al lado de `zokaexplorer`, solo local, sin
    remotos): React + Vite + TS, 30 pruebas; `scripts/e2e-browser.mjs` lo
    probó en un Chrome sin ventana contra la testnet real (4.448 bloques en
    37 s, sin violaciones de CSP, ningún pedido con las palabras).
  - zoka.network: rama `billetera-web` (`cbdea17`, encima de `zkapriv-3.0.0`)
    con el enlace en el menú y en Download. No desplegada.
  - **Falta, con OK explícito:** nodo 4.0.6 (= 4.0.5 + W3) y seed2; repos
    `zokawallet` en GitHub y GitLab; proyecto Pages `zokawallet` y el dominio
    `wallet.zoka.network` (lo agrega el usuario); prueba con dinero real.
  - ZKAPriv: `zkapriv-v2` ya está commiteada y subida (`e356c24`); el APK
    3.0.0 está armado y espera la prueba de envío del usuario.
- 4/11, 15:30–17:40 (Claude): **rediseño de la billetera web y publicación**,
  con el OK del usuario («ahora me gustó, procedé con todo lo demás»).
  - Billetera web rediseñada como web, estilo ZokaScan (barra de secciones,
    Overview con grillas, tabla y QR, diálogos en Settings, responsive):
    `zokawallet` `main` = `a4cc024`, 33 pruebas, Chrome en 1440 y 390 px.
  - Publicado y verificado (bajado otra vez, hashes OK): nodo **v4.0.5**
    (Latest) y **ZSilent v4.0.7**; 4.0.4 y ZSilent 4.0.6 marcadas como
    reemplazadas. zoka.network desplegada con esas descargas (`8793fa7`).
  - **Nodo v4.0.6** (release de servidor, sin "Latest"): `main` = `c8d793d6`
    + docs `309d5b76`, etiqueta en GitHub y GitLab; binario Linux compilado en
    el VPS 2 (`daf2e738…`), probado ahí con datos temporales sin tocar la
    semilla (sincroniza; preflight de `POST /address` 204 con Content-Type).
  - Billetera en Cloudflare Pages, proyecto `zokawallet` (creado con `--force`
    para que sea Pages clásico): https://zokawallet.pages.dev, encabezados y
    hashes verificados en vivo, e2e contra la copia publicada OK.
  - zoka.network: rama `billetera-web-sobre-4.0.7` (`bc2de00`, sin lo de
    ZKAPriv 3.0.0) con el enlace "Web wallet"; se despliega cuando exista el
    dominio.
  - **Frenado por el clasificador, lo corre el usuario**
    (`scratchpad/comandos-para-vos.ps1`): crear los repos `zokawallet`
    (GitHub público con Actions apagado, GitLab), desplegar ZokaScan
    `056eea8`, instalar 4.0.6 en seed3 y seed2 (respaldo `zoka-4.0.4`) y
    borrar el swap del VPS 2. El dominio `wallet.zoka.network` lo agrega en
    Pages → zokawallet → Custom domains.
  - Después: verificar semillas y api.zokascan.cc 4.0.6, desplegar el enlace
    en zoka.network, prueba con dinero (ZSilent → web → ZKAPriv 3.0.0 de
    prueba `com.zoka.zkapriv.benchmark`), y el anuncio de Discord.
- 4/11, 21:20–22:20 (Claude): **billetera web y ZKAPriv 3.0.0 en vivo.**
  - El usuario agregó `wallet.zoka.network` en Pages (gratis; la sesión de
    wrangler no puede tocar DNS) e instaló la 4.0.6 en seed3 y seed2:
    `api.zokascan.cc` responde 4.0.6, directorio con 6 direcciones cortas.
  - Prueba con dinero del usuario: ZSilent → web → ZKAPriv → ZSilent, bloques
    4789, 4791 y 4809.
  - ZKAPriv **v3.0.0** publicada (`zkapriv` `main` = `01d40ca`): APK rearmado
    sin tres `.so` sobrantes que git ignoraba (guardados en
    `target/zkapriv-3-review/jnilibs-sobrantes-armeabi-v7a/`), misma firma que
    la 2.1.3 (`061376f8…`), SHA-256 `e701a319…`; 2.1.2 marcada como
    reemplazada.
  - zoka.network `main` = `f315785`: descarga de ZKAPriv 3.0.0, guía de
    Android y enlace a la billetera web, verificado en vivo.
  - Anuncio: `docs/ANUNCIO-BILLETERA-WEB-ZKAPRIV-BORRADOR.md` (después del de
    la 4.0.5).
  - **Sigue pendiente del usuario:** repo público `zokawallet` (GitHub y
    GitLab; los enlaces "Source code" apuntan ahí), desplegar ZokaScan
    `056eea8` y borrar el swap del VPS 2 (pasos 1, 3 y 6 de
    `scratchpad/comandos-para-vos.ps1`).
- 4/11, 22:20–23:00 (Claude): **repos y sitios al día.**
  - Auditoría de ramas y etiquetas contra GitHub y GitLab (GitLab por el 443:
    el 22 está cortado). Etiquetas iguales en los dos; subidas las ramas que
    faltaban en un lado (nodo `arreglo-mempool-entre-pares`,
    `etapa5-investigacion`; zokaweb `billetera-web`; zokaexplorer y
    zoka-guard-vault `seguridad-octubre-2026`). Quedan solo locales cuatro
    ramas viejas sin publicar ni mergear: nodo `etapa5-quantic-nodo`; zkapriv
    `envio-sin-cuerpos-perdidos`, `quantic-motor`, `wallet-liviana`.
  - Pie con GitHub, X y Discord (íconos de zoka.network) en la billetera
    (`zokawallet` `fcf109e`, desplegada) y en ZokaScan (`6e8eecf`,
    desplegada: esta vez el clasificador dejó; incluye la búsqueda por txid).
    zokaexplorer `main` avanzado a `explorer-quantic-v2` en los dos remotos.
  - `zokawallet`: proyecto de GitLab creado al subir `main` (privado); el de
    GitHub lo crea el usuario (el clasificador frenó dos veces "Create Public
    Surface"). Remotos ya configurados. Pendiente también: swap del VPS 2.
  - 22:37: el usuario creó `zoka-network/zokawallet` (público, Actions
    apagado, `main` = `fcf109e`) y borró el swap del VPS 2. Falta pasar
    `gitlab.com/zokanetwork/zokawallet` a Public desde el panel.
