#!/usr/bin/env bash
# Check that the library builds warning-free under every supported feature slice:
# the runtime tiers alone, and each symbology with encode-only, decode-only, both,
# and scanning. Usage: ci/check-features.sh [symbology...]
set -euo pipefail

CODES=(qr microqr rmqr aztec datamatrix maxicode hanxin dotcode gridmatrix appclip
       pdf417 code16k code49 codablockf ean code128 code39 code93 code11 itf twoof5
       codabar msi telepen pharmacode dxfilm databar postal)
if [ "$#" -gt 0 ]; then CODES=("$@"); fi

export RUSTFLAGS="${RUSTFLAGS:-} -D warnings"
check() {
  echo "::group::features [$1]"
  cargo check --lib --no-default-features --features "$1"
  echo "::endgroup::"
}

if [ "$#" -eq 0 ]; then
  for f in "" alloc std encode decode scan all-codes "all-codes,encode" \
           "all-codes,alloc,encode" "all-codes,decode" "all-codes,std,encode"; do
    check "$f"
  done
fi
for c in "${CODES[@]}"; do
  for f in "$c" "$c,encode" "$c,alloc" "$c,alloc,encode" "$c,decode" "$c,alloc,encode,decode" "$c,scan"; do
    check "$f"
  done
done
