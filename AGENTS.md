# Para agentes (Codex, Claude u otros) que retoman ZOKA

Leé, en este orden:

1. [docs/TRASPASO.md](docs/TRASPASO.md): qué se está haciendo **ahora**, en
   qué paso quedó y cómo seguir. Lo actualiza cada agente a medida que avanza.
2. [HOJA-DE-RUTA-CLAUDE.md](HOJA-DE-RUTA-CLAUDE.md): la historia completa,
   con las decisiones del usuario y lo publicado.

## Reglas que no se negocian

- Todo lo que lee el usuario va en castellano rioplatense (vos, tocá,
  mandame). El código y sus comentarios siguen el estilo de cada archivo.
- **Publicar pide el OK explícito del usuario**: releases en GitHub, deploys de
  zoka.network y zokascan.cc, anuncios. Commits y push de ramas sí, sin
  preguntar.
- Nunca: `push --force`, `reset --hard`, `rm -rf`, `git clean`. No se reescribe
  historia publicada.
- No leer claves: ni las del portapapeles, ni las de autoridad de checkpoints,
  ni las wallets del usuario. Los envíos de prueba los hace el usuario.
- No sugerir abrir puertos en el router: el usuario no puede.
- No tocar estos archivos sin seguimiento en git: `Zoka Network Web Applications.docx`,
  `apelacion-github.md`, `discord-bot/`, `zoka-guard-vault/`.
- Los procesos que tienen que seguir corriendo (ZSilent, XMRig, el nodo del
  explorador, el túnel) se abren con `explorer.exe`, nunca como hijos de la
  sesión del agente: mueren con ella.

## Repositorios (cada uno con remotos `origin` = GitHub y `gitlab`)

| Carpeta | Qué es | Rama de trabajo |
| --- | --- | --- |
| `.` (ZOKA) | nodo `zoka`, pool STARK, minero CLI | `main` |
| `zsilent-core/` | ZSilent, la billetera de escritorio | `quantic-v2` (= `main`) |
| `zokaweb/` | zoka.network | `main` |
| `zokaexplorer/` | zokascan.cc | `explorer-quantic-v2` (= `main`) |
| `zkapriv/` | ZKAPriv, la billetera del teléfono | ver su propio repo |

GitLab se empuja por SSH:
`GIT_SSH_COMMAND="ssh -i ~/.ssh/gitlab_zoka -o IdentitiesOnly=yes" git push gitlab <rama>`.
