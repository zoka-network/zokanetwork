"""Pasos 3 y 4 de docs/LANZAMIENTO-quantic.md para la Zoka Quantic Testnet.

Uso, desde cualquier carpeta:
    python scripts/lanzar-testnet-quantic.py <zoka.exe> <dirección zqtest1 del equipo> <pública Ed25519 hex> <pública ML-DSA-65 hex>

<zoka.exe> es el 3.0.x de release, compilado con `--features randomx,quantic`
y todavía sin génesis fijo. Ensayado de punta a punta el 22/10/2026 con datos
descartables: el génesis embebido arranca la testnet, otro nodo la sigue, la
wallet del equipo ve el premine y los checkpoints híbridos pasan.

Solo recibe datos PÚBLICOS. Arma el génesis (bloque 1 con el premine público a
la dirección del equipo), lo deja en docs/quantic-testnet-genesis.v1.json, fija
en src/economics.rs el hash del génesis, la dirección y las dos mitades públicas
de la autoridad, y pasa el manifiesto de la testnet de "pre-genesis" a "active".
Revisa todo antes de escribir: si algún dato o archivo no tiene la forma
esperada, no toca nada.
"""
import datetime
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CR, LF = chr(13), chr(10)

zoka, address, ed25519, mldsa = sys.argv[1:5]
address, ed25519, mldsa = address.strip(), ed25519.strip().lower(), mldsa.strip().lower()

if not (address.startswith("zqtest1") and len(address) > 1500):
    raise SystemExit("la dirección no parece de la testnet de Quantic (zqtest1…, unos 1.770 caracteres)")
if not re.fullmatch(r"[0-9a-f]{64}", ed25519):
    raise SystemExit("la pública Ed25519 tiene que ser de 64 caracteres hex")
if not re.fullmatch(r"[0-9a-f]{3904}", mldsa):
    raise SystemExit("la pública ML-DSA-65 tiene que ser de 3.904 caracteres hex (1.952 bytes)")

# 1. Revisar economics.rs y el manifiesto antes de crear nada.
economics = ROOT / "src" / "economics.rs"
raw = economics.read_bytes().decode("utf-8")
crlf = (CR + LF) in raw
source = raw.replace(CR + LF, LF)
GENESIS_NONE = "pub const QUANTIC_TESTNET_GENESIS: Option<QuanticGenesis> = None;"
GENESIS_DOC = LF.join([
    "/// `None` hasta que se cree: el premine va a la dirección del equipo de la",
    "/// testnet, que la crea el usuario (`zoka quantic-wallet new --network",
    "/// quantic-testnet`), y el bloque sale de `zoka quantic-genesis`.",
])
OLD_ED = "pub const CHECKPOINT_AUTHORITY_KEYS: &[(&str, &str)] = &[("
OLD_ML = "pub const CHECKPOINT_AUTHORITY_MLDSA_KEYS: &[(&str, &str)] = &[];"
OLD_ML_DOC = LF.join([
    "/// Zenith todavía no tiene: la crea el usuario con",
    "/// `zoka checkpoint-keygen --mldsa` en la PC de la autoridad.",
])
if source.count(GENESIS_NONE) != 1:
    raise SystemExit("la testnet ya tiene génesis fijo en economics.rs: no lo piso")
if source.count(OLD_ED) != 1:
    raise SystemExit("no encontré CHECKPOINT_AUTHORITY_KEYS")
if source.count(OLD_ML) != 1:
    raise SystemExit("no encontré CHECKPOINT_AUTHORITY_MLDSA_KEYS vacía")

manifest_file = ROOT / "docs" / "quantic-testnet-network.v1.json"
manifest = json.loads(manifest_file.read_text(encoding="utf-8"))
PRE_GENESIS_NOTE = "Pre-genesis: there is no genesis block yet. The team's PC will be the first seed once the genesis is created."
if manifest.get("status") != "pre-genesis" or PRE_GENESIS_NOTE not in manifest.get("notes", []):
    raise SystemExit("el manifiesto de la testnet no está en pre-genesis: no lo piso")
