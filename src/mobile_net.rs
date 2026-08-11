//! Cliente de red para la wallet movil: hablar con la red sin ser un nodo.
//!
//! ZKAPriv usaba una API REST contra nodos centrales. Esos nodos se dieron de
//! baja y la wallet quedo muerta: apuntaba a dos IPs que ya no existen. Este
//! modulo la conecta a la red igual que cualquier otro participante, sin
//! depender de que nadie mantenga un servidor.
//!
//! **Lo que hace y lo que no.** Publica transacciones por gossip y pide
//! bloques por el protocolo punto a punto para que la wallet escanee sus
//! notas. No mina, no valida la cadena entera y no guarda bloques: los pide,
//! el escaner los mira y se descartan. El telefono nunca almacena la cadena.
//!
//! **Por que no hay codigo de red nuevo.** El crate del nodo ya se compila
//! dentro del `.so` de Android -- solo con RandomX apagado --, asi que libp2p,
//! gossipsub y el protocolo de bloques ya viajaban en el APK sin estar
//! expuestos. Esto es una fachada sobre lo que ya estaba.
//!
//! **Tor.** El nodo alcanza onions por un proxy SOCKS5 configurable. En el
//! telefono ese proxy lo levanta Arti (Tor en Rust) dentro del mismo proceso,
//! asi que no hace falta instalar ninguna otra aplicacion.

use crate::network::message::ZokaMessage;
use crate::network::p2p::Node;
use crate::private_wallet::PrivateTransaction;
use serde::{Deserialize, Serialize};
use std::sync::mpsc as std_mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::mpsc;

/// Cuanto se deja respirar al bucle de eventos entre comandos.
///
/// libp2p solo avanza cuando alguien lo bombea: los apretones de manos, los
/// anuncios de gossip y las respuestas entrantes ocurren dentro de `pump_for`.
/// Un valor corto mantiene la red viva sin quemar bateria, que en un telefono
/// importa mas que en un servidor.
const PUMP_SLICE: Duration = Duration::from_millis(250);

/// Cuanto esperar a que aparezca el primer par antes de dar por fallida una
/// conexion.
///
/// Generoso a proposito: un circuito Tor recien armado tarda decenas de
/// segundos, y en una red movil mas todavia. Fallar antes de tiempo obligaria
/// al usuario a reintentar cuando solo hacia falta esperar.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, Deserialize)]
pub struct MobileNetConfig {
    /// Direcciones a las que marcar al arrancar. Onions del manifiesto o las
    /// que el usuario haya agregado a mano.
    pub seeds: Vec<String>,
    /// Proxy SOCKS5 para alcanzar onions, normalmente el de Arti en este mismo
    /// proceso.
    #[serde(default)]
    pub tor_proxy: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MobileNetStatus {
    pub connected: bool,
    pub peers: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

enum Cmd {
    Status(std_mpsc::Sender<MobileNetStatus>),
    PublishPrivateTx(Box<PrivateTransaction>, std_mpsc::Sender<Result<(), String>>),
    Shutdown,
}

struct Handle {
    cmd_tx: mpsc::UnboundedSender<Cmd>,
    /// El hilo dedicado donde vive el nodo. Se guarda para que el runtime no se
    /// caiga mientras haya comandos en vuelo.
    _hilo: std::thread::JoinHandle<()>,
}

static HANDLE: OnceLock<Mutex<Option<Handle>>> = OnceLock::new();

fn handle_slot() -> &'static Mutex<Option<Handle>> {
    HANDLE.get_or_init(|| Mutex::new(None))
}

/// Levanta el cliente y marca a las semillas. Idempotente: si ya estaba
/// arriba, devuelve el estado actual en vez de arrancar un segundo nodo -- dos
/// nodos en el mismo proceso pelearian por la misma identidad de red.
pub fn connect_json(config_json: &str) -> Result<String, String> {
    let config: MobileNetConfig =
        serde_json::from_str(config_json).map_err(|e| format!("config invalida: {e}"))?;
    if config.seeds.is_empty() {
        return Err("no hay semillas a las que conectarse".to_string());
    }

    let mut slot = handle_slot().lock().map_err(|_| "estado envenenado".to_string())?;
    if slot.is_some() {
        drop(slot);
        return status_json();
    }

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (listo_tx, listo_rx) = std_mpsc::channel();
    let seeds = config.seeds.clone();
    let tor_proxy = config.tor_proxy.clone();

    // El nodo vive en un hilo propio con su runtime de un solo hilo, no en un
    // pool compartido.
    //
    // No es una preferencia: el nodo guarda devoluciones de llamada que son
    // `Send` pero no `Sync`, asi que su futuro no puede moverse entre hilos y
    // `spawn` sobre un runtime multi-hilo ni siquiera compila. Darle un hilo
    // fijo tambien le sienta mejor a un telefono -- una sola cosa haciendo red,
    // sin robarle nucleos al prover ni a la interfaz.
    let hilo = std::thread::Builder::new()
        .name("zoka-mobile-net".to_string())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = listo_tx.send(Err(format!("no pude crear el runtime: {e}")));
                    return;
                }
            };
            rt.block_on(run_client(seeds, tor_proxy, cmd_rx, listo_tx));
        })
        .map_err(|e| format!("no pude crear el hilo de red: {e}"))?;

    match listo_rx.recv_timeout(CONNECT_TIMEOUT) {
        Ok(Ok(())) => {
            *slot = Some(Handle { cmd_tx, _hilo: hilo });
            drop(slot);
            status_json()
        }
        Ok(Err(err)) => Err(err),
        Err(_) => Err(format!(
            "ningun par respondio en {} segundos",
            CONNECT_TIMEOUT.as_secs()
        )),
    }
}

