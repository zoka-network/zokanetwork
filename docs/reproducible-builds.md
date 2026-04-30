# ZOKA Reproducible Builds (v1)

Objetivo: cualquier tercero puede tomar el commit publicado de un release,
ejecutar el comando documentado, y obtener un binario byte-a-byte identico
al que el equipo core publica. Sin reproducibilidad, "firmamos los binarios"
no significa nada — solo prueba que el firmante tenia acceso a la clave, no
que el binario corresponde al source.

## Build matrix

Mainnet v1 publica binarios para dos targets:

| Target               | Triple                       | Binario              |
|----------------------|------------------------------|----------------------|
| Linux x86_64         | `x86_64-unknown-linux-gnu`   | `zoka` + `zokahd`    |
| Windows x86_64       | `x86_64-pc-windows-msvc`     | `zoka.exe` + `zokahd.exe` |

Otros targets (macOS, ARM64) se publicaran cuando haya pipeline de release
estable; mientras tanto, los usuarios de esos sistemas compilan desde source.

## Toolchain pin

Cada release publica:

1. **Commit hash** del repositorio (`git rev-parse HEAD`).
2. **Rust toolchain version** (ej. `rustc 1.87.0 (stable)`).
3. **Cargo.lock** EXACTO usado para generar el binario.
4. **Lista completa de features** activadas. Para mainnet:
   `cargo build --release --features randomx`.

El `rust-toolchain.toml` en raiz fija la version de toolchain. Cualquiera que
clone el repo con rustup obtiene exactamente la misma version.

## Comando canonico de build

### Linux x86_64

```sh
git checkout <release-tag>
git submodule update --init --recursive
cargo --version  # debe coincidir con la version publicada del release
powershell -ExecutionPolicy Bypass -File scripts/check_randomx_toolchain.ps1 -Strict
cargo build --release --features randomx --target x86_64-unknown-linux-gnu
```

Binarios resultantes en:

- `target/x86_64-unknown-linux-gnu/release/zoka`
- `target/x86_64-unknown-linux-gnu/release/zokahd`

### Windows x86_64

```powershell
git checkout <release-tag>
git submodule update --init --recursive
cargo --version
powershell -ExecutionPolicy Bypass -File scripts/check_randomx_toolchain.ps1 -Strict
cargo build --release --features randomx --target x86_64-pc-windows-msvc
```

Binarios resultantes en:

- `target\x86_64-pc-windows-msvc\release\zoka.exe`
- `target\x86_64-pc-windows-msvc\release\zokahd.exe`

## Fuentes de no-determinismo conocidas

Estas son las cosas que pueden hacer que dos compilaciones del mismo source
generen binarios distintos. El equipo de release controla todas:

- **Timestamps embebidos**: `cargo` por default no incrusta timestamps. OK.
- **Embedded build metadata** (`CARGO_BUILD_RUSTC_WRAPPER`, `CARGO_PKG_*`):
  los `version` y `name` vienen del `Cargo.toml`. OK.
- **Random debug info paths**: en `dev` profile el path absoluto se incrusta
  en `--debuginfo`. En `release` no aplica (sin debug info).
- **Linker order**: GNU ld puede reorganizar simbolos. Para release usamos
  `-Clinker-plugin-lto=off` y un linker fijo (gnu ld 2.40+ en Linux,
  MSVC linker 14.39+ en Windows).
- **Submodules** (`randomx-rs` -> `RandomX` C++): el submodule esta pinned a
  un commit hash en `Cargo.lock`. Re-clonarlo da los mismos bytes.

## Re-build del verifier

Un tercero verifica el binario asi:

```sh
git clone https://github.com/<org>/zoka
cd zoka
git checkout <release-tag>
git submodule update --init --recursive
cargo build --release --features randomx --target x86_64-unknown-linux-gnu

sha256sum target/x86_64-unknown-linux-gnu/release/zoka
# Comparar contra el hash publicado en docs/release-verification.md
```

Si el hash difiere:

1. Confirmar `rustc --version` exacto, mismo OS major.
2. Si todavia difiere, abrir issue en GitHub. El equipo de release publica
   el ambiente exacto (Docker image / `Dockerfile.release`) para casos
   edge.

## Release packaging

El empaquetado oficial usa:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/package_release.ps1 `
  -Version v1.0.0-rc1 `
  -Targets x86_64-pc-windows-msvc,x86_64-unknown-linux-gnu `
  -Features randomx
```

El script:

- construye `zoka` y `zokahd`;
- crea un paquete por target;
- copia la documentacion minima de verificacion/usuario/red;
- genera `RELEASE-MANIFEST.txt` con commit, toolchain, features y comando;
- genera `SHA256SUMS.txt` para todos los paquetes.

El archivo `SHA256SUMS.txt` se firma fuera del script con:

```sh
gpg --detach-sign --armor SHA256SUMS.txt
```

## Diferencias justificadas (cuando aplique)

Si una compilacion produce un binario distinto por una razon documentada
(ej. ABI de glibc 2.41 vs 2.40), se documenta en
[`docs/release-verification.md`](release-verification.md) bajo
"Known reproducibility caveats". Toda diferencia no documentada es bug.

## Roadmap

- [x] v1: build determinista en mismo toolchain + mismo OS major.
- [ ] v2: docker-based build script publicado en `Dockerfile.release` para
      eliminar dependencia del entorno del verifier.
- [ ] v3: incluir el hash del binario como parte del firmado al release
      (concept de "binary attestation").

Quien firma esto: equipo core de ZOKA. Cualquier cambio en este pipeline
requiere bumpear la version de release y publicar nueva firma.
