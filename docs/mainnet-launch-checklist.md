# Mainnet Launch Checklist — 30 June 2026

Genesis timestamp: `1782777600` → 2026-06-30 00:00:00 UTC

---

## T-24h (29 de junio)

- [ ] Verificar que ZSilent v1.1.5 está instalado y funcionando
- [ ] Confirmar que el trusted-setup release está disponible en GitHub
- [ ] Avisar a los mineros externos: habrá reset a las 00:00 UTC del 30
- [ ] Asegurarse de que BOOT y VPS2 tienen espacio en disco (>5 GB libres)

---

## T-0: Genesis Reset (00:00 UTC, 30 junio)

**Orden de operaciones — seguir en secuencia:**

### 1. Detener minería local
- Cerrar ZSilent o pausar minería desde la UI antes de iniciar el reset

### 2. Reset BOOT (nodo primario)
- Ir a: `https://github.com/ZokaNetwork/ZokaNetwork/actions/workflows/reset-genesis.yml`
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

### 5. Iniciar minería desde PC (ZSilent)
- Abrir ZSilent → el nodo local sincronizará desde BOOT automáticamente
- Activar minería cuando el nodo local esté sincronizado (altura = altura BOOT)
- El wallet local empezará desde cero (balance fantasma desaparecido)

### 6. Verificar primer bloque
- Confirmar que se mina el bloque #1 en la nueva cadena
- Verificar que aparece en ZokaScan: `https://www.zokascan.cc`

---

## T+1h (post-lanzamiento)

- [ ] Monitorear que la dificultad sube correctamente con los mineros activos
- [ ] Confirmar que VPS2 está sincronizado con BOOT (sin fork)
- [ ] Verificar que el explorador (ZokaScan) muestra bloques nuevos
- [ ] Verificar que ZSilent muestra balance inmaduro creciendo

---

## T+24h

- [ ] Revisar logs en BOOT y VPS2 (sin errores críticos)
- [ ] Confirmar que los coinbases del día 1 están madurando (bloque 60+)
- [ ] Revisar uso de disco en nodos DO:
  ```
  df -h /   # BOOT y VPS2
  ```

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

## Contactos / recursos

- Repo: `https://github.com/ZokaNetwork/ZokaNetwork`
- Trusted setup: `https://github.com/ZokaNetwork/ZokaNetwork/releases/tag/trusted-setup-mainnet-v1`
- Explorador: `https://www.zokascan.cc`
- Reset workflow: `https://github.com/ZokaNetwork/ZokaNetwork/actions/workflows/reset-genesis.yml`
- Deploy workflow: `https://github.com/ZokaNetwork/ZokaNetwork/actions/workflows/deploy-mainnet.yml`