/// Estado actual: si hay red y con cuantos pares.
pub fn status_json() -> Result<String, String> {
    let slot = handle_slot().lock().map_err(|_| "estado envenenado".to_string())?;
    let Some(handle) = slot.as_ref() else {
        return serde_json::to_string(&MobileNetStatus {
            connected: false,
            peers: 0,
            error: None,
        })
        .map_err(|e| e.to_string());
    };
    let (tx, rx) = std_mpsc::channel();
    handle
        .cmd_tx
        .send(Cmd::Status(tx))
        .map_err(|_| "el cliente de red se detuvo".to_string())?;
    let estado = rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| "el cliente de red no responde".to_string())?;
    serde_json::to_string(&estado).map_err(|e| e.to_string())
}

/// Publica una transaccion privada ya construida y probada localmente.
///
/// La wallet arma y prueba la transaccion en el telefono; esto solo la pone en
/// la red. Los mineros la validan como a cualquier otra: este cliente no pide
/// ni recibe ningun trato especial.
pub fn publish_private_tx_json(tx_json: &str) -> Result<String, String> {
    let tx: PrivateTransaction =
        serde_json::from_str(tx_json).map_err(|e| format!("transaccion invalida: {e}"))?;

    let slot = handle_slot().lock().map_err(|_| "estado envenenado".to_string())?;
    let handle = slot
        .as_ref()
        .ok_or_else(|| "la red no esta conectada".to_string())?;

    let (tx_resp, rx_resp) = std_mpsc::channel();
    handle
        .cmd_tx
        .send(Cmd::PublishPrivateTx(Box::new(tx), tx_resp))
        .map_err(|_| "el cliente de red se detuvo".to_string())?;
    drop(slot);

    match rx_resp.recv_timeout(Duration::from_secs(60)) {
        Ok(Ok(())) => Ok(r#"{"published":true}"#.to_string()),
        Ok(Err(err)) => Err(err),
        Err(_) => Err("la publicacion no confirmo a tiempo".to_string()),
    }
}

/// Corta la red y libera el nodo. La wallet la llama al pasar a segundo plano:
/// mantener circuitos Tor abiertos con la pantalla apagada gasta bateria sin
/// que nadie mire el resultado.
pub fn disconnect() -> Result<String, String> {
    let mut slot = handle_slot().lock().map_err(|_| "estado envenenado".to_string())?;
    if let Some(handle) = slot.take() {
        let _ = handle.cmd_tx.send(Cmd::Shutdown);
    }
    Ok(r#"{"connected":false}"#.to_string())
}


/// Publica una transaccion privada por el mismo camino que usa el nodo
/// completo, que son tres pasos y no uno.
///
/// La red no difunde la transaccion entera y ya: publica un ANUNCIO con el
/// hash, y los pares que lo ven piden el cuerpo punto a punto. Por eso hay que
/// dejar el cuerpo en cache ANTES de anunciar -- si no, quien pida ese hash se
/// encuentra con que el unico que lo tiene no se lo puede dar, y la
/// transaccion muere sin llegar a ningun minero.
///
/// Despues se publica tambien la transaccion completa, que es lo que hace el
/// nodo: cubre a los pares que la reciben directo sin pasar por el anuncio.
async fn publicar_tx_privada(node: &mut Node, tx: &PrivateTransaction) -> Result<(), String> {
    let cuerpo = serde_json::to_vec(tx).map_err(|e| format!("no pude serializar la tx: {e}"))?;
    let msg: crate::network::message::PrivateTransactionMessage = serde_json::to_value(tx)
        .and_then(serde_json::from_value)
        .map_err(|e| format!("no pude convertir la tx al mensaje de red: {e}"))?;

    node.cache_private_tx_body(&msg.tx_hash, cuerpo.clone());

    let anuncio = crate::network::message::PrivateTxAnnounceMessage::from_private_tx(
        &msg,
        Some(cuerpo.len() as u64),
    );

    // Reintentar bombeando entre intentos: gossipsub rechaza publicar mientras
    // el tema todavia no tiene pares en la malla, y sobre Tor eso tarda unos
    // segundos en armarse despues de conectar.
    node.publish_message_with_retry(&ZokaMessage::PrivateTxAnnounce(anuncio), 10, PUMP_SLICE)
        .await
        .map_err(|e| format!("no pude anunciar la tx: {e}"))?;

    // La transaccion completa es el segundo camino, no el principal: si el
    // anuncio llego, la red ya puede pedir el cuerpo. Un fallo aca no invalida
    // la publicacion.
    let _ = node
        .publish_message_with_retry(&ZokaMessage::PrivateTransaction(msg), 5, PUMP_SLICE)
        .await;
    Ok(())
}

async fn run_client(
    seeds: Vec<String>,
    tor_proxy: Option<String>,
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    listo_tx: std_mpsc::Sender<Result<(), String>>,
) {
    // Puerto 0: este cliente no escucha. Un telefono no es alcanzable desde
    // afuera y no tiene por que serlo -- solo necesita salir.
    let mut node = match Node::new_with_tor_proxy(0, tor_proxy).await {
        Ok(node) => node,
        Err(err) => {
            let _ = listo_tx.send(Err(format!("no pude crear el nodo: {err}")));
            return;
        }
    };

    let mut alguno = false;
    for seed in &seeds {
        if node.connect_to_peer(seed).await.is_ok() {
            alguno = true;
        }
        // Bombear entre marcados: el apreton de manos ocurre dentro del bucle
        // de eventos, no dentro de connect_to_peer.
        let _ = node.pump_for(PUMP_SLICE).await;
    }

    if !alguno {
        let _ = listo_tx.send(Err(
            "ninguna semilla acepto la conexion; revisa Tor y la lista de pares".to_string(),
        ));
        return;
    }
    let _ = listo_tx.send(Ok(()));

    loop {
        tokio::select! {
            biased;
            cmd = cmd_rx.recv() => match cmd {
                Some(Cmd::Status(resp)) => {
                    let _ = resp.send(MobileNetStatus {
                        connected: node.connected_peer_count() > 0,
                        peers: node.connected_peer_count(),
                        error: None,
                    });
                }
                Some(Cmd::PublishPrivateTx(tx, resp)) => {
                    let r = publicar_tx_privada(&mut node, &tx).await;
                    let _ = resp.send(r);
                }
                Some(Cmd::Shutdown) | None => return,
            },
            _ = async {
                let _ = node.pump_for(PUMP_SLICE).await;
            } => {}
        }
    }
}
