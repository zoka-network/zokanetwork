# Traspaso: qué se está haciendo ahora

Registro vivo para que otro agente (Codex, Claude) siga donde quedó este. Cada
paso se marca al terminarlo, con su commit; lo que queda a medias dice en qué
punto exacto está. Lo histórico está en [HOJA-DE-RUTA-CLAUDE.md](../HOJA-DE-RUTA-CLAUDE.md).

## Estado al 3/11/2026, retomado por Codex

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
