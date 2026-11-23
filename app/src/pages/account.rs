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

/// After signing in: if this account has no passkey yet, offer Touch ID / Face ID for
/// next time. "Not now" hides it for this browser session.
#[component]
pub fn PasskeyNudge() -> impl IntoView {
    let show = RwSignal::new(false);
    let added = RwSignal::new(false);
    // The account has passkeys, just not in this browser.
    let elsewhere = RwSignal::new(false);
    const KEY: &str = "jugg-passkey-nudge";
    Effect::new(move |_| {
        if session_flag(KEY) || crate::client::passkey_here() {
            return;
        }
        leptos::task::spawn_local(async move {
            if let Ok(m) = crate::api::get_sign_in_methods().await {
                elsewhere.set(!m.passkeys.is_empty());
                show.set(true);
            }
        });
    });
    let dismiss = move || {
        set_session_flag(KEY);
        show.set(false);
    };
    let done = Callback::new(move |_| {
        added.set(true);
        set_session_flag(KEY);
        set_timeout(move || show.set(false), std::time::Duration::from_millis(1800));
    });
    view! {
        <Show when=move || show.get()>
            <div class="modal-backdrop" on:click=move |_| dismiss()>
                <div class="modal" role="dialog" aria-modal="true" aria-labelledby="pk-title" on:click=|ev| ev.stop_propagation()>
                    <h2 id="pk-title">{move || if added.get() { "You're set" } else if elsewhere.get() { "Add your fingerprint in this browser too" } else { "Sign in with your fingerprint next time" }}</h2>
                    <p>{move || if elsewhere.get() {
                        "Your passkey lives in another browser, so this one can't use it. Add one here and next time you just touch Touch ID. On a Mac, save it to iCloud Keychain when asked and it works in Safari, Chrome and on your iPhone too."
                    } else {
                        "Add a passkey and next time you just touch Touch ID (or use Face ID or Windows Hello) instead of waiting for an email link. On a Mac, save it to iCloud Keychain when asked and it works in Safari, Chrome and on your iPhone too. It can't be phished."
                    }}</p>
                    <AddPasskey on_done=done/>
                    <Show when=move || !added.get()>
                        <button class="linkish" type="button" on:click=move |_| dismiss()>"Not now"</button>
                    </Show>
                </div>
            </div>
        </Show>
    }
}

fn session_flag(key: &str) -> bool {
    #[cfg(feature = "hydrate")]
    {
        return web_sys::window().and_then(|w| w.session_storage().ok().flatten()).and_then(|s| s.get_item(key).ok().flatten()).is_some();
    }
    #[allow(unreachable_code)]
    {
        let _ = key;
        true
    }
}

fn set_session_flag(key: &str) {
    #[cfg(feature = "hydrate")]
    if let Some(s) = web_sys::window().and_then(|w| w.session_storage().ok().flatten()) {
        let _ = s.set_item(key, "1");
    }
    let _ = key;
}
