//! Sidebar panels. Everything here is also doable from chat; panels are for looking.

use crate::api::{chatgpt_check, chatgpt_install, chatgpt_signin_frame, chatgpt_signin_input, chatgpt_signin_start, image_settings, image_status, save_image_settings, get_site, publish_site, rollback_site, get_brand, get_providers, get_security, get_sign_in_methods, get_standards, get_tools, save_provider_settings};
use crate::pages::account::{AddPasskey, RegenerateRecovery, TotpEnroll};
use leptos::prelude::*;

#[component]
fn Panel(title: &'static str, lede: &'static str, children: Children) -> impl IntoView {
    view! {
        <section class="panel">
            <header class="panel-head"><h1>{title}</h1><p>{lede}</p></header>
            {children()}
        </section>
    }
}

#[component]
pub fn BrandPanel() -> impl IntoView {
    let brand = Resource::new(|| (), |_| get_brand());
    view! {
        <Panel title="Brand" lede="Your standards. Every asset and page is checked against these before it's final.">
            <crate::pages::setup::LogoPicker/>
            <Suspense fallback=|| view! { <div class="skeleton"></div> }>
                {move || Suspend::new(async move {
                    brand.await.map(|b| view! {
                        <div class="swatch-grid">
                            {b.colors.into_iter().map(|(slug, hex, usage)| { let style = format!("--c:{hex}"); view! {
                                <figure class="token" style=style>
                                    <div class="token-chip"></div>
                                    <figcaption><strong>{slug}</strong><code>{hex}</code><span>{usage}</span></figcaption>
                                </figure>
                            }}).collect_view()}
                        </div>
                        <div class="type-specimen">
                            <p class="eyebrow">"Headings"</p><p class="specimen-h">{b.heading_font}</p>
                            <p class="eyebrow">"Body"</p><p class="specimen-b">{b.body_font}</p>
                        </div>
                        <details class="raw"><summary>"Machine-readable standards (DESIGN.md)"</summary><pre>{b.design_md}</pre></details>
                    })
                })}
            </Suspense>
        </Panel>
    }
}

