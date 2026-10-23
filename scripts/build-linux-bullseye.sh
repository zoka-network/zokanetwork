#!/usr/bin/env bash
# Compila el nodo de Zoka para Linux sobre Debian 11 (glibc 2.31), dentro de un
# sistema aislado en /srv/zoka-bullseye (WSL o cualquier Linux, como root).
#
# Por qué: un binario pide la glibc con la que se compiló. Compilado en Debian
# 13 pedía glibc 2.39 (por los __isoc23_* del código C/C++) y no arrancaba en
# Ubuntu 22.04 ni en Debian 12. Con glibc 2.31 anda de Ubuntu 20.04 y Debian 11
# en adelante.
#
# Uso: build-linux-bullseye.sh <carpeta del repo> [versión de Rust]
# Deja el binario en /srv/zoka-bullseye/root/target/release/zoka. No borra nada.
set -euo pipefail

SRC="$(cd "$1" && pwd)"
RUST_VERSION="${2:-1.97.1}"
ROOT=/srv/zoka-bullseye

if [ ! -x "$ROOT/bin/bash" ]; then
    apt-get update -qq
    apt-get install -y -qq debootstrap > /dev/null
    debootstrap --variant=minbase bullseye "$ROOT" http://deb.debian.org/debian \
        || debootstrap --variant=minbase bullseye "$ROOT" http://archive.debian.org/debian
fi

mounted=()
mount_into() {
    mkdir -p "$ROOT$2"
    if ! mountpoint -q "$ROOT$2"; then
        mount --bind "$1" "$ROOT$2"
        mounted+=("$ROOT$2")
    fi
}
cleanup() {
    for m in "${mounted[@]}"; do umount "$m" || true; done
}
trap cleanup EXIT
mount_into /proc /proc
mount_into /sys /sys
mount_into /dev /dev
mount_into "$SRC" /src
cp /etc/resolv.conf "$ROOT/etc/resolv.conf"

in_root() { chroot "$ROOT" /usr/bin/env -i HOME=/root PATH=/root/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin bash -lc "$1"; }

in_root "apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
    build-essential cmake clang libclang-dev pkg-config git curl ca-certificates perl > /dev/null"
if ! in_root "command -v cargo > /dev/null"; then
    in_root "curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain $RUST_VERSION"
fi
in_root "rustc --version && ldd --version | head -1"
in_root "cd /src && CARGO_TARGET_DIR=/root/target cargo build --release --locked --features randomx,quantic --bins"
in_root "/root/target/release/zoka --version"
