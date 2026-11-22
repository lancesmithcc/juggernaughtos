use crate::api::{get_overview, image_status, start_turn, ApprovalView, AssetCard, StreamEvent, ToolActivity};
use crate::pages::assets::{ApprovalCard, AssetTile};
use leptos::prelude::*;

#[derive(Clone)]
struct Msg {
    id: usize,
    user: bool,
    text: RwSignal<String>,
    tools: RwSignal<Vec<ToolActivity>>,
    live: RwSignal<Option<String>>,
    streaming: RwSignal<bool>,
    assets: RwSignal<Vec<AssetCard>>,
    approvals: RwSignal<Vec<ApprovalView>>,
    /// An image needed ChatGPT and it isn't connected: show the Connect card.
    needs_connect: RwSignal<bool>,
}

impl Msg {
    fn new(id: usize, user: bool, text: String) -> Self {
        Self { id, user, text: RwSignal::new(text), tools: RwSignal::new(vec![]), live: RwSignal::new(None), streaming: RwSignal::new(!user), assets: RwSignal::new(vec![]), approvals: RwSignal::new(vec![]), needs_connect: RwSignal::new(false) }
    }
}

#[component]
pub fn ChatPage() -> impl IntoView {
    let overview = Resource::new(|| (), |_| get_overview());
    let messages = RwSignal::new(Vec::<Msg>::new());
    let draft = RwSignal::new(String::new());
    let conversation = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let next_id = StoredValue::new(0usize);
    let chatgpt = use_context::<crate::pages::panels::ChatGptPrompt>();
    // Set when an image failed for want of ChatGPT; once the owner connects, carry on.
    let retry_image = StoredValue::new(false);

    let submit = move |text: String| {
        let text = text.trim().to_string();
        if text.is_empty() || busy.get_untracked() {
            return;
        }
        busy.set(true);
        let id = next_id.get_value();
        next_id.set_value(id + 2);
        messages.update(|m| m.push(Msg::new(id, true, text.clone())));
        let reply = Msg::new(id + 1, false, String::new());
        messages.update(|m| m.push(reply.clone()));
        draft.set(String::new());

        leptos::task::spawn_local(async move {
            match start_turn(conversation.get_untracked(), text).await {
                Ok(ticket) => {
                    conversation.set(Some(ticket.conversation_id));
                    crate::client::open_stream(&ticket.turn_id, move |ev| match ev {
                        StreamEvent::TextDelta { text } => reply.text.update(|t| t.push_str(&text)),
                        StreamEvent::ToolStarted { name } => reply.live.set(Some(format!("Using {}…", name.replace('_', " ")))),
                        StreamEvent::ToolFinished { name, summary, is_error } => {
                            reply.live.set(None);
                            // An image needs the owner's ChatGPT: if it isn't connected, ask right away.
                            if name.ends_with("generate_image") {
                                let failed = is_error || summary.contains("No image this time");
                                leptos::task::spawn_local(async move {
                                    let Ok(st) = image_status().await else { return };
                                    if !(st.installed && st.session == "signed_in") {
                                        reply.needs_connect.set(true);
                                        if failed {
                                            retry_image.set_value(true);
                                            if let Some(p) = chatgpt {
                                                p.open.set(true);
                                            }
                                        }
                                    }
                                });
                            }
                            reply.tools.update(|t| t.push(ToolActivity { name, summary: summary.chars().take(240).collect(), is_error }));
                        }
                        StreamEvent::ApprovalRequired { tool, summary } => {
                            reply.tools.update(|t| t.push(ToolActivity { name: format!("Needs your OK · {tool}"), summary, is_error: false }))
                        }
                        StreamEvent::Error { message } => reply.text.update(|t| t.push_str(&format!("\n\n_{message}_"))),
                        StreamEvent::Done => {
                            reply.streaming.set(false);
                            reply.live.set(None);
                            busy.set(false);
                        }
                        StreamEvent::Assets { items } => reply.assets.update(|a| a.extend(items)),
                        StreamEvent::Approvals { items } => reply.approvals.update(|a| a.extend(items)),
                        StreamEvent::Usage {} => {}
                    });
                }
                Err(e) => {
                    reply.text.set(format!("Something went wrong: {e}"));
                    reply.streaming.set(false);
                    busy.set(false);
                }
            }
        });
    };

    if let Some(p) = chatgpt {
        Effect::new(move |prev: Option<u32>| {
            let n = p.connected.get();
            if prev.is_some_and(|before| before != n) && retry_image.get_value() {
                retry_image.set_value(false);
                messages.update(|m| {
                    for msg in m.iter() {
                        msg.needs_connect.set(false);
                    }
                });
                submit("ChatGPT is connected now. Please go ahead and make the image I asked for.".into());
            }
            n
        });
    }

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

            <div class="thread" role="log" aria-live="polite" aria-busy=move || busy.get().to_string()>
                <For each=move || messages.get() key=|m| m.id let:m>
                    <Message msg=m/>
                </For>
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
                                        view! { <button class="chip" style=format!("--i:{i}") on:click=move |_| submit(label.clone())>{s}</button> }
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
                            if ev.key() == "Enter" && !ev.shift_key() && !ev.is_composing() {
                                ev.prevent_default();
                                submit(draft.get_untracked());
                            }
                        }
                    ></textarea>
                    <div class="composer-bar">
                        <button type="button" class="icon-btn" aria-label="Attach a file" title="Attach">"+"</button>
                        <button type="submit" class="send" aria-label="Send" disabled=move || draft.with(|d| d.trim().is_empty()) || busy.get()>"↑"</button>
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
        return view! { <div class="msg user"><div class="bubble">{move || msg.text.get()}</div></div> }.into_any();
    }
    let Msg { text, tools, live, streaming, assets, approvals, needs_connect, .. } = msg;
    let chatgpt = use_context::<crate::pages::panels::ChatGptPrompt>();
    let waiting = move || streaming.get() && text.with(|t| t.is_empty()) && tools.with(|t| t.is_empty());
    view! {
        <div class="msg assistant" class:streaming=move || streaming.get()>
            <span class="avatar" aria-hidden="true">"J"</span>
            <div class="body">
                <Show when=move || !tools.with(|t| t.is_empty())>
                    <ul class="tools">
                        <For each=move || { tools.get().into_iter().enumerate().collect::<Vec<_>>() } key=|(i, _)| *i let:item>
                            <li class="tool" class:err=item.1.is_error><span class="tool-name">{item.1.name.clone()}</span><span class="tool-sum">{item.1.summary.clone()}</span></li>
                        </For>
                    </ul>
                </Show>
                {move || live.get().map(|l| view! { <p class="live">{l}</p> })}
                <Show when=waiting fallback=move || view! { <div class="text md" inner_html=move || crate::markdown::render(&text.get())></div> }>
                    <span class="dots" aria-label="Working"><i></i><i></i><i></i></span>
                </Show>
                <Show when=move || needs_connect.get()>
                    <div class="connect-card">
                        <div>
                            <strong>"Connect your ChatGPT for images"</strong>
                            <p>"New images are made with GPT Image through your own ChatGPT account. Connect it once (about a minute) and I'll carry on with your image."</p>
                        </div>
                        <button class="btn primary" type="button" on:click=move |_| { if let Some(p) = chatgpt { p.open.set(true); } }>"Connect ChatGPT"</button>
                    </div>
                </Show>
                <Show when=move || !approvals.with(|a| a.is_empty())>
                    <div class="chat-approvals">
                        <For each=move || approvals.get() key=|a| a.id.clone() let:a>
                            <ApprovalCard item=a/>
                        </For>
                    </div>
                </Show>
                <Show when=move || !assets.with(|a| a.is_empty())>
                    <div class="chat-assets">
                        <For each=move || assets.get() key=|a| a.id.clone() let:card>
                            <AssetTile card=card/>
                        </For>
                    </div>
                </Show>
            </div>
        </div>
    }
    .into_any()
}

fn greeting() -> &'static str {
    #[cfg(feature = "hydrate")]
    {
        let h = js_sys::Date::new_0().get_hours();
        return match h {
            5..=11 => "Good morning.",
            12..=17 => "Good afternoon.",
            _ => "Good evening.",
        };
    }
    #[allow(unreachable_code)]
    "Welcome back."
}
