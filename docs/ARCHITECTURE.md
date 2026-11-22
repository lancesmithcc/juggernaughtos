# Juggernaught OS — Architecture

Self-hosted, single-tenant, AI-run business system. One install = one business.
It builds and hosts the website, produces every brand asset, and keeps itself
secure and up to date. The owner talks to it in plain language.

## 1. Shape of the system

```
                          Internet
                             │  443/tcp + 443/udp (TLS 1.3, X25519MLKEM768 preferred)
                             ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│ juggernaught  (one Rust binary, systemd, user `jugg`, no root)               │
│                                                                              │
│  security_layer ─ honeytokens → correlator, bans, CSP nonce, sec headers    │
│        │                                                                     │
│  ┌─────┴───────────────┐   ┌──────────────────────┐  ┌────────────────────┐  │
│  │ admin.<domain>      │   │ <domain>             │  │ /forms/* (POST)    │  │
│  │ Leptos SSR+hydrate  │   │ static site, built   │  │ honeypot + rate    │  │
│  │ chat, wizard, panels│   │ from validated blocks│  │ limit, encrypted   │  │
│  └─────┬───────────────┘   └──────────────────────┘  └────────────────────┘  │
│        │ server fns                                                          │
│  ┌─────▼──────────┐  ┌────────────┐  ┌──────────────┐  ┌──────────────────┐  │
│  │ jugg-ai        │─▶│ jugg-tools │─▶│ sandbox      │─▶│ photocraft, …    │  │
│  │ providers +    │  │ registry + │  │ (bubblewrap) │  │ (pinned, signed) │  │
│  │ agent loop     │  │ approvals  │  └──────────────┘  └──────────────────┘  │
│  └─────┬──────────┘  └─────▲──────┘                                          │
│        │ spawn             │ MCP stdio (`juggernaught mcp`)                  │
│        ▼                   │                                                 │
│  claude -p / codex exec ───┘   (built-in shell/file tools disabled)          │
│                                                                              │
│  jugg-db: SQLite WAL ── field-encrypted columns ── keys in /var/lib/…/keys   │
└──────────────────────────────────────────────────────────────────────────────┘
         │ unix socket (events in, status out)
┌────────▼─────────────────────────────────────────────────────────────────────┐
│ jugg-steward (separate unit + user, CAP_NET_ADMIN only, own SQLite)          │
│ sensors: http, sshd, canary, FIM, health, CVE, crowdsec, [falco], [suricata] │
│ correlator → nftables timed bans, incidents, AI triage, self-healing restarts│
│ maintenance timers: OS patches, platform+tool updates, backups, TLS, restore │
└──────────────────────────────────────────────────────────────────────────────┘
```

### Processes

| Process | Runs as | Privilege | Notes |
|---|---|---|---|
| `juggernaught serve` | `jugg` | none; `CAP_NET_BIND_SERVICE` for 80/443 | admin app, public site, API, job workers |
| `juggernaught mcp` | `jugg` | none | spawned per conversation by Claude Code / Codex; stdio only |
| `jugg-steward` | `jugg-steward` | `CAP_NET_ADMIN` | owns `steward.db`; app cannot delete security history |
| external tools | `jugg` inside bubblewrap | none | RO system, job dir only writable, no network unless declared, cgroup memory/CPU caps |
| `claude` / `codex` CLIs | `jugg` | none | private HOME, cwd = job dir, only our MCP server attached |

## 2. Workspace

