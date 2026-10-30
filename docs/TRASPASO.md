# Traspaso: qué se está haciendo ahora

Registro vivo para que otro agente (Codex, Claude) siga donde quedó este. Cada
paso se marca al terminarlo, con su commit; lo que queda a medias dice en qué
punto exacto está. Lo histórico está en [HOJA-DE-RUTA-CLAUDE.md](../HOJA-DE-RUTA-CLAUDE.md).

## Estado al 30/10/2026, mediodía (Claude)

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
5. [ ] **Modo Tor:** diseño en [docs/MODO-TOR.md](MODO-TOR.md), con sus etapas
   E1 a E5 y su registro. Código en la rama `modo-tor`. Seguir por la primera
   etapa sin marcar.
6. [ ] **Caché de /scan en el borde** (opcional): una Pages Function en
   zokascan.cc, para que la PC no sirva cada página de escaneo.

## Bitácora

- 30/10: se crean `AGENTS.md` y este archivo.
