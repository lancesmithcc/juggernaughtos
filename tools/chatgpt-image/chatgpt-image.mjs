#!/usr/bin/env node
// Juggernaught: generate images through the owner's own ChatGPT session (GPT Image)
// with Playwright. The owner signs in once (remotely through the admin, or in a
// visible window on a desktop); the login lives in a private browser profile.
//
//   chatgpt-image status   --profile DIR
//   chatgpt-image generate --profile DIR --prompt-file FILE --out FILE [--timeout SECS]
//   chatgpt-image login    --profile DIR [--headed]
//   --browser chrome   use the installed Google Chrome instead of the bundled Chromium.
//                      On a Mac this is how the owner signs in with Touch ID: Chrome is a
//                      signed app the system lets use passkeys; bundled Chromium is not.
//   chatgpt-image selftest   (install canary: Chromium starts headless)
//
// Every command prints JSON lines on stdout. `generate` ends with one result line:
//   {"ok":true,"file":"...","bytes":N,"type":"image/png","capture":"original|screenshot"}
//   {"ok":false,"reason":"not_signed_in|limit|refused|blocked|timeout|ui_changed|error","message":"..."}
// Remote login streams {"frame":"<jpeg base64>","w":..,"h":..,"signed_in":bool} and
// reads input lines on stdin: {"click":{"x":..,"y":..}} {"text":".."} {"key":"Enter"}
// {"scroll":dy} {"goto":"url"} {"stop":true}.

import { chromium } from "playwright-core";
import fs from "node:fs";
import readline from "node:readline";

const HOME = "https://chatgpt.com/";
const VIEW = { width: 1280, height: 900 };
const USAGE = `usage: chatgpt-image <status|generate|login> --profile DIR [--prompt-file FILE --out FILE] [--headed] [--browser chrome|chromium] [--timeout SECS]`;

function args(argv) {
  const o = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a.startsWith("--")) {
      const k = a.slice(2);
      const v = argv[i + 1] && !argv[i + 1].startsWith("--") ? argv[++i] : true;
      o[k] = v;
    } else o._.push(a);
  }
  return o;
}

const out = (obj) => process.stdout.write(JSON.stringify(obj) + "\n");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let BROWSER = "chromium";

async function open(profile, headed) {
  fs.mkdirSync(profile, { recursive: true, mode: 0o700 });
  const opts = {
    channel: BROWSER,
    // Playwright gives Chrome a fake keychain by default; passkeys and the saved
    // session need the real one. (Nothing here hides that the browser is automated.)
    ...(BROWSER === "chrome" ? { ignoreDefaultArgs: ["--use-mock-keychain", "--password-store=basic"] } : {}),
    headless: !headed,
    viewport: VIEW,
    locale: "en-US",
    args: ["--disable-dev-shm-usage", "--no-first-run", "--no-default-browser-check"],
  };
  // Keep Chromium's own sandbox on (Playwright turns it off by default). Hosts without
  // unprivileged user namespaces (some containers) can't run it; fall back and say so.
  let ctx;
  try {
    ctx = await chromium.launchPersistentContext(profile, { ...opts, chromiumSandbox: true });
  } catch (e) {
    out({ warning: "chromium sandbox unavailable on this host: " + String(e.message || e).split("\n")[0].slice(0, 160) });
    ctx = await chromium.launchPersistentContext(profile, { ...opts, chromiumSandbox: false });
  }
  // No bot-check evasion: the browser identifies itself honestly. If ChatGPT asks for a
  // human check, we stop and the owner completes it in the sign-in view.
  const page = ctx.pages()[0] || (await ctx.newPage());
  return { ctx, page };
}

async function state(page) {
  return page
    .evaluate(() => {
      const visible = (el) => !!el && !!(el.offsetWidth || el.offsetHeight || el.getClientRects().length);
      const loginBtn = [...document.querySelectorAll('[data-testid="login-button"], button, a')].find(
        (b) => visible(b) && (b.getAttribute("data-testid") === "login-button" || /^\s*log in\s*$/i.test(b.textContent || ""))
      );
      const composer = document.querySelector('#prompt-textarea, div[contenteditable="true"], textarea[name="prompt-textarea"]');
      const challenge =
        !!document.querySelector('iframe[src*="challenges.cloudflare.com"], #challenge-form, .cf-turnstile, [id^="cf-chl"]') ||
        /just a moment|verify you are human|attention required/i.test(document.title + " " + (document.body?.innerText || "").slice(0, 400));
      return { signed_in: !loginBtn && visible(composer), composer: visible(composer), challenge, title: document.title };
    })
    .catch(() => ({ signed_in: false, composer: false, challenge: false, title: "" }));
}

