//! Sidebar panels. Everything here is also doable from chat; panels are for looking.

use crate::api::{codex_images_ready, image_settings, save_image_settings, get_site, publish_site, rollback_site, get_brand, get_providers, get_security, get_sign_in_methods, get_standards, get_tools, save_provider_settings};
use crate::pages::account::{AddPasskey, RegenerateRecovery, TotpEnroll};
use leptos::prelude::*;

/// A manual section: the brand-colour plate with its number, then the page.
#[component]
pub fn Panel(no: &'static str, title: &'static str, lede: &'static str, children: Children) -> impl IntoView {
    view! {
        <section class="panel">
            <SectionPlate no=no title=title lede=lede/>
            {children()}
        </section>
    }
}

#[component]
pub fn SectionPlate(no: &'static str, title: &'static str, lede: &'static str) -> impl IntoView {
    view! {
        <header class="panel-head">
            <div class="plate"><span class="plate-no" aria-hidden="true">{no}</span><h1>{title}</h1></div>
            <p>{lede}</p>
        </header>
    }
}

#[component]
pub fn BrandPanel() -> impl IntoView {
    let brand = Resource::new(|| (), |_| get_brand());
    view! {
        <Panel no="4" title="Brand" lede="Your standards. Every asset and page is checked against these before it's final.">
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
                            <p class="specimen-h">{b.heading_font.clone()}</p>
                            <p class="specimen-b">{b.body_font.clone()}</p>
                            <dl class="kv spec"><dt>"Headings"</dt><dd>{b.heading_font.clone()}</dd><dt>"Body text"</dt><dd>{b.body_font.clone()}</dd></dl>
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
        <Panel no="3" title="Website" lede="Your site is built from checked blocks: secure, fast and accessible by construction. Ask in chat to add or change pages; publish here or there.">
            <Suspense fallback=|| view! { <div class="skeleton"></div> }>
                {move || Suspend::new(async move {
                    site.await.map(|s| {
                        let has_drafts = s.pages.iter().any(|p| p.2 == "draft");
                        let can_rollback = s.deploys.iter().filter(|d| d.1 == "superseded").count() > 0;
                        let no_pages = s.pages.is_empty();
                        view! {
                            <div class="site-status">
                                <div>
                                    <a class="site-url" href=s.public_url.clone() target="_blank" rel="noopener">{s.public_url.clone()}</a>
                                    <p class="note" data-tone=if s.live { "done" } else { "needs" }>{if s.live { "Live." } else { "Not published yet." }}</p>
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
        <Panel no="5" title="Security" lede="The Steward watches, patches and backs up on its own. You'll hear from it only when something needs you.">
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
        <Panel no="6" title="Settings" lede="Connect intelligence, manage how you sign in, and see the tools I can use.">
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
                            <p class="hint">"A passkey works in the browser or keychain it was made in. On a Mac, choose iCloud Keychain when asked and it also works in Safari, Chrome and on your iPhone. Add one in each browser you sign in from otherwise."</p>
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

/// How images are made: the owner's own ChatGPT, by hand-off. No sign-in needed here.
#[component]
fn ChatGptImages() -> impl IntoView {
    let ready = Resource::new(|| (), |_| codex_images_ready());
    view! {
        <div class="intelligence chatgpt-images">
            <h3 class="sub-h">"Your ChatGPT"</h3>
            <p class="hint">"Images are made with GPT Image in your own ChatGPT plan. I write the prompt with your brand's colours and imagery rules; the finished image lands in the chat and in Assets, and gets checked against your brand like everything else."</p>
            <Suspense fallback=|| view! { <p class="hint">"Checking…"</p> }>
                {move || ready.get().map(|r| match r {
                    Ok(true) => view! {
                        <p class="ok-line">"Automatic: Codex is signed in with your ChatGPT account, so images are made without you lifting a finger."</p>
                    }.into_any(),
                    _ => view! {
                        <p class="hint">"Right now you make each image yourself: an Open in ChatGPT button opens it with the prompt filled in, and you drop the picture back into the chat. To have them made automatically, sign Codex in with your ChatGPT account once, from the server's shell:"</p>
                        <p><code class="cmd-line">"juggernaught connect codex-images"</code></p>
                    }.into_any(),
                })}
            </Suspense>
        </div>
    }
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
            <h3 class="sub-h">"OpenAI API key (optional)"</h3>
            <p class="hint">"Only needed without Codex. With a key saved, image requests also get a Use my API key button that uses the OpenAI Images API instead of ChatGPT (billed to your key, paced). Stored encrypted. Logos, cards, posts and the brand book never need an image model."</p>
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
