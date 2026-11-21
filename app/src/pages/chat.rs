use crate::api::{get_overview, send_message, Reply, ToolActivity};
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

#[derive(Clone, Debug, PartialEq)]
struct Msg {
    id: usize,
    user: bool,
    text: String,
    tools: Vec<ToolActivity>,
}

#[component]
pub fn ChatPage() -> impl IntoView {
    let overview = Resource::new(|| (), |_| get_overview());
    let messages = RwSignal::new(Vec::<Msg>::new());
    let draft = RwSignal::new(String::new());
    let conversation = RwSignal::new(None::<String>);
    let next_id = StoredValue::new(0usize);

    let send = Action::new(move |text: &String| {
        let text = text.clone();
        let conv = conversation.get_untracked();
        async move { send_message(conv, text).await }
    });

    let submit = move |text: String| {
        let text = text.trim().to_string();
        if text.is_empty() || send.pending().get_untracked() {
            return;
        }
        let id = next_id.get_value();
        next_id.set_value(id + 2);
        messages.update(|m| m.push(Msg { id, user: true, text: text.clone(), tools: vec![] }));
        draft.set(String::new());
        send.dispatch(text);
    };

    // Append the assistant reply when the action resolves.
    Effect::new(move |_| {
        if let Some(result) = send.value().get() {
            let id = next_id.get_value();
            next_id.set_value(id + 1);
            let msg = match result {
                Ok(Reply { conversation_id, text, tools, .. }) => {
                    conversation.set(Some(conversation_id));
                    Msg { id, user: false, text, tools }
                }
                Err(e) => Msg { id, user: false, text: format!("Something went wrong: {e}"), tools: vec![] },
            };
            messages.update(|m| m.push(msg));
        }
    });

    // First run: send the owner to the setup wizard.
    let navigate = use_navigate();
    Effect::new(move |_| {
        if let Some(Ok(o)) = overview.get() {
            if !o.setup_complete {
                navigate("/setup", Default::default());
            }
        }
    });

    // Server renders a neutral greeting; the browser swaps in time-of-day (no hydration mismatch).
    let greet = RwSignal::new("Welcome back.");
    Effect::new(move |_| greet.set(greeting()));

    let empty = move || messages.with(|m| m.is_empty());

    view! {
        <section class="chat" class:is-empty=empty>
            <Show when=empty>
                <div class="hero-greeting">
                    <div class="orb" aria-hidden="true"></div>
                    <h1 class="greeting">{move || greet.get()}</h1>
                    <p class="sub">
                        <Suspense fallback=|| "What are we making today?">
                            {move || Suspend::new(async move {
                                match overview.await {
                                    Ok(o) if o.business != "Juggernaught" => format!("What are we making for {} today?", o.business),
                                    _ => "What are we making today?".to_string(),
                                }
                            })}
                        </Suspense>
                    </p>
                </div>
            </Show>

            <div class="thread" role="log" aria-live="polite">
                <For each=move || messages.get() key=|m| m.id let:m>
                    <Message msg=m/>
                </For>
                <Show when=move || send.pending().get()>
                    <div class="msg assistant thinking" aria-label="Juggernaught is working">
                        <span class="avatar" aria-hidden="true">"J"</span>
                        <span class="dots"><i></i><i></i><i></i></span>
                    </div>
                </Show>
            </div>

            <div class="dock">
                <Suspense fallback=|| ()>
                    {move || Suspend::new(async move {
                        let suggestions = overview.await.map(|o| o.suggestions).unwrap_or_default();
                        view! {
                            <Show when=empty>
                                <div class="suggestions">
                                    {suggestions.clone().into_iter().enumerate().map(|(i, s)| {
                                        let label = s.clone();
                                        view! {
                                            <button class="chip" style=format!("--i:{i}") on:click=move |_| submit(label.clone())>{s}</button>
                                        }
                                    }).collect_view()}
                                </div>
                            </Show>
                        }
                    })}
                </Suspense>
                <form class="composer" on:submit=move |ev| { ev.prevent_default(); submit(draft.get_untracked()); }>
                    <label for="composer-input" class="sr-only">"Message Juggernaught"</label>
                    <textarea
                        id="composer-input"
                        rows="1"
                        placeholder="Ask for a logo, a post, a page, a poster…"
                        prop:value=move || draft.get()
                        on:input=move |ev| draft.set(event_target_value(&ev))
                        on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                            if ev.key() == "Enter" && !ev.shift_key() {
                                ev.prevent_default();
                                submit(draft.get_untracked());
                            }
                        }
                    ></textarea>
                    <div class="composer-bar">
                        <button type="button" class="icon-btn" aria-label="Attach a file" title="Attach">"+"</button>
                        <button type="submit" class="send" aria-label="Send" disabled=move || draft.with(|d| d.trim().is_empty()) || send.pending().get()>"↑"</button>
                    </div>
                </form>
                <p class="fineprint">"Drafts are checked against your brand before they are final. Publishing always asks first."</p>
            </div>
        </section>
    }
}

#[component]
fn Message(msg: Msg) -> impl IntoView {
    if msg.user {
        return view! { <div class="msg user"><div class="bubble">{msg.text}</div></div> }.into_any();
    }
    // Words fade in one after another: alive, but done in < 1.2s whatever the length.
    let words: Vec<String> = msg.text.split_inclusive(' ').map(str::to_string).collect();
    let step = (1100.0 / words.len().max(1) as f64).min(28.0);
    view! {
        <div class="msg assistant">
            <span class="avatar" aria-hidden="true">"J"</span>
            <div class="body">
                {(!msg.tools.is_empty()).then(|| view! {
                    <ul class="tools">
                        {msg.tools.into_iter().map(|t| view! {
                            <li class="tool" class:err=t.is_error><span class="tool-name">{t.name}</span><span class="tool-sum">{t.summary}</span></li>
                        }).collect_view()}
                    </ul>
                })}
                <p class="text">
                    {words.into_iter().enumerate().map(|(i, w)| view! {
                        <span class="w" style=format!("animation-delay:{}ms", (i as f64 * step) as u32)>{w}</span>
                    }).collect_view()}
                </p>
            </div>
        </div>
    }
    .into_any()
}

fn greeting() -> &'static str {
    #[cfg(feature = "hydrate")]
    {
        let h = js_sys_hour();
        return match h {
            5..=11 => "Good morning.",
            12..=17 => "Good afternoon.",
            _ => "Good evening.",
        };
    }
    #[allow(unreachable_code)]
    "Welcome back."
}

#[cfg(feature = "hydrate")]
fn js_sys_hour() -> u32 {
    // Avoid a js-sys dependency: Date is reachable through wasm-bindgen.
    #[wasm_bindgen::prelude::wasm_bindgen]
    extern "C" {
        type Date;
        #[wasm_bindgen::prelude::wasm_bindgen(constructor)]
        fn new() -> Date;
        #[wasm_bindgen::prelude::wasm_bindgen(method, js_name = getHours)]
        fn get_hours(this: &Date) -> u32;
    }
    Date::new().get_hours()
}
