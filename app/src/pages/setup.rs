//! First-run wizard: builds the brand, then the owner's passwordless sign-in.

use crate::api::{adopt_logo_concept, complete_setup, logo_concept_previews, BrandKitView, WizardInput};
use crate::pages::assets::AssetTile;
use crate::pages::account::{AddPasskey, RecoveryCodes, TotpEnroll};
use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

const TRAITS: &[&str] = &["Warm", "Bold", "Calm", "Playful", "Premium", "Down-to-earth", "Technical", "Rebellious", "Trustworthy", "Minimal"];
const SWATCHES: &[(&str, &str)] = &[
    ("#c2502a", "Terracotta"),
    ("#1f6f5c", "Forest"),
    ("#2b4acb", "Cobalt"),
    ("#7a3fa8", "Plum"),
    ("#b8860b", "Ochre"),
    ("#1b1a17", "Ink"),
];
const PAIRINGS: &[(&str, &str, &str)] = &[
    ("Plus Jakarta Sans", "Plus Jakarta Sans", "Clean & friendly"),
    ("Fraunces", "Inter", "Editorial"),
    ("Space Grotesk", "Inter", "Modern tech"),
    ("DM Serif Display", "DM Sans", "Classic"),
    ("Syne", "Manrope", "Expressive"),
];

#[component]
pub fn SetupPage() -> impl IntoView {
    let step = RwSignal::new(0usize);
    let form = RwSignal::new(WizardInput {
        primary: SWATCHES[0].0.into(),
        heading_font: PAIRINGS[0].0.into(),
        body_font: PAIRINGS[0].1.into(),
        methods: vec!["passkey".into(), "magic_link".into()],
        ..Default::default()
    });
    let query = use_query_map();
    let save = Action::new(move |input: &WizardInput| {
        let mut input = input.clone();
        input.token = query.with_untracked(|q| q.get("token").unwrap_or_default());
        async move { complete_setup(input).await }
    });
    let finished = move || save.value().with(|v| matches!(v, Some(Ok(_))));

    let titles = ["Your business", "Who it's for", "The look", "Your logo", "Signing in"];
    let can_next = move || {
        form.with(|f| match step.get() {
            0 => !f.business.trim().is_empty(),
            1 => !f.traits.is_empty(),
            4 => f.owner_email.contains('@') && !f.methods.is_empty(),
            _ => true,
        })
    };

    view! {
        <div class="setup">
            <Show when=finished fallback=move || view! { <WizardCard step=step form=form save=save titles=titles can_next=can_next/> }>
                <SecureStep save=save form=form/>
            </Show>
        </div>
    }
}

