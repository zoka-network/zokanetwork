# ZOKA Infrastructure Logs Policy (v1)

Aplica a la infraestructura oficial operada por el equipo core: seed nodes,
mirrors de snapshots, y RPC publicos opcionales. **No** aplica a nodos
operados por la comunidad — esos son responsabilidad del operador individual.

## Principio

ZOKA es una privacy chain. Todo log de infraestructura oficial respeta:

> "Si no podemos publicarlo en GitHub manana sin filtrar nada de un usuario,
> no se loguea."

## Que SI se loguea

- Versiones de binario corriendo (`PROTOCOL_FREEZE_ID`, commit hash de release).
- Estado de PoW: altura local, `cumulative_work` local, dificultad actual.
- Conexiones P2P: numero de peers, version de protocolo, transporte
  (Tor / I2P / Direct), pero nunca la `PeerId` mapeada a un usuario humano.
- Tasa de mensajes por tipo (`tx`, `private_tx`, `block_response`), agregada
  por minuto. Sirve para detectar spam o downtime.
- Errores de validacion de bloque (variante de `BlockValidationError`,
  altura). Sirve para detectar peers maliciosos.

## Que NO se loguea

- Direcciones IP de peers, ni siquiera los seed nodes oficiales (los peers
  conocen sus propias IPs; el seed node no las re-publica).
- Hashes de transacciones publicas o privadas individuales fuera de
  agregados.
- Contenidos de mensajes (`encrypted_block`, `encrypted_amount`, etc.).
- Wallet addresses observadas en transacciones publicas.
- Rangos horarios de actividad por peer (deanonimizable cruzando con
  network metadata).

## Retencion

- Logs operacionales (errores y conteos): 30 dias en disco caliente, despues
  agregados mensuales sin granularidad por evento, retenidos 12 meses.
- Logs de debug (verbose): 7 dias maximo, rotados.
- Snapshots de telemetria pre-release (latencias, hashrates): publicos en
  formato anonimo.

## Verificabilidad

El equipo core publica:

1. Esta politica.
2. La configuracion de logging que cumple esta politica (`tracing-subscriber`
   con `EnvFilter` recomendado en seed nodes).
3. Un dump anonimizado periodico (mensual) de los conteos publicos.

Si un usuario o auditor encuentra un log que viola esta politica, abrir un
issue en GitHub con prioridad maxima — se trata como un bug de privacidad,
no como un bug operacional.

## Operadores comunitarios

Los nodos comunitarios (`zokahd` corriendo en una laptop o servidor de
terceros) NO estan cubiertos por esta politica — son responsables de su
propia configuracion de logging. La recomendacion para esos nodos:

- Default es `tracing` con `info` para errores, `warn` para tasa de
  mensajes anomala. Sin DEBUG en produccion.
- Si el operador habilita DEBUG, asegurarse de que los logs no salen de su
  maquina (no enviar a servicios cloud de logging por default).

## Relacion con la politica de transporte

La privacidad de transporte (Tor / I2P / clearnet) la garantiza
`network::p2p::PrivacyTransportMode` y el flag
`enforce_anonymous_transport`. Los logs de infraestructura oficial NO
pueden des-anonimizar a un peer que ya esta corriendo bajo Tor / I2P,
porque por construccion no observan IPs de origen.
