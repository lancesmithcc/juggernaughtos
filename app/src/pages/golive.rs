//! Go live: put the workspace online at the owner's own domain, step by step.
use crate::api::{go_live, go_live_check, go_live_domain, go_live_forget_token, go_live_target, go_live_token, GoLiveStep, GoLiveView};
use leptos::prelude::*;

fn clean(e: ServerFnError) -> String {
    e.to_string().replace("error running server function: ", "")
}

#[component]
pub fn GoLivePage() -> impl IntoView {
    let state = RwSignal::new(None::<GoLiveView>);
    let busy = RwSignal::new(None::<&'static str>);
    let msg = RwSignal::new(None::<String>);
    let domain = RwSignal::new(String::new());
    let token = RwSignal::new(String::new());
    let editing_domain = RwSignal::new(false);
    // The sidebar's "Needs you" badge reads the workspace overview.
    let overview = use_context::<Resource<Result<crate::api::Overview, ServerFnError>>>();

    // One place to run an action: shows what's happening, keeps the latest state.
    let run = move |label: &'static str, fut: std::pin::Pin<Box<dyn std::future::Future<Output = Result<GoLiveView, ServerFnError>>>>| {
        busy.set(Some(label));
        msg.set(None);
        leptos::task::spawn_local(async move {
            match fut.await {
                Ok(v) => {
                    if let Some(d) = &v.domain {
                        domain.set(d.clone());
                    }
                    state.set(Some(v));
                    if let Some(o) = overview {
                        o.refetch();
                    }
                }
                Err(e) => msg.set(Some(clean(e))),
            }
            busy.set(None);
        });
    };
    Effect::new(move |_| run("Loading…", Box::pin(go_live())));

    let check = move |_| run("Checking…", Box::pin(go_live_check()));
    let submit_domain = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        editing_domain.set(false);
        run("Looking up your domain…", Box::pin(go_live_domain(domain.get_untracked())));
    };
    let submit_token = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let t = token.get_untracked();
        token.set(String::new());
        run("Connecting to Cloudflare…", Box::pin(go_live_token(t)));
    };

    view! {
        <section class="panel golive">
            <header class="panel-head">
                <h1>"Go live"</h1>
                <p>"Put your workspace and website online at your own domain. I do the technical parts and tell you when I need you: making an account, paying, or pasting something in."</p>
            </header>
            {move || busy.get().map(|b| view! { <p class="hint" role="status"><span class="spinner"></span>{b}</p> })}
            {move || msg.get().map(|m| view! { <p class="hint err" role="alert">{m}</p> })}
            {move || state.get().map(|v| {
                let err = v.error.clone();
                view! {
                    {err.map(|e| view! { <p class="hint err" role="alert">{format!("Last check: {e}")}</p> })}
                    <ol class="golive-steps">
                        {v.steps.iter().enumerate().map(|(i, s)| view! {
                            <StepCard n=i + 1 step=s.clone() v=v.clone() domain=domain token=token editing_domain=editing_domain busy=busy
                                submit_domain=submit_domain submit_token=submit_token run=run check=check/>
                        }).collect_view()}
                    </ol>
                }
            })}
        </section>
    }
}

