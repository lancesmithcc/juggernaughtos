---
name: Juggernaught OS
description: The business's own standards manual, opened to the page you need.
colors:
  juggernaught-green: "#9fcf6b"
  juggernaught-green-text: "#3b6b1f"
  juggernaught-green-text-dark: "#a9d77a"
  ink: "#111111"
  ink-soft: "#4d4d4d"
  ink-faint: "#6b6b6b"
  page: "#ffffff"
  page-sunk: "#f3f3f1"
  rule: "#e3e3e0"
  rule-strong: "#c9c9c5"
  rail: "#111111"
  rail-ink: "#f4f4f2"
  rail-soft: "#a3a3a0"
  rail-rule: "#2a2a2a"
  work-ground: "#f1f1ef"
  ok: "#1f7a45"
  warn: "#8f5600"
  warn-marker: "#e0a64b"
  err: "#b3261e"
  dark-ink: "#f2f2ef"
  dark-page: "#151515"
  dark-page-sunk: "#0f0f0f"
  dark-raised: "#1c1c1c"
  dark-rule: "#2d2d2b"
  dark-rail: "#000000"
typography:
  plate:
    fontFamily: "Archivo, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(34px, 4.6vw, 60px)"
    fontWeight: 800
    lineHeight: 0.98
    letterSpacing: "-0.03em"
    fontVariation: "'wdth' 125"
  plate-number:
    fontFamily: "Archivo, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(34px, 4.6vw, 60px)"
    fontWeight: 300
    lineHeight: 0.98
    letterSpacing: "-0.02em"
    fontVariation: "'wdth' 125"
  plate-slim:
    fontFamily: "Archivo, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(24px, 2.6vw, 34px)"
    fontWeight: 800
    lineHeight: 0.98
    letterSpacing: "-0.03em"
    fontVariation: "'wdth' 125"
  headline:
    fontFamily: "Archivo, ui-sans-serif, system-ui, sans-serif"
    fontSize: "22px"
    fontWeight: 800
    lineHeight: 1.15
    fontVariation: "'wdth' 112"
  title:
    fontFamily: "Archivo, ui-sans-serif, system-ui, sans-serif"
    fontSize: "18px"
    fontWeight: 750
    letterSpacing: "-0.01em"
    fontVariation: "'wdth' 108"
  section-head:
    fontFamily: "Archivo, ui-sans-serif, system-ui, sans-serif"
    fontSize: "13px"
    fontWeight: 750
    letterSpacing: "0.06em"
    fontVariation: "'wdth' 110"
  body:
    fontFamily: "Archivo, ui-sans-serif, system-ui, sans-serif"
    fontSize: "15px"
    fontWeight: 400
    lineHeight: 1.55
    fontFeature: "'tnum'"
  label:
    fontFamily: "Archivo, ui-sans-serif, system-ui, sans-serif"
    fontSize: "12px"
    fontWeight: 750
    letterSpacing: "0.06em"
  mono:
    fontFamily: "ui-monospace, SF Mono, Menlo, Consolas, monospace"
    fontSize: "12px"
    fontWeight: 600
rounded:
  tag: "2px"
  base: "4px"
  overlay: "6px"
spacing:
  grid: "8px"
  inset: "clamp(20px, 5vw, 56px)"
  rail: "232px"
  s-1: "8px"
  s-2: "16px"
  s-3: "24px"
  s-4: "32px"
  s-5: "40px"
components:
  plate:
    backgroundColor: "{colors.juggernaught-green}"
    textColor: "{colors.ink}"
    typography: "{typography.plate}"
    padding: "32px clamp(20px, 5vw, 56px) 24px"
    height: "176px"
  button-primary:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.page}"
    rounded: "{rounded.base}"
    padding: "0 18px"
    height: "42px"
  button-primary-hover:
    backgroundColor: "{colors.juggernaught-green-text}"
    textColor: "{colors.page}"
  button-ghost:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    rounded: "{rounded.base}"
    padding: "0 18px"
    height: "42px"
  button-small:
    rounded: "{rounded.base}"
    padding: "0 12px"
    height: "34px"
  input:
    backgroundColor: "{colors.page}"
    textColor: "{colors.ink}"
    rounded: "{rounded.base}"
    padding: "11px 13px"
  composer:
    backgroundColor: "{colors.page}"
    rounded: "{rounded.base}"
    padding: "16px 18px 6px"
  chip:
    backgroundColor: "{colors.page}"
    textColor: "{colors.ink}"
    rounded: "{rounded.base}"
    padding: "8px 14px"
  chip-on:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.page}"
  rail:
    backgroundColor: "{colors.rail}"
    textColor: "{colors.rail-soft}"
    width: "232px"
  rail-item-current:
    backgroundColor: "{colors.juggernaught-green}"
    textColor: "{colors.ink}"
    padding: "11px 20px"
  badge:
    backgroundColor: "{colors.page-sunk}"
    textColor: "{colors.ink-soft}"
    rounded: "{rounded.tag}"
    padding: "3px 7px"
  asset-cell:
    backgroundColor: "{colors.page}"
    padding: "12px 14px 14px"