#[component]
pub fn WebsitePanel() -> impl IntoView {
    let site = Resource::new(|| (), |_| get_site());
    let std = Resource::new(|| (), |_| get_standards());
    let msg = RwSignal::new(None::<String>);
    let working = RwSignal::new(false);
    let act = move |publish: bool| {
        working.set(true);
        msg.set(None);
        leptos::task::spawn_local(async move {
            let r = if publish { publish_site().await } else { rollback_site().await };
            msg.set(Some(match r { Ok(m) => m, Err(e) => e.to_string().replace("error running server function: ", "") }));
            working.set(false);
            site.refetch();
        });
    };
    view! {
        <Panel title="Website" lede="Your site is built from checked blocks: secure, fast and accessible by construction. Ask in chat to add or change pages; publish here or there.">
            <Suspense fallback=|| view! { <div class="skeleton"></div> }>
                {move || Suspend::new(async move {
                    site.await.map(|s| {
                        let has_drafts = s.pages.iter().any(|p| p.2 == "draft");
                        let can_rollback = s.deploys.iter().filter(|d| d.1 == "superseded").count() > 0;
                        let no_pages = s.pages.is_empty();
                        view! {
                            <div class="site-status">
                                <div>
                                    <p class="eyebrow">{if s.live { "Live" } else { "Not published yet" }}</p>
                                    <a class="site-url" href=s.public_url.clone() target="_blank" rel="noopener">{s.public_url.clone()}</a>
                                    {has_drafts.then(|| view! { <p class="hint">"You have unpublished changes."</p> })}
                                </div>
                                <div class="site-actions">
                                    {can_rollback.then(|| view! { <button class="btn ghost small" disabled=move || working.get() on:click=move |_| act(false)>"Roll back"</button> })}
                                    <button class="btn primary small" disabled=move || working.get() || no_pages on:click=move |_| act(true)>
                                        {move || if working.get() { "Publishing…" } else { "Publish" }}
                                    </button>
                                </div>
                            </div>
                            {move || msg.get().map(|m| view! { <p class="site-msg" role="status">{m}</p> })}
                            <h2 class="section-h">"Pages"</h2>
                            <ul class="rules">{s.pages.into_iter().map(|(slug, title, status)| {
                                let preview = format!("/_preview{}", if slug == "/" { "/".to_string() } else { slug.clone() });
                                view! {
                                    <li class="rule">
                                        <code>{slug}</code>
                                        <span class=format!("badge page-{status}")>{if status == "draft" { "unpublished changes".to_string() } else { status.clone() }}</span>
                                        <p><a href=preview target="_blank" rel="noopener">{title}" — preview ↗"</a></p>
                                    </li>
                                }
                            }).collect_view()}</ul>
                            {(!s.submissions.is_empty()).then(|| view! {
                                <h2 class="section-h">"Messages from your site"</h2>
                                <ul class="rules">{s.submissions.into_iter().map(|m| view! {
                                    <li class="rule submission"><code>{m.form}</code>
                                        <dl class="kv">{m.fields.into_iter().map(|(k, v)| view! { <dt>{k}</dt><dd>{v}</dd> }).collect_view()}</dl>
                                    </li>
                                }).collect_view()}</ul>
                            })}
                            {(!s.overrides.is_empty()).then(|| view! {
                                <h2 class="section-h">"Standards you chose to override"</h2>
                                <ul class="rules">{s.overrides.into_iter().map(|(rule, target, reason)| view! {
                                    <li class="rule"><code>{rule}</code><span class="badge warn">{target}</span><p>{reason}</p></li>
                                }).collect_view()}</ul>
                            })}
                        }
                    })
                })}
            </Suspense>
            <h2 class="section-h">"Web standards"</h2>
            <Suspense fallback=|| view! { <div class="skeleton"></div> }>
                {move || Suspend::new(async move {
                    std.await.map(|s| view! {
                        <ul class="rules">
                            {s.rules.into_iter().map(|(id, level, overridable, why)| view! {
                                <li class="rule">
                                    <code>{id}</code>
                                    <span class=format!("badge {level}")>{level.clone()}</span>
                                    {(!overridable).then(|| view! { <span class="badge locked" title="Security rule: change in Settings → Advanced only">"locked"</span> })}
                                    <p>{why}</p>
                                </li>
                            }).collect_view()}
                        </ul>
                    })
                })}
            </Suspense>
        </Panel>
    }
}

#[component]
pub fn SecurityPanel() -> impl IntoView {
    let sec = Resource::new(|| (), |_| get_security());
    view! {
        <Panel title="Security" lede="The Steward watches, patches and backs up on its own. You'll hear from it only when something needs you.">
            <Suspense fallback=|| view! { <div class="skeleton"></div> }>
                {move || Suspend::new(async move {
                    sec.await.map(|s| {
                        let (attention, caution) = (s.posture.starts_with("Needs attention"), s.posture.contains("keep an eye"));
                        view! {
                        <div class="posture" class:attention=attention class:caution=caution><span class="pulse" aria-hidden="true"></span><p>{s.posture}</p></div>
                        {(!s.checks.is_empty()).then(|| view! {
                            <h2 class="section-h">"Health checks"</h2>
                            <ul class="rules">{s.checks.into_iter().map(|(name, status, detail)| view! {
                                <li class="rule"><code>{name}</code><span class=format!("badge check-{status}")>{status.clone()}</span><p>{detail}</p></li>
                            }).collect_view()}</ul>
                        })}
                        {(!s.incidents.is_empty()).then(|| view! {
                            <h2 class="section-h">"Open incidents"</h2>
                            <ul class="rules">{s.incidents.into_iter().map(|(subject, score)| view! {
                                <li class="rule"><code>{subject}</code><span class="badge check-fail">{format!("score {score:.2}")}</span></li>
                            }).collect_view()}</ul>
                        })}
                        {(s.bans > 0).then(|| view! { <p class="hint">{format!("{} address(es) currently banned at the firewall.", s.bans)}</p> })}
                        <h2 class="section-h">"Encryption"</h2>
                        <dl class="kv">{s.crypto.into_iter().map(|(k, v)| view! { <dt>{k}</dt><dd>{v}</dd> }).collect_view()}</dl>
                        <h2 class="section-h">"Signed-in devices"</h2>
                        <ul class="rules">{s.sessions.into_iter().map(|(device, method, _)| view! {
                            <li class="rule"><strong>{device}</strong><span class="badge">{method}</span></li>
                        }).collect_view()}</ul>
                        <h2 class="section-h">"Sensors on this machine"</h2>
                        <div class="filters">{s.sensors.into_iter().map(|x| view! { <span class="chip static">{x}</span> }).collect_view()}</div>
                        <h2 class="section-h">"Automatic maintenance"</h2>
                        <ul class="rules">{s.tasks.into_iter().map(|(n, when, what)| view! {
                            <li class="rule"><code>{n}</code><span class="badge">{when}</span><p>{what}</p></li>
                        }).collect_view()}</ul>
                        }
                    })
                })}
            </Suspense>
        </Panel>
    }
}

