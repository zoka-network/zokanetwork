# Verifying Zoka Quantic releases

Every release of the node and CLI miner, ZSilent and ZKAPriv publishes two
files next to its downloads:

- `SHA256SUMS.txt`: the SHA-256 of each file in the release.
- `SHA256SUMS.txt.sig`: a hybrid signature of that list, made with **Ed25519
  and ML-DSA-65** (FIPS 204) over the same message. Both halves must verify:
  forging the signature requires breaking Ed25519 *and* ML-DSA-65.

The release key is used only for releases. It is not the checkpoint key, and
its signatures use their own domain (`ZOKA release SHA256SUMS v1`), so a
release signature can never pass for a checkpoint or the other way round.

## The release key

Fingerprint (SHA-256 of the hybrid public key):

```text
61c2afa5 67d79ad5 c43a89c5 82656f55 ffc888a5 4ac06ddc 8445dece 63ed5c99
```

The public key is in [docs/keys](keys/): `zoka-release-ed25519.pem`,
`zoka-release-ml-dsa-65.pem` and `zoka-release-key.hex` (the hybrid key, as
`zoka` reads it). The same fingerprint is published on zoka.network. Compare it
from more than one place before you trust it.

## 1. Check the files against the list

```sh
# Linux, macOS or Git Bash on Windows, in the folder with the downloads
sha256sum -c --ignore-missing SHA256SUMS.txt
```

```powershell
# Windows PowerShell: compare with the line of that file in SHA256SUMS.txt
Get-FileHash .\ZSilent-Core-5.1.0.msi -Algorithm SHA256
```

Do not run a file whose checksum does not match.

## 2. Check the signature of the list

### With OpenSSL 3.5 or later (no Zoka software needed)

OpenSSL 3.5 added ML-DSA. Git for Windows ships it (`openssl version` in Git
Bash); older systems, such as Ubuntu 24.04 with OpenSSL 3.0, need a newer
build. In the folder with `SHA256SUMS.txt`, its `.sig` and the two `.pem`
files:

```sh
# The exact bytes that were signed: the domain, one 0x1f byte, then the list.
{ printf 'ZOKA release SHA256SUMS v1\037'; cat SHA256SUMS.txt; } > signed-message.bin
sed -n 's/^ed25519: //p'   SHA256SUMS.txt.sig | xxd -r -p > ed25519.sig.bin
sed -n 's/^ml-dsa-65: //p' SHA256SUMS.txt.sig | xxd -r -p > ml-dsa-65.sig.bin

openssl pkeyutl -verify -pubin -inkey zoka-release-ed25519.pem -rawin \
  -in signed-message.bin -sigfile ed25519.sig.bin
openssl pkeyutl -verify -pubin -inkey zoka-release-ml-dsa-65.pem -rawin \
  -in signed-message.bin -sigfile ml-dsa-65.sig.bin \
  -pkeyopt 'context-string:ZOKA release SHA256SUMS v1'
```

Both must print `Signature Verified Successfully`. ML-DSA-65 also signs the
domain as its context string, so the second command needs the `-pkeyopt`.

To check that the two `.pem` files are the published key, rebuild its
fingerprint from them (one version byte, then both raw keys):

```sh
{ printf '\001'
  openssl pkey -pubin -in zoka-release-ed25519.pem -outform DER | tail -c 32
  openssl pkey -pubin -in zoka-release-ml-dsa-65.pem -outform DER | tail -c 1952
} | sha256sum
```

### With zoka

```sh
zoka release verify SHA256SUMS.txt
```

It checks the signature with the release key compiled into `zoka`, then the
SHA-256 of every listed file that sits in the same folder. Use a `zoka` you
already trust (the one you run, or one built from source): a fresh download
cannot vouch for itself, so check a first download with OpenSSL.

## 3. Build from source (optional)

The release notes name the commit and tag each binary was built from:

```sh
git clone https://github.com/zoka-network/zokanetwork.git
cd zokanetwork
git checkout v5.1.0
cargo build --release --locked --features randomx,quantic-pq --bins
```

## For maintainers

The private key lives on the team's PC, protected for its Windows user
(DPAPI), with a password-encrypted copy kept apart (`zoka release backup`).
After building a release and writing its `SHA256SUMS.txt`:

```sh
zoka release sign SHA256SUMS.txt     # writes SHA256SUMS.txt.sig, never overwrites
zoka release verify SHA256SUMS.txt   # checks it with the compiled key
```

`zoka release sign` refuses to sign with a key other than the one compiled into
that `zoka`.
