//! Sign-in method components, shared by the setup wizard and Settings.

use crate::api::{regenerate_recovery_codes, totp_begin, totp_confirm, TotpSetup};
use leptos::prelude::*;

/// "Add a passkey" button. Calls `on_done` after a successful registration.
#[component]
pub fn AddPasskey(#[prop(into)] on_done: Callback<()>) -> impl IntoView {
    let status = RwSignal::new(None::<String>);
    let working = RwSignal::new(false);
    let add = move |_| {
        working.set(true);
        status.set(None);
        leptos::task::spawn_local(async move {
            let r = crate::client::register_passkey(String::new()).await;
            working.set(false);
            if r == "ok" {
                status.set(Some("Passkey added. Next time, just use your face or fingerprint.".into()));
                on_done.run(());
            } else {
                status.set(Some(r));
            }
        });
    };
    view! {
        <div class="method-row">
            <button class="btn primary" on:click=add disabled=move || working.get()>
                {move || if working.get() { "Waiting for your device…" } else { "Add a passkey" }}
            </button>
            {move || status.get().map(|s| view! { <p class="hint" role="status">{s}</p> })}
        </div>
    }
}

/// Authenticator-app enrolment: QR → confirm with a code.
#[component]
pub fn TotpEnroll(#[prop(into)] on_done: Callback<()>) -> impl IntoView {
    let setup = RwSignal::new(None::<TotpSetup>);
    let code = RwSignal::new(String::new());
    let msg = RwSignal::new(None::<String>);
    let done = RwSignal::new(false);
    let begin = move |_| {
        msg.set(None);
        leptos::task::spawn_local(async move {
            match totp_begin().await {
                Ok(s) => setup.set(Some(s)),
                Err(e) => msg.set(Some(e.to_string())),
            }
        });
    };
    let confirm = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let c = code.get_untracked();
        leptos::task::spawn_local(async move {
            match totp_confirm(c).await {
                Ok(()) => {
                    done.set(true);
                    setup.set(None);
                    on_done.run(());
                }
                Err(e) => msg.set(Some(e.to_string().replace("error running server function: ", ""))),
            }
        });
    };
    view! {
        <div class="method-row">
            <Show when=move || done.get() fallback=move || view! {
                <Show when=move || setup.get().is_some() fallback=move || view! {
                    <button class="btn ghost" on:click=begin>"Set up an authenticator app"</button>
                }>
                    {move || setup.get().map(|s| view! {
                        <div class="totp">
                            <img class="qr" alt="QR code for your authenticator app" width="180" height="180" src=format!("data:image/png;base64,{}", s.qr_png_base64)/>
                            <div class="totp-side">
                                <p>"Scan this with Google Authenticator, 1Password, Authy or similar."</p>
                                <p class="hint">"Can't scan? Enter this key:"</p>
                                <code class="secret">{s.secret.clone()}</code>
                                <form class="inline-form" on:submit=confirm>
                                    <label class="sr-only" for="totp-code">"6-digit code"</label>
                                    <input id="totp-code" inputmode="numeric" autocomplete="one-time-code" maxlength="6" placeholder="123456"
                                        prop:value=move || code.get() on:input=move |ev| code.set(event_target_value(&ev))/>
                                    <button class="btn primary" type="submit">"Confirm"</button>
                                </form>
                            </div>
                        </div>
                    })}
                </Show>
            }>
                <p class="ok-line">"Authenticator app is on."</p>
            </Show>
            {move || msg.get().map(|m| view! { <p class="error" role="alert">{m}</p> })}
        </div>
    }
}

#[component]
pub fn RecoveryCodes(codes: Vec<String>) -> impl IntoView {
    view! {
        <div class="recovery">
            <ol class="codes">{codes.into_iter().map(|c| view! { <li><code>{c}</code></li> }).collect_view()}</ol>
            <p class="hint">"Each code works once. Store them in your password manager or print them. They're shown only now."</p>
        </div>
    }
}

/// Button that replaces recovery codes and shows the new set.
#[component]
pub fn RegenerateRecovery(#[prop(into)] on_done: Callback<()>) -> impl IntoView {
    let codes = RwSignal::new(None::<Vec<String>>);
    let go = move |_| {
        leptos::task::spawn_local(async move {
            if let Ok(c) = regenerate_recovery_codes().await {
                codes.set(Some(c));
                on_done.run(());
            }
        });
    };
    view! {
        <div class="method-row">
            {move || match codes.get() {
                Some(c) => view! { <RecoveryCodes codes=c/> }.into_any(),
                None => view! { <button class="btn ghost" on:click=go>"Make new recovery codes"</button> }.into_any(),
            }}
        </div>
    }
}