| Crate | Responsibility |
|---|---|
| `app` (`juggernaught`) | Leptos admin UI, server functions, axum router, security middleware, CLI (`serve`, `mcp`, `doctor`) |
| `jugg-core` | Domain enums, config, risk/approval policy |
| `jugg-crypto` | Keyring (KEK outside DB, HKDF purpose keys), AES-256-GCM fields + chunked files, ML-KEM-768+X25519 hybrid wrap, ML-DSA-65 |
| `jugg-db` | SQLite WAL pool, embedded migrations, hash-chained signed audit log, job queue |
| `jugg-auth` | Magic links, passkeys (WebAuthn), TOTP w/ replay guard, Argon2id recovery codes, sessions |
| `jugg-brand` | Brand standards schema, CSS tokens, DESIGN.md export, token linter, WCAG contrast |
| `jugg-ai` | Provider trait; Claude Code CLI, Codex CLI, OpenRouter; our agent loop; prompts |
| `jugg-tools` | Tool trait + registry, approval enforcement, sandbox, `tools.lock.toml`, CLI adapters, MCP server |
| `jugg-web` | Page/Block schema, standards validator w/ overrides, static renderer, CSP |
| `jugg-steward` | Events, correlator, honeytokens, nftables, FIM, maintenance schedule |
| `jugg-updater` | Signed manifests, artifact verification, stage → canary → promote → rollback |
| `vectorverter-core` | Vendored Vectorverter 5000 engine, linked directly for logo vectorization |

## 3. Data

SQLite in WAL mode, one writer. Schema: [`migrations/0001_init.sql`](../migrations/0001_init.sql),
steward: [`migrations/steward/0001_steward.sql`](../migrations/steward/0001_steward.sql).

Encrypted at field level (`*_enc`, AES-256-GCM, AAD = `table:column:id`):
user emails, TOTP secrets, passkey credentials, chat messages and titles, provider API keys,
website form submissions. Equality lookups use HMAC blind indexes (`*_bidx`).
Private files (uploads, client documents) use `FileCipher` (per-file DEK, STREAM chunks).
Public brand assets and the public site are not encrypted (they are public by definition).

Never stored: passwords (none exist), raw magic-link/session tokens (SHA-256 only),
recovery codes (Argon2id only), keys (filesystem, 0600, outside the DB).

## 4. Intelligence

```
owner message ─▶ system prompt (brand DESIGN.md + business context + house rules)
              ─▶ Provider::run
                   ├─ ClaudeCodeCli: claude -p --output-format stream-json
                   │     --strict-mcp-config --mcp-config {jugg: juggernaught mcp}
                   │     --allowedTools mcp__jugg --disallowedTools Bash,Edit,Write,…
                   ├─ CodexCli:      codex exec --json --sandbox read-only
                   │     -c mcp_servers.jugg.command=juggernaught
                   └─ OpenRouter / APIs: agent_loop (≤24 steps) over the same Registry
              ─▶ AgentEvent stream (text, tool started/finished, approval, usage)
              ─▶ UI + encrypted transcript + usage table
```

`Registry::call` is the single choke point: `Risk::Publish` and `Risk::Destructive`
return `NeedsApproval` unless the owner approved that specific call, whichever
provider asked. Long work (video renders, batches, site builds, backups) goes
through the durable `jobs` queue with heartbeats and retries.

### Brand gate (draft → final)
1. Tool output lands as a `draft` asset.
2. Checks: token linter (colours/fonts), WCAG contrast, `design_audit` (impeccable),
   image checks (palette distance from brand, logo clear-space/min-size).
3. Pass → `review` (owner sees it) → owner approves → `final`. Fail → `rejected`
   with the report; the agent retries once automatically.

## 5. The creative suite

| Capability | Tool | Integration |
|---|---|---|
| Logo concepts, textures, icon sets, hero images | GPT Image (see §9 risks) | adapter: OpenAI Images API default; browser session optional for owner-operated boxes |
| Vectorize logo | Vectorverter 5000 core | linked crate, brand palette lock |
| Photo editing, compositing, social sizes | PhotoCraft | `photocraft-cli run/batch`, MCP for open-ended work |
| Vector layouts: business cards, posters, signatures | VectorCraft | `vectorcraft-cli run --export pdf/svg` |
| PDFs: brochures, proposals, print packs | PdfCraft | `pdfcraft-cli combine/extract/split` |
| Voice-over | Kokoros | `koko text … --timestamps` (word timings → captions) |
| Video | HyperFrames | HTML compositions → MP4 via headless Chrome + ffmpeg (job queue) |
| Design systems & prototypes | open-design | DESIGN.md generated from brand tokens |
| Design QA | impeccable | `design_audit` in the brand gate |
| Fonts | Google Fonts mirror | local mirror of google/fonts, subset + self-host; no Google CDN calls from sites |

