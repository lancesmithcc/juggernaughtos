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

    let passkey = move |_| {
        working.set(true);
        msg.set(None);
        leptos::task::spawn_local(async move {
            let r = crate::client::login_passkey().await;
            working.set(false);
            if r == "ok" {
                crate::client::go("/");
            } else {
                msg.set(Some(r));
            }
        });
    };

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
                Ok(()) => sent.set(true),
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
                <header class="setup-head"><span class="mark" aria-hidden="true">"J"</span></header>
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
                                <p class="sent" role="status">"If that address belongs to this workspace, a sign-in link is on its way. It works once and expires in 15 minutes."</p>
                            </Show>
                        }.into_any(),
                    }}
                    {move || msg.get().map(|m| view! { <p class="error" role="alert">{m}</p> })}
                    <div class="login-foot">
                        <Show when=move || mode.get() != Mode::Choose>
                            <button class="linkish" type="button" on:click=move |_| { msg.set(None); sent.set(false); mode.set(Mode::Choose) }>"← Other ways to sign in"</button>
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
