# Juggernaught Web Standards

Every site Juggernaught builds follows these rules. They exist so a site stays
secure, fast, accessible and on-brand no matter how many times the AI edits it.
The owner can override most rules; they always see why the rule exists first,
and the override is recorded (`standards_overrides` + signed audit log).

## How sites are built

1. **Pages are data.** A page is a list of typed blocks (`hero`, `section`, `cards`,
   `gallery`, `cta`, `contact`). The AI edits blocks; it never writes HTML, CSS or JS.
2. **Styling comes only from brand tokens.** The renderer emits brand CSS variables and a
   fixed component stylesheet that itself passes the token linter. Off-palette colours and
   off-brand fonts are errors.
3. **No JavaScript on public pages.** Forms post to the box; animations are CSS and respect
   `prefers-reduced-motion`.
4. **Everything self-hosted.** Images, fonts (from the local Google Fonts mirror, subset),
   styles. No third-party origins → strict CSP, no visitor tracking leaks.
5. **Static output, atomic deploys.** Each publish builds a new immutable directory,
   validates it, swaps a symlink, keeps the previous deploy for one-click rollback.

## Rules

Source of truth: `crates/jugg-web/src/validate.rs` (`RULES`).

| Rule | Level | Overridable in chat | Why |
|---|---|---|---|
| `seo.title` | error | yes | 10–60 char titles |
| `seo.description` | warn | yes | 50–160 char descriptions |
| `a11y.single_h1` | error | yes | exactly one hero/h1 per page |
| `a11y.alt_text` | error | yes | WCAG 2.2 1.1.1 |
| `a11y.link_text` | error | yes | descriptive link labels |
| `design.tone_rhythm` | warn | yes | alternate light/dark sections |
| `design.single_primary_cta` | warn | yes | one primary CTA per view |
| `perf.image_weight` | error | yes | ≤ 300 KB per image (pipeline re-encodes) |
| `perf.page_weight` | error | yes | ≤ 1 MB images per page |
| `sec.external_resource` | error | **no** | hotlinks break CSP and leak visitor data |
| `sec.link_scheme` | error | **no** | only https:, mailto:, tel:, relative |
| `sec.raw_html` | warn | yes | raw HTML is sanitized and logged |

"No" rules can only be changed in Settings → Advanced with typed confirmation.

## Design quality bar

- Alternate section tones; never two same-tone neighbours (accent excepted).
- Eyebrow label above major section headings; `clamp()` for display type.
- Cards: column layout, link pinned to the bottom.
- Grids collapse to one column on phones; 44 px minimum tap targets.
- Contrast ≥ 4.5:1 for body text and button labels (checked from tokens).
- Visible focus rings; skip link; landmarks.

## How it works in practice

- Ask in chat ("build my homepage", "add a pricing page", "put the bakery photo in the hero").
  The agent saves pages as drafts; you see each one at `/_preview/…` before anything goes live.
- Publishing always asks you: an **Approve** card appears in chat (or press **Publish** in Website).
- Every publish is a new immutable release; **Roll back** restores the previous one instantly.
- Contact forms land encrypted in **Website → Messages from your site**. A hidden honeypot field
  catches bots; each address can send five messages per 15 minutes.
- In development the public site is at `http://site.localhost:3700`.

## Publish pipeline

```
blocks ─▶ validate (rules + overrides) ─▶ render HTML + site.css
      ─▶ image pipeline (resize, AVIF/WebP, budgets) ─▶ font subset
      ─▶ token lint on CSS ─▶ link check ─▶ impeccable audit
      ─▶ owner approval (Risk::Publish) ─▶ atomic deploy ─▶ watchdog
```
