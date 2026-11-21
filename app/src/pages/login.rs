use crate::api::request_magic_link;
use leptos::prelude::*;

#[component]
pub fn LoginPage() -> impl IntoView {
    let email = RwSignal::new(String::new());
    let send = Action::new(move |e: &String| {
        let e = e.clone();
        async move { request_magic_link(e).await }
    });
    view! {
        <div class="setup">
            <div class="setup-card narrow">
                <header class="setup-head"><span class="mark" aria-hidden="true">"J"</span></header>
                <div class="setup-body">
                    <h1 class="setup-title">"Welcome back"</h1>
                    <div class="methods stacked">
                        <button class="method primary-method" type="button">
                            <strong>"Sign in with a passkey"</strong><span>"Face ID, Touch ID, Windows Hello or a security key"</span>
                        </button>
                        <button class="method" type="button">
                            <strong>"Use an authenticator code"</strong><span>"6 digits from your app"</span>
                        </button>
                    </div>
                    <div class="divider"><span>"or get a link"</span></div>
                    <Show when=move || send.value().get().is_none() fallback=|| view! {
                        <p class="sent" role="status">"If that address belongs to this workspace, a sign-in link is on its way. It works once and expires in 15 minutes."</p>
                    }>
                        <form class="inline-form" on:submit=move |ev| { ev.prevent_default(); send.dispatch(email.get_untracked()); }>
                            <label class="sr-only" for="login-email">"Email"</label>
                            <input id="login-email" type="email" autocomplete="email webauthn" placeholder="you@business.com"
                                prop:value=move || email.get() on:input=move |ev| email.set(event_target_value(&ev))/>
                            <button class="btn primary" type="submit" disabled=move || send.pending().get()>"Email me a link"</button>
                        </form>
                    </Show>
                    <p class="hint">"Lost your device? Use a recovery code."</p>
                </div>
            </div>
        </div>
    }
}
