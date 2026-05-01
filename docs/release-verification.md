# ZOKA Release Verification

Antes de correr cualquier binario `zoka` o `zokahd` etiquetado como release de
mainnet, verifica que viene del equipo core. Este doc es el procedimiento
canonico. Tres pasos: descargar, verificar checksums, verificar firma.

## 1. Descargar artefactos

Cada release publica en GitHub Releases (o el mirror oficial):

```
zoka-<version>-x86_64-unknown-linux-gnu.tar.gz
zoka-<version>-x86_64-pc-windows-msvc.zip
SHA256SUMS.txt
SHA256SUMS.txt.asc
```

Tambien la URL de los artefactos se publica en el mismo anuncio que el
genesis canonico (`docs/genesis.mainnet.v1.json`).

El paquete oficial se genera con:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/package_release.ps1 `
  -Version v1.0.0-rc1 `
  -Targets x86_64-pc-windows-msvc,x86_64-unknown-linux-gnu `
  -Features randomx
```

Cada archivo comprimido incluye `RELEASE-MANIFEST.txt` con commit, toolchain,
features, comando de build y `PROTOCOL_FREEZE_ID`.

## 2. Verificar checksums

### Linux / macOS

```sh
sha256sum -c SHA256SUMS.txt
```

Salida esperada: `OK` por cada archivo. Cualquier `FAILED` significa que el
archivo fue modificado o esta corrupto. **No correr el binario.**

### Windows (PowerShell)

```powershell
Get-Content .\SHA256SUMS.txt | ForEach-Object {
    $parts = $_ -split '\s+', 2
    if ($parts.Count -lt 2) { return }
    $expected = $parts[0]
    $name = $parts[1].Trim()
    $actual = (Get-FileHash -Algorithm SHA256 -Path $name).Hash.ToLower()
    if ($actual -eq $expected) {
        Write-Output "$name`: OK"
    } else {
        Write-Output "$name`: FAILED (expected $expected, got $actual)"
    }
}
```

## 3. Verificar firma GPG

```sh
gpg --import zoka-release-signing.asc   # primera vez solamente
gpg --verify SHA256SUMS.txt.asc SHA256SUMS.txt
```

Salida esperada: `Good signature from "ZOKA Release <release@zoka.local>"` con
fingerprint igual al publicado mas abajo.

### Fingerprint de la clave de release


```
B9B8 0871 6252 5D33 771F 0562 2834 2DF9 834A E872
```

Si tu `gpg --verify` imprime un fingerprint distinto, **no confiar**: ni en
el binario, ni en `SHA256SUMS.txt`. Reportar a `security@zoka...`.

### Custodia de la clave de release

La clave publica oficial esta versionada en `docs/zoka-release-signing.asc`.
La clave privada y su passphrase no se versionan: en esta maquina quedaron
en `.zoka/gnupg-official-release` y
`.zoka/gnupg-official-release-passphrase.txt`, ambos ignorados por Git.
Antes de reinstalar Windows, limpiar `.zoka/`, mover el repo o publicar un
release final, respaldar esos secretos en almacenamiento seguro offline.

La regla de oro: el fingerprint documentado debe coincidir exactamente con
la clave que firma `SHA256SUMS.txt`. Si no coincide, no confiar en el
release.

## 4. Reproducir el build (opcional)

Para anclar la confianza al source code (no a la firma de quien construyo el
binario), seguir [`reproducible-builds.md`](reproducible-builds.md):

```sh
git clone https://github.com/<org>/zoka
cd zoka
git checkout <release-tag>
cargo build --release --features randomx
sha256sum target/release/zoka  # debe coincidir con SHA256SUMS.txt
```

Para reproducir el mismo layout de paquete:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/package_release.ps1 `
  -Version <release-tag> `
  -Targets native `
  -Features randomx
```

## Known reproducibility caveats

Diferencias documentadas entre builds en distintos entornos. Si tu
re-compilacion produce un hash distinto y la diferencia NO esta listada
aqui, abrir issue en GitHub:

> v1: ninguno conocido todavia. Documentar aqui en cuanto aparezca el
> primer caso real.

## Genesis verification (paso adicional para nodos mainnet)

Tras instalar el binario, antes de arrancar `zokahd` contra mainnet:

```sh
zoka genesis-verify docs/genesis.mainnet.v1.json
```

Debe imprimir el `SHA-256 digest` que coincide con el digest publicado en
[`docs/genesis.mainnet.v1.spec.md`](genesis.mainnet.v1.spec.md). Si no
coincide, NO arrancar el nodo â€” el genesis fue alterado.

## Checklist pre-launch

Antes de correr `zokahd --network mainnet` por primera vez:

- [ ] Binario descargado.
- [ ] `sha256sum -c SHA256SUMS.txt` => OK.
- [ ] `gpg --verify SHA256SUMS.txt.asc SHA256SUMS.txt` => Good signature.
- [ ] Fingerprint coincide con el publicado por el equipo core.
- [ ] `zoka genesis-verify ./docs/genesis.mainnet.v1.json` => digest
      coincide con anuncio oficial.
- [ ] Si vas a usar fast sync: `zoka snapshot-import <file>
      --public-key <pubkey>` => verifica firma y replaya cada bloque.

