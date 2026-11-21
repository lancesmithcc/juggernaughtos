//! Sidebar panels. Everything here is also doable from chat; panels are for looking.

use crate::api::{get_brand, get_security, get_standards, get_tools};
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
                    sec.await.map(|s| view! {
                        <div class="posture"><span class="pulse" aria-hidden="true"></span><p>{s.posture}</p></div>
                        <h2 class="section-h">"Encryption"</h2>
                        <dl class="kv">{s.crypto.into_iter().map(|(k, v)| view! { <dt>{k}</dt><dd>{v}</dd> }).collect_view()}</dl>
                        <h2 class="section-h">"Sensors on this machine"</h2>
                        <div class="filters">{s.sensors.into_iter().map(|x| view! { <span class="chip static">{x}</span> }).collect_view()}</div>
                        <h2 class="section-h">"Automatic maintenance"</h2>
                        <ul class="rules">{s.tasks.into_iter().map(|(n, when, what)| view! {
                            <li class="rule"><code>{n}</code><span class="badge">{when}</span><p>{what}</p></li>
                        }).collect_view()}</ul>
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
        <Panel title="Settings" lede="Connect intelligence, manage sign-in methods, and see the tools I can use.">
            <h2 class="section-h">"Intelligence"</h2>
            <div class="providers">
                <div class="provider"><strong>"Claude Code"</strong><span>"Your Claude subscription, signed in on this machine."</span><code>"juggernaught connect claude"</code></div>
                <div class="provider"><strong>"Codex"</strong><span>"Your ChatGPT subscription via the Codex CLI."</span><code>"juggernaught connect codex"</code></div>
                <div class="provider"><strong>"OpenRouter"</strong><span>"Any model with an API key. Pay as you go."</span><code>"juggernaught connect openrouter"</code></div>
            </div>
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
        </Panel>
    }
}
