use crate::api::{get_overview, handoff_retry_codex, handoff_status, handoff_use_api, start_turn, ApprovalView, AssetCard, HandoffView, StreamEvent, ToolActivity};
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
    /// Images for the owner to make in ChatGPT and drop back here.
    handoffs: RwSignal<Vec<HandoffView>>,
}

/// Set when the owner adds a hand-off image, so the chat can tell the agent.
#[derive(Clone, Copy)]
struct HandoffAdded(RwSignal<Option<String>>);

impl Msg {
    fn new(id: usize, user: bool, text: String) -> Self {
        Self { id, user, text: RwSignal::new(text), tools: RwSignal::new(vec![]), live: RwSignal::new(None), streaming: RwSignal::new(!user), assets: RwSignal::new(vec![]), approvals: RwSignal::new(vec![]), handoffs: RwSignal::new(vec![]) }
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
    let added = RwSignal::new(None::<String>);
    provide_context(HandoffAdded(added));

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
                        StreamEvent::Handoffs { items } => reply.handoffs.update(|h| h.extend(items)),
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

    // The owner added an image from ChatGPT: let the agent carry on with it.
    // Run it under the page's owner: signals made inside an Effect are disposed when it re-runs.
    let page_owner = Owner::current();
    Effect::new(move |_| {
        if let Some(text) = added.get() {
            added.set(None);
            match &page_owner {
                Some(o) => o.with(|| submit(text)),
                None => submit(text),
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
    let Msg { text, tools, live, streaming, assets, approvals, handoffs, .. } = msg;
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
                <Show when=move || !handoffs.with(|h| h.is_empty())>
                    <div class="chat-handoffs">
                        <For each=move || handoffs.get() key=|h| h.id.clone() let:h>
                            <HandoffCard item=h assets=assets/>
                        </For>
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

/// One image request. With Codex signed in it's made automatically (the card waits
/// and fills itself in); otherwise, or if that fails, the owner makes it in ChatGPT
/// and drops it back here.
#[component]
fn HandoffCard(item: HandoffView, assets: RwSignal<Vec<AssetCard>>) -> impl IntoView {
    let added = use_context::<HandoffAdded>();
    let done = RwSignal::new(item.status == "done");
    let making = RwSignal::new(item.status == "making");
    let working = RwSignal::new(None::<&'static str>);
    let msg = RwSignal::new(item.note.clone().map(|n| format!("Couldn't make it automatically: {n}")));
    let over = RwSignal::new(false);
    let copied = RwSignal::new(false);
    let (id, title, prompt, url, api, codex) = (item.id.clone(), item.title.clone(), item.prompt.clone(), item.open_url.clone(), item.api_key_set, item.codex_ready);
    let finish = move |v: HandoffView| {
        working.set(None);
        making.set(false);
        if done.get_untracked() {
            return;
        }
        done.set(true);
        if let Some(a) = v.asset.clone() {
            let name = a.title.clone();
            let aid = a.id.clone();
            assets.update(|x| x.push(a));
            if let Some(HandoffAdded(s)) = added {
                s.set(Some(format!("The image from ChatGPT for “{name}” is ready. It's in Assets now (asset {aid}).")));
            }
        }
    };
    // While Codex works, check every few seconds until the image lands or it gives up.
    let watch = {
        let id = id.clone();
        move || {
            let id = id.clone();
            leptos::task::spawn_local(async move {
                let mut misses = 0;
                loop {
                    #[cfg(feature = "hydrate")]
                    crate::client::sleep_ms(3000).await;
                    match handoff_status(id.clone()).await {
                        Ok(v) if v.status == "making" => misses = 0,
                        Ok(v) if v.status == "done" => return finish(v),
                        Ok(v) => {
                            making.set(false);
                            msg.set(Some(format!("Couldn't make it automatically: {}", v.note.unwrap_or_else(|| "unknown error".into()))));
                            return;
                        }
                        Err(_) if misses < 20 => misses += 1,
                        Err(e) => {
                            making.set(false);
                            msg.set(Some(e.to_string().replace("error running server function: ", "")));
                            return;
                        }
                    }
                    if done.get_untracked() {
                        return;
                    }
                }
            });
        }
    };
    if making.get_untracked() {
        watch();
    }
    let take = {
        let id = id.clone();
        move |ev: leptos::wasm_bindgen::JsValue| {
            #[cfg(feature = "hydrate")]
            {
                match crate::client::image_in(&ev) {
                    Ok(Some(file)) => {
                        let id = id.clone();
                        working.set(Some("Adding your image…"));
                        msg.set(None);
                        leptos::task::spawn_local(async move {
                            match crate::client::upload_handoff(&id, &file).await {
                                Ok(v) => finish(v),
                                Err(e) => {
                                    working.set(None);
                                    msg.set(Some(e));
                                }
                            }
                        });
                    }
                    Ok(None) => msg.set(Some("That wasn't an image. Drop or paste the picture itself.".into())),
                    Err(_) => msg.set(Some("Your browser only passed the image's web address. In ChatGPT, right-click the image → Copy Image, then click here and paste (⌘V). Or download it and drop the file.".into())),
                }
            }
            #[cfg(not(feature = "hydrate"))]
            let _ = (ev, &id);
        }
    };
    let (t1, t2, t3) = (take.clone(), take.clone(), take);
    let prompt_copy = prompt.clone();
    let open_copy = prompt.clone();
    let use_api = {
        let id = id.clone();
        move |_| {
            let id = id.clone();
            working.set(Some("Making it with your API key…"));
            msg.set(None);
            leptos::task::spawn_local(async move {
                match handoff_use_api(id).await {
                    Ok(v) => finish(v),
                    Err(e) => {
                        working.set(None);
                        msg.set(Some(e.to_string().replace("error running server function: ", "")));
                    }
                }
            });
        }
    };
    let retry = {
        let id = id.clone();
        let watch = watch.clone();
        move |_| {
            let (id, watch) = (id.clone(), watch.clone());
            msg.set(None);
            leptos::task::spawn_local(async move {
                match handoff_retry_codex(id).await {
                    Ok(_) => {
                        making.set(true);
                        watch();
                    }
                    Err(e) => msg.set(Some(e.to_string().replace("error running server function: ", ""))),
                }
            });
        }
    };
    let input_id = format!("handoff-file-{id}");
    view! {
        <div class="handoff" class:done=move || done.get()>
            {move || {
                if done.get() {
                    return view! { <p class="handoff-done">"✓ Added to Assets"</p> }.into_any();
                }
                if making.get() {
                    return view! {
                        <div class="handoff-head" role="status">
                            <span class="handoff-badge" aria-hidden="true">"GPT"</span>
                            <div>
                                <strong><span class="spinner"></span>{format!("Making “{title}” in your ChatGPT…")}</strong>
                                <p>"Usually under a minute. It shows up here and in Assets by itself; you can keep chatting or close this tab."</p>
                            </div>
                        </div>
                    }.into_any();
                }
                let (t1, t2, t3) = (t1.clone(), t2.clone(), t3.clone());
                let (open_copy, prompt_copy) = (open_copy.clone(), prompt_copy.clone());
                view! {
                    <div class="handoff-head">
                        <span class="handoff-badge" aria-hidden="true">"GPT"</span>
                        <div>
                            <strong>{format!("Make “{title}” in ChatGPT")}</strong>
                            <p>"1 · Open ChatGPT (the prompt is filled in and copied).  2 · Send it.  3 · Bring the image back here."</p>
                        </div>
                    </div>
                    <div class="row-actions">
                        {codex.then(|| view! { <button class="btn primary" type="button" on:click=retry.clone()>"Try again automatically"</button> })}
                        <a class=if codex { "btn ghost" } else { "btn primary" } href=url.clone() target="_blank" rel="noopener noreferrer" on:click=move |_| crate::client::copy(&open_copy)>"Open in ChatGPT ↗"</a>
                        <button class="btn ghost" type="button" on:click=move |_| { crate::client::copy(&prompt_copy); copied.set(true); }>{move || if copied.get() { "Copied" } else { "Copy prompt" }}</button>
                        {api.then(|| view! { <button class="btn ghost" type="button" disabled=move || working.get().is_some() on:click=use_api.clone()>"Use my API key"</button> })}
                    </div>
                    <details class="handoff-prompt"><summary>"See the prompt"</summary><p>{prompt.clone()}</p></details>
                    <label class="dropzone" class:over=move || over.get() for=input_id.clone() tabindex="0"
                        on:dragover=move |ev: leptos::ev::DragEvent| { ev.prevent_default(); over.set(true); }
                        on:dragleave=move |_| over.set(false)
                        on:drop=move |ev: leptos::ev::DragEvent| { ev.prevent_default(); over.set(false); t1(leptos::wasm_bindgen::JsValue::from(ev)); }
                        on:paste=move |ev: leptos::ev::ClipboardEvent| { ev.prevent_default(); t2(leptos::wasm_bindgen::JsValue::from(ev)); }>
                        {move || match working.get() {
                            Some(w) => view! { <span><span class="spinner"></span>{w}</span> }.into_any(),
                            None => view! { <span><strong>"Drop the image here"</strong>" or click to choose it · paste with ⌘V"</span> }.into_any(),
                        }}
                    </label>
                    <input id=input_id.clone() class="sr-only" type="file" accept="image/png,image/jpeg,image/webp,image/gif" on:change=move |ev: leptos::ev::Event| t3(leptos::wasm_bindgen::JsValue::from(ev))/>
                }.into_any()
            }}
            {move || msg.get().filter(|_| !done.get()).map(|m| view! { <p class="hint err" role="alert">{m}</p> })}
        </div>
    }
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
