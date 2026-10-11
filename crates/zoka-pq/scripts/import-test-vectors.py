"""Import a small, deterministic selection of published independent KATs.

No expected value is calculated using zoka-pq or RustCrypto. Run manually when
reviewing/updating fixtures; normal cargo tests work offline. Fixed upstream
commits and SHA-256 digests of the downloaded sources are recorded in JSON.
"""

import hashlib
import json
from pathlib import Path
from urllib.request import Request, urlopen


NIST_COMMIT = "975de31eb83d87039ec88934fdc47d8c312b892d"
XWING_COMMIT = "984c2f7a93b8f8d8f8073ebb53f9f4ce50b5babd"
OUTPUT = Path(__file__).resolve().parents[1] / "tests" / "vectors"


def download(url):
    request = Request(url, headers={"User-Agent": "zoka-pq-kat-import/0.1"})
    with urlopen(request, timeout=120) as response:
        body = response.read()
    return json.loads(body), hashlib.sha256(body).hexdigest()


def write(name, value):
    OUTPUT.mkdir(parents=True, exist_ok=True)
    (OUTPUT / name).write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def nist(suite, filename, select_group, fields, count=3):
    url = (
        f"https://raw.githubusercontent.com/usnistgov/ACVP-Server/{NIST_COMMIT}"
        f"/gen-val/json-files/{suite}/internalProjection.json"
    )
    data, digest = download(url)
    groups = []
    for group in data["testGroups"]:
        if not select_group(group):
            continue
        selected = [
            {key: case[key] for key in ["tcId", *fields] if key in case}
            for case in group["tests"][:count]
        ]
        groups.append({
            **{key: value for key, value in group.items() if key != "tests"},
            "tests": selected,
        })
    assert groups and all(group["tests"] for group in groups)
    write(filename, {
        "source": url,
        "source_sha256": digest,
        "selection": f"First {count} tests of each matching group, unchanged hex values",
        "groups": groups,
    })
    print(filename, [(g["tgId"], [t["tcId"] for t in g["tests"]]) for g in groups])


def main():
    nist("ML-KEM-keyGen-FIPS203", "mlkem768-keygen.json",
         lambda g: g["parameterSet"] == "ML-KEM-768", ["d", "z", "ek", "dk"])
    nist("ML-KEM-encapDecap-FIPS203", "mlkem768-encap-decap.json",
         lambda g: g["parameterSet"] == "ML-KEM-768" and
         g["function"] in ("encapsulation", "decapsulation"), ["ek", "dk", "m", "c", "k"])
    nist("ML-DSA-keyGen-FIPS204", "mldsa65-keygen.json",
         lambda g: g["parameterSet"] == "ML-DSA-65", ["seed", "pk", "sk"])
    nist("ML-DSA-sigGen-FIPS204", "mldsa65-sign.json",
         lambda g: g["parameterSet"] == "ML-DSA-65" and
         g["signatureInterface"] == "external" and g["deterministic"] and
         g["preHash"] == "pure", ["message", "pk", "sk", "context", "signature"])
    url = (
        "https://raw.githubusercontent.com/dconnolly/draft-connolly-cfrg-xwing-kem/"
        f"{XWING_COMMIT}/spec/test-vectors.json"
    )
    vectors, digest = download(url)
    write("xwing.json", {"source": url, "source_sha256": digest,
                         "selection": "All published test vectors, unchanged", "tests": vectors})
    print("xwing.json", len(vectors))


if __name__ == "__main__":
    main()
