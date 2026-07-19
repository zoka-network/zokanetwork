# Hoja de ruta — Explorer estático (sin servidores)

Estado al 2026-07-18. Documento de trabajo: se va tachando a medida que avanza.

---

## Punto de partida

`zokascan.cc` está caído y **lleva roto desde el 9 de julio**, antes del 402 de
Vercel. Su fuente de datos es:

```
zokaexplorer/api/rpc.ts  →  http://159.223.119.216:3000
```

Esa IP es uno de los droplets de DigitalOcean que destruimos. Aunque Vercel
estuviera pago, el explorer no mostraría nada.

Además `api/rpc.ts` es una **función serverless**: cada visita gastaba
invocaciones, y eso fue lo que agotó el plan gratis. Migrar sin cambiar la
arquitectura solo mueve el problema más lejos.

### Lo que ya está resuelto

- El front compila (Vite → `dist/`), no hay que reescribirlo.
- El proyecto `zokaexplorer` ya existe en Cloudflare Pages.
- Supabase es **código muerto**: 0 usos fuera de `src/integrations/supabase/client.ts`.
  Se borra sin tocar nada más.

---

## La restricción que define todo

**Un navegador no puede hablar libp2p sobre Tor ni iroh.** No existe forma de
que una web sea un explorer descentralizado de ZOKA. Alguien tiene que publicar
los datos, y ese alguien es el proyecto.

Consecuencia honesta, que debe verse en la interfaz:

- El explorer web es una **vista firmada del proyecto**, con fecha de última
  actualización visible. Si el publicador se cae, el usuario ve datos viejos —
  no datos falsos disfrazados de tiempo real.
- El **explorer realmente descentralizado ya existe y es ZSilent**: lee el nodo
  propio del usuario, sin intermediarios. Eso hay que decirlo en la web.

---

## Decisión pendiente: cómo se sirven los datos

El explorer tiene 7 páginas y varias consultan por clave arbitraria:

| Página | Consulta | ¿Estático directo? |
|---|---|---|
| `Index` | últimos bloques, stats | sí |
| `BlockDetail` | bloque por altura | necesita partición |
| `TxDetail` | tx por hash | necesita índice |
| `AddressDetail` | cuenta por dirección | necesita índice |
| `RecordDetail`, `ViewScanDetail` | detalle puntual | según lo anterior |

Altura actual: **10.516** bloques, creciendo ~720/día (bloques de 120 s).

### Opción A — un archivo por bloque

10.516 archivos hoy, +720 por día.
Búsqueda por altura trivial. Muchísimas escrituras a R2.

### Opción B — bloques agrupados de a 100 *(recomendada)*

~106 archivos para toda la cadena. `BlockDetail` baja el trozo que contiene la
altura pedida. Solo el último trozo cambia seguido: **~8 escrituras por día**,
nada contra el límite gratuito de R2.

Contras: para ver un bloque se baja el trozo entero (~100 bloques). A ~14 KB por
bloque son ~1,4 MB — aceptable, y se puede bajar el tamaño del trozo si molesta.

### Opción C — solo resumen

Dashboard y últimos bloques, sin navegación profunda. Lo más simple; el explorer
pierde la mitad de su razón de ser.

**Recomendación: B.** Empezar por el dashboard (Fase 2) y agregar la navegación
por trozos (Fase 3) una vez que el circuito funcione punta a punta.

> La Opción C queda descartada de hecho por el buscador de scan key: ese sí
> necesita el historial completo. Ver abajo.

---

## El buscador por scan key manda sobre el diseño

`ViewScanDetail` deja pegar una scan key para encontrar los pagos propios. Está
implementado bien y **hay que conservarlo exactamente así**:

```js
await ensureScanWasm();                       // la criptografía corre en WASM local
address = derive_payment_address(cleanKey);   // la clave nunca sale del navegador
if (cb.recipient_commitment === address) {…}  // la comparación es local
```

La clave solo se guarda en `sessionStorage`, así que no sobrevive al cierre de la
pestaña. **Nunca** hay que mover esto al publicador ni a una función: una scan key
entregada a un servidor le da visibilidad permanente sobre todo lo que esa
persona reciba.

La consecuencia para el diseño estático es fuerte. El escaneo recorre la cadena
entera de a 1.000 bloques:

```js
const CHUNK = 1000;
for (let from = 1; from <= height; from += CHUNK) {
  const coinbases = await fetchCoinbaseRecords(from, to);
```

Es decir, el publicador **debe exportar todo el historial**, no solo lo reciente:

- [ ] `coinbases/000001-001000.json` … para toda la cadena (recompensas de minería)
- [ ] `private-tx-hashes/…` con la misma partición

Sin eso, el buscador por scan key deja de funcionar — y es lo más útil que tiene
el explorer para un minero, porque es cómo verifica que cobró.

Estimación: ~10.500 bloques con registros chicos dan pocos MB en total. Los
trozos históricos **no cambian nunca**, así que se suben una vez y solo se
reescribe el último. Encaja sin problema en R2.

---

## Fase 1 — El publicador

Un proceso que corre junto al nodo, lee su API local y escribe JSON a R2.

- [ ] Decidir dónde vive: script aparte (Node/Python) o subcomando del binario.
      *Sugerencia: script aparte primero — itera más rápido y no toca consenso.*
- [ ] Leer del nodo local (`http://127.0.0.1:3000`): `/sync/status`, `/stats`,
      `/network/runtime`, `/blocks/...`
- [ ] Generar `network-status.json` con altura, dificultad, peers, supply
      emitido y `updated_at`
- [ ] Subir a R2 con las credenciales S3 ya existentes
- [ ] Programarlo cada N minutos (Programador de tareas de Windows)

**Punto de control:** el JSON aparece en la URL pública de R2 y se actualiza solo.

> **Antes de empezar:** rotar las credenciales R2 que quedaron en texto plano en
> `clou.txt`, y no volver a guardarlas en la carpeta de capturas.

---

## Fase 2 — El explorer lee estático

- [ ] Borrar `api/rpc.ts` y `api/rpc2.ts`
- [ ] Borrar `src/integrations/supabase/` y `supabase/` (código muerto)
- [ ] Reemplazar las llamadas del front por `fetch` al JSON público de R2
- [ ] Mostrar **"última actualización: hace X"** de forma prominente
- [ ] Estado de datos viejos: si `updated_at` tiene más de ~15 min, avisarlo en
      la interfaz en vez de mostrar números como si fueran del momento
- [ ] Enlace visible: *"¿Querés datos en vivo sin intermediarios? Corré ZSilent"*
- [ ] Desplegar a Pages y enganchar `explorer.zoka.network`

**Punto de control:** el explorer muestra la altura real de la red sin ninguna
función serverless.

---

## Fase 3 — Navegación por la cadena

- [ ] El publicador exporta trozos de 100 bloques (`blocks/000000-000099.json`)
- [ ] `BlockDetail` calcula qué trozo bajar a partir de la altura
- [ ] Índice `tx-index.json` (hash → altura) para `TxDetail`,
      particionado por prefijo de hash si crece demasiado
- [ ] Revisar si `AddressDetail` tiene sentido en una cadena privada, o si
      conviene retirarla

**Punto de control:** se puede abrir un bloque cualquiera y una tx por hash.

---

## Fase 4 — Firma

- [ ] El publicador firma cada JSON (ed25519, la misma clave de snapshots)
- [ ] El explorer verifica la firma en el navegador antes de mostrar
- [ ] Si la firma no valida: decirlo, no mostrar los datos igual

Esto no vuelve descentralizado al explorer — lo que hace es que **nadie pueda
suplantarlo**, ni siquiera alguien que tome control de R2.

---

## Fuera de alcance, a propósito

- **Un RPC público.** Es lo que estamos sacando; no vuelve por la puerta de atrás.
- **Una base de datos.** Supabase ya está muerto, no se resucita.
- **Workers / Pages Functions.** Todo lo que se resuelva estático se resuelve
  estático: es lo que rompió el plan de Vercel.

---

## Riesgo conocido

La frescura de los datos depende de que **la PC del usuario esté encendida**.
Es el mismo punto único de falla que antes, ahora gratis en vez de pago. La
firma y el timestamp lo hacen honesto, no lo eliminan.

Mitigación futura: que cualquier nodo pueda publicar, y que el explorer acepte
el JSON firmado más reciente de varios publicadores. Encaja con el modelo de
"todos beben de todos" que ya está en el protocolo.
