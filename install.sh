#!/usr/bin/env bash
# Juggernaught OS installer — Ubuntu 24.04+ / Debian 12+ (x86_64, aarch64).
#
#   curl -fsSLO https://github.com/lancesmithcc/juggernaughtos/releases/latest/download/install.sh
#   sudo bash install.sh --domain example.com --email you@example.com
#
# Idempotent: re-running upgrades the release and leaves data, keys and config alone.
# Options:
#   --domain D          apex domain (admin at admin.D). Omit with --tls proxy for tunnels.
#   --email E           Let's Encrypt contact
#   --tls acme|proxy    acme (default): built-in HTTPS on :443. proxy: plain HTTP on
#                       127.0.0.1:8080 for a tunnel/reverse proxy (e.g. Cloudflare Tunnel).
#   --tarball PATH      install this release tarball instead of downloading
#   --version vX.Y.Z    release to download (default: latest)
#   --ssh-allow CIDR    only allow SSH from this range (repeatable; e.g. 100.64.0.0/10 for Tailscale)
#   --no-firewall       don't touch nftables
#   --no-ssh-hardening  don't touch sshd
#   --no-systemd        install files only (containers / CI)
#   --staging           Let's Encrypt staging certificates (testing)
#   --yes               don't prompt
set -euo pipefail

REPO="lancesmithcc/juggernaughtos"
PREFIX=/opt/juggernaught
ETC=/etc/juggernaught
DATA=/var/lib/juggernaught

DOMAIN="" EMAIL="" TLS="acme" TARBALL="" VERSION="latest" YES=0 STAGING=false
FIREWALL=1 SSH_HARDEN=1 SYSTEMD=1
SSH_ALLOW=()

say()  { printf '\033[1m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[33mwarning:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --domain) DOMAIN="${2:?}"; shift 2 ;;
    --email) EMAIL="${2:?}"; shift 2 ;;
    --tls) TLS="${2:?}"; shift 2 ;;
    --tarball) TARBALL="${2:?}"; shift 2 ;;
    --version) VERSION="${2:?}"; shift 2 ;;
    --ssh-allow) SSH_ALLOW+=("${2:?}"); shift 2 ;;
    --no-firewall) FIREWALL=0; shift ;;
    --no-ssh-hardening) SSH_HARDEN=0; shift ;;
    --no-systemd) SYSTEMD=0; shift ;;
    --staging) STAGING=true; shift ;;
    --yes|-y) YES=1; shift ;;
    -h|--help) sed -n '2,22p' "$0"; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
done

# ------------------------------------------------------------------ preflight
[[ $EUID -eq 0 ]] || die "run as root (sudo bash install.sh …)"
[[ "$TLS" == "acme" || "$TLS" == "proxy" ]] || die "--tls must be acme or proxy"
if [[ "$TLS" == "acme" ]]; then
  [[ -n "$DOMAIN" ]] || die "--domain is required with --tls acme (or use --tls proxy)"
  [[ "$DOMAIN" =~ ^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$ ]] || die "--domain looks invalid: $DOMAIN"
  [[ -n "$EMAIL" ]] || die "--email is required for Let's Encrypt"
fi

. /etc/os-release
case "$ID:${VERSION_ID%%.*}" in
  ubuntu:2[4-9]|debian:1[2-9]) ;;
  *) [[ $YES -eq 1 ]] || die "unsupported OS ($PRETTY_NAME). Ubuntu 24.04+ or Debian 12+ required (--yes to try anyway)" ;;
esac
ARCH=$(uname -m); case "$ARCH" in x86_64|aarch64) ;; *) die "unsupported CPU architecture: $ARCH" ;; esac

MEM_MB=$(awk '/MemTotal/ {print int($2/1024)}' /proc/meminfo)
(( MEM_MB >= 3500 )) || warn "only ${MEM_MB} MB RAM: 4 GB is the minimum; video rendering will struggle"
FREE_GB=$(df -Pk / | awk 'NR==2 {print int($4/1048576)}')
(( FREE_GB >= 20 )) || warn "only ${FREE_GB} GB free disk: 40 GB recommended"

if [[ $SYSTEMD -eq 1 && "$TLS" == "acme" ]]; then
  for p in 80 443; do
    if ss -ltnH "sport = :$p" 2>/dev/null | grep -q . && ! systemctl is-active -q juggernaught 2>/dev/null; then
      die "port $p is already in use (another web server?). Stop it or use --tls proxy."
    fi
  done
fi

say "Installing Juggernaught OS on $PRETTY_NAME ($ARCH, ${MEM_MB} MB RAM)"

# ------------------------------------------------------------------ packages
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends \
  ca-certificates curl tar nftables bubblewrap sqlite3 unattended-upgrades libssl3 >/dev/null \
  || apt-get install -y -qq --no-install-recommends ca-certificates curl tar nftables bubblewrap sqlite3 unattended-upgrades libssl3t64 >/dev/null