#[component]
fn WizardCard(
    step: RwSignal<usize>,
    form: RwSignal<WizardInput>,
    save: Action<WizardInput, Result<crate::api::SetupResult, ServerFnError>>,
    titles: [&'static str; 5],
    can_next: impl Fn() -> bool + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! {
            <div class="setup-card">
                <header class="setup-head">
                    <img class="mark-logo" src="/juggernaught.svg" alt="" width="30" height="30"/>
                    <ol class="steps" aria-label="Progress">
                        {titles.iter().enumerate().map(|(i, t)| view! {
                            <li class:done=move || { step.get() > i } class:current=move || { step.get() == i }>
                                <span class="sr-only">{*t}</span>
                            </li>
                        }).collect_view()}
                    </ol>
                </header>

                <div class="setup-body">
                    {move || {
                        let i = step.get();
                        view! { <h1 class="setup-title" style="--enter:1">{titles[i]}</h1> }
                    }}
                    {move || match step.get() {
                        0 => view! {
                            <div class="fields" style="--enter:2">
                                <label>"Business name"
                                    <input autofocus placeholder="Acme Bakery" prop:value=move || form.with(|f| f.business.clone())
                                        on:input=move |ev| form.update(|f| f.business = event_target_value(&ev))/>
                                </label>
                                <label>"What do you do, in a sentence?"
                                    <textarea rows="3" placeholder="Small-batch sourdough and pastries, baked every morning downtown."
                                        prop:value=move || form.with(|f| f.what_you_do.clone())
                                        on:input=move |ev| form.update(|f| f.what_you_do = event_target_value(&ev))></textarea>
                                </label>
                            </div>
                        }.into_any(),
                        1 => view! {
                            <div class="fields" style="--enter:2">
                                <label>"Who are your customers?"
                                    <input placeholder="Neighbours, office workers, weekend food lovers"
                                        prop:value=move || form.with(|f| f.audience.clone())
                                        on:input=move |ev| form.update(|f| f.audience = event_target_value(&ev))/>
                                </label>
                                <fieldset>
                                    <legend>"Pick up to three words for your personality"</legend>
                                    <div class="chips">
                                        {TRAITS.iter().map(|t| {
                                            let t = t.to_string();
                                            let t2 = t.clone();
                                            let on = move || form.with(|f| f.traits.contains(&t2));
                                            view! {
                                                <button type="button" class="chip" class:on=on.clone() aria-pressed=move || on().to_string()
                                                    on:click=move |_| form.update(|f| {
                                                        if let Some(p) = f.traits.iter().position(|x| x == &t) { f.traits.remove(p); }
                                                        else if f.traits.len() < 3 { f.traits.push(t.clone()); }
                                                    })>{t.clone()}</button>
                                            }
                                        }).collect_view()}
                                    </div>
                                </fieldset>
                            </div>
                        }.into_any(),
                        2 => view! {
                            <div class="fields" style="--enter:2">
                                <fieldset>
                                    <legend>"Signature colour"</legend>
                                    <div class="swatches">
                                        {SWATCHES.iter().map(|(hex, name)| {
                                            let hex = hex.to_string();
                                            let h2 = hex.clone();
                                            view! {
                                                <button type="button" class="swatch" style=format!("--c:{hex}")
                                                    class:on=move || form.with(|f| f.primary == h2)
                                                    aria-label=*name on:click=move |_| form.update(|f| f.primary = hex.clone())></button>
                                            }
                                        }).collect_view()}
                                        <label class="swatch custom" title="Custom colour">
                                            <input type="color" prop:value=move || form.with(|f| f.primary.clone())
                                                on:input=move |ev| form.update(|f| f.primary = event_target_value(&ev))/>
                                            <span class="sr-only">"Custom colour"</span>
                                        </label>
                                    </div>
                                </fieldset>
                                <fieldset>
                                    <legend>"Type"</legend>
                                    <div class="pairings">
                                        {PAIRINGS.iter().map(|(h, b, label)| {
                                            let (h, b) = (h.to_string(), b.to_string());
                                            let h2 = h.clone();
                                            view! {
                                                <button type="button" class="pairing" class:on=move || form.with(|f| f.heading_font == h2)
                                                    on:click={let (h, b) = (h.clone(), b.clone()); move |_| form.update(|f| { f.heading_font = h.clone(); f.body_font = b.clone(); })}>
                                                    <strong>{h.clone()}</strong><span>{format!("{label} · body {b}")}</span>
                                                </button>
                                            }
                                        }).collect_view()}
                                    </div>
                                    <p class="hint">"Every Google Font is available later — just ask in chat."</p>
                                </fieldset>
                            </div>
                        }.into_any(),
                        3 => view! {
                            <div class="fields logo-step" style="--enter:2">
                                <div class="logo-preview" style=move || form.with(|f| format!("--c:{}", f.primary))>
                                    <span>{move || form.with(|f| f.business.chars().next().unwrap_or('J').to_uppercase().to_string())}</span>
                                </div>
                                <p>"Once an intelligence provider is connected, I'll sketch four logo concepts in your colour and personality. Pick one, refine it in chat, and I'll vectorize it into a crisp SVG with light, dark and icon versions."</p>
                                <p class="hint">"Already have a logo? Drop it in chat any time and I'll vectorize and file it."</p>
                            </div>
                        }.into_any(),
                        _ => view! {
                            <div class="fields" style="--enter:2">
                                <label>"Your email"
                                    <input type="email" autocomplete="email" placeholder="you@business.com"
                                        prop:value=move || form.with(|f| f.owner_email.clone())
                                        on:input=move |ev| form.update(|f| f.owner_email = event_target_value(&ev))/>
                                </label>
                                <fieldset>
                                    <legend>"How do you want to sign in? No passwords, ever."</legend>
                                    <div class="methods">
                                        <Method form=form key="passkey" title="Face or fingerprint" body="Passkey on this device. Fastest and phishing-proof."/>
                                        <Method form=form key="totp" title="Authenticator app" body="6-digit codes from Google Authenticator, 1Password, Authy…"/>
                                        <Method form=form key="magic_link" title="Email link" body="A one-time sign-in link, valid 15 minutes."/>
                                    </div>
                                    <p class="hint">"We'll also give you recovery codes. Keep at least two ways in."</p>
                                </fieldset>
                            </div>
                        }.into_any(),
                    }}
                    {move || save.value().get().and_then(|r| r.err()).map(|e| view! { <p class="error" role="alert">{e.to_string()}</p> })}
                </div>

                <footer class="setup-foot">
                    <button class="btn ghost" on:click=move |_| step.update(|s| *s = s.saturating_sub(1)) disabled=move || step.get() == 0>"Back"</button>
                    {move || if step.get() < 4 {
                        view! { <button class="btn primary" disabled=move || !can_next() on:click=move |_| step.update(|s| *s += 1)>"Continue"</button> }.into_any()
                    } else {
                        view! { <button class="btn primary" disabled=move || !can_next() || save.pending().get() on:click=move |_| { save.dispatch(form.get_untracked()); }>
                            {move || if save.pending().get() { "Setting up…" } else { "Create my workspace" }}
                        </button> }.into_any()
                    }}
                </footer>
            </div>
    }
}

/// After the workspace exists: lock it down before going in.
#[component]
fn SecureStep(save: Action<WizardInput, Result<crate::api::SetupResult, ServerFnError>>, form: RwSignal<WizardInput>) -> impl IntoView {
    let codes = save.value().get_untracked().and_then(|r| r.ok()).map(|r| r.recovery_codes).unwrap_or_default();
    let wants = move |m: &str| form.with_untracked(|f| f.methods.iter().any(|x| x == m));
    let (want_passkey, want_totp) = (wants("passkey"), wants("totp"));
    view! {
        <div class="setup-card">
            <header class="setup-head"><img class="mark-logo" src="/juggernaught.svg" alt="" width="30" height="30"/><span class="pill ok">"Workspace created"</span></header>
            <div class="setup-body">
                <h1 class="setup-title">"Secure your sign-in"</h1>
                <div class="fields">
                    {want_passkey.then(|| view! {
                        <section class="secure-block"><h2>"Face or fingerprint"</h2><AddPasskey on_done=|_| ()/></section>
                    })}
                    {want_totp.then(|| view! {
                        <section class="secure-block"><h2>"Authenticator app"</h2><TotpEnroll on_done=|_| ()/></section>
                    })}
                    <section class="secure-block"><h2>"Recovery codes"</h2><RecoveryCodes codes=codes/></section>
                    <LogoPicker/>
                    {form.with_untracked(|f| f.methods.iter().any(|m| m == "magic_link")).then(|| view! {
                        <p class="hint">{format!("Email links will go to {}.", form.with_untracked(|f| f.owner_email.clone()))}</p>
                    })}
                </div>
            </div>
            <footer class="setup-foot end">
                <button class="btn ghost" on:click=|_| crate::client::go("/go-live")>"Put it online"</button>
                <button class="btn primary" on:click=|_| crate::client::go("/")>"Open my workspace"</button>
            </footer>
        </div>
    }
}

#[component]
fn Method(form: RwSignal<WizardInput>, key: &'static str, title: &'static str, body: &'static str) -> impl IntoView {
    let on = move || form.with(|f| f.methods.iter().any(|m| m == key));
    view! {
        <button type="button" class="method" class:on=on aria-label=title aria-pressed=move || on().to_string()
            on:click=move |_| form.update(|f| {
                if let Some(p) = f.methods.iter().position(|m| m == key) { f.methods.remove(p); } else { f.methods.push(key.into()); }
            })>
            <span class="check" aria-hidden="true"></span>
            <strong>{title}</strong>
            <span>{body}</span>
        </button>
    }
}

/// Pick a logo concept → brand kit (logo files, guidelines PDF, launch post).
#[component]
pub fn LogoPicker() -> impl IntoView {
    let concepts = Resource::new(|| (), |_| logo_concept_previews());
    let chosen = RwSignal::new(None::<String>);
    let kit = RwSignal::new(None::<BrandKitView>);
    let error = RwSignal::new(None::<String>);
    let adopt = move |key: String| {
        chosen.set(Some(key.clone()));
        error.set(None);
        leptos::task::spawn_local(async move {
            match adopt_logo_concept(key).await {
                Ok(k) => kit.set(Some(k)),
                Err(e) => {
                    chosen.set(None);
                    error.set(Some(e.to_string().replace("error running server function: ", "")));
                }
            }
        });
    };
    view! {
        <section class="secure-block logo-picker">
            <h2>"Pick your logo"</h2>
            <Show when=move || kit.get().is_none() fallback=move || view! {
                <p class="ok-line">{move || kit.get().map(|k| k.summary).unwrap_or_default()}</p>
                <div class="chat-assets">{move || kit.get().map(|k| k.assets.into_iter().map(|c| view! { <AssetTile card=c/> }).collect_view())}</div>
            }>
                <p class="hint">"Four starting points in your colours and type. Pick one; you can refine it in chat any time."</p>
                <Suspense fallback=|| view! { <div class="skeleton"></div> }>
                    {move || Suspend::new(async move {
                        concepts.await.map(|list| view! {
                            <div class="concepts">
                                {list.into_iter().map(|c| {
                                    let key = c.key.clone();
                                    let k2 = c.key.clone();
                                    view! {
                                        <button type="button" class="concept" class:busy=move || chosen.with(|x| x.as_deref() == Some(k2.as_str()))
                                            disabled=move || chosen.get().is_some() on:click=move |_| adopt(key.clone())>
                                            <img src=c.image alt=format!("{} logo concept", c.label)/>
                                            <span>{c.label}</span>
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                        })
                    })}
                </Suspense>
                {move || chosen.get().map(|_| view! { <p class="hint" role="status">"Making your logo files and brand guidelines…"</p> })}
            </Show>
            {move || error.get().map(|e| view! { <p class="error" role="alert">{e}</p> })}
        </section>
    }
}
