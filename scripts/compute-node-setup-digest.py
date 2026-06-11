#!/usr/bin/env python3
"""Recompute the Zoka trusted-setup DIGEST exactly like
src/zk/setup_io.rs::compute_directory_digest.

For each circuit (REQUIRED in order, then OPTIONAL private_tx_v2_i1 if present)
it feeds:  name_bytes  ||  0x1f  ||  sha256(<name>.params)  ||  0x1e
into a single SHA-256, and prints the final digest hex.

Run it ON a node AFTER copying private_tx_v2_i1.params into the trusted-setup
dir, then write the printed value into both the DIGEST file and the
ZOKA_EXPECTED_SETUP_DIGEST env var.

Usage:  python3 compute-node-setup-digest.py /opt/zoka/trusted-setup
"""
import hashlib
import os
import sys

REQUIRED = ["balance", "merkle", "nullifier", "private_tx"]
OPTIONAL = ["private_tx_v2_i1"]

def main() -> int:
    d = sys.argv[1] if len(sys.argv) > 1 else "/opt/zoka/trusted-setup"
    h = hashlib.sha256()
    for circuit in REQUIRED + OPTIONAL:
        path = os.path.join(d, circuit + ".params")
        if not os.path.exists(path):
            if circuit in OPTIONAL:
                continue
            print(f"ERROR: missing required params file {path}", file=sys.stderr)
            return 1
        with open(path, "rb") as f:
            file_hash = hashlib.sha256(f.read()).digest()
        h.update(circuit.encode("utf-8"))
        h.update(b"\x1f")
        h.update(file_hash)
        h.update(b"\x1e")
    print(h.hexdigest())
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
