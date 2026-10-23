#!/usr/bin/env bash
# Arma el minero CLI de Zoka Quantic para Linux:
# <salida>/zoka-cli-miner-<versión>-linux-x64.tar.gz
#
# Como build-cli-miner.ps1 (el de Windows): el binario tiene que ser la versión
# de Cargo.toml y traer Zoka Quantic; además no puede pedir una glibc más nueva
# que la 2.31 (ver build-linux-bullseye.sh), así anda de Ubuntu 20.04 y Debian 11
# en adelante. Tor no va adentro: en Linux se usa el de la distribución. Adentro
# va un SHA256SUMS de todos los archivos, y al lado del archivo, el hash del
# archivo. No publica nada.
#
# Uso: build-cli-miner-linux.sh <carpeta del repo> <binario zoka> <carpeta de salida>
set -euo pipefail

REPO="$(cd "$1" && pwd)"
ZOKA="$2"
OUT="$3"
MAX_GLIBC="2.31"

version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$REPO/Cargo.toml" | head -1)"
[ -n "$version" ] || { echo "Could not read the version from Cargo.toml" >&2; exit 1; }

reported="$("$ZOKA" --version)"
[ "$reported" = "zoka $version" ] || { echo "The binary says '$reported', the source is $version" >&2; exit 1; }

probe="$(mktemp -d)"
ZOKA_HOME="$probe" "$ZOKA" quantic-wallet list --network quantic-testnet --json | grep -q '"wallets"' \
    || { echo "The binary was built without Zoka Quantic" >&2; exit 1; }
rmdir "$probe" 2> /dev/null || true

newest="$(objdump -T "$ZOKA" | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -u -V | tail -1)"
if [ "$(printf '%s\n%s\n' "$newest" "$MAX_GLIBC" | sort -V | tail -1)" != "$MAX_GLIBC" ]; then
    echo "The binary needs glibc $newest; build it on Debian 11 (scripts/build-linux-bullseye.sh)" >&2
    exit 1
fi

name="zoka-cli-miner-$version-linux-x64"
pkg="$OUT/$name"
[ ! -e "$pkg" ] || { echo "$pkg already exists. Move it away first; this script does not delete anything." >&2; exit 1; }
mkdir -p "$pkg"
install -m 755 "$ZOKA" "$pkg/zoka"
tr -d '\r' < "$REPO/packaging/cli-miner/linux/zoka-miner.sh" > "$pkg/zoka-miner.sh"
chmod 755 "$pkg/zoka-miner.sh"
tr -d '\r' < "$REPO/packaging/cli-miner/README.md" > "$pkg/README.md"
tr -d '\r' < "$REPO/packaging/cli-miner/LEEME.md" > "$pkg/LEEME.md"

(cd "$pkg" && find . -type f ! -name SHA256SUMS.txt | sed 's|^\./||' | sort | xargs sha256sum > SHA256SUMS.txt)
tar -C "$OUT" --owner=0 --group=0 -czf "$OUT/$name.tar.gz" "$name"
(cd "$OUT" && sha256sum "$name.tar.gz" >> SHA256SUMS.txt)

echo "Prepared (not published): $OUT/$name.tar.gz"
echo "SHA-256: $(sha256sum "$OUT/$name.tar.gz" | cut -d' ' -f1)"
echo "Needs glibc $newest or newer."
