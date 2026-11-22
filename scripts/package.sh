#!/usr/bin/env bash
# Assemble a release tarball from a finished `cargo leptos build --release`.
#   scripts/package.sh <workspace-root> <out-dir>
set -euo pipefail
ROOT=${1:?workspace root}; OUT=${2:?out dir}
ARCH=$(uname -m)
VER=$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d'"' -f2)
SHA=$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo local)
NAME="juggernaught-linux-${ARCH}"
STAGE=$(mktemp -d)/$NAME
mkdir -p "$STAGE/bin" "$OUT"
cp "$ROOT/target/release/juggernaught" "$STAGE/bin/"
strip "$STAGE/bin/juggernaught" 2>/dev/null || true
cp -r "$ROOT/target/site" "$STAGE/site"
cp -r "$ROOT/deploy" "$STAGE/deploy"
cp "$ROOT/tools.lock.toml" "$ROOT/install.sh" "$ROOT/README.md" "$STAGE/"
echo "v${VER}-${SHA}" > "$STAGE/VERSION"
if [[ -f "$ROOT/target/sbom.cdx.json" ]]; then
  cp "$ROOT/target/sbom.cdx.json" "$STAGE/sbom.cdx.json"
  cp "$ROOT/target/sbom.cdx.json" "$OUT/$NAME.sbom.cdx.json"
fi
tar -C "$(dirname "$STAGE")" -czf "$OUT/$NAME.tar.gz" "$NAME"
(cd "$OUT" && sha256sum "$NAME.tar.gz" > "$NAME.tar.gz.sha256")
echo "$OUT/$NAME.tar.gz ($(du -h "$OUT/$NAME.tar.gz" | cut -f1)) v${VER}-${SHA}"