---

# Design System: Juggernaught OS

## Overview

**Creative North Star: "The Standards Manual"**

The workspace is the business's own standards manual, opened to the page you need. White manual pages carry ink text and 1px rules. Work is read as spec sheets: ruled rows, label left, value right, tabular figures. An ink-black rail holds the seven numbered sections. Each section opens on a full-width plate flooded with the brand's primary colour, set in heavy, wide Archivo. Juggernaught frames the business's work and does not compete with it.

There are two states. Before a business brand exists, and always on sign-in, setup and loading, the manual is Juggernaught's own: a green plate (#9fcf6b) with ink text and the tree-of-life mark (`public/juggernaught.svg`). Once the brand exists, `/brand.css` takes over the `.workspace` scope. The brand primary fills the plate. Text on the plate is black or white, whichever reads better. Brand-as-text is mixed toward black (light theme) or white (dark theme) until it clears WCAG AA 4.5:1. The plate heading uses the brand's heading font when there is one. Contrast is protected whatever colour the owner picks.

The density is calm and moderate: one reading column hangs from a fluid page inset, and prose keeps a 68ch measure. This rejects the soft-card SaaS chat look of bubbles, pastel accents and floating shadowed cards.

**Key Characteristics:**
- Brand-coloured section plate at the head of every section, with a light-weight number and a heavy wide title.
- Ink rail with numbered sections; the numbers are also keyboard shortcuts 1–7.
- Spec-sheet reading: ruled `dl` rows, 2px ink rules over section heads, tabular figures everywhere.
- Square corners (4px, tags 2px), flat surfaces, no shadows except overlays.
- One authored motion: an ink rule that draws across the plate foot.

## Colors

Ink on white paper with one flood colour. The brand gets the plate and the current rail item; everything else is ink, paper and rule.

### Primary
- **Tree-of-Life Green** (juggernaught-green): the plate fill, the current rail item, the New chat plus tile, the modal title band, and the selection/focus wash. This is Juggernaught's colour only before a brand exists. After that, `--brand` is the business primary, set at runtime. Never hardcode a brand hex into workspace components. Sign-in and setup are the exception: they always use this green.
- **Deep Leaf** (juggernaught-green-text): the brand used as text, links, rules and hover fills on a light page. It is the AA-safe twin of the plate colour (`--brand-text`).
- **Pale Leaf** (juggernaught-green-text-dark): the brand used as text on the dark page (`--brand-text-dark`).

### Neutral
- **Ink** (ink): body text, the 2px rules over section heads and spec tables, primary buttons, composer border. Also ink on the plate (`--brand-ink`) when the plate is light.
- **Ink Soft / Ink Faint** (ink-soft, ink-faint): ledes, hints and secondary values; spec labels, placeholders and role labels.
- **Paper / Sunk Paper** (page, page-sunk): the page; code, secrets, posture panels and recessed wells.
- **Rule / Strong Rule** (rule, rule-strong): 1px hairlines between rows and cells; the borders of inputs, chips, choice tiles and ghost buttons.
- **Cover Stock** (rail, rail-ink, rail-soft, rail-rule): the section rail. Pure black (dark-rail) in the dark theme.
- **Proof Ground** (work-ground): a fixed light ground behind asset previews in both themes, so dark logos stay visible.
- **Dark theme** (dark-ink, dark-page, dark-page-sunk, dark-raised, dark-rule): follows `prefers-color-scheme`. No toggle.

### Status
- **Done** (ok), **Needs you** (warn, with warn-marker for the square marker), **Error** (err). Status colour appears as text and as a small square marker, never as a filled pill.

### Named Rules
**The Plate Owns the Brand Rule.** The brand primary floods the plate and the current rail item. Everywhere else it appears only as its AA-safe text twin (`--brand-text`, `--brand-text-dark`) or as a translucent wash (`--primary-soft`, 16–18% mix). Never set body text in raw `--brand`.