# ------------------------------------------------------------------ users + dirs
id jugg >/dev/null 2>&1 || useradd --system --home-dir "$DATA" --shell /usr/sbin/nologin --user-group jugg
id jugg-steward >/dev/null 2>&1 || useradd --system --home-dir /var/lib/juggernaught-steward --shell /usr/sbin/nologin --no-create-home --user-group jugg-steward
getent group systemd-journal >/dev/null && usermod -aG systemd-journal jugg-steward

install -d -m 0755 -o root -g root "$PREFIX" "$PREFIX/releases"
install -d -m 0750 -o root -g jugg "$ETC"
install -d -m 0750 -o jugg -g jugg "$DATA" "$DATA/backups"
install -d -m 0700 -o jugg -g jugg "$DATA/keys"
install -d -m 0700 -o jugg-steward -g jugg-steward /var/lib/juggernaught-steward

# ------------------------------------------------------------------ release
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
if [[ -z "$TARBALL" ]]; then
  ASSET="juggernaught-linux-${ARCH}.tar.gz"
  if [[ "$VERSION" == "latest" ]]; then URL="https://github.com/$REPO/releases/latest/download"; else URL="https://github.com/$REPO/releases/download/$VERSION"; fi
  say "Downloading $ASSET ($VERSION)"
  curl -fsSL --proto '=https' --tlsv1.2 -o "$WORK/$ASSET" "$URL/$ASSET"
  curl -fsSL --proto '=https' --tlsv1.2 -o "$WORK/$ASSET.sha256" "$URL/$ASSET.sha256"
  TARBALL="$WORK/$ASSET"
fi
[[ -f "$TARBALL" ]] || die "tarball not found: $TARBALL"
if [[ -f "$TARBALL.sha256" ]]; then
  (cd "$(dirname "$TARBALL")" && sha256sum -c --status "$(basename "$TARBALL").sha256") || die "checksum mismatch for $TARBALL"
  say "Checksum verified"
else
  warn "no $TARBALL.sha256 next to the tarball; skipping checksum (local builds only)"
fi

tar -xzf "$TARBALL" -C "$WORK"
SRC=$(find "$WORK" -maxdepth 2 -name VERSION -printf '%h\n' | head -1)
[[ -n "$SRC" && -x "$SRC/bin/juggernaught" ]] || die "tarball does not look like a Juggernaught release"
REL=$(tr -d '[:space:]' < "$SRC/VERSION")
DEST="$PREFIX/releases/$REL"
rm -rf "$DEST"; mkdir -p "$DEST"; cp -a "$SRC/." "$DEST/"
chown -R root:root "$DEST"; chmod -R go-w "$DEST"
ln -sfn "$DEST" "$PREFIX/current.tmp" && mv -T "$PREFIX/current.tmp" "$PREFIX/current"
ln -sfn "$PREFIX/current/bin/juggernaught" /usr/local/bin/juggernaught
say "Installed release $REL"

# ------------------------------------------------------------------ config (never overwritten)
CONF="$ETC/juggernaught.toml"
if [[ ! -f "$CONF" ]]; then
  {
    echo "# Juggernaught OS configuration. No secrets in this file."
    [[ -n "$DOMAIN" ]] && echo "domain = \"$DOMAIN\""
    [[ -n "$EMAIL" ]] && echo "acme_email = \"$EMAIL\""
    echo "data_dir = \"$DATA\""
    echo "keys_dir = \"$DATA/keys\""
    echo "tools_dir = \"$PREFIX/tools\""
    if [[ "$TLS" == "proxy" ]]; then echo 'listen_http = "127.0.0.1:8080"'; else echo 'listen_http = "0.0.0.0:80"'; fi
    echo 'listen_https = "0.0.0.0:443"'
    echo 'steward_socket = "/run/juggernaught/steward.sock"'
    echo 'steward_dir = "/var/lib/juggernaught-steward"'
    echo
    echo "[tls]"
    echo "mode = \"$TLS\""
    echo "staging = $STAGING"
    echo
    echo "[backup]"
    echo "dir = \"$DATA/backups\""
    echo "keep = 7"
    echo "recovery_public_key = \"$ETC/recovery.pub.json\""
    echo "# rclone_remote = \"offsite:bucket/juggernaught\""
  } > "$CONF"
  chown root:jugg "$CONF"; chmod 0640 "$CONF"
  say "Wrote $CONF"
fi