async function gotoHome(page) {
  for (let i = 0; i < 3; i++) {
    try {
      await page.goto(HOME, { waitUntil: "domcontentloaded", timeout: 45000 });
      break;
    } catch (e) {
      if (i === 2) throw e;
      await sleep(3000);
    }
  }
  // The app hydrates after DOMContentLoaded; give it a moment to show the composer or the login button.
  for (let i = 0; i < 20; i++) {
    const s = await state(page);
    if (s.composer || s.challenge) return s;
    await sleep(750);
  }
  return state(page);
}

const LIMIT = /(image (generation )?limit|rate.?limit|reached (your|the)[^.]*limit|too many (requests|images)|hit (your|the)[^.]*limit|try again (later|in|after))/i;
const RETRY = /try again (?:in|after|at) ([^.\n]{1,40})/i;

async function generate(o) {
  const prompt = fs.readFileSync(o["prompt-file"], "utf8").trim();
  const timeout = Number(o.timeout || 480) * 1000;
  let { ctx, page } = await open(o.profile, false);
  try {
    let s = await gotoHome(page);
    if (s.challenge && BROWSER === "chrome") {
      // On the owner's own computer: show the window so they can tick the check themselves.
      await ctx.close().catch(() => {});
      ({ ctx, page } = await open(o.profile, true));
      await page.bringToFront().catch(() => {});
      out({ needs_owner: "human_check" });
      s = await gotoHome(page);
      for (let i = 0; i < 100 && s.challenge && !page.isClosed(); i++) {
        await sleep(1500);
        s = await state(page);
      }
    }
    if (s.challenge) return { ok: false, reason: "blocked", message: "ChatGPT showed a bot check to this server. Sign in again from Settings, or use the API key fallback." };
    if (!s.signed_in) return { ok: false, reason: "not_signed_in", message: "Not signed in to ChatGPT. Sign in from Settings → Intelligence → Image generation." };

    const composer = page.locator('#prompt-textarea, div[contenteditable="true"]').first();
    await composer.click({ timeout: 15000 });
    await page.keyboard.insertText(`Generate an image: ${prompt}`);
    await sleep(400);
    const send = page.locator('[data-testid="send-button"]');
    if (await send.isVisible().catch(() => false)) await send.click();
    else await page.keyboard.press("Enter");

    const deadline = Date.now() + timeout;
    let last = null,
      stable = 0,
      quietNoImage = 0;
    await sleep(4000);
    while (Date.now() < deadline) {
      const st = await page.evaluate(() => {
        const stop = !!document.querySelector('[data-testid="stop-button"]');
        const imgs = [...document.querySelectorAll("main img")].filter((i) => {
          const s = i.currentSrc || i.src || "";
          return i.naturalWidth >= 512 && i.naturalHeight >= 512 && /oaiusercontent|estuary|backend-api|files\.|blob:/i.test(s);
        });
        const img = imgs[imgs.length - 1];
        const turns = [...document.querySelectorAll('[data-message-author-role="assistant"]')];
        const text = (turns.length ? turns[turns.length - 1].innerText : (document.querySelector("main")?.innerText || "").slice(-1500)) || "";
        return { stop, src: img ? img.currentSrc || img.src : null, w: img ? img.naturalWidth : 0, h: img ? img.naturalHeight : 0, text: text.slice(0, 1500) };
      });
      if (!st.src && LIMIT.test(st.text)) {
        const m = st.text.match(RETRY);
        return { ok: false, reason: "limit", message: st.text.slice(0, 300), retry: m ? m[1].trim() : null };
      }
      if (st.src && !st.stop) {
        const key = `${st.src}|${st.w}x${st.h}`;
        stable = key === last ? stable + 1 : 0;
        last = key;
        if (stable >= 2) {
          const fetched = await page.evaluate(async (src) => {
            try {
              const r = await fetch(src, { credentials: "include" });
              if (!r.ok) return { ok: false, status: r.status };
              const b = new Uint8Array(await r.arrayBuffer());
              let bin = "";
              for (let i = 0; i < b.length; i += 0x8000) bin += String.fromCharCode.apply(null, b.subarray(i, i + 0x8000));
              return { ok: true, b64: btoa(bin), type: r.headers.get("content-type") };
            } catch (e) {
              return { ok: false, status: String(e) };
            }
          }, st.src);
          if (fetched.ok) {
            const bytes = Buffer.from(fetched.b64, "base64");
            fs.writeFileSync(o.out, bytes);
            return { ok: true, file: o.out, bytes: bytes.length, type: fetched.type, capture: "original", w: st.w, h: st.h };
          }
          const el = page.locator(`main img[src="${st.src.replace(/"/g, '\\"')}"]`).last();
          await el.screenshot({ path: o.out, type: "png" });
          return { ok: true, file: o.out, bytes: fs.statSync(o.out).size, type: "image/png", capture: "screenshot", w: st.w, h: st.h };
        }
      } else if (!st.src && !st.stop && st.text.trim().length > 0) {
        // Answered with words and no image: a refusal or a question back.
        quietNoImage += 1;
        if (quietNoImage >= 10) return { ok: false, reason: "refused", message: st.text.slice(0, 400) };
      } else quietNoImage = 0;
      await sleep(2500);
    }
    return { ok: false, reason: "timeout", message: `no finished image after ${Math.round(timeout / 1000)}s` };
  } finally {
    await ctx.close().catch(() => {});
  }
}

