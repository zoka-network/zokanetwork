# ironwood-bench

Medición sintética del circuito de Ironwood (Orchard corregido, Halo 2) con el
crate `orchard` 0.16.0, para decidir la etapa 5 de Zoka Quantic
([docs/ETAPA5-gasto-sin-key-images.md](../../docs/ETAPA5-gasto-sin-key-images.md)).
No abre wallets ni bases: arma bundles con datos sintéticos, mide armado de
claves, prueba y verificación, e imprime JSON. Resultados del 13/10/2026 en
[docs/benchmarks/2026-10-13-ironwood.json](../../docs/benchmarks/2026-10-13-ironwood.json).

```powershell
cargo run --release --locked --manifest-path crates/ironwood-bench/Cargo.toml -- 3
```

En el teléfono (ARMv7), sin instalar ninguna app:

```powershell
$env:ANDROID_NDK_HOME = "$env:LOCALAPPDATA\Android\Sdk\ndk\27.3.13750724"
cd crates/ironwood-bench
cargo ndk -t armeabi-v7a -P 26 build --release --locked
adb push target/armv7-linux-androideabi/release/ironwood-bench /data/local/tmp/ironwood-bench
adb shell chmod 755 /data/local/tmp/ironwood-bench
adb shell "cd /data/local/tmp && ./ironwood-bench 2"
adb shell rm /data/local/tmp/ironwood-bench
```

El argumento es la cantidad de repeticiones por caso (1 y 2 salidas).