#[component]
pub fn SettingsPanel() -> impl IntoView {
    let tools = Resource::new(|| (), |_| get_tools());
    view! {
        <Panel title="Settings" lede="Connect intelligence, manage how you sign in, and see the tools I can use.">
            <h2 class="section-h">"Intelligence"</h2>
            <Intelligence/>
            <h2 class="section-h">"Image generation"</h2>
            <ChatGptImages/>
            <ImageSettings/>
            <h2 class="section-h">"Signing in"</h2>
            <SignIn/>
            <h2 class="section-h">"Tools"</h2>
            <Suspense fallback=|| view! { <div class="skeleton"></div> }>
                {move || Suspend::new(async move {
                    tools.await.map(|t| view! {
                        <ul class="rules">{t.into_iter().map(|x| view! {
                            <li class="rule" class:unavailable=!x.available><code>{x.name}</code><span class=format!("badge {}", x.risk)>{x.risk.clone()}</span>
                                {(!x.available).then(|| view! { <span class="badge" title="Install with: juggernaught tools install --all">"not installed"</span> })}
                                <p>{x.description}</p></li>
                        }).collect_view()}</ul>
                    })
                })}
            </Suspense>
            <form method="post" action="/auth/logout" class="signout"><button class="btn ghost" type="submit">"Sign out of this device"</button></form>
        </Panel>
    }
}

const PROVIDERS: &[(&str, &str, &str)] = &[
    ("claude_code_cli", "Claude Code", "Your Claude subscription. Sign in once on the server: juggernaught connect claude"),
    ("codex_cli", "Codex", "Your ChatGPT subscription via the Codex CLI: juggernaught connect codex"),
    ("openrouter", "OpenRouter", "Any model with an API key, pay as you go. The key is stored encrypted."),
];

#[component]
fn Intelligence() -> impl IntoView {
    let current = Resource::new(|| (), |_| get_providers());
    let kind = RwSignal::new(String::new());
    let model = RwSignal::new(String::new());
    let key = RwSignal::new(String::new());
    let msg = RwSignal::new(None::<String>);
    let saving = RwSignal::new(false);
    let sidebar = use_context::<Resource<Result<crate::api::Overview, ServerFnError>>>();
    Effect::new(move |_| {
        if let Some(Ok(p)) = current.get() {
            kind.set(p.active_kind.clone());
            model.set(p.model.clone());
        }
    });
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        saving.set(true);
        let (k, m, a) = (kind.get_untracked(), model.get_untracked(), key.get_untracked());
        leptos::task::spawn_local(async move {
            let r = save_provider_settings(k, Some(m), Some(a).filter(|a| !a.is_empty())).await;
            saving.set(false);
            key.set(String::new());
            msg.set(Some(match r { Ok(s) => s, Err(e) => e.to_string() }));
            current.refetch();
            if let Some(o) = sidebar {
                o.refetch();
            }
        });
    };
    view! {
        <form class="intelligence" on:submit=save>
            <div class="providers" role="radiogroup" aria-label="Intelligence provider">
                {PROVIDERS.iter().map(|(id, name, blurb)| {
                    let id = id.to_string();
                    let (id2, id3) = (id.clone(), id.clone());
                    view! {
                        <button type="button" role="radio" class="provider" class:on=move || kind.with(|k| *k == id2)
                            aria-checked=move || kind.with(|k| *k == id).to_string() on:click=move |_| kind.set(id3.clone())>
                            <strong>{*name}</strong><span>{*blurb}</span>
                        </button>
                    }
                }).collect_view()}
            </div>
            <div class="provider-fields">
                <label>"Model (optional)"
                    <input placeholder="Leave blank for the default" prop:value=move || model.get() on:input=move |ev| model.set(event_target_value(&ev))/>
                </label>
                <Show when=move || kind.with(|k| k == "openrouter")>
                    <label>"OpenRouter API key"
                        <input type="password" autocomplete="off" placeholder=move || if current.get().and_then(|r| r.ok()).is_some_and(|p| p.has_openrouter_key) { "Saved · enter a new key to replace" } else { "sk-or-…" }
                            prop:value=move || key.get() on:input=move |ev| key.set(event_target_value(&ev))/>
                    </label>
                </Show>
                <button class="btn primary" type="submit" disabled=move || saving.get() || kind.with(|k| k.is_empty() || k == "offline")>
                    {move || if saving.get() { "Checking…" } else { "Save and test" }}
                </button>
            </div>
            <Suspense fallback=|| ()>
                {move || Suspend::new(async move {
                    current.await.map(|p| view! {
                        <p class="provider-status" data-ready=p.ready.to_string()><span class="dot" aria-hidden="true"></span>{format!("{} · {}", p.label, p.status)}</p>
                    })
                })}
            </Suspense>
            {move || msg.get().map(|m| view! { <p class="hint" role="status">{m}</p> })}
        </form>
    }
}

