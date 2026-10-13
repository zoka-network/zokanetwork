# Versión 2.1.0 de testnet (Zenith): procedimiento

Preparado el 13/10/2026. Esta versión activa en Zenith lo que ya está en código y
probado en la red local. **Nada de esto está publicado.** Las alturas de
activación se fijan el día en que se compila para publicar, con la punta de ese
día; por eso este documento es un procedimiento y no un parche aplicado.

## Qué lleva

Ramas apiladas, cada una sobre la anterior; la última contiene todo:

| Rama (nodo) | Contenido |
|---|---|
| `postcuantica-integracion-nodo` … `quantic-interfaz` | Base postcuántica detrás de `pq-experimental`, exchange retirado, `--sync-only` arreglado, Zenith presentada como testnet con la marca Zoka Quantic |
| `etapa3-direcciones-hibridas` | Salidas v2 y direcciones `zkq1` (X-Wing), comisión del minero en sobre v2, paquete de VPS |
| `checkpoints-autoritativos` | La PC del equipo firma checkpoints que los nodos obedecen; decisiones del 13/10 |
| `arreglo-notas-gastadas` | Ninguna copia vieja de wallets vuelve a borrar la reserva de un envío (saldo inflado de ZSilent) |
| `checkpoints-correccion-manual` | `checkpoint-sign` y regla que impide volver a firmar la rama corregida; versiones alineadas |

ZSilent y ZKAPriv: ramas `etapa3-apps-zkq1` (muestran y envían a la `zkq1` solo
cuando la red fija la altura).

## Decisiones de esta versión

- **El identificador de protocolo (`PROTOCOL_FREEZE_ID`) no cambia.** Cambiarlo
  desconecta en el acto a todo nodo 2.0.x, una semana antes de la activación. Los
  que no actualicen quedan afuera recién en la altura de activación, y los
  actualizados siguen los checkpoints firmados.
- **El nodo que va en ZSilent y el independiente se compilan con
  `pq-experimental`**: sin la feature no se puede recibir ni enviar a una `zkq1`.
  Las reglas de consenso de la v2 están en todas las builds, con o sin ella.
- **Alturas:** bundles canónicos en la punta + 5.040 bloques (una semana al
  objetivo de 120 s; recalcular con el ritmo medido ese día) y salidas v2 720
  bloques después. Criterio en [DECISIONES-2026-10-13.md](DECISIONES-2026-10-13.md).

## Pasos

1. **Integrar.** Crear `release-2.1.0` desde `checkpoints-correccion-manual`. No
   tocar `main` sin aprobación del usuario.
2. **Versiones** (todas a la vez):
   - `Cargo.toml` del nodo: `version = "2.1.0"` (y `Cargo.lock`).
   - `src/api/mod.rs`: `NETWORK_MIN_CLIENT_VERSION = "2.1.0"`, y
     `minimum_client_version` en `docs/mainnet-network.v1.json` igual. La prueba
     `tests/mainnet_recovery_policy.rs` exige que coincidan.
   - ZSilent: `releaseVersion` en `zsilent-core/build.gradle.kts`.
   - ZKAPriv: `versionName = "1.1.0"` y `versionCode` + 1 en
     `zkapriv/app/build.gradle.kts`.
3. **Alturas.** Leer la punta del nodo de la PC (`GET /chain/height`) y escribir
   en `src/economics.rs`:
   - `PRIVATE_TX_CANONICAL_BUNDLE_ACTIVATION_HEIGHT` (la variante sin `mobile`)
     `= Some(punta + 5_040)`;
   - `PQ_HYBRID_ADDRESSES_ACTIVATION_HEIGHT = Some(punta + 5_760)`.
   La variante `mobile` de los bundles queda en `Some(0)`. Las pruebas que
   suponen alturas nulas (`pq_features_stay_dormant_until_a_height_is_fixed` y
   las de economía) se actualizan para fijar las nuevas.
4. **Probar.** `cargo test --release` completo, con y sin `pq-experimental`;
   Gradle de ZSilent y de ZKAPriv.
5. **Compilar.**
   - Nodo: `cargo build --release --features pq-experimental`.
   - Motor móvil: `native/build-android.ps1` (los dos ABIs; toma las alturas del
     nodo) y APK firmado.
   - ZSilent con ese `zoka.exe` adentro.
6. **Verificar hashes** (incidente de la v1.5.0: el instalador llevaba otro
   binario): el `zoka.exe` compilado tiene que ser el mismo que va dentro del
   instalador, y el `.so`, el que va dentro del APK.
7. **Instalar primero en la PC del usuario** (es la autoridad de checkpoints y
   la seed) y en el teléfono, con su autorización, y ver un bloque nuevo y un
   checkpoint firmado (`zoka checkpoint-status`).
8. **Publicar** en `github.com/zoka-network` (Actions está apagado: subir los
   artefactos a mano con sus SHA-256). Ahí no hay ningún release todavía: sumar
   el `trusted-setup-mainnet-v1` que enlaza la guía de mineros, con los cinco
   archivos de `trusted-setup/` (DIGEST `219fdb5d…0d8e`, verificado el 13/10
   contra los parámetros) y sin marcarlo como el último. Después, **anunciar** con el borrador
   [ANUNCIO-zenith-testnet-BORRADOR.md](ANUNCIO-zenith-testnet-BORRADOR.md),
   completando la altura y la fecha. Las dos cosas, solo con aprobación del
   usuario.
9. **Después de la activación:** envíos reales ZSilent ⇄ ZKAPriv a direcciones
   `zkq1`, recuperación desde la frase y resincronización.

## Volver atrás

Antes de la altura de activación, volver es reinstalar el binario anterior
(los datos no se mueven). Después, un nodo 2.0.x ya no sigue la cadena de los
2.1.0: volver atrás exigiría otra versión que posponga las alturas.
