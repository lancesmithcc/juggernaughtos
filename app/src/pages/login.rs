use crate::api::{login_recovery, login_totp, request_magic_link};
use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Choose,
    Code,
    Recovery,
    Link,
}

#[component]
pub fn LoginPage() -> impl IntoView {
    let mode = RwSignal::new(Mode::Choose);
    let email = RwSignal::new(String::new());
    let code = RwSignal::new(String::new());
    let msg = RwSignal::new(None::<String>);
    let working = RwSignal::new(false);
    let sent = RwSignal::new(false);
    // Development without a mail server: the link, shown right here.
    let dev_link = RwSignal::new(None::<String>);
    let note = RwSignal::new(None::<String>);
    let query = use_query_map();
    Effect::new(move |_| {
        let e = query.with(|q| q.get("e"));
        msg.set(match e.as_deref() {
            Some("link") => Some("That sign-in link has expired or was already used. Request a new one.".into()),
            Some("limited") => Some("Too many attempts. Wait a few minutes and try again.".into()),
            Some(_) => Some("Something went wrong. Try again.".into()),
            None => None,
        });
    });
    let clean = |e: ServerFnError| e.to_string().replace("error running server function: ", "");

    let try_passkey = move |any_device: bool| {
        working.set(true);
        msg.set(None);
        leptos::task::spawn_local(async move {
            let r = crate::client::login_passkey(any_device).await;
            working.set(false);
            if r == "ok" {
                crate::client::go("/");
            } else if r.contains("No passkeys") || r == "not_here" {
                // First time in this browser: one email link, then the workspace offers a passkey.
                mode.set(Mode::Link);
                note.set(Some(if r.contains("No passkeys") {
                    "No passkey on this account yet. Sign in with an email link this once; right after, you'll be asked to add your fingerprint."
                } else if any_device {
                    "That didn't work. Sign in with an email link here once; right after, you'll be asked to add your fingerprint in this browser."
                } else {
                    "This browser doesn't have your passkey yet (passkeys stay in the browser or keychain they were made in). Sign in with an email link here once; right after, you'll be asked to add your fingerprint, and from then on it's just Touch ID."
                }.to_string()));
            } else {
                msg.set(Some(r));
            }
        });
    };
    let passkey = move |_| try_passkey(false);

    let submit_code = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        working.set(true);
        msg.set(None);
        let (e, c, m) = (email.get_untracked(), code.get_untracked(), mode.get_untracked());
        leptos::task::spawn_local(async move {
            let r = if m == Mode::Recovery { login_recovery(e, c).await } else { login_totp(e, c).await };
            working.set(false);
            match r {
                Ok(()) => crate::client::go("/"),
                Err(err) => msg.set(Some(clean(err))),
            }
        });
    };

    let submit_link = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        working.set(true);
        msg.set(None);
        let e = email.get_untracked();
        leptos::task::spawn_local(async move {
            let r = request_magic_link(e).await;
            working.set(false);
            match r {
                Ok(link) => {
                    dev_link.set(link);
                    sent.set(true);
                }
                Err(err) => msg.set(Some(clean(err))),
            }
        });
    };

    let email_input = move |id: &'static str| view! {
        <label class="sr-only" for=id>"Email"</label>
        <input id=id type="email" required autocomplete="username webauthn" placeholder="you@business.com"
            prop:value=move || email.get() on:input=move |ev| email.set(event_target_value(&ev))/>
    };

    view! {
        <div class="setup">
            <div class="setup-card narrow">
                <header class="setup-head"><img class="mark-logo" src="/juggernaught.svg" alt="" width="30" height="30"/></header>
                <div class="setup-body">
                    <h1 class="setup-title">"Welcome back"</h1>
                    {move || match mode.get() {
                        Mode::Choose => view! {
                            <div class="methods stacked">
                                <button class="method primary-method" type="button" on:click=passkey disabled=move || working.get()>
                                    <strong>{move || if working.get() { "Waiting for your device…" } else { "Sign in with a passkey" }}</strong>
                                    <span>"Face ID, Touch ID, Windows Hello or a security key"</span>
                                </button>
                                <button class="method" type="button" on:click=move |_| { msg.set(None); mode.set(Mode::Code) }>
                                    <strong>"Use an authenticator code"</strong><span>"6 digits from your app"</span>
                                </button>
                                <button class="method" type="button" on:click=move |_| { msg.set(None); mode.set(Mode::Link) }>
                                    <strong>"Email me a sign-in link"</strong><span>"Works once, expires in 15 minutes"</span>
                                </button>
                            </div>
                        }.into_any(),
                        Mode::Code | Mode::Recovery => view! {
                            <form class="inline-form" on:submit=submit_code>
                                {email_input("login-email")}
                                <label class="sr-only" for="login-code">"Code"</label>
                                <input id="login-code" required autocomplete="one-time-code"
                                    inputmode=move || if mode.get() == Mode::Code { "numeric" } else { "text" }
                                    placeholder=move || if mode.get() == Mode::Code { "6-digit code" } else { "XXXXX-XXXXX" }
                                    prop:value=move || code.get() on:input=move |ev| code.set(event_target_value(&ev))/>
                                <button class="btn primary" type="submit" disabled=move || working.get()>"Sign in"</button>
                            </form>
                        }.into_any(),
                        Mode::Link => view! {
                            <Show when=move || sent.get() fallback=move || view! {
                                <form class="inline-form" on:submit=submit_link>
                                    {email_input("link-email")}
                                    <button class="btn primary" type="submit" disabled=move || working.get()>"Email me a link"</button>
                                </form>
                            }>
                                {move || match dev_link.get() {
                                    Some(l) => view! {
                                        <p class="sent" role="status">"Development mode, no mail server set up, so here's your link (only shown on this computer)."</p>
                                        <a class="btn primary" href=l rel="external">"Sign in now"</a>
                                    }.into_any(),
                                    None => view! { <p class="sent" role="status">"If that address belongs to this workspace, a sign-in link is on its way. It works once and expires in 15 minutes."</p> }.into_any(),
                                }}
                            </Show>
                        }.into_any(),
                    }}
                    {move || note.get().filter(|_| mode.get() == Mode::Link).map(|n| view! {
                        <p class="hint" role="status">{n}</p>
                        <button class="linkish" type="button" on:click=move |_| try_passkey(true)>"Use a passkey from my phone or another device"</button>
                    })}
                    {move || msg.get().map(|m| view! { <p class="error" role="alert">{m}</p> })}
                    <div class="login-foot">
                        <Show when=move || mode.get() != Mode::Choose>
                            <button class="linkish" type="button" on:click=move |_| { msg.set(None); sent.set(false); mode.set(Mode::Choose) }><crate::icons::Icon name="back" size=14/>"Other ways to sign in"</button>
                        </Show>
                        <Show when=move || mode.get() != Mode::Recovery>
                            <button class="linkish" type="button" on:click=move |_| { msg.set(None); code.set(String::new()); mode.set(Mode::Recovery) }>"Lost your device? Use a recovery code"</button>
                        </Show>
                    </div>
                </div>
            </div>
        </div>
    }
}