#[component]
fn SignIn() -> impl IntoView {
    let methods = Resource::new(|| (), |_| get_sign_in_methods());
    let refresh = Callback::new(move |_| methods.refetch());
    view! {
        <Suspense fallback=|| view! { <div class="skeleton"></div> }>
            {move || Suspend::new(async move {
                methods.await.map(|m| view! {
                    <div class="signin-grid">
                        <section class="secure-block">
                            <h3>"Passkeys"</h3>
                            {if m.passkeys.is_empty() {
                                view! { <p class="hint">"None yet."</p> }.into_any()
                            } else {
                                view! { <ul class="plain">{m.passkeys.iter().map(|p| view! { <li>{p.clone()}</li> }).collect_view()}</ul> }.into_any()
                            }}
                            <AddPasskey on_done=refresh/>
                        </section>
                        <section class="secure-block">
                            <h3>"Authenticator app"</h3>
                            {if m.totp {
                                view! { <p class="ok-line">"On"</p> }.into_any()
                            } else {
                                view! { <TotpEnroll on_done=refresh/> }.into_any()
                            }}
                        </section>
                        <section class="secure-block">
                            <h3>"Email link"</h3>
                            <p class="hint">{format!("Sent to {}", m.email)}</p>
                        </section>
                        <section class="secure-block">
                            <h3>"Recovery codes"</h3>
                            <p class="hint">{format!("{} unused", m.recovery_left)}</p>
                            <RegenerateRecovery on_done=refresh/>
                        </section>
                    </div>
                })
            })}
        </Suspense>
    }
}

/// App-wide "Connect ChatGPT" pop-up. Chat opens it when an image needs it and is told
/// (via `connected`) when the owner finishes, so the image request can carry on.
#[derive(Clone, Copy)]
pub struct ChatGptPrompt {
    pub open: RwSignal<bool>,
    pub connected: RwSignal<u32>,
}

impl ChatGptPrompt {
    pub fn new() -> Self {
        Self { open: RwSignal::new(false), connected: RwSignal::new(0) }
    }
}