## 6. Phased roadmap

| Phase | Scope | Acceptance |
|---|---|---|
| **0 — Skeleton** ✅ | Workspace, crypto, schema, auth primitives, tool registry + MCP, providers, web validator/renderer, steward core, updater core, admin shell (wizard, chat, panels) | `cargo test --workspace` green; wizard → chat works in browser; `doctor` shows PQ TLS group first |
| **1 — Real auth + streaming** ✅ | Passkey/TOTP/magic-link HTTP flows + WebAuthn JS, SMTP via lettre, setup token, session gate on pages + every server fn, sign-in rate limits, SSE streaming chat, sanitized markdown, provider settings + `juggernaught connect` | TOTP, magic link, recovery verified in browser; Claude Code → MCP bridge → tool verified live. Passkey ceremony needs a real authenticator test (options verified) |
| **2 — Install + TLS + steward daemon** ✅ | `install.sh` (Ubuntu 24.04+/Debian 12+), service users, hardened units, nftables, SSH/sysctl hardening, unattended-upgrades, rustls-acme (PQ-hybrid), host routing + public site, steward daemon (socket, sshd sensor, FIM, health, disk, backup age, timed bans), encrypted signed backups with owner key escrow + weekly restore test | Linux tarball installs on clean Ubuntu 24.04; honeytoken hit lands in the nftables ban set; backup + restore test pass (scripts/smoke-linux.sh). Real-VPS ACME issuance still to verify |
| **3 — Tools for real** ✅ | Hash-pinned tool installer (`juggernaught tools install/status/verify`, upstream release tarballs, canary before activation), only-installed tools offered to the agent, native print/email tools (business card PDF with bleed + TrimBox, email signature), brand gate (Lab ΔE colour checks, fonts, active-content checks for SVG/HTML/PDF), content-addressed asset library with previews, approve/override flow, background job queue + worker | Chat “make a business card” → Claude Code → MCP → PDF in Assets with gate report, approved in the UI; 17/17 Linux smoke checks incl. real pinned tool installs |
| **4 — Website builder** ✅ | Agent tools (site_pages, site_save_page, site_override, site_publish, site_rollback), owner previews, approval cards in chat for publish/rollback, immutable releases + atomic switch + rollback, image pipeline (approved assets only, ≤1600 px, JPEG/PNG), self-hosted brand fonts (bundled Jakarta, Google Fonts repo on demand), hashed CSS, sitemap/robots/404/thanks, favicon, encrypted contact-form inbox with honeypot + rate limit, starter homepage from the setup wizard | Chat → 2 pages → approval → live; Lighthouse 100/100/100/100 on the generated homepage (twice) |
| **5 — Brand studio** ✅ | Vector logo concepts from the brand (monogram, wordmark, badge, initials; text outlined, on-palette), adopt → primary/reversed/one-colour/icon + brand guidelines PDF (6 pages) + launch post; logo in site header, favicon and admin sidebar; social posts at four platform sizes; brand book regeneration; versioned brand history; optional image generation with the owner's OpenAI key; Google Fonts loaded into the print renderer on demand | Fresh wizard → pick logo → full kit in Assets; published starter site uses logo + brand fonts; Lighthouse 100×4 |
| **6 — Hardening** | TPM-sealed KEK / boot unlock, audit-chain export, pen-test, fuzzing of parsers, SBOM | External pen-test: no high findings |
