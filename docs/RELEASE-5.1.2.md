# ZSilent 5.1.2 y la auditoría del 27/11/2026

El 27/11 el usuario vio que la sección Network de ZokaScan seguía diciendo
«Testnet v3» y pidió una auditoría de ZokaScan, zoka.network, la billetera
web, ZSilent y ZKAPriv. La cadena sigue siendo la v3 (se actualiza, no se
reinicia), pero las páginas no mostraban lo que cambió desde el 12/11, y
quedaban textos de la v2, de la v1 y de Zenith. Plan aprobado:
`~/.claude/plans/hashed-roaming-gray-of-a736f0d3-cb5b-46ab-8f93-fe5c10780c5d.md`.

ZSilent 5.1.2 lleva el **mismo nodo 5.1.1**. No es actualización de red.

**Publicado el 27/11 con el OK del usuario:**

- Release `v5.1.2` de zsilent-core como Latest. La bajé de vuelta y el hash y la firma dan OK. La 5.1.1 quedó marcada «Superseded».
- Etiqueta `v5.1.2` = `be923da`.
- `main` en GitHub y GitLab en los seis repos.
- Desplegados zoka.network (`9d32e4c`, con ZSilent 5.1.2 y los papers 2.7), ZokaScan (`5c5bd01`) y la billetera web (`19e1a46`). Los bundles publicados son iguales a los `dist`.
- La política que abre ZKAPriv 4.0.2 ya es la del 27/11.
- Aviso de Discord enviado el 27/11 (`discord-bot/notice.release-5.1.1.json`): cuenta la 5.1.0, cuyo aviso nunca había salido, y la 5.1.1, con ZSilent 5.1.2 en las descargas. Comprobado con `check-notice.js`.
- ZSilent 5.1.2 instalada en la PC el 27/11 (`msiexec /passive`, salida 0, binarios iguales a la release).

## Ramas `auditoria-27-11`

| Repo | Commits | Qué |
| --- | --- | --- |
| zokaexplorer | `83afa72`, `5c5bd01` | Network sin número en el título, filas de la prueba v4, la comisión y los relays; checkpoint de 2 de 3 claves |
| zokaweb | `c1b8fa5`, `013a974` | Tech (peso de la v4), `release verify`, direcciones de exchange, carta 06, repos en GitHub; papers 2.7 |
| zokawallet | `19e1a46` | Versión 0.4.0, checkpoint de 2 de 3, README v3 |
| zsilent-core | `be923da` (worktree `C:\tmp\zoka-wt\zsilent-sin-cola`) | 5.1.2: privacidad con relays propios y Modo Tor, solo 24 palabras, sin «v2» |
| zkapriv | `28bd720` | README de la 4.0.2, política con relays y exchanges, enlace de Privacy a `main` |
| zkapriv `zkapriv-v2` | `f59341b` | La política al día en la rama que abre la app instalada |
| ZOKA | `80653328` y este archivo | README de la 5.1.1, notas de histórico en el GitBook y en `cli-node-over-tor.md` |

## Archivos de ZSilent 5.1.2 (`target/release-5.1.2/zsilent`)

| Archivo | SHA-256 |
| --- | --- |
| `ZSilent-Core-5.1.2.msi` (Tor 0.4.9.14) | `2ef0cc01b619b17186122414108f61babcb38e431c0e16699ee911455d51d8c5` |
| `zoka.exe` (dentro del MSI) | `4e16ab553d3be64b04edb844aa5576d864b738c60ce6406bfb1ff85fe2bebced` |
| `zokahd.exe` (dentro del MSI) | `af7fb383ed39d926397b6cac26386c916e4a402990db29a5f1e229ac9ac7e80a` |

`SHA256SUMS.txt` firmado (`SHA256SUMS.txt.sig`, clave `61c2afa5…`) y
verificado con `zoka release verify`. El MSI abierto por extracción
administrativa trae esos dos binarios, `tor.exe` 0.4.9.14 y el jar
`zsilent-core-5.1.2` con los textos nuevos (y sin los viejos); la app lee
su versión de `-Dzsilent.version=5.1.2`.

## Verificación