#[component]
fn ChatGptImages() -> impl IntoView {
    let status = Resource::new(|| (), |_| image_status());
    let prompt = use_context::<ChatGptPrompt>().map(|p| p.open).unwrap_or_else(|| RwSignal::new(false));
    let msg = RwSignal::new(None::<String>);
    // Refresh the status card whenever the pop-up closes.
    Effect::new(move |prev: Option<bool>| {
        let open = prompt.get();
        if prev == Some(true) && !open {
            status.refetch();
        }
        open
    });
    let check = move |_| {
        msg.set(Some("Checking…".into()));
        leptos::task::spawn_local(async move {
            msg.set(Some(match chatgpt_check().await {
                Ok(s) if s == "signed_in" => "Connected: signed in to ChatGPT.".into(),
                Ok(s) if s == "bot_check" => "ChatGPT wants a quick human check. Press Connect ChatGPT.".into(),
                Ok(_) => "Not connected yet. Press Connect ChatGPT.".into(),
                Err(e) => e.to_string(),
            }));
            status.refetch();
        });
    };
    view! {
        <div class="intelligence chatgpt-images">
            <p class="hint">"New images (hero photos, textures, icons, illustrations) are made with GPT Image through your own ChatGPT account, in a private browser on this server. You connect once; nothing you type is stored. Requests are paced so your account isn't flooded. Note: OpenAI's terms don't allow automated use of ChatGPT, so there's some risk your account gets limited."</p>
            <Suspense fallback=|| view! { <div class="skeleton"></div> }>
                {move || Suspend::new(async move {
                    status.await.map(|s| {
                        let (ready, label) = match (s.installed, s.session.as_str()) {
                            (false, _) => (false, "Not set up yet: press Connect ChatGPT".to_string()),
                            (true, "signed_in") => (true, "ChatGPT connected (default image maker)".to_string()),
                            (true, "bot_check") => (false, "ChatGPT wants a quick human check".to_string()),
                            (true, "signed_out") => (false, "ChatGPT not connected yet".to_string()),
                            _ => (false, "ChatGPT: press Check connection".to_string()),
                        };
                        let paused = s.paused_until.map(|_| format!("Paused: {}. ", s.paused_reason.clone().unwrap_or_default()));
                        view! {
                            <p class="provider-status" data-ready=ready.to_string()><span class="dot"></span>{label}</p>
                            <p class="hint">{paused}{format!("{} of {} this hour · {} of {} today · one at a time, at least {} s apart{}", s.used_hour, s.per_hour, s.used_day, s.per_day, s.min_gap_secs, if s.api_key_set { " · API key set as fallback" } else { "" })}</p>
                            <div class="row-actions">
                                <button class="btn primary" type="button" on:click=move |_| prompt.set(true)>{if ready { "Reconnect ChatGPT" } else { "Connect ChatGPT" }}</button>
                                {s.installed.then(|| view! { <button class="btn ghost" type="button" on:click=check>"Check connection"</button> })}
                            </div>
                        }
                    })
                })}
            </Suspense>
            {move || msg.get().map(|m| view! { <p class="hint" role="status">{m}</p> })}
        </div>
    }
}