**The Contrast Is Computed Rule.** Plate ink is chosen by measured contrast, black (#111111) or white. Brand-as-text is mixed until it reads at 4.5:1. Any new brand-coloured surface must take its text colour from `--brand-ink`, never a fixed value.

## Typography

**Display Font:** Archivo variable, self-hosted (`public/fonts/Archivo-Variable.woff2`, wdth 62–125, wght 100–900), with ui-sans-serif/system-ui fallback. The brand heading font replaces it on plate headings and the type specimen when a brand exists, at 100% width.
**Body Font:** Archivo at regular width.
**Label/Mono Font:** ui-monospace stack for tool names, codes, secrets and nameserver values.

**Character:** One family doing two jobs. It is stretched wide and heavy for the plates, like a manual's section dividers, and set plain and regular for reading.

### Hierarchy
- **Plate** (800, wdth 125, clamp(34px, 4.6vw, 60px), 0.98): the section title or chat greeting on the plate. There is one per page.
- **Plate Number** (300, wdth 125, same size, 0.98, 55% opacity): the section number set before the plate title.
- **Plate Slim** (800, clamp(24px, 2.6vw, 34px)): the compact plate once a chat thread has started.
- **Headline** (800, wdth 112, 22px): modal titles and empty-state headings.
- **Title** (750, wdth 108, 18px): Go live step titles. The site URL is set at 20px.
- **Section Head** (750, wdth 110, 13px, uppercase, 0.06em): a sub-section heading under a 2px ink rule. A 12.5px variant sits under a 1px rule. These are real headings (`h2`/`h3`) and name their content.
- **Body** (400, 15px, 1.55, tabular figures): all running text, kept to a 68ch measure. Ledes are 16px.
- **Label** (750, 12px, uppercase, 0.06em): the You / Juggernaught role labels in the chat thread's left column, and spec table headers.

### Named Rules
**The Wide Means Plate Rule.** 125% width is reserved for plates, plate numbers, the setup title and Go live step numbers. Reading text stays at normal width.

**The Tabular Rule.** `font-variant-numeric: tabular-nums` is set on the body. Figures line up in every spec row.

## Layout

The page is a two-column grid: the 232px ink rail and the main column. Every column inside main hangs from one fluid page margin, `--inset: clamp(20px, 5vw, 56px)`. Plates run full width, edge to edge of main, and their content is padded by the inset. Below the plate, panel content is inset on both sides with a 1000px max width. The chat thread and composer are capped at 860px plus the inset. Prose is capped at 68ch.

Spacing follows an 8px construction grid (`--g`), written as multiples: plate padding is 4g/inset/3g, section heads have 5g above, the rail has 3g top padding. Component internals use small off-grid values (10–14px) for optical fit.

Grids of work are packed and ruled. Asset cells, brand tokens and logo concepts sit gap-free with shared 1px borders, like a contact sheet. Cells are not floating cards.

Responsive: at 820px and below, the rail becomes an off-canvas drawer (84vw, max 300px) with a scrim, a menu button sits on the plate, and chat messages stack their role label above the text. At 640px and below, two-column choice grids and concepts collapse to one column, and key-value tables stack.

**The One Margin Rule.** New content aligns to `--inset`. Never introduce a second page margin.

## Elevation & Depth

Flat. Depth comes from ink weight (2px vs 1px rules), tone (sunk paper wells, the black rail) and the brand flood of the plate, not from shadows. Selected tiles use an inset 1px ink ring plus a border. Focus uses a 2px outline or a 3–4px translucent brand ring. These are state signals, not elevation.

### Shadow Vocabulary
- **Overlay** (`box-shadow: 0 18px 40px -16px rgb(0 0 0 / 0.28), 0 2px 6px rgb(0 0 0 / 0.08)`; dark: `0 18px 40px -16px rgb(0 0 0 / 0.7), 0 2px 6px rgb(0 0 0 / 0.4)`): modals and the mobile rail drawer only.

### Named Rules
**The Overlays Only Rule.** A shadow means "this floats above the page". Only modals and the off-canvas rail float.

## Shapes

The corners are square. Most elements use a 4px radius: buttons, inputs, chips, tiles, wells and the composer. Tags, badges, code chips, checkboxes and the steps bar use 2px. Modals and the logo preview use 6px. Status markers are 7–8px squares with no radius or a 1px radius. The provider-ready dot is the one circle. Borders are 1px hairlines for structure and 1.5px for interactive edges. The composer and approval cards use a heavier 1.5–2px ink edge. The plate's foot is a 4px rule in its own ink colour.

## Components

### Section Plate (signature)
Every section opens on it. It is a full-width brand-flood block, 176px minimum height (140px on mobile), with content aligned to the bottom. The light-weight section number is followed by the heavy wide title, and the lede sits on the page below in ink-soft at 16px. A 4px rule in `currentColor` (90% opacity) runs along the foot and draws in left to right with `scaleX` (700ms, `cubic-bezier(0.16, 1, 0.3, 1)`, 80ms delay). That draw is the system's one authored motion. It is decorative only. The chat page uses a slim plate once the thread has messages. Sign-in and setup use the same plate in fixed Juggernaught green, 220px tall, with the mark knocked to ink.

### Navigation (the rail)
The rail is ink cover stock. At the top are the mark and business name (750, wdth 112, 14px), followed by a bordered New chat row with a brand-filled plus tile. Below that are seven ruled rows: a section number (700, wdth 112, 13px) in a 34px column, then the label. Hover lightens the text and background (#181818). The current section fills with `--brand` and `--brand-ink`. A "needs you" note shows as a warn-marker square and text. The provider status pill sits at the foot. Number keys 1–7 jump between sections.

### Buttons
- **Shape:** square-cornered (4px), 42px tall, 1.5px border, weight 650.
- **Primary:** ink fill, paper text. Hover fills with the AA-safe brand text colour.
- **Ghost:** transparent with a strong-rule border. Hover darkens the border to ink.
- **Small:** 34px tall, 13px text. **Warn:** err text with an err-tinted border.
- **Send:** a 38px ink square with an authored arrow icon. It follows primary hover. Disabled is 30% opacity.
- Transitions are 120ms on colour and border only.

### Inputs / Fields
- **Style:** paper fill, 1.5px strong-rule border, 4px radius, 11px 13px padding, label above at 650 weight.
- **Focus:** the border goes to ink, plus a 3px brand ring at 45% mix.
- **Composer:** a 2px ink border. On focus-within it gets a 4px brand ring. Dragging a file over it fills it with the brand wash.

### Chips and Choice Tiles
Chips are 4px with a strong-rule border, and turn ink-filled when on. Choice tiles (sign-in methods, providers, pairings) are 1.5px bordered blocks. When selected they get an ink border, a brand-wash fill and an inset ink ring. Method tiles carry a square 2px checkbox.

### Spec Sheet
A ruled definition list. Label on the left in ink-faint at 600 weight, value on the right in ink, 6px row padding, 1px rules, 13px text. It is used in asset cells (Kind / Format / Made) and in key-value panels.

### Asset Cells
Asset cells sit packed in a ruled grid (min 232px columns, no gap). Each has a 4:3 preview on the proof ground (image contained, 14px padding, 1.03 scale on hover), then a 14.5px bold title, the spec sheet, status text and actions.

### Chat Thread
There are no bubbles. Each message is a ruled row with a 136px left column holding the role label: "You" in ink-faint, "Juggernaught" in the brand text colour. The user's message is in 550 weight. Tool calls show as a ruled table with mono names. Approval and hand-off cards have a 1.5px ink edge. Approved ones soften to a green-tinted rule.

### Notes and Badges
A "needs you" item is a manual note: an 8px square marker and a plain sentence, coloured by tone. It is never a pill. Badges are 2px uppercase tags (11.5px, 700) on tinted grounds for draft, published, error and locked.

### Go live Procedure
A numbered list under a 2px ink rule. Each step has a wide-800 number in a 48px column, a title, and a status note. Upcoming steps fade to ink-faint and done numbers turn green.

### Icons
The icon set is authored SVG (`app/src/icons.rs`): a 24px grid, 2px stroke, square caps, miter joins, `currentColor`. New icons join this set. They are not glyphs or icon fonts.

## Do's and Don'ts

### Do:
- **Do** open every section with the Section Plate: brand flood, light number and heavy wide title at wdth 125, lede on the page below.
- **Do** take plate text colour from `--brand-ink` and brand-as-text from `--primary` (`--brand-text` / `--brand-text-dark`), so any brand colour stays AA-readable in both themes.
- **Do** present made work as spec sheets: ruled rows, tabular figures, label left and value right.
- **Do** hang every column from `--inset` and build spacing in multiples of the 8px grid.
- **Do** keep corners at 4px (tags 2px, overlays 6px).
- **Do** start entrances from a 6px rise or a `scaleX` draw with content already visible.
- **Do** keep sign-in, setup and loading in Juggernaught green #9fcf6b with the tree-of-life mark, whatever the business brand is.

### Don't:
- **Don't** put chat messages in bubbles or use avatars. The role label column does that job.
- **Don't** use shadows on cards, tiles or buttons. Shadows are for overlays only.
- **Don't** start an entrance from opacity 0. Text and plates are readable from the first frame.
- **Don't** add a second authored motion. The plate rule draw is the one moment. Everything else stays quiet: 120–220ms colour transitions and a 6px rise.
- **Don't** set "needs you", step or approval status as filled pills. Use a square marker and a plain sentence.
- **Don't** use 125% width for reading text, or fill large areas with raw `--brand` outside the plate and current rail item.
- **Don't** put small uppercase labels above headings. Section heads are themselves the headings.
