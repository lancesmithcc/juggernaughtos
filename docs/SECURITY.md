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
- **No Vault/KMS**: a single box has no external KMS. KEKs start in `keys_dir` (0700 dir,
  0600 files, owned by `jugg`, outside the DB and outside backups of the DB). `juggernaught keys seal`
  then moves them, and the ML-DSA signing key, into **systemd encrypted credentials** bound to the
  TPM2 chip plus the host key (host key only when there is no TPM). See §8.
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
- Let's Encrypt issuance (TLS-ALPN-01) has not yet been exercised on a real public VPS.
- Unauthenticated requests for unknown admin paths redirect to sign-in rather than 404, so random-path
  scanning of the admin host scores lower than on the public host.
- Backups hold files in one archive; very large media libraries will want incremental backups (phase 6).
- webauthn-rs links OpenSSL; Linux installs need `libssl3`. Track a pure-Rust/aws-lc option.
- No external penetration test yet. Phase 6 is an internal review plus automated tests (§8); the
  roadmap's acceptance bar ("external pen-test, no high findings") needs an independent firm.

## 8. Phase 6 hardening review

Internal review of the whole system, done 2026-11-22. Each finding and what changed.

| # | Finding | Severity | Fix |
|---|---|---|---|
| 1 | `cargo audit` flagged advisories in the `sqlx 0.8` / bundled SQLite dependency tree | per advisory | `sqlx 0.9` + `libsqlite3-sys 0.40`; `cargo audit`: 0 vulnerabilities |
| 2 | Claude Code / Codex ran as `jugg` with full view of the filesystem: a prompt-injected agent could read `keys_dir`, the DB or other jobs through any tool the vendor CLI adds later | high | Agent CLIs run in **bubblewrap**: read-only system dirs, a private `agent-home`, the job dir as the only writable path, fresh PID/IPC/UTS namespaces, cleared environment, `/etc/juggernaught` masked (an integration test proves keys and the parent environment are invisible inside). Shell, file, web, task and skill tools are explicitly disallowed on top of `--allowedTools mcp__jugg` |
| 3 | The MCP server the CLI spawned was a second `juggernaught` process that opened the DB and keys itself | high | The CLI now spawns `juggernaught mcp-proxy`, which holds nothing: it relays stdio to the app's tool socket (`run/mcp.sock`, 0600). Each chat turn mints a 256-bit token, valid for that turn only; a wrong token gets a JSON-RPC error. The per-turn `.mcp.json` (mode 0600) is deleted when the run ends, even on error. All tool calls go through the app's one `Registry`, so approvals and audit are unchanged |
| 4 | Raw HTML blocks went through a hand-written stripper | medium | **ammonia** allow-list: fixed tag set, per-tag attributes, `https`/`mailto`/`tel` URLs only, `rel="noopener noreferrer"`. Property-tested against hostile fragment soups (handlers, `javascript:`/`data:` URLs, `srcdoc`, SVG/MathML, mutation-XSS shapes) |
| 5 | KEKs and the signing key were plaintext files: any read of `keys_dir` (backup misconfig, stolen disk, root shell) gave every encrypted field | medium | `juggernaught keys seal`: `systemd-creds encrypt` with `tpm2+host` (or `host`), round-trip checked, drop-ins add `LoadCredentialEncrypted=` to the app, backup and restore-test units, the service is restarted on the sealed keys, then the plaintext is overwritten and deleted. A sealed box refuses to mint new keys or run without its credentials. Admin commands run from a shell re-run themselves as a transient unit via `systemd-run` (root only). `keys unseal` reverses it for hardware moves; sealing requires the owner's recovery key to exist first |
| 6 | Audit checkpoints were signed but nothing outside the box could check them | medium | `juggernaught audit export` writes the log, checkpoints and a head signature; `juggernaught audit verify FILE --fingerprint …` checks the hash chain, every checkpoint signature, missing checkpoints and the head signature on any machine. `doctor` prints the signing-key fingerprint to record off-box, and now verifies checkpoint signatures too |
| 7 | Parsers of attacker-controlled input had example tests only | medium | **proptest** suites: field/file decryption (tampering, garbage), backup files, sshd journal lines, honeytoken paths, IPs headed for nftables, the brand gate (SVG/HTML/PDF/raster/CSS), chat markdown, the HTML sanitizer, page slugs, the MCP handler |
| 8 | No machine-readable inventory of what ships | low | CycloneDX 1.5 SBOM (`scripts/sbom.py`): every crate linked into the binary with its Cargo.lock checksum and licence, plus every pinned tool with its per-arch SHA-256. Shipped in the tarball and next to it as `*.sbom.cdx.json` |

### Images from the owner's ChatGPT (hand-off)

- No browser automation and no ChatGPT credentials on the box. `generate_image` stores a request
  (`image_handoffs`) and the chat shows an Open in ChatGPT link (`https://chatgpt.com/?q=` with the
  prompt URL-encoded); the owner makes the image in their own browser.
- The owner's image comes back through `POST /files/handoff/{id}`: session cookie required
  (`SameSite=Strict`, so no cross-site posts), 25 MB body limit, decoded with a 50-megapixel cap,
  re-encoded to PNG (drops EXIF and other metadata), filed through the normal brand gate into
  Assets, one image per request, audit-logged (`images.handoff_added`).
- The optional OpenAI Images API key is sealed like other provider secrets; API generations are
  paced (`image_generations`, `image_cooldowns`) and the pacing fails closed.
- An earlier Playwright worker that drove ChatGPT was removed: ChatGPT's Cloudflare check and
  Google sign-in block automated browsers, and we don't build bot-check evasion.

What sealing does not protect: a root attacker on the running box can read the decrypted credentials
from the service's ramfs or its memory. Sealing defends the disk, backups of the disk and offline
copies. Overwriting a file on an SSD does not guarantee the old blocks are gone; full-disk encryption
on the host covers that.

Accepted advisories (`cargo audit` warnings, no known vulnerability): `chrono` (RUSTSEC-2026-0543),
`paste` (RUSTSEC-2024-0436, compile-time only), `rustybuzz` (RUSTSEC-2026-0206) and `ttf-parser`
(RUSTSEC-2026-0192), all unmaintained. The last two come from the PDF/SVG renderer (usvg/resvg);
move when the upstream switches to `harfrust`/`skrifa`. `rsa` appears in Cargo.lock only through
sqlx's MySQL driver, which is never compiled.
