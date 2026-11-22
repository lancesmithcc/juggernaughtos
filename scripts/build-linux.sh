#!/usr/bin/env bash
# Build a Linux release tarball.
#  1. Frontend (WASM/JS/CSS, platform independent) with cargo-leptos on this machine.
#  2. Server binary inside Debian 12 (glibc 2.36) so it runs on Debian 12+ and Ubuntu 24.04+.
# Output: dist/juggernaught-linux-<arch>.tar.gz (+ .sha256). Arch = Docker host arch
# (use `DOCKER_DEFAULT_PLATFORM=linux/amd64` for x86_64 from an arm64 host; slow under emulation).
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p dist
cargo leptos build --release --frontend-only
docker run --rm \
  -v "$PWD":/src:ro -v "$PWD/dist":/out \
  -v jugg-cargo-registry:/usr/local/cargo/registry \
  -v "jugg-build-target-$(docker info --format '{{.Architecture}}')":/build/target \
  rust:1-bookworm bash -euo pipefail -c '
    apt-get update -qq && apt-get install -y -qq --no-install-recommends libssl-dev pkg-config clang git >/dev/null
    mkdir -p /build && cd /src && tar --exclude=./target --exclude=./dist --exclude=./.jugg-dev -cf - . | tar -C /build -xf -
    mkdir -p /build/target && rm -rf /build/target/site && cp -r /src/target/site /build/target/site
    cd /build
    LEPTOS_OUTPUT_NAME=juggernaught cargo build --release -p juggernaught --no-default-features --features ssr
    git config --global --add safe.directory /build
    bash scripts/package.sh /build /out
  '