if not any(seed.get("status") == "active" and seed.get("onion") for seed in manifest.get("seed_nodes", [])):
    raise SystemExit("el manifiesto de la testnet no tiene su semilla activa")

# 2. El génesis.
genesis_file = ROOT / "docs" / "quantic-testnet-genesis.v1.json"
run = subprocess.run(
    [zoka, "quantic-genesis", "--network", "quantic-testnet", "--premine-to", address, "--out", str(genesis_file)],
    capture_output=True, text=True, cwd=ROOT,
)
print(run.stdout, run.stderr)
if run.returncode != 0:
    raise SystemExit("quantic-genesis falló")
block = json.loads(genesis_file.read_text(encoding="utf-8"))
block_hash = block["header"]["hash"]
if not re.fullmatch(r"[0-9a-f]{64}", block_hash) or block["header"]["block_height"] != 1:
    raise SystemExit(f"el génesis no tiene la forma esperada: {block_hash}")
created = datetime.datetime.fromtimestamp(block["header"]["timestamp"], datetime.timezone.utc)
created_text = created.strftime("%Y-%m-%d %H:%M UTC")

# 3. economics.rs: génesis, dirección y las dos mitades públicas de la autoridad.
genesis_const = LF.join([
    "pub const QUANTIC_TESTNET_GENESIS: Option<QuanticGenesis> = Some(QuanticGenesis {",
    f'    block_hash: "{block_hash}",',
    f'    premine_address: "{address}",',
    "});",
])
if source.count(GENESIS_DOC + LF + GENESIS_NONE) == 1:
    source = source.replace(
        GENESIS_DOC + LF + GENESIS_NONE,
        LF.join([
            f"/// Creado el {created_text} con `zoka quantic-genesis`: el premine va a la",
            "/// wallet del equipo de la testnet (`zoka quantic-wallet new --network",
            "/// quantic-testnet`).",
            genesis_const,
        ]),
    )
else:
    source = source.replace(GENESIS_NONE, genesis_const)
source = source.replace(
    OLD_ED,
    LF.join([
        "pub const CHECKPOINT_AUTHORITY_KEYS: &[(&str, &str)] = &[",
        "    // Zoka Quantic Testnet: la PC del equipo (`zoka checkpoint-keygen --network",
        "    // quantic-testnet`). La privada no sale de esa PC.",
        f'    (QUANTIC_TESTNET_CHAIN_ID, "{ed25519}"),',
        "    (",
    ]),
    1,
)
if source.count(OLD_ML_DOC) == 1:
    source = source.replace(
        OLD_ML_DOC,
        LF.join([
            "/// Zoka Quantic la exige desde el génesis: `zoka checkpoint-keygen --network",
            "/// quantic-testnet` crea las dos mitades en la PC de la autoridad.",
        ]),
    )
source = source.replace(
    OLD_ML,
    LF.join([
        "pub const CHECKPOINT_AUTHORITY_MLDSA_KEYS: &[(&str, &str)] = &[(",
        "    QUANTIC_TESTNET_CHAIN_ID,",
        f'    "{mldsa}",',
        ")];",
    ]),
)
if crlf:
    source = source.replace(LF, CR + LF)
economics.write_bytes(source.encode("utf-8"))

# 4. El manifiesto: la testnet existe.
manifest["status"] = "active"
manifest["notes"] = [
    f"Genesis: block 1 {block_hash}, created {created_text}. The team's PC is the first seed."
    if note == PRE_GENESIS_NOTE else note
    for note in manifest["notes"]
]
# write_bytes y no write_text: en Windows write_text pasa los saltos a CRLF.
manifest_file.write_bytes((json.dumps(manifest, indent=2, ensure_ascii=False) + LF).encode("utf-8"))

print(f"génesis de la testnet: bloque 1 {block_hash} ({created_text})")
print("economics.rs: génesis, dirección y las dos mitades públicas de la autoridad fijadas")
print("manifiesto de la testnet: status active")
