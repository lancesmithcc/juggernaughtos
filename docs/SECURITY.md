# Juggernaught OS — Security

Design goal: a small business gets the security posture of a hardened, monitored
server without ever having to think about it, and nothing in this box weakens if
large quantum computers arrive.

## 1. Post-quantum crypto stack

Adapted from `post-quantum-stack.md`. One crypto provider in the binary: **aws-lc-rs**.

| Layer | Choice | Where |
|---|---|---|
| Network | TLS 1.3, hybrid **X25519MLKEM768** first, classical fallback | rustls 0.23 + aws-lc-rs, `prefer-post-quantum` (verified by `juggernaught doctor` and a unit test) |
| Field data | **AES-256-GCM**, AAD-bound to `table:column:id`, purpose keys via HKDF-SHA256 | `jugg-crypto::seal/open` |
| Files | AES-256-GCM STREAM chunks (64 KiB), per-file DEK wrapped by KEK | `jugg-crypto::FileCipher` |
| Keys leaving the box | **ML-KEM-768 + X25519** hybrid wrap (both must break) | backups → owner recovery key; future multi-box |
| Signatures | **ML-DSA-65** | update manifests, audit-log checkpoints, backup manifests |
| Recovery codes | Argon2id | `jugg-auth::recovery` |
| Sign-in | Passkeys / WebAuthn, TOTP, magic links | no passwords exist |

Deviations from the reference doc, on purpose:
- **SQLite, not PostgreSQL** (owner decision): single-node, zero-ops. Isolation is per install.
- **No Vault/KMS**: a single box has no external KMS. KEKs live in `keys_dir` (0700 dir,
  0600 files, owned by `jugg`, outside the DB and outside backups of the DB).
  Phase 6 adds TPM2 sealing when present, or an Argon2id-derived unlock passphrase at boot.
- **Argon2id is not for passwords** (there are none) — only for recovery codes.

## 2. Authentication

- **Magic link**: 256-bit token, SHA-256 stored, single use (atomic `UPDATE … RETURNING`),
  15 min, issuing a new link revokes older ones. Same response for known/unknown emails.
- **Passkeys**: webauthn-rs, user verification required; credentials stored encrypted;
  signature counters updated on use.
- **TOTP**: RFC 6238, secret encrypted, ±1 step skew, each step usable once (replay guard),
  must be confirmed before it can be used to log in.
- **Recovery codes**: 10 × ~50-bit codes, Argon2id, single use, regenerated on demand.
- **Sessions**: `__Host-` cookie, `Secure; HttpOnly; SameSite=Strict`, SHA-256 at rest,
  30-day absolute / 7-day idle, revocable per device.
- **Policy**: owners keep ≥ 2 methods (or 1 + recovery codes).

## 3. Web surface

- Admin CSP: `default-src 'none'`; scripts only `'self'` + per-request nonce + `'wasm-unsafe-eval'`;
  styles `'self'` (+ `style-src-attr 'unsafe-inline'` for CSS custom-property attributes only);
  `frame-ancestors 'none'`, `base-uri 'none'`, `form-action 'self'`.
- Public site CSP: `default-src 'none'`, everything `'self'`, **no scripts at all**.
- Headers: HSTS (preload), nosniff, strict referrer, permissions-policy, COOP/CORP, XFO DENY.
- Sites are rendered from typed blocks; all text escaped; raw HTML only via an owner
  override, sanitized and logged.
- Admin is served on `admin.<domain>`; public site on `<domain>`; separate origins.

## 4. The Steward (automated security + maintenance)

Modelled on the owner's reference hardened server (fail2ban, UFW, unattended-upgrades,
key-only SSH, Falco, Suricata, OpenCanary + honeytoken files + decoy MCP, a confidence-scoring
correlator, Wazuh), redesigned to fit a 4 GB VPS:

| Reference box | Juggernaught |
|---|---|
| UFW deny-incoming | nftables `inet juggernaught`, policy drop, 80/443 + rate-limited/allow-listed SSH |
| fail2ban | correlator: decaying per-IP score → timed nftables set bans (kernel-side expiry) |
| OpenCanary + canary files + decoy MCP | honeytoken routes in the web app (`/.env`, `/.git/`, `/wp-login.php`, robots.txt bait…), canary files, decoy credentials |
| Security Steward correlator | `jugg-steward::correlator` (confidence × severity, half-life decay, allow-list) |
| Falco (eBPF) | optional, auto-enabled ≥ 7.5 GB RAM |
| Suricata | optional, auto-enabled ≥ 15 GB RAM |
| Wazuh manager | not bundled (4–8 GB); optional agent forwarding to an external manager |
| unattended-upgrades | kept, plus signed platform/tool updates with canary + rollback |
| watchdog timers | site watchdog every 5 min, self-healing restarts |
| — | FIM on release/config/units, CVE scan of every pinned tool, nightly encrypted backups + weekly restore test, TLS expiry, signed audit-log verification |

Incidents get an AI-written, plain-language triage note (redacted: no secrets, no customer data
sent to the model).

## 5. Install-time hardening (phase 2 `install.sh`)

- Dedicated users `jugg`, `jugg-steward`; no login shells.
- SSH: key-only, `PermitRootLogin no`, `MaxAuthTries 3`, forwarding off, optional Tailscale-only.
- unattended-upgrades (security pocket), automatic reboot in the maintenance window.
- systemd sandboxing on every unit: `ProtectSystem=strict`, `ProtectHome=yes`, `PrivateTmp`,
  `NoNewPrivileges`, `RestrictAddressFamilies`, `SystemCallFilter=@system-service`,
  `MemoryMax`, `CPUQuota`.
- bubblewrap for every external tool.
- Kernel: `kernel.unprivileged_bpf_disabled=1`, `net.ipv4.tcp_syncookies=1`, rp_filter.

## 6. Backups

- Nightly (`juggernaught-backup.timer`): online SQLite snapshot + assets + site + config, streamed through
  AES-256-GCM (64 KiB STREAM chunks) — never loaded into memory.
- The data key is wrapped to the box (routine restore tests) **and** to the owner's offline recovery key
  (ML-KEM-768 + X25519). The box KEKs are escrowed the same way, so a restore on a new machine can read
  every field-encrypted value. The owner's recovery key never lives on the server.
- Every backup is signed (ML-DSA-65 over header ‖ body hash); tampering, truncation, path traversal and
  foreign signers are rejected.
- Weekly restore test (`juggernaught-restore-test.timer`) decrypts the newest backup, re-hashes every file
  and runs `PRAGMA integrity_check`. Optional off-box copy via `rclone`.

## 7. Known gaps (honest list)

- Passkey registration/sign-in is implemented end to end but has only been exercised up to the
  WebAuthn options; a real-authenticator test (phone + laptop) is still owed.
- Fresh-install takeover: until the owner finishes setup, `/setup` requires the one-time token from
  `juggernaught setup-url` (written 0600 to `keys_dir/setup-token`, deleted after setup). Dev mode skips it.
- Sign-in limits: 10 failures per IP and per account per 15 minutes; failures also feed the steward correlator.
- The raw-HTML sanitizer is a conservative stripper; phase 4 replaces it with an allow-list parser.
- Let's Encrypt issuance (TLS-ALPN-01) has not yet been exercised on a real public VPS.
- Unauthenticated requests for unknown admin paths redirect to sign-in rather than 404, so random-path
  scanning of the admin host scores lower than on the public host.
- Backups hold files in one archive; very large media libraries will want incremental backups (phase 6).
- webauthn-rs links OpenSSL; Linux installs need `libssl3`. Track a pure-Rust/aws-lc option.