- zokaexplorer: vitest 18 y tsc; zokaweb: vitest 17 y tsc; zokawallet: vitest
  40 (1 omitida), tsc y build. Vistas locales capturadas con Chrome sin
  ventana (Network, inicio, GitHub, pie de la billetera).
- ZSilent: `gradlew test`, 96 sin fallas.
- Papers: los dos PDF (19 y 6 páginas) con el texto verificado por el armado;
  las frases nuevas están y las viejas (1,9 MB, 8,7 MB, 0,0001 ZKA) no.

## Pasos para publicar (con el OK del usuario)

1. ZSilent: `auditoria-27-11` a `main`, etiqueta `v5.1.2`, push a GitHub y a
   GitLab (443). Release `v5.1.2` con el MSI, `SHA256SUMS.txt`, `.sig` y
   `release-manifest.json`, Latest; la 5.1.1 pasa a «Superseded».
2. zoka.network: `ZSILENT_VERSION` a 5.1.2 en `links.ts`, después de que la
   release exista; armar y desplegar. ZokaScan y la billetera web: desplegar.
3. README y documentos: `main` de ZOKA, zkapriv (y `zkapriv-v2`), zokawallet,
   zokaexplorer y zokaweb, en GitHub y GitLab.
4. Comprobar: descargas con hash y firma, bundles publicados iguales a los
   `dist`, la política que abre ZKAPriv 4.0.2.
5. Instalar ZSilent 5.1.2 en la PC (con ZSilent cerrada) en lugar de la 5.1.1.
   Hecho el 27/11.

## Notas para GitHub

### zsilent-core `v5.1.2`: «ZSilent 5.1.2»

ZSilent 5.1.2 para **Zoka Quantic Testnet v3**, con el mismo nodo Zoka 5.1.1. No es una actualización de red: tu billetera, tus direcciones y tu saldo quedan como están.

- **Privacidad:** la pantalla muestra los relays de Zoka (relay1 y relay2.zoka.network) junto a los de n0, la búsqueda de direcciones de n0, y cambia con el Modo Tor: ruta, relays y quién ve tu IP. Antes decía que el relay era n0 y que no había Tor.
- **Restaurar:** pide las 24 palabras, lo único que acepta Zoka Quantic. La opción de 12 palabras, que siempre fallaba, ya no aparece; tampoco al crear otra billetera.
- **Textos al día:** ya no habla del «nodo v2» ni de la «testnet v2».

Cerrá ZSilent antes de instalar `ZSilent-Core-5.1.2.msi`. Se instala encima de la 5.1.1 o la 5.1.0. Verificá su SHA-256 con `SHA256SUMS.txt` y su firma `SHA256SUMS.txt.sig` (docs/release-verification.md del repo del nodo; huella de la clave `61c2afa5 67d79ad5 c43a89c5 82656f55 ffc888a5 4ac06ddc 8445dece 63ed5c99`); `release-manifest.json` identifica los binarios y el manifiesto de red incluidos.

```text
2ef0cc01b619b17186122414108f61babcb38e431c0e16699ee911455d51d8c5  ZSilent-Core-5.1.2.msi
```

Incluye `zoka.exe` `4e16ab553d3be64b04edb844aa5576d864b738c60ce6406bfb1ff85fe2bebced` y `zokahd.exe` `af7fb383ed39d926397b6cac26386c916e4a402990db29a5f1e229ac9ac7e80a` (nodo 5.1.1, `6ef0da2d`) y Tor 0.4.9.14. ZSilent compilado de `be923da`.

## Queda para otro día

- `zoka help` de la 5.1.1 muestra los comandos de la testnet v1, y la ayuda de
  `quantic-pq-node` dice «STARK v2 node» y «n0 relays»: necesita una versión
  del nodo.
- ZKAPriv: el enlace de Privacy a `main` sale con la próxima APK; textos
  menores («team's hybrid checkpoint»).
- El repo `Docs` (Technology Brief del 5/11, de la v2), las descripciones
  «About» de audit, media y Docs, el pie de ZokaScan sin la billetera web ni el
  faucet, y los ejemplos de altura 513 en la página API de ZokaScan.
