# Juggernaught OS

An all-in-one, AI-run business system you install on a VPS or a spare computer.
It builds and hosts your website, makes your brand assets (logos, social posts,
videos, email signatures, business cards, posters, PDFs, icons, textures), keeps
everything on-brand, and secures, patches and backs itself up.

Rust everywhere: Leptos (SSR + hydrate) admin, Axum server, SQLite (WAL),
aws-lc-rs post-quantum crypto. Intelligence comes from your own Claude Code or
Codex subscription, or an OpenRouter key.

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): system shape, crates, data, AI, roadmap
- [docs/SECURITY.md](docs/SECURITY.md): PQ crypto, passwordless auth, the Steward
- [docs/WEB_STANDARDS.md](docs/WEB_STANDARDS.md): how sites are built, rules, overrides
- [tools.lock.toml](tools.lock.toml): pinned creative suite

## Run it (development)

```bash
cargo install cargo-leptos --locked
```

```bash
cargo leptos serve
```

Open http://localhost:3700 (use `localhost`, not `127.0.0.1`: passkeys need a domain).
The first visit opens the setup wizard. Dev data lives in `./.jugg-dev` (delete it to
start over). Without SMTP, magic links are printed in the server log.

```bash
cargo test --workspace
```

```bash
./target/debug/juggernaught doctor
```

Connect intelligence in **Settings → Intelligence**, or from the server shell:

```bash
./target/debug/juggernaught connect claude
```

`connect` accepts `claude`, `codex` (each runs the vendor's own login) or `openrouter`
(asks for a key, stored encrypted). On a fresh production box, get the one-time setup link with:

```bash
juggernaught setup-url
```

## Install on a server

On a fresh Ubuntu 24.04+ or Debian 12+ machine with DNS for `example.com`, `www.` and `admin.`
pointing at it:

```bash
curl -fsSLO https://github.com/lancesmithcc/juggernaughtos/releases/latest/download/install.sh
```

```bash
sudo bash install.sh --domain example.com --email you@example.com
```

Behind a tunnel (Cloudflare Tunnel, Tailscale Funnel, a reverse proxy) use `--tls proxy`, and the app
listens on `127.0.0.1:8080`. `--ssh-allow 100.64.0.0/10` limits SSH to your tailnet. The installer
prints a one-time setup link and the command for the owner's offline recovery key.

Build a release tarball yourself (needs Docker):

```bash
scripts/build-linux.sh
```

```bash
scripts/smoke-linux.sh
```

## Hardware

The LLM runs remotely (subscription or API), so no GPU is needed. What costs
resources is rendering: HyperFrames video (headless Chrome + ffmpeg), PhotoCraft
on large layered files, and optional heavy sensors.

| Tier | CPU | RAM | Disk | Good for |
|---|---|---|---|---|
| **Minimum** | 2 vCPU (x86-64 or arm64) | 4 GB + 2 GB swap | 40 GB SSD | Website, chat, images, PDFs, voice. Videos render slowly (≈ 3–5× real time). Steward core sensors only. |
| **Recommended** | 4 vCPU | 8 GB | 100 GB NVMe | Everything comfortably; one video render at a time at ≈ real time; Falco runtime detection enabled. |
| **Video-heavy / agency** | 8+ vCPU | 16–32 GB | 250 GB+ NVMe | Parallel renders, large PSDs, Suricata IDS enabled, long asset history. |

Per-component budget (peak): Juggernaught server 150–300 MB · SQLite negligible ·
Kokoro TTS ~0.5–1 GB · HyperFrames render 1.5–3 GB per worker · PhotoCraft 0.5–4 GB
(document size) · Node runtime for open-design/HyperFrames ~300 MB · Claude Code /
Codex CLI ~300–500 MB per active session · Falco ~0.5–1 GB · Suricata 1.5–3 GB.
Disk: Google Fonts mirror ~1.2 GB, Kokoro model ~350 MB, Chrome ~400 MB, toolchain
builds happen in our release pipeline, not on the box.

OS: Ubuntu 24.04 LTS or Debian 13, 64-bit. A public IPv4 (or a tunnel) and a domain.
A spare computer works the same; a UPS and an off-site backup target are strongly recommended.

## Decisions to confirm

1. **Consumer subscriptions in a distributed product.** Anthropic's terms do not allow
   third-party products to offer Claude.ai subscription login unless approved; OpenAI's
   Codex/ChatGPT terms are similarly scoped. Recommended stance: the box's owner installs
   and signs in to the vendor CLI themselves (`juggernaught connect` just runs the vendor's
   own login), Juggernaught never handles those credentials, and API-key providers
   (OpenRouter / Anthropic / OpenAI) are the default for anything resold or managed.
   Get a quick legal read before selling managed installs on subscriptions.
2. **GPT Image via browser automation.** Driving a logged-in ChatGPT web session with
   Playwright breaks OpenAI's terms on automated access and is brittle. Default adapter:
   OpenAI Images API (`gpt-image` models) with the owner's key, fallback fal.ai/OpenRouter
   image models. The Playwright path stays available only as an opt-in for owner-operated boxes.
3. **SQLite over PostgreSQL** (as chosen). Fine for one business per box; the schema stays
   portable if a hosted multi-tenant edition ever happens.
4. **Licences.** Creative suite is MIT/Apache-2.0 (redistributable). Google Fonts are
   OFL/Apache/UFL per family; we mirror and self-host, keeping each family's licence file.
   Vectorverter 5000 engine is vendored from the owner's own MIT project.
