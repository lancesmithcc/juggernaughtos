#!/usr/bin/env bash
# End-to-end smoke test of a release tarball on a clean Ubuntu 24.04 container:
# installer, app, steward with CAP_NET_ADMIN, honeytoken -> nftables ban, backups.
set -euo pipefail
cd "$(dirname "$0")/.."
ARCH=$(docker info --format '{{.Architecture}}'); [[ "$ARCH" == "arm64" ]] && ARCH=aarch64; [[ "$ARCH" == "amd64" ]] && ARCH=x86_64
TARBALL="dist/juggernaught-linux-${ARCH}.tar.gz"
[[ -f "$TARBALL" ]] || { echo "missing $TARBALL (run scripts/build-linux.sh)"; exit 1; }
docker run --rm --cap-add NET_ADMIN -v "$PWD/dist":/dist:ro -v "$PWD/install.sh":/install.sh:ro ubuntu:24.04 bash -euo pipefail -c '
  ok()   { printf "\033[32mPASS\033[0m %s\n" "$*"; }
  fail() { printf "\033[31mFAIL\033[0m %s\n" "$*"; exit 1; }
  apt-get update -qq && apt-get install -y -qq --no-install-recommends curl ca-certificates iproute2 util-linux >/dev/null
  JUGG_SKIP_TOOLS=1 bash /install.sh --tls proxy --tarball /dist/'"$(basename "$TARBALL")"' --no-systemd --no-ssh-hardening --yes
  C=/etc/juggernaught/juggernaught.toml
  sed -i "s/^listen_http = .*/listen_http = \"0.0.0.0:8080\"/" $C
  id jugg >/dev/null && id jugg-steward >/dev/null && ok "service users created" || fail "service users created"
  [[ "$(stat -c %a /var/lib/juggernaught/keys)" == 700 ]] && ok "keys dir 0700" || fail "keys dir 0700"
  nft list table inet juggernaught > /tmp/nft.txt; grep -q "policy drop" /tmp/nft.txt && ok "nftables table loaded (policy drop)" || fail "nftables table loaded (policy drop)"

  as_jugg() { runuser -u jugg -- env JUGG_CONFIG=$C LEPTOS_SITE_ROOT=/opt/juggernaught/current/site "$@"; }
  install -d -o jugg-steward -g jugg -m 0750 /run/juggernaught
  setpriv --reuid jugg-steward --regid jugg --init-groups --inh-caps +net_admin --ambient-caps +net_admin \
    env JUGG_CONFIG=$C /usr/local/bin/juggernaught steward >/tmp/steward.log 2>&1 &
  as_jugg /usr/local/bin/juggernaught serve >/tmp/app.log 2>&1 &
  for i in $(seq 1 30); do curl -s -o /dev/null http://127.0.0.1:8080/favicon.svg && break; sleep 1; done
  curl -sf -o /dev/null http://127.0.0.1:8080/favicon.svg && ok "app serving" || { cat /tmp/app.log; fail "app not serving"; }

  as_jugg /usr/local/bin/juggernaught doctor > /tmp/doctor.txt 2>&1 || { cat /tmp/doctor.txt; fail "doctor exited non-zero"; }
  grep -q "tls groups   X25519MLKEM768" /tmp/doctor.txt && ok "doctor: PQ-hybrid TLS group first" || fail "doctor: PQ-hybrid TLS group first"
  grep -q "audit chain  ok" /tmp/doctor.txt && ok "doctor: audit chain ok" || fail "doctor: audit chain ok"

  URL=$(as_jugg /usr/local/bin/juggernaught setup-url)
  [[ "$URL" == *"token="* ]] && ok "setup token issued" || fail "setup token issued"
  [[ "$(curl -s -o /dev/null -w %{http_code} http://127.0.0.1:8080/)" == 303 ]] && ok "admin gated (redirects to setup)" || fail "admin gated (redirects to setup)"
  curl -s http://127.0.0.1:8080/_site/ > /tmp/site.html; grep -q "coming soon" /tmp/site.html && ok "public placeholder site" || fail "public placeholder site"
  curl -sI http://127.0.0.1:8080/_site/ > /tmp/site.hdr; grep -qi "content-security-policy: default-src .none." /tmp/site.hdr && ok "public CSP strict" || fail "public CSP strict"

  sleep 2
  runuser -u jugg -- env JUGG_CONFIG=$C /usr/local/bin/juggernaught steward --status > /tmp/status.json; grep -q "\"firewall\": \"nftables\"" /tmp/status.json && ok "steward enforcing with nftables" || { cat /tmp/steward.log; fail "steward not enforcing"; }
  IP=$(hostname -I | awk "{print \$1}")
  curl -s -o /dev/null -m 3 "http://$IP:8080/.env" || true
  sleep 2
  nft list set inet juggernaught banned4 > /tmp/ban.txt; grep -q "$IP" /tmp/ban.txt && ok "honeytoken hit banned $IP at the firewall" || { cat /tmp/steward.log; nft list set inet juggernaught banned4; fail "no ban"; }

  as_jugg /usr/local/bin/juggernaught backup create >/tmp/backup.txt 2>&1 && ok "backup created" || { cat /tmp/backup.txt; fail "backup"; }
  as_jugg /usr/local/bin/juggernaught backup test > /tmp/rt.txt 2>&1; grep -q "restores cleanly" /tmp/rt.txt && ok "restore test passes" || fail "restore test passes"
  JUGG_CONFIG=$C /usr/local/bin/juggernaught tools install pdfcraft impeccable > /tmp/tools.txt 2>&1 && ok "pinned tools downloaded, hash-verified, smoke-tested" || { cat /tmp/tools.txt; fail "tool install"; }
  JUGG_CONFIG=$C /usr/local/bin/juggernaught tools verify > /tmp/verify.txt 2>&1 && ok "tool canaries pass" || { cat /tmp/verify.txt; fail "tool verify"; }
  JUGG_CONFIG=$C /usr/local/bin/juggernaught tools status > /tmp/status.txt; grep -q "pdfcraft .* ready" /tmp/status.txt && ok "tools status reports pdfcraft ready" || { cat /tmp/status.txt; fail "tools status"; }
  echo; echo "ALL SMOKE TESTS PASSED"
'