/// The pop-up: a live view of the server's private browser. The owner clicks on the
/// picture and just types; keys go straight to the browser. Each step says what to do.
#[component]
pub fn ChatGptSignInDialog() -> impl IntoView {
    let Some(ChatGptPrompt { open, connected }) = use_context::<ChatGptPrompt>() else { return ().into_any() };
    let frame = RwSignal::new(None::<String>);
    let step = RwSignal::new(String::from("starting"));
    let err = RwSignal::new(None::<String>);
    let poll = StoredValue::new(None::<IntervalHandle>);
    let screen = NodeRef::<leptos::html::Div>::new();

    let send = move |kind: &'static str, x: f64, y: f64, text: String| {
        leptos::task::spawn_local(async move {
            if let Err(e) = chatgpt_signin_input(kind.into(), x, y, text).await {
                err.set(Some(e.to_string()));
            }
        });
    };
    let stop_polling = move || {
        if let Some(h) = poll.get_value() {
            h.clear();
            poll.set_value(None);
        }
    };
    let tick = move || {
        leptos::task::spawn_local(async move {
            let Ok(f) = chatgpt_signin_frame().await else { return };
            if f.jpeg_b64.is_some() {
                frame.set(f.jpeg_b64);
            }
            step.set(f.step.clone());
            if f.step == "done" {
                stop_polling();
                connected.update(|n| *n += 1);
                set_timeout(move || open.set(false), std::time::Duration::from_millis(2200));
            } else if !f.active && frame.get_untracked().is_some() {
                stop_polling();
            }
        });
    };
    let begin_sign_in = move || {
        step.set("starting".into());
        leptos::task::spawn_local(async move {
            match chatgpt_signin_start().await {
                Ok(()) => {
                    if poll.get_value().is_none() {
                        poll.set_value(set_interval_with_handle(tick, std::time::Duration::from_millis(700)).ok());
                    }
                    if let Some(el) = screen.get_untracked() {
                        let _ = el.focus();
                    }
                }
                Err(e) => {
                    step.set("closed".into());
                    err.set(Some(e.to_string()));
                }
            }
        });
    };
    // First time on this server: set the image maker up, then go straight to sign-in.
    let setup_poll = StoredValue::new(None::<IntervalHandle>);
    let stop_setup = move || {
        if let Some(h) = setup_poll.get_value() {
            h.clear();
            setup_poll.set_value(None);
        }
    };
    let setup_tick = move || {
        leptos::task::spawn_local(async move {
            let Ok(s) = image_status().await else { return };
            if s.installed || s.install == "ok" {
                stop_setup();
                begin_sign_in();
            } else if let Some(why) = s.install.strip_prefix("failed: ") {
                stop_setup();
                step.set("setup_failed".into());
                err.set(Some(why.to_string()));
            }
        });
    };
    Effect::new(move |_| {
        if open.get() {
            frame.set(None);
            step.set("starting".into());
            err.set(None);
            leptos::task::spawn_local(async move {
                match image_status().await {
                    Ok(s) if !s.installed => {
                        step.set("setup".into());
                        let _ = chatgpt_install().await;
                        if setup_poll.get_value().is_none() {
                            setup_poll.set_value(set_interval_with_handle(setup_tick, std::time::Duration::from_millis(2000)).ok());
                        }
                    }
                    _ => begin_sign_in(),
                }
            });
        } else {
            stop_polling();
            stop_setup();
        }
    });
    let close = move || {
        if !matches!(step.get_untracked().as_str(), "done" | "setup" | "setup_failed" | "closed") {
            send("stop", 0.0, 0.0, String::new());
        }
        open.set(false);
    };
    let click = move |ev: leptos::ev::MouseEvent| {
        let el = event_target::<leptos::web_sys::Element>(&ev);
        let (w, h) = (el.client_width().max(1) as f64, el.client_height().max(1) as f64);
        send("click", ev.offset_x() as f64 * 1280.0 / w, ev.offset_y() as f64 * 900.0 / h, String::new());
        if let Some(s) = screen.get_untracked() {
            let _ = s.focus();
        }
    };
    let keydown = move |ev: leptos::ev::KeyboardEvent| {
        if ev.meta_key() || ev.ctrl_key() {
            return; // let the owner paste with ⌘V / Ctrl+V (handled by on:paste)
        }
        let k = ev.key();
        if k == "Escape" {
            ev.prevent_default();
            close();
            return;
        }
        if k.chars().count() == 1 {
            ev.prevent_default();
            send("text", 0.0, 0.0, k);
        } else if matches!(k.as_str(), "Enter" | "Tab" | "Backspace" | "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight") {
            ev.prevent_default();
            let k = if k == "Tab" && ev.shift_key() { "Shift+Tab".to_string() } else { k };
            send("key", 0.0, 0.0, k);
        }
    };
    let paste = move |ev: leptos::ev::ClipboardEvent| {
        #[cfg(feature = "hydrate")]
        if let Some(t) = ev.clipboard_data().and_then(|d| d.get_data("text").ok()) {
            ev.prevent_default();
            send("text", 0.0, 0.0, t);
        }
        #[cfg(not(feature = "hydrate"))]
        let _ = ev;
    };
    let wheel = move |ev: leptos::ev::WheelEvent| {
        ev.prevent_default();
        send("scroll", 0.0, ev.delta_y(), String::new());
    };
    let instruction = move || match step.get().as_str() {
        "setup" => "One-time setup: getting the image maker ready on your server (a few minutes the first time)…",
        "setup_failed" => "Setup needs the server's admin once. Run this on the server, then press Try again:",
        "starting" => "Opening a private browser on your server…",
        "log_in" => "Click “Log in” on the picture, then sign in the way you usually do.",
        "signing_in" => "Sign in: click a box on the picture and type. Email codes and Google/Apple sign-in work too.",
        "human_check" => "ChatGPT wants to check you're human. Click the checkbox on the picture yourself.",
        "done" => "Connected. Your images now come from your ChatGPT account.",
        _ => "The browser closed. Press Try again.",
    };
    view! {
        <Show when=move || open.get()>
            <div class="modal-backdrop" on:click=move |_| close()>
                <div class="modal chatgpt-modal" role="dialog" aria-modal="true" aria-labelledby="cg-title" on:click=|ev| ev.stop_propagation()>
                    <header class="modal-head">
                        <div>
                            <h2 id="cg-title">"Connect ChatGPT"</h2>
                            <ol class="cg-steps">
                                <li class:on=move || matches!(step.get().as_str(), "setup" | "setup_failed")>"Set up"</li>
                                <li class:on=move || matches!(step.get().as_str(), "starting" | "log_in" | "signing_in")>"Sign in"</li>
                                <li class:on=move || step.get() == "human_check">"Human check, if asked"</li>
                                <li class:on=move || step.get() == "done">"Done"</li>
                            </ol>
                        </div>
                        <button class="btn small ghost" type="button" on:click=move |_| close()>{move || if step.get() == "done" { "Close" } else { "Cancel" }}</button>
                    </header>
                    <p class="modal-say" role="status" data-step=move || step.get()>{instruction}</p>
                    {move || (step.get() == "setup_failed").then(|| view! {
                        <pre class="cmd"><code>"sudo juggernaught tools install chatgpt-image"</code></pre>
                        <details class="hint"><summary>"Why"</summary>{move || err.get()}</details>
                    })}
                    {move || err.get().filter(|_| step.get() != "setup_failed").map(|e| view! { <p class="hint err" role="alert">{e}</p> })}
                    <div class="remote-browser" tabindex="0" node_ref=screen on:keydown=keydown on:paste=paste>
                        {move || match (frame.get(), step.get().as_str()) {
                            (Some(f), _) => view! { <img src=format!("data:image/jpeg;base64,{f}") alt="Your server's private browser showing ChatGPT" width="1280" height="900" on:click=click on:wheel=wheel/> }.into_any(),
                            (None, "setup") => view! { <div class="remote-wait"><span class="spinner"></span>"Setting up the image maker…"</div> }.into_any(),
                            (None, "setup_failed" | "closed") => view! { <div class="remote-wait">"Nothing running yet."</div> }.into_any(),
                            _ => view! { <div class="remote-wait"><span class="spinner"></span>"Starting the browser…"</div> }.into_any(),
                        }}
                    </div>
                    <p class="hint">"Click on the picture, then just type: your keys and pastes go to that browser and are never saved. Scroll with your mouse or trackpad."</p>
                    {move || matches!(step.get().as_str(), "closed" | "setup_failed").then(|| view! { <button class="btn primary" type="button" on:click=move |_| { open.set(false); set_timeout(move || open.set(true), std::time::Duration::from_millis(50)); }>"Try again"</button> })}
                </div>
            </div>
        </Show>
    }
    .into_any()
}

