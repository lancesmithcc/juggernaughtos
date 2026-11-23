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

Change the email the owner signs in with (for example after a test setup), from the box:

```bash
juggernaught owner set-email you@example.com
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

## Security operations

Seal the encryption and signing keys into systemd credentials (TPM2-bound when the machine has
one) after creating the owner's recovery key:

```bash
sudo juggernaught keys seal
```

```bash
sudo juggernaught keys status
```

Export the signed audit log and check it anywhere, against the key fingerprint `doctor` prints:

```bash
sudo juggernaught audit export > audit.json
```

```bash
juggernaught audit verify audit.json --fingerprint 0123:4567:…
```

Every release ships a CycloneDX SBOM (`sbom.cdx.json`, also published next to the tarball).

## Creative tools

Pinned in [tools.lock.toml](tools.lock.toml) with a SHA-256 per CPU; anything that doesn't match is refused.
The installer fetches them; manage them later with:

```bash
sudo juggernaught tools status
```

```bash
sudo juggernaught tools install --all
```

Built in (always available, no API keys): logo concepts and the full logo set, brand guidelines
PDF, social posts at every platform size, business cards (print-ready PDF), email signatures,
logo vectorizing, brand guide, CSS brand check. Photo-style images, textures and icons are made with
GPT Image in your own ChatGPT plan. Sign Codex in with your ChatGPT account once and ask in chat:
the image is made in the background and shows up in the chat and Assets by itself.

```bash
juggernaught connect codex-images
```

Without Codex you get an Open in ChatGPT button with the brand-aware prompt filled in; make it there
and drop the image back into the chat. An OpenAI API key (Settings) is a third option. PhotoCraft, VectorCraft, PdfCraft and impeccable install from their
upstream releases; HyperFrames video needs Node 20+; Kokoro voice and open-design are not yet
available as server binaries upstream.

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
2. **GPT Image through Codex or hand-off, not browser automation (decided).** The agent writes the
   prompt. With the official Codex CLI signed in to the owner's ChatGPT account, the image is made
   headless and comes back by itself (Codex's own image tool, the owner's plan limits apply).
   Otherwise the owner makes it in their own ChatGPT (one click opens it with the prompt filled in)
   and drops it back into the chat. Driving ChatGPT with Playwright was tried and dropped:
   ChatGPT's Cloudflare check and Google sign-in both refuse automated browsers, getting past them
   would mean disguising the automation, and OpenAI's terms restrict automated access anyway. The
   OpenAI Images API key is the automatic option (billed to the key, paced: ≤ 60 an hour).
3. **SQLite over PostgreSQL** (as chosen). Fine for one business per box; the schema stays
   portable if a hosted multi-tenant edition ever happens.
4. **Licences.** Creative suite is MIT/Apache-2.0 (redistributable). Google Fonts are
   OFL/Apache/UFL per family; we mirror and self-host, keeping each family's licence file.
   Vectorverter 5000 engine is vendored from the owner's own MIT project.
