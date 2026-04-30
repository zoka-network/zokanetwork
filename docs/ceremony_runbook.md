# ZOKA Ceremony Runbook (5 Contributors)

Runbook operativo para ejecutar la ceremonia MPC real de `trusted setup` y cerrar el bloqueante `#5`.

Este flujo usa tooling ya implementado:
- `zoka-trusted-setup`
- `zoka-ceremony-contribute` (`init`, `prepare-adapter`, `finalize-adapter`, `verify`, `readiness`, `publish-bundle`, `verify-bundle`, `release-pin`)
- scripts:
  - `scripts/publish_ceremony_bundle.ps1`
  - `scripts/verify_ceremony_bundle.ps1`
  - `scripts/build_release_with_setup_digest.ps1`
  - `scripts/ceremony_closeout.ps1`
  - `scripts/init_ceremony_round.ps1`
- templates/kit:
  - `docs/ceremony_contributor_kit.md`
  - `docs/ceremony_execution_board.md`
  - `docs/templates/ceremony_round_checklist.md`
  - `docs/templates/ceremony_attestation_template.md`

## 1) Convenciones

Variables sugeridas (PowerShell):

```powershell
$CER = "C:\ceremony\zoka-mainnet-v1"
$CURRENT = "$CER\current"
$TRANSCRIPT = "$CER\transcript.json"
$PUB = "$CER\public_bundle"
```

IDs de contribuidores (ejemplo):
- `contrib-a`
- `contrib-b`
- `contrib-c`
- `contrib-d`
- `contrib-e`

## 2) Coordinador: bootstrap

1. Generar setup base:

```powershell
cargo run --bin zoka-trusted-setup -- --out $CURRENT --force
```

2. Inicializar transcript:

```powershell
cargo run --bin zoka-ceremony-contribute -- init `
  --setup-dir $CURRENT `
  --transcript $TRANSCRIPT `
  --label "zoka-mainnet-v1"
```

3. Verificar estado inicial:

```powershell
cargo run --bin zoka-ceremony-contribute -- verify --transcript $TRANSCRIPT --final-dir $CURRENT
```

## 3) Ciclo por ronda (R1..R5)

Para cada ronda `Rk`, con contribuidor `contrib-x`:

1. Preparar paquete de ronda desde estado actual:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/init_ceremony_round.ps1 `
  -CeremonyDir $CER `
  -CurrentDir $CURRENT `
  -RoundNumber 1 `
  -ContributorId "contrib-a" `
  -Label "zoka-mainnet-v1" `
  -Force
```

Esto genera:
- `$ROUND\input\*.params`
- `$ROUND\output_from_contributor\`
- `$ROUND\CHECKLIST.md`
- `$ROUND\ATTESTATION_contrib-<id>.md`
- `$ROUND\ROUND_SUMMARY.txt`

2. (Alternativa manual) preparar adapter directamente:

```powershell
$ROUND = "$CER\round-R1"
$IN = "$ROUND\input"
$OUT = "$ROUND\output_from_contributor"

cargo run --bin zoka-ceremony-contribute -- prepare-adapter `
  --setup-dir $CURRENT `
  --adapter-dir $ROUND `
  --label "zoka-mainnet-v1-R1" `
  --force
```

2. Compartir con contribuidor:
- Enviar contenido de `$IN\*.params` (o paquete de ronda completo).
- El contribuidor ejecuta su fase2 externa y devuelve `*.params` modificados.
- Guardar archivos devueltos en `$OUT`.

3. Finalizar ronda y anexar transcript:

```powershell
$NEXT = "$CER\current-R1"

cargo run --bin zoka-ceremony-contribute -- finalize-adapter `
  --adapter-dir $ROUND `
  --phase2-out-dir $OUT `
  --out-setup-dir $NEXT `
  --transcript $TRANSCRIPT `
  --contributor "contrib-a" `
  --note "round R1"
```

4. Avanzar puntero:

```powershell
$CURRENT = $NEXT
```

5. Verificar cadena tras cada ronda:

```powershell
cargo run --bin zoka-ceremony-contribute -- verify --transcript $TRANSCRIPT --final-dir $CURRENT
```

Repetir exactamente para R2, R3, R4 y R5 con IDs distintos.

## 4) Gate de readiness (umbral 5)

```powershell
cargo run --bin zoka-ceremony-contribute -- readiness `
  --transcript $TRANSCRIPT `
  --final-dir $CURRENT `
  --min-unique-contributors 5 `
  --out-json "$CER\readiness.json"
```

Debe devolver `ready=true`.

## 5) Publicación verificable

1. Construir paquete público:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/publish_ceremony_bundle.ps1 `
  -Transcript $TRANSCRIPT `
  -FinalDir $CURRENT `
  -OutDir $PUB `
  -MinUniqueContributors 5 `
  -Force
```

2. Verificar paquete publicado:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/verify_ceremony_bundle.ps1 `
  -BundleDir $PUB
```

3. Artefactos que se publican:
- `transcript.json`
- `readiness.json`
- `RELEASE.env`
- `setup/*.params`
- `setup/DIGEST`
- `manifest.json` (hashes sha256 de todos los archivos)

## 6) Pin de digest en release

Con el setup final:

```powershell
cargo run --bin zoka-ceremony-contribute -- release-pin `
  --setup-dir $CURRENT `
  --transcript $TRANSCRIPT `
  --out-env "$CER\release.env"
```

Build de release con digest compilado:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/build_release_with_setup_digest.ps1 `
  -SetupDir $CURRENT `
  -ExpectedDigestFile "$CURRENT\DIGEST"
```

## 7) Checklist de cierre (go/no-go)

- [ ] 5 contribuidores únicos en transcript.
- [ ] `verify --transcript --final-dir` OK.
- [ ] `readiness` con umbral 5 devuelve `ready=true`.
- [ ] `publish-bundle` ejecutado y artefactos publicados.
- [ ] `verify-bundle` OK sobre artefactos publicados.
- [ ] `release-pin` generado contra digest final.
- [ ] build release con digest compilado OK.
- [ ] prueba de arranque release con setup correcto OK.
- [ ] prueba negativa: release rechaza setup con digest distinto.

## 8) Recuperación ante fallos

- Si falla una ronda:
  - No borres transcript.
  - Repetir esa ronda con nuevo `--adapter-dir` y mismo `$CURRENT`.
- Si `finalize-adapter` falla por mismatch:
  - Revisar que `phase2_out_dir` corresponda al `adapter_dir` de esa ronda.
- Si `readiness` falla:
  - Verificar IDs de `--contributor` (sin vacíos/duplicados).
  - Completar rondas faltantes hasta umbral.
