#!/usr/bin/env bash
# Download the KZG params (k=16, 8 MB) the prover loads from SRS_PATH into resources/.
# The file is not committed. Its hash must equal the SRS checked into solana-proof-generator.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
url="https://axiom-crypto.s3.amazonaws.com/challenge_0085/kzg_bn254_16.srs"
expected_sha256="05ab2fea166aeffa3ed42b0389ea310bba37946f035a385df563357959e5b9dc"
dst="$root/resources/kzg_bn254_16.srs"

mkdir -p "$(dirname "$dst")"
curl -fL --retry 3 -o "$dst.tmp" "$url"

actual_sha256="$(shasum -a 256 "$dst.tmp" | cut -d' ' -f1)"
if [[ "$actual_sha256" != "$expected_sha256" ]]; then
    rm -f "$dst.tmp"
    echo "error: $url has sha256 $actual_sha256, expected $expected_sha256." >&2
    echo "Copy it from a local solana-proof-generator checkout instead:" >&2
    echo "  cp ../solana-proof-generator/srs/kzg_bn254_16.srs $dst" >&2
    exit 1
fi

mv "$dst.tmp" "$dst"
echo "SRS saved: $dst"
