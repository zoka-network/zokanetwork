#!/usr/bin/env bash
# Generates SHA-256 checksums for the release artifacts in a release directory.
# Usage:
#   ./scripts/release_checksums.sh ./release [./release/SHA256SUMS.txt]
#
# The output uses the GNU coreutils `sha256sum` format so that any verifier
# can run `sha256sum -c SHA256SUMS.txt` and confirm every artifact matches.

set -euo pipefail

if [[ $# -lt 1 ]]; then
    echo "usage: $0 <release-dir> [out-file]" >&2
    exit 64
fi

release_dir="$1"
out_file="${2:-}"

if [[ ! -d "$release_dir" ]]; then
    echo "release directory not found: $release_dir" >&2
    exit 66
fi

# Sorted list of artifacts, excluding any prior checksum manifest.
mapfile -t files < <(
    find "$release_dir" -maxdepth 1 -type f \
        ! -name 'SHA256SUMS.txt' ! -name 'SHA256SUMS.txt.asc' \
        -printf '%P\n' | sort
)

if [[ ${#files[@]} -eq 0 ]]; then
    echo "no release artifacts found in: $release_dir" >&2
    exit 65
fi

# Compute checksums while pwd matches the release dir so the manifest
# carries plain filenames (not paths).
(
    cd "$release_dir"
    if [[ -n "$out_file" ]]; then
        : > "$out_file"
    fi
    for f in "${files[@]}"; do
        line=$(sha256sum "$f")
        if [[ -n "$out_file" ]]; then
            echo "$line" >> "$out_file"
        else
            echo "$line"
        fi
    done
)

if [[ -n "$out_file" ]]; then
    echo "Wrote ${#files[@]} checksums to $out_file" >&2
    echo "" >&2
    echo "Next steps:" >&2
    echo "  1. Sign the file with GPG: gpg --detach-sign --armor $out_file" >&2
    echo "  2. Publish $out_file and ${out_file}.asc together with the artifacts" >&2
    echo "  3. Document the signing key fingerprint in docs/release-verification.md" >&2
fi
