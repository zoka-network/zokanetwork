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
  | XMRig | `target\v2-genesis-2026-10-29\launch\xmrig-equipo.cmd` | su `xmrig.log`, "accepted" |
  | Nodo del explorador | `%APPDATA%\Zoka\.zoka\explorer-node\explorer-node-oculto.vbs` (sin ventana) | `http://127.0.0.1:9181/status` |
  | Túnel de Cloudflare | `%APPDATA%\Zoka\.zoka\cloudflared\tunnel-zokascan-oculto.vbs` (sin ventana) | `https://api.zokascan.cc/status` (error 1033 = túnel caído) |

## Trabajo en curso (pedido del usuario: «avanzá con todo lo que puedas»)

Orden elegido. Lo público (deploy, release, anuncio) se deja listo y se
publica solo con el OK del usuario.

1. [ ] **XMRig se relanza solo.** Se cerró las dos veces que se apagó ZSilent.
2. [ ] **Anuncio para Discord** de la testnet v2, ZokaScan, la clave de vista
   y ZSilent 4.0.4. El texto lo pega el usuario.
3. [ ] **Papers:** la clave de vista se describe como solo la clave X-Wing; la
   `ViewKey` de la 4.0.2 agrega la clave de salida y el dueño. Hay que
   corregirla y rearmar los PDF.
4. [ ] **Segunda semilla:** la red depende de la PC. Guía para que alguien de
   la comunidad corra un nodo permanente, y cómo se suma una semilla al
   manifiesto.
5. [ ] **Modo Tor:** documento de diseño y prototipo en una red de prueba
   local, en una rama aparte.
6. [ ] **Caché de /scan en el borde** (opcional): una Pages Function en
   zokascan.cc, para que la PC no sirva cada página de escaneo.

## Bitácora

- 30/10: se crean `AGENTS.md` y este archivo.
