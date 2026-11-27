# Zoka ID: a visual fingerprint for every address

Status: code version 2 and mosaic version 1, 27 November 2026.

Every Zoka Quantic address has a **Zoka ID**: a 9 × 9 mosaic and a code such as
`F4A4-595C-AF1B-A80B-914F`. Both come from the address itself, so anyone can
compute them and every wallet shows the same ID for the same address.

Use it to confirm who you are paying. The recipient reads you their code, or
shows you their mosaic, and you compare it with what your wallet shows before
you send. Malware that swaps a copied address cannot keep the same code.

The ID identifies an **address**, not a person. It does not prove who controls
the address, and it reveals nothing that the address does not already show.

## Input: the short address

The ID is computed from the canonical **short address** (`zq2…`, `zqtest2…`,
`zqregtest2…`) as the exact ASCII string of its encoding, case-sensitive.

- A short address is used as it is.
- A full address is first turned into its short address (`Address::short`).
- An exchange's integrated address (`zqi2…`, `zqtesti2…`) is turned into the
  short address it contains. The customer code is not part of the ID: all the
  deposit addresses of one exchange wallet share that wallet's ID.

A wallet shows its own ID, on its card, from its **main** address (index 0).
Each extra address of the same wallet has its own ID.

## Seed

```text
seed = SHA-256( UTF8("ZKAPriv/avatar/v1/seed") || 0x00 || UTF8(short address) )
```

The domain keeps the name it had in ZKAPriv 4.1.0, where the ID first appeared,
so the mosaics already shown do not change.

## Code (version 2)

The first **10 bytes** of the seed (80 bits), in upper-case hexadecimal, in five
groups of four characters joined by `-`:

```text
F4A4-595C-AF1B-A80B-914F
```

The first two groups are the version 1 code (4 bytes) that ZKAPriv 4.1.0 shows
on its card. A wallet can show the first two groups where space is short, but
it should show all five when confirming a payment.

## Mosaic (version 1)

- Expansion block `n` (from 0):
  `SHA-256( UTF8("ZKAPriv/avatar/v1/grid") || 0x00 || seed || uint32_be(n) )`.
- Read the bytes in order. Skip 255; every other byte gives `byte mod 5`. The
  first 81 values fill the grid row by row, left to right, top to bottom.
- Palette by value, sRGB: `#080809`, `#303036`, `#AAAAAF`, `#FAFAFA`,
  `#F2ABD0`. Background `#101012`. The colours do not change with the theme.
- An input that is empty, longer than 256 characters or not made of
  `A-Z a-z 0-9 : . _ -` has no ID: show a neutral grid instead.

## Vectors

| Input | Code (v2) | Mosaic (81 values) |
| --- | --- | --- |
| `public-account:alice` | `F4A4-595C-AF1B-A80B-914F` | `432401304002143331324404132221311134034044240240320441133232224123324403241422013` |
| `zqtest2ZokaIdExampleShortAddressForTheSpecVectors0000000` | `7E6B-EFD1-09F3-E0AC-979C` | `344213322211120313430011324432441024221440333433223012304030433043022104420412341` |

The inputs only exercise the function; the second one is not a real address.
A reference implementation in Python:

```python
import hashlib

def seed(short):
    return hashlib.sha256(b"ZKAPriv/avatar/v1/seed\x00" + short.encode()).digest()

def code(short):
    h = seed(short)[:10].hex().upper()
    return "-".join(h[i:i + 4] for i in range(0, 20, 4))

def mosaic(short):
    s, cells, n = seed(short), [], 0
    while len(cells) < 81:
        block = hashlib.sha256(b"ZKAPriv/avatar/v1/grid\x00" + s + n.to_bytes(4, "big")).digest()
        n += 1
        cells += [b % 5 for b in block if b < 255]
    return cells[:81]
```

## How strong it is

- Making a different address with the same 80-bit code takes about 2^80
  attempts, each one a key generation: out of reach. The version 1 code (32
  bits) could be matched by someone with enough computers, which is why it was
  only a recognition aid.
- The mosaic is a quick visual check. Two mosaics can look alike to a hurried
  eye; when in doubt, compare the code.
- Version 1 of the mosaic and version 2 of the code are frozen. A later version
  takes a new domain and keeps these for the IDs already shown.

## Where it appears

- **ZKAPriv:** the wallet card, Receive (the ID of the address shown) and Send
  (the recipient's ID, before you confirm).
- **ZSilent** and the **web wallet:** Receive and the payment review.
- **ZokaScan:** the page of a short address and the explanation of an exchange
  deposit address.
