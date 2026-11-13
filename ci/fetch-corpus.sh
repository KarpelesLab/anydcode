#!/usr/bin/env bash
# Fetch the ZXing "blackbox" barcode photographs (Apache-2.0) into testdata/corpus/
# (git-ignored) and convert them to PNG for `cargo run --features cli --example corpus`.
# Needs git and either sips (macOS) or ImageMagick's `magick`.
set -euo pipefail
cd "$(dirname "$0")/.."
dest=testdata/corpus
src=$dest/zxing/core/src/test/resources/blackbox
mkdir -p "$dest"
if [ ! -d "$dest/zxing" ]; then
  git clone -q --depth 1 --filter=blob:none --sparse https://github.com/zxing/zxing.git "$dest/zxing"
  git -C "$dest/zxing" sparse-checkout set core/src/test/resources/blackbox
fi
for dir in "$src"/*/; do
  set=$(basename "$dir")
  out=$dest/png/$set
  mkdir -p "$out"
  for f in "$dir"*; do
    stem=$(basename "${f%.*}")
    case "$f" in
      *.bin) cp "$f" "$out/" ;;
      *.txt)
        cp "$f" "$out/"
        # The expectations are UTF-8; a Japanese symbol carries Shift-JIS.
        iconv -f UTF-8 -t CP932 "$f" > "$out/$stem.sjis" 2>/dev/null || rm -f "$out/$stem.sjis" ;;
      *.png|*.jpg|*.jpeg|*.JPG|*.gif)
        [ -f "$out/$stem.png" ] && continue
        if command -v sips >/dev/null; then
          sips -s format png "$f" --out "$out/$stem.png" >/dev/null 2>&1 || echo "skip $f"
        else
          magick "$f" "$out/$stem.png" || echo "skip $f"
        fi ;;
    esac
  done
done
echo "corpus ready in $dest/png"
