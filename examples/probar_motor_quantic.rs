//! El motor de Zoka Quantic del teléfono, de punta a punta, desde una PC.
//!
//! Corre el mismo código que va en el `.so` de ZKAPriv (`mobile_net` +
//! `mobile_quantic`) contra un nodo de Quantic regtest de verdad, por P2P:
//! conecta, pide la punta y los bloques compactos por `/zoka-quantic/light/1`,
//! sincroniza la wallet del equipo de regtest (su raíz es pública), arma un
//! envío con la prueba Halo 2, lo publica con `submit_tx` y espera a verlo
//! minado. Si esto anda, un fallo en el teléfono es de Arti, del puente JNI o
//! de la interfaz, no del motor.
//!
//! ```text
//! zoka fullnode --network quantic-regtest --listen /ip4/127.0.0.1/tcp/29411 \
//!     --no-network-config --allow-isolated-start --quantic-mine-to <dirección> ...
//! cargo run --features quantic --example probar_motor_quantic -- /ip4/127.0.0.1/tcp/29411
//! ```

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use zoka::mobile_net;
use zoka::mobile_quantic;

const NETWORK: &str = "quantic-regtest";

fn call(what: &str, result: Result<String, String>) -> Value {
    match result {
        Ok(text) => {
            let value: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
            if let Some(error) = value.get("error") {
                panic!("{what}: {error}");
            }
            value
        }
        Err(error) => panic!("{what}: {error}"),
    }
}

fn light(request: Value) -> Value {
    call("quantic_light", mobile_net::quantic_light_json(&request.to_string()))
}

/// Sincroniza desde `current` hasta la punta, de a 500 bloques.
fn sync(seed_hex: &str, mut current: Value) -> Value {
    let tip = light(json!({ "kind": "info" }))["height"].as_u64().expect("height");
    let mut last = Value::Null;
    loop {
        let from = current["next_height"].as_u64().unwrap_or(1);
        if from > tip {
            return if last.is_null() { current_as_response(current) } else { last };
        }
        let to = (from + 499).min(tip);
        let blocks = light(json!({ "kind": "compact", "from": from, "to": to }))["blocks"].clone();
        let started = Instant::now();
        let response = call(
            "quantic_sync",
            mobile_quantic::quantic_sync_json(
                &json!({
                    "seed_hex": seed_hex, "network": NETWORK,
                    "state_hex": current["state_hex"], "last_hash": current["last_hash"],
                    "tip_height": tip, "blocks": blocks,
                })
                .to_string(),
            ),
        );
        println!(
            "  bloques {from}..{to}: saldo {} átomos ({} ms de escaneo, {} ms en total)",
            response["balance_atoms"],
            response["scan_ms"],
            started.elapsed().as_millis()
        );
        current = response["current"].clone();
        last = response;
    }
}

fn current_as_response(current: Value) -> Value {
    json!({ "current": current, "balance_atoms": Value::Null })
}

fn main() {
    let seed = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("uso: cargo run --features quantic --example probar_motor_quantic -- <multiaddr del nodo>");
        std::process::exit(2);
    });
    let data_dir = std::env::temp_dir().join("zoka-prueba-motor-quantic");
    let _ = std::fs::create_dir_all(&data_dir);
    let config = json!({
        "seeds": [seed], "data_dir": data_dir.display().to_string(), "network": NETWORK,
    });
    let started = Instant::now();
    println!("conectar: {}", call("connect", mobile_net::connect_json(&config.to_string())));

    let team_seed = hex::encode(zoka::quantic::REGTEST_TEAM_ROOT);
    let info = call(
        "wallet_info",
        mobile_quantic::quantic_wallet_info_json(&json!({ "seed_hex": team_seed, "network": NETWORK }).to_string()),
    );
    println!("wallet del equipo: {}…", &info["address"].as_str().unwrap_or("")[..24]);
    println!("punta: {}", light(json!({ "kind": "info" })));

    println!("sincronizar desde el génesis:");
    let synced = sync(&team_seed, json!({ "next_height": 1 }));
    let balance = synced["balance_atoms"].as_u64().unwrap_or(0);
    assert!(balance > 0, "la wallet del equipo tiene que ver el premine");

    let key_started = Instant::now();
    call("preload", mobile_quantic::quantic_preload_proving_key_json());
    println!("clave de prueba: {} ms", key_started.elapsed().as_millis());

    let bob_seed = hex::encode([0x42u8; 32]);
    let bob = call(
        "wallet_info",
        mobile_quantic::quantic_wallet_info_json(&json!({ "seed_hex": bob_seed, "network": NETWORK }).to_string()),
    )["address"]
        .as_str()
        .expect("address")
        .to_string();
    let transfer = call(
        "build_transfer",
        mobile_quantic::quantic_build_transfer_json(
            &json!({
                "seed_hex": team_seed, "network": NETWORK,
                "state_hex": synced["current"]["state_hex"],
                "to": bob, "amount_atoms": 3_000_000u64, "memo_text": "desde el motor del telefono",
            })
            .to_string(),
        ),
    );
    println!(
        "envío armado: {} acciones, comisión {} átomos, {} ms",
        transfer["actions"], transfer["fee_atoms"], transfer["build_ms"]
    );
    let submitted = light(json!({ "kind": "submit_tx", "tx_hex": transfer["tx_hex"] }));
    println!("publicado: {submitted}");
    assert_eq!(submitted["accepted"], true, "el nodo tiene que admitir el envío");

    // Bob, una wallet nueva, desde el génesis: cuando el envío entra en un bloque, lo ve.
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let view = sync(&bob_seed, json!({ "next_height": 1 }));
        if view["balance_atoms"].as_u64() == Some(3_000_000) {
            println!("Bob ve sus 3 ZKA: {}", view["history"][0]);
            break;
        }
        assert!(Instant::now() < deadline, "el envío no se minó en 3 minutos");
        std::thread::sleep(Duration::from_secs(5));
    }
    println!("listo en {} s", started.elapsed().as_secs());
    let _ = mobile_net::disconnect();
}
