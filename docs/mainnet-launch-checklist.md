# Mainnet Launch Checklist — Zenith

**Reset de génesis**: 29 de junio 2026, 15:00 UTC (= **30 de junio, 00:00 JST**, hora anunciada públicamente)
**Apertura oficial / anuncio**: 30 de junio 2026

Genesis timestamp: `1782745200` → 2026-06-29 15:00:00 UTC (= 2026-06-30 00:00:00 JST)

> ⚠️ Importante: el reset real ocurre el **29** en hora UTC. El tweet de anuncio usa JST como referencia ("00:00 JST, 30 de junio"), que cae en 29 de junio 15:00 UTC. Esto es intencional — da un margen de varias horas entre el reset (silencioso) y la apertura pública del 30, para detectar y resolver cualquier problema de sincronización antes de que la red esté siendo usada públicamente.

---

## T-24h (28 de junio)

- [ ] Confirmar que ZSilent (última versión estable) está listo para distribuir
- [ ] Confirmar que el trusted-setup release está disponible en GitHub
- [ ] Asegurarse de que BOOT y VPS2 tienen espacio en disco (>5 GB libres)
- [ ] Revisar que no quede ningún proceso de minería de prueba corriendo en BOOT/VPS2 (ninguno de los dos debe minar)

---

## T-0: Genesis Reset — 29 junio, 15:00 UTC (30 junio 00:00 JST)

**Orden de operaciones — seguir en secuencia:**

### 1. Detener minería local
- Cerrar ZSilent o pausar minería desde la UI antes de iniciar el reset

### 2. Reset BOOT (nodo primario)
- Ir a: `https://github.com/zoka-network/zokanetwork/actions/workflows/reset-genesis.yml`
- Parámetros: `target = bootstrap`, `confirm = RESET`
- Esperar que el workflow complete (~60s)
- Verificar: `curl http://159.223.119.216:3000/chain/height` → debe devolver `{"height":0}` o `{"height":1}`

### 3. Reset VPS2 (nodo secundario)
- Mismo workflow: `target = rpc`, `confirm = RESET`
- Esperar que complete
- Verificar: `curl http://157.230.5.103:3000/chain/height` → altura 0 o 1

### 4. Confirmar sincronización entre nodos
- Esperar ~2 minutos para que VPS2 encuentre a BOOT por P2P
- Verificar que ambos nodos tienen la misma altura:
  ```
  curl http://159.223.119.216:3000/chain/height
  curl http://157.230.5.103:3000/chain/height
  ```

### 5. Iniciar minería desde PC (ZSilent) — modo silencioso

- Abrir ZSilent → el nodo local sincronizará desde BOOT automáticamente
- Activar minería propia para validar que la red funciona de punta a punta
- **No anunciar ni invitar mineros externos todavía** — esta es la ventana de verificación privada

### 6. Verificar primer bloque
- Confirmar que se mina el bloque #1 en la nueva cadena
- Verificar que aparece en ZokaScan: `https://www.zokascan.cc`

---

## T+1h a T+varias horas (ventana de verificación, 29 jun 15:00 UTC → 30 jun)

- [ ] Monitorear que la dificultad sube correctamente con los mineros activos
- [ ] Confirmar que VPS2 está sincronizado con BOOT (sin fork)
- [ ] Verificar que el explorador (ZokaScan) muestra bloques nuevos
- [ ] Verificar que ZSilent muestra balance inmaduro creciendo
- [ ] Revisar logs en BOOT y VPS2 (sin errores críticos)
- [ ] Confirmar que los coinbases del día 1 están madurando (bloque 60+)
- [ ] Si todo está estable: preparar el anuncio/apertura pública para el 30

---

## T-Apertura pública (30 de junio)

- [ ] Publicar el anuncio (tweet ya redactado: "Mainnet Zenith goes live...")
- [ ] Compartir enlaces de descarga de ZSilent y guía de onboarding para mineros externos
- [ ] Monitorear salud de la red con más mineros conectándose (dificultad, peers, disco)

---

## T+24h desde apertura

- [ ] Revisar uso de disco en nodos DO:
  ```
  df -h /   # BOOT y VPS2
  ```
- [ ] Confirmar que la red soporta el aumento de mineros externos sin saturarse

---

## Comandos rápidos de diagnóstico

```bash
# Altura de ambos nodos
curl -s http://159.223.119.216:3000/chain/height
curl -s http://157.230.5.103:3000/chain/height

# Bloque tip (dificultad actual)
HEIGHT=$(curl -s http://159.223.119.216:3000/chain/height | jq .height)
curl -s http://159.223.119.216:3000/blocks/$HEIGHT | jq '{height: .block_height, difficulty: .difficulty_bits, hash: .hash[:16]}'

# Logs en tiempo real (vía SSH)
# ssh root@159.223.119.216 "journalctl -u zoka-bootstrap -f"
# ssh root@157.230.5.103 "journalctl -u zoka-rpc -f"
```

---

## Plan de testnet (después del lanzamiento de mainnet)

- La red actual (con balances de prueba) **no se convierte automáticamente en testnet** — queda como cadena de desarrollo hasta que se decida formalizarla.
- Unos días después del lanzamiento de mainnet, construir la testnet oficial con su propio génesis distinto (`chain_id` testnet, ya existe el perfil en `economics.testnet.v1.json` / `genesis.testnet.v1.json`).
- Definir si la testnet corre en infraestructura separada (droplets nuevos) o comparte droplet con la mainnet en rutas/puertos distintos.

---

## Contactos / recursos

- Repo: `https://github.com/zoka-network/zokanetwork`
- Trusted setup: `https://github.com/zoka-network/zokanetwork/releases/tag/trusted-setup-mainnet-v1`
- Explorador: `https://www.zokascan.cc`
- Reset workflow: `https://github.com/zoka-network/zokanetwork/actions/workflows/reset-genesis.yml`
- Deploy workflow: `https://github.com/zoka-network/zokanetwork/actions/workflows/deploy-mainnet.yml`
