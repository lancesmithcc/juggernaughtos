# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

- **Business owners.** Owners and small teams (shops, gyms, studios, trades) who aren't technical. They run their brand, website and content from one chat-first workspace on their own server. Mostly on a laptop, sometimes on a phone.
- **Agencies and freelancers.** They set up and run Juggernaught boxes for several client businesses, and hand the workspace over to the owner.

## Product Purpose

Juggernaught OS is an all-in-one, AI-run business system. You install it on a VPS or a spare computer, and it does three jobs:
- It builds and hosts the business's website.
- It makes the business's brand assets: logos, social posts, email signatures, business cards, PDFs, images and video.
- It keeps everything on-brand, and secures, patches and backs itself up.

The owner talks to it in plain language. Success means the owner gets professional, on-brand work and a live website without learning tools or hiring someone, and without giving up ownership of their data.

## Positioning

One install is one business, on hardware the owner controls. Intelligence comes from the owner's own Claude Code or Codex subscription, or an API key. Everything is checked against the business's own machine-readable brand standards before it becomes final. Publishing always asks first.

## Operating Context

- **The admin.** Chat is the front door. Around it sit Assets, Website, Brand, Security, Settings and Go live.
- **Approvals.** Work arrives as drafts that the owner approves. Publishing and anything risky asks first.
- **Waiting on others.** Some steps need the owner to act outside Juggernaught: ChatGPT for images, Cloudflare for domains, a VPS host. The interface tells them clearly when something "needs you".
- **Sign-in.** Passkeys, email links, an authenticator app, or recovery codes.

## Capabilities and Constraints

- **Stack.** Leptos 0.8 with server-side rendering plus browser hydration, axum, SQLite. Styles live in `app/style/main.css`. No client-side framework beyond Leptos.
- **Brand standards.** These live in the app (`jugg_brand::BrandStandards`): palette, fonts, logo assets, voice, and imagery rules. They are versioned, and the brand gate checks every asset against them.
- **Fonts.** Google Fonts are mirrored and self-hosted. There are no third-party font or script CDNs, and the admin's CSP is strict.
- **Light and dark.** The interface follows the system setting.

## Brand Commitments

- **Name and mark.** The product is "Juggernaught OS". Its mark is the green tree-of-life logo (`public/juggernaught.svg`).
- **Juggernaught's own look** shows in two places:
  - always, on the sign-in and loading screens;
  - everywhere, until the business's brand is established.
- **The business's brand.** Once its brand standards exist (palette, fonts, logo), the workspace takes them on. The interface becomes the business's own studio.
- **Juggernaught's base personality** is a calm studio: quiet and confident, so the business's brand and its work are the hero.

## Evidence on Hand

- `public/juggernaught.svg` is the product mark.
- The brand standards and assets of each install are made in the app.
- There are no testimonials, customer logos or metrics. Don't invent any.

## Product Principles

1. **The owner's work is the hero.** The interface frames their brand and output; it doesn't compete with them.
2. **Say plainly what happens next.** Every step either happens by itself or clearly says it needs the owner.
3. **Nothing goes out without the owner.** Drafts, checks and approvals are visible and calm, not alarming.
4. **Their brand, their box.** The workspace looks like their business as soon as it has a brand.

## Accessibility & Inclusion

- **Users aren't technical**, so plain-language copy is required.
- **Contrast stays readable** whatever brand colours a business picks. The theme has to protect legibility, not just apply the palette.