# ------------------------------------------------------------------ firewall
if [[ $FIREWALL -eq 1 ]]; then
  SSH_PORT=$(sshd -T 2>/dev/null | awk '/^port / {print $2; exit}'); SSH_PORT=${SSH_PORT:-22}
  ARGS=(--ssh-port "$SSH_PORT"); for a in "${SSH_ALLOW[@]}"; do ARGS+=(--ssh-allow "$a"); done
  install -d /etc/nftables.d
  /usr/local/bin/juggernaught firewall "${ARGS[@]}" > "$WORK/juggernaught.nft"
  nft -c -f "$WORK/juggernaught.nft" || die "generated firewall rules failed validation"
  install -m 0640 "$WORK/juggernaught.nft" /etc/nftables.d/juggernaught.nft
  grep -q 'include "/etc/nftables.d/\*.nft"' /etc/nftables.conf 2>/dev/null || echo 'include "/etc/nftables.d/*.nft"' >> /etc/nftables.conf
  nft list table inet juggernaught >/dev/null 2>&1 && nft delete table inet juggernaught
  nft -f /etc/nftables.d/juggernaught.nft
  [[ $SYSTEMD -eq 1 ]] && systemctl enable -q nftables
  say "Firewall: inbound denied except 80/443 and SSH on port $SSH_PORT${SSH_ALLOW:+ from ${SSH_ALLOW[*]}}"
fi

# ------------------------------------------------------------------ SSH hardening (only if keys exist)
if [[ $SSH_HARDEN -eq 1 && -d /etc/ssh/sshd_config.d ]]; then
  if compgen -G "/root/.ssh/authorized_keys" >/dev/null || compgen -G "/home/*/.ssh/authorized_keys" >/dev/null; then
    cat > /etc/ssh/sshd_config.d/10-juggernaught.conf <<'SSHD'
# Installed by Juggernaught OS
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin prohibit-password
MaxAuthTries 3
X11Forwarding no
AllowAgentForwarding no
SSHD
    if sshd -t 2>/dev/null; then
      [[ $SYSTEMD -eq 1 ]] && (systemctl reload ssh 2>/dev/null || systemctl reload sshd 2>/dev/null || true)
      say "SSH: key-only sign-in, no passwords"
    else
      rm -f /etc/ssh/sshd_config.d/10-juggernaught.conf
      warn "sshd rejected the hardening drop-in; left SSH unchanged"
    fi
  else
    warn "no SSH keys found for any user; leaving password SSH on so you are not locked out. Add a key, then re-run."
  fi
fi

# ------------------------------------------------------------------ kernel + updates
cat > /etc/sysctl.d/90-juggernaught.conf <<'SYSCTL'
kernel.kptr_restrict = 2
kernel.dmesg_restrict = 1
kernel.unprivileged_bpf_disabled = 1
net.core.bpf_jit_harden = 2
net.ipv4.tcp_syncookies = 1
net.ipv4.conf.all.rp_filter = 1
net.ipv4.conf.default.rp_filter = 1
net.ipv4.conf.all.accept_redirects = 0
net.ipv6.conf.all.accept_redirects = 0
net.ipv4.conf.all.send_redirects = 0
net.ipv4.conf.all.accept_source_route = 0
SYSCTL
sysctl -q --system >/dev/null 2>&1 || true

cat > /etc/apt/apt.conf.d/20auto-upgrades <<'APT'
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
APT
cat > /etc/apt/apt.conf.d/52juggernaught-upgrades <<'APT'
Unattended-Upgrade::Automatic-Reboot "true";
Unattended-Upgrade::Automatic-Reboot-Time "04:30";
APT

# ------------------------------------------------------------------ services
if [[ $SYSTEMD -eq 1 ]]; then
  for u in juggernaught.service jugg-steward.service juggernaught-backup.service juggernaught-backup.timer juggernaught-restore-test.service juggernaught-restore-test.timer; do
    install -m 0644 "$PREFIX/current/deploy/systemd/$u" "/etc/systemd/system/$u"
  done
  [[ -d /etc/polkit-1/rules.d ]] && install -m 0644 "$PREFIX/current/deploy/polkit/50-juggernaught.rules" /etc/polkit-1/rules.d/50-juggernaught.rules
  systemctl daemon-reload
  systemctl enable -q --now jugg-steward.service juggernaught.service juggernaught-backup.timer juggernaught-restore-test.timer
  systemctl restart jugg-steward.service juggernaught.service
  say "Services running"
fi

# ------------------------------------------------------------------ done
echo
say "Juggernaught OS is installed."
if [[ "$TLS" == "acme" ]]; then
  echo "    Point DNS A/AAAA records for $DOMAIN, www.$DOMAIN and admin.$DOMAIN at this server."
  echo "    Certificates are issued automatically on first visit."
else
  echo "    Point your tunnel/proxy at http://127.0.0.1:8080 (admin.<domain> and <domain>)."
fi
echo "    Finish setup (one-time link):"
if [[ $SYSTEMD -eq 1 ]]; then
  for _ in $(seq 1 20); do sudo -u jugg JUGG_CONFIG="$CONF" /usr/local/bin/juggernaught setup-url 2>/dev/null | grep -q token && break; sleep 1; done
  sudo -u jugg JUGG_CONFIG="$CONF" /usr/local/bin/juggernaught setup-url || true
else
  echo "    sudo -u jugg JUGG_CONFIG=$CONF juggernaught setup-url"
fi
echo
echo "    Then create the owner's offline recovery key, copy it OFF this server, and delete it here:"
echo "    sudo JUGG_CONFIG=$CONF juggernaught backup init-recovery --out /root/juggernaught-recovery-key.json"