async function status(o) {
  const { ctx, page } = await open(o.profile, false);
  try {
    const s = await gotoHome(page);
    return { signed_in: s.signed_in, challenge: s.challenge };
  } finally {
    await ctx.close().catch(() => {});
  }
}

async function login(o) {
  const headed = !!o.headed;
  const { ctx, page } = await open(o.profile, headed);
  let stop = false;
  let signed = false;
  try {
    if (headed) await page.bringToFront().catch(() => {});
    ctx.on("close", () => (stop = true));
    await gotoHome(page);
    {
      const rl = readline.createInterface({ input: process.stdin });
      rl.on("line", async (line) => {
        let m;
        try {
          m = JSON.parse(line);
        } catch {
          return;
        }
        try {
          if (m.stop) stop = true;
          else if (headed) return; // the owner uses the real window
          else if (m.click) await page.mouse.click(Math.max(0, Math.min(VIEW.width, +m.click.x)), Math.max(0, Math.min(VIEW.height, +m.click.y)));
          else if (typeof m.text === "string") await page.keyboard.insertText(m.text.slice(0, 500));
          else if (typeof m.key === "string" && /^(Enter|Tab|Backspace|Escape|ArrowUp|ArrowDown|ArrowLeft|ArrowRight|Shift\+Tab)$/.test(m.key)) await page.keyboard.press(m.key);
          else if (typeof m.scroll === "number") await page.mouse.wheel(0, Math.max(-2000, Math.min(2000, m.scroll)));
          else if (typeof m.goto === "string" && /^https:\/\/(chatgpt\.com|auth\.openai\.com)\//.test(m.goto)) await page.goto(m.goto, { waitUntil: "domcontentloaded" });
        } catch {}
      });
      rl.on("close", () => (stop = true));
    }
    const deadline = Date.now() + 15 * 60 * 1000;
    while (!stop && Date.now() < deadline) {
      if (page.isClosed()) break;
      const s = await state(page);
      if (!headed) {
        const shot = await page.screenshot({ type: "jpeg", quality: 60 }).catch(() => null);
        if (shot) out({ frame: shot.toString("base64"), w: VIEW.width, h: VIEW.height, signed_in: s.signed_in, challenge: s.challenge, url: page.url().split("?")[0] });
      } else out({ view: "window", signed_in: s.signed_in, challenge: s.challenge, url: page.url().split("?")[0] });
      if (s.signed_in && !page.url().includes("/auth")) {
        if (!signed) out({ signed_in: true });
        signed = true;
        // Let the session cookies settle before closing the profile.
        await sleep(3000);
        break;
      }
      await sleep(headed ? 1500 : 800);
    }
    return { signed_in: signed };
  } finally {
    await ctx.close().catch(() => {});
  }
}

const o = args(process.argv.slice(2));
const cmd = o._[0];
if (!cmd || o.help || cmd === "help" || cmd === "--help") {
  console.log(USAGE);
  process.exit(0);
}
if (cmd === "selftest") {
  const b = await chromium.launch({ channel: "chromium", headless: true, args: ["--disable-dev-shm-usage"] }); // root during install: no sandbox possible
  const p = await b.newPage();
  await p.setContent("<p>ok</p>");
  const ok = (await p.textContent("p")) === "ok";
  await b.close();
  console.log(ok ? "usage: chatgpt-image selftest ok" : "selftest failed");
  process.exit(ok ? 0 : 1);
}
if (o.browser === "chrome") BROWSER = "chrome";
else if (o.browser && o.browser !== "chromium") {
  console.error("--browser must be chrome or chromium");
  process.exit(2);
}
if (!o.profile) {
  console.error(USAGE);
  process.exit(2);
}
try {
  let r;
  if (cmd === "status") r = await status(o);
  else if (cmd === "generate") {
    if (!o["prompt-file"] || !o.out) throw new Error(USAGE);
    r = await generate(o);
  } else if (cmd === "login") r = await login(o);
  else throw new Error(USAGE);
  out({ done: true, ...r });
  process.exit(0);
} catch (e) {
  out({ done: true, ok: false, reason: "error", message: String(e && e.message ? e.message : e).slice(0, 400) });
  process.exit(1);
}