#[component]
fn StepCard(
    n: usize,
    step: GoLiveStep,
    v: GoLiveView,
    domain: RwSignal<String>,
    token: RwSignal<String>,
    editing_domain: RwSignal<bool>,
    busy: RwSignal<Option<&'static str>>,
    submit_domain: impl Fn(leptos::ev::SubmitEvent) + Copy + Send + Sync + 'static,
    submit_token: impl Fn(leptos::ev::SubmitEvent) + Copy + Send + Sync + 'static,
    run: impl Fn(&'static str, std::pin::Pin<Box<dyn std::future::Future<Output = Result<GoLiveView, ServerFnError>>>>) + Copy + Send + Sync + 'static,
    check: impl Fn(leptos::ev::MouseEvent) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let status = step.status.clone();
    let label = match status.as_str() {
        "done" => "Done",
        "needs_you" => "Needs you",
        "waiting" => "Checking",
        "active" => "Now",
        _ => "Later",
    };
    let body = match (step.key.as_str(), status.as_str()) {
        ("domain", "active") => Some(domain_form(domain, busy, submit_domain).into_any()),
        ("domain", "done") => Some(view! {
            <Show when=move || editing_domain.get() fallback=move || view! {
                <button class="linkish" type="button" on:click=move |_| editing_domain.set(true)>"Use a different domain"</button>
            }>
                {domain_form(domain, busy, submit_domain)}
            </Show>
        }.into_any()),
        ("domain", "needs_you") => {
            let d = v.domain.clone().unwrap_or_default();
            Some(view! {
                <ol class="golive-todo">
                    <li>"Open Cloudflare's domain page (it asks you to sign in or make a free account), search for "<strong>{d.clone()}</strong>", and buy it. You pay Cloudflare directly; I never see your card."</li>
                    <li>"Come back here. I check every few minutes and carry on by myself, or press Check now."</li>
                </ol>
                <div class="row-actions">
                    <a class="btn primary" href=v.buy_url.clone() target="_blank" rel="noopener noreferrer">{format!("Buy {d} on Cloudflare ↗")}</a>
                    <button class="btn ghost" type="button" disabled=move || busy.get().is_some() on:click=check>"I bought it · check now"</button>
                    <button class="linkish" type="button" on:click=move |_| editing_domain.set(true)>"Use a different domain"</button>
                </div>
                <Show when=move || editing_domain.get()>{domain_form(domain, busy, submit_domain)}</Show>
            }.into_any())
        }
        ("cloudflare", "needs_you") => Some(view! {
            <ol class="golive-todo">
                <li>"No Cloudflare account yet? "<a href=v.sign_up_url.clone() target="_blank" rel="noopener noreferrer">"Make a free one ↗"</a>" (no card needed)."</li>
                <li>
                    <a class="btn primary small" href=v.token_url.clone() target="_blank" rel="noopener noreferrer">"Create the token on Cloudflare ↗"</a>
                    <span class="hint">" It opens with the right permissions already ticked. Scroll down, press "<em>"Continue to summary"</em>", then "<em>"Create Token"</em>", and copy the token."</span>
                </li>
                <li>
                    "Paste it here:"
                    <form class="golive-inline" on:submit=submit_token>
                        <label class="sr-only" for="cf-token">"Cloudflare API token"</label>
                        <input id="cf-token" type="password" autocomplete="off" spellcheck="false" placeholder="Cloudflare API token"
                            prop:value=move || token.get() on:input=move |ev| token.set(event_target_value(&ev))/>
                        <button class="btn primary" type="submit" disabled=move || busy.get().is_some() || token.with(|t| t.trim().is_empty())>"Connect"</button>
                    </form>
                    <span class="hint">"It stays encrypted on this box and only manages your domain's DNS."</span>
                </li>
            </ol>
        }.into_any()),
        ("cloudflare", "done") => Some(view! {
            <button class="linkish" type="button" on:click=move |_| run("Disconnecting…", Box::pin(go_live_forget_token()))>"Disconnect Cloudflare"</button>
        }.into_any()),
        ("dns", "needs_you") => {
            let ns = v.nameservers.clone();
            let current = v.current_ns.clone();
            Some(view! {
                <ol class="golive-todo">
                    <li>{format!("Sign in at {} and open {}'s DNS or nameserver settings.", v.registrar.clone().unwrap_or_else(|| "the company you bought the domain from".into()), v.domain.clone().unwrap_or_default())}</li>
                    <li>"Choose custom nameservers and replace what's there with these two:"
                        <ul class="ns-list">{ns.into_iter().map(|n| { let c = n.clone(); view! {
                            <li><code>{n}</code><button class="btn ghost small" type="button" on:click=move |_| crate::client::copy(&c)>"Copy"</button></li>
                        } }).collect_view()}</ul>
                    </li>
                    <li>"Save. The switch can take a few hours; I keep checking and carry on when it's done."</li>
                </ol>
                {(!current.is_empty()).then(|| view! { <p class="hint">{format!("Right now the domain uses: {}", current.join(", "))}</p> })}
                <div class="row-actions"><button class="btn ghost" type="button" disabled=move || busy.get().is_some() on:click=check>"I changed them · check now"</button></div>
            }.into_any())
        }
        ("dns", "waiting") => Some(view! {
            <div class="row-actions"><button class="btn ghost" type="button" disabled=move || busy.get().is_some() on:click=check>"Check now"</button></div>
        }.into_any()),
        ("where", "active") => {
            let target = v.target.clone();
            let chosen = move |t: &str| target.as_deref() == Some(t);
            Some(view! {
                <div class="golive-choices">
                    <button class="method" class:on=chosen("vps") type="button" on:click=move |_| run("Saving…", Box::pin(go_live_target("vps".into())))>
                        <strong>"A VPS (recommended)"</strong>
                        <span>"A small rented server that's always on, from about €5 a month at Hetzner, or any host with Ubuntu. I set it up over SSH and move everything across."</span>
                    </button>
                    <button class="method" class:on=chosen("home") type="button" on:click=move |_| run("Saving…", Box::pin(go_live_target("home".into())))>
                        <strong>"This computer"</strong>
                        <span>"Keep it here as a home server. A Cloudflare Tunnel connects it: no router changes, and your home address stays private."</span>
                    </button>
                </div>
            }.into_any())
        }
        _ => None,
    };
    view! {
        <li class="golive-step" data-status=status.clone()>
            <div class="golive-step-head">
                <span class="golive-num" aria-hidden="true">{if status == "done" { "✓".to_string() } else { n.to_string() }}</span>
                <div>
                    <h2>{step.title.clone()}</h2>
                    {(!step.detail.is_empty()).then(|| view! { <p>{step.detail.clone()}</p> })}
                </div>
                <span class=format!("golive-badge s-{status}")>{label}</span>
            </div>
            {body.map(|b| view! { <div class="golive-step-body">{b}</div> })}
        </li>
    }
}

fn domain_form(domain: RwSignal<String>, busy: RwSignal<Option<&'static str>>, submit: impl Fn(leptos::ev::SubmitEvent) + 'static) -> impl IntoView {
    view! {
        <form class="golive-inline" on:submit=submit>
            <label class="sr-only" for="golive-domain">"Domain"</label>
            <input id="golive-domain" type="text" inputmode="url" autocomplete="off" spellcheck="false" placeholder="yourbusiness.com"
                prop:value=move || domain.get() on:input=move |ev| domain.set(event_target_value(&ev))/>
            <button class="btn primary" type="submit" disabled=move || busy.get().is_some() || domain.with(|d| d.trim().is_empty())>"Continue"</button>
        </form>
        <p class="hint">"Don't have one yet? Type the name you'd like and I'll check if it's free."</p>
    }
}
