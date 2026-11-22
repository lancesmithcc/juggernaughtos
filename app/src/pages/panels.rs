//! Sidebar panels. Everything here is also doable from chat; panels are for looking.

use crate::api::{get_brand, get_providers, get_security, get_sign_in_methods, get_standards, get_tools, save_provider_settings};
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
pub fn AssetsPanel() -> impl IntoView {
    let kinds = ["Logos", "Social", "Video", "Print", "Email signatures", "Icons & textures", "Documents"];
    view! {
        <Panel title="Assets" lede="Everything we've made, filed by kind. Drafts show a badge until they pass the brand check.">
            <div class="filters">{kinds.iter().map(|k| view! { <button class="chip">{*k}</button> }).collect_view()}</div>
            <div class="empty-state">
                <div class="empty-art" aria-hidden="true"><i></i><i></i><i></i></div>
                <h2>"Nothing here yet"</h2>
                <p>"Ask for a logo, a launch post or a business card and it will land here."</p>
            </div>
        </Panel>
    }
}

#[component]
pub fn BrandPanel() -> impl IntoView {
    let brand = Resource::new(|| (), |_| get_brand());
    view! {
        <Panel title="Brand" lede="Your standards. Every asset and page is checked against these before it's final.">
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
    let std = Resource::new(|| (), |_| get_standards());
    view! {
        <Panel title="Website" lede="Your site is built from checked blocks: secure, fast and accessible by construction. You can override a rule — you'll see why it exists first.">
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
            <h2 class="section-h">"Signing in"</h2>
            <SignIn/>
            <h2 class="section-h">"Tools"</h2>
            <Suspense fallback=|| view! { <div class="skeleton"></div> }>
                {move || Suspend::new(async move {
                    tools.await.map(|t| view! {
                        <ul class="rules">{t.into_iter().map(|x| view! {
                            <li class="rule"><code>{x.name}</code><span class=format!("badge {}", x.risk)>{x.risk.clone()}</span><p>{x.description}</p></li>
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