#[component]
fn ImageSettings() -> impl IntoView {
    let current = Resource::new(|| (), |_| image_settings());
    let key = RwSignal::new(String::new());
    let model = RwSignal::new(String::new());
    let msg = RwSignal::new(None::<String>);
    Effect::new(move |_| {
        if let Some(Ok((_, m))) = current.get() {
            model.set(m);
        }
    });
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let (k, m) = (key.get_untracked(), model.get_untracked());
        leptos::task::spawn_local(async move {
            msg.set(Some(match save_image_settings(Some(k).filter(|k| !k.is_empty()), Some(m)).await { Ok(()) => "Saved. Ask for a hero image, a texture or icons in chat.".into(), Err(e) => e.to_string() }));
            key.set(String::new());
            current.refetch();
        });
    };
    view! {
        <form class="intelligence" on:submit=save>
            <h3 class="sub-h">"OpenAI API key (optional fallback)"</h3>
            <p class="hint">"Only used when ChatGPT can't make an image: not signed in, paused, or out of images for now. Stored encrypted. Logos, cards, posts and the brand book never need an image model."</p>
            <div class="provider-fields">
                <label>"OpenAI API key"
                    <input type="password" autocomplete="off" placeholder=move || if current.get().and_then(|r| r.ok()).is_some_and(|c| c.0) { "Saved · enter a new key to replace" } else { "sk-…" }
                        prop:value=move || key.get() on:input=move |ev| key.set(event_target_value(&ev))/>
                </label>
                <label>"Model"
                    <input prop:value=move || model.get() on:input=move |ev| model.set(event_target_value(&ev))/>
                </label>
                <button class="btn primary" type="submit">"Save"</button>
            </div>
            {move || msg.get().map(|m| view! { <p class="hint" role="status">{m}</p> })}
        </form>
    }
}
