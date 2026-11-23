use crate::api::{get_overview, handoff_retry_codex, handoff_status, handoff_use_api, start_turn, ApprovalView, AssetCard, ChatImage, HandoffView, StreamEvent, ToolActivity};
use crate::pages::assets::{ApprovalCard, AssetTile};
use leptos::prelude::*;

#[derive(Clone)]
struct Msg {
    id: usize,
    user: bool,
    text: RwSignal<String>,
    images: Vec<ChatImage>,
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
        Self { id, user, images: vec![], text: RwSignal::new(text), tools: RwSignal::new(vec![]), live: RwSignal::new(None), streaming: RwSignal::new(!user), assets: RwSignal::new(vec![]), approvals: RwSignal::new(vec![]), handoffs: RwSignal::new(vec![]) }
    }
}

const ACCEPT_IMAGES: &str = "image/png,image/jpeg,image/webp,image/gif";

struct PendingImage {
    id: usize,
    name: String,
    preview: String,
    #[cfg(feature = "hydrate")]
    file: web_sys::File,
}

#[cfg(feature = "hydrate")]
impl Drop for PendingImage {
    fn drop(&mut self) {
        let _ = web_sys::Url::revoke_object_url(&self.preview);
    }
}

#[cfg(feature = "hydrate")]
fn add_images(
    queued: RwSignal<Vec<PendingImage>, LocalStorage>, next_image: StoredValue<usize>,
    error: RwSignal<Option<String>>, files: Vec<web_sys::File>,
) {
    error.set(None);
    queued.update(|images| {
        for file in files {
            if images.len() >= crate::api::MAX_CHAT_IMAGES {
                error.set(Some("Attach up to four images per message.".into()));
                break;
            }
            let mime = file.type_();
            if !mime.is_empty() && !ACCEPT_IMAGES.split(',').any(|supported| supported == mime) {
                error.set(Some("Choose a PNG, JPEG, WebP, or GIF image.".into()));
                continue;
            }
            if file.size() > crate::api::MAX_CHAT_IMAGE_BYTES as f64 {
                error.set(Some("Images can be up to 10 MB each.".into()));
                continue;
            }
            let Ok(preview) = web_sys::Url::create_object_url_with_blob(&file) else {
                error.set(Some("Couldn't preview the image.".into()));
                continue;
            };
            let id = next_image.get_value();
            next_image.set_value(id + 1);
            images.push(PendingImage { id, name: file.name(), preview, file });
        }
    });
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

    // Browser files aren't Send, so the browser keeps them in local storage. The server
    // must not: a local signal panics when the render drops it on another thread.
    #[cfg(feature = "hydrate")]
    let queued = RwSignal::new_local(Vec::<PendingImage>::new());
    #[cfg(not(feature = "hydrate"))]
    let queued = RwSignal::new(Vec::<PendingImage>::new());
    #[cfg(feature = "hydrate")]
    let next_image = StoredValue::new(0usize);
    let sending = RwSignal::new(false);
    let attachment_error = RwSignal::new(None::<String>);
    let dragging = RwSignal::new(false);
    let file_input = NodeRef::<leptos::html::Input>::new();

    let submit = move |text: String, manual: bool| {
        let text = text.trim().to_string();
        let has_images = manual && queued.with_untracked(|items| !items.is_empty());
        if (text.is_empty() && !has_images) || busy.get_untracked() {
            return;
        }
        busy.set(true);
        if manual {
            sending.set(true);
            attachment_error.set(None);
        }

        leptos::task::spawn_local(async move {
            let conv = conversation.get_untracked();
            let result = async {
                #[cfg(feature = "hydrate")]
                if has_images {
                    let files = queued.with_untracked(|items| items.iter().map(|image| image.file.clone()).collect());
                    return crate::client::send_chat_images(conv, text.clone(), files).await;
                }
                start_turn(conv, text.clone()).await.map_err(|e| e.to_string().replace("error running server function: ", ""))
            }.await;
            if manual {
                sending.set(false);
            }
            match result {
                Ok(ticket) => {
                    if manual {
                        draft.set(String::new());
                        queued.update(Vec::clear);
                    }
                    let id = next_id.get_value();
                    next_id.set_value(id + 2);
                    let mut message = Msg::new(id, true, text);
                    message.images = ticket.images;
                    let reply = Msg::new(id + 1, false, String::new());
                    messages.update(|m| m.extend([message, reply.clone()]));
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
                    attachment_error.set(Some(e));
                    busy.set(false);
                }
            }
        });
    };

    // The owner added an image from ChatGPT: let the agent carry on with it.
    // Run it under the page's owner: signals made inside an Effect are disposed when it re-runs.
    let page_owner = Owner::current();
    Effect::new(move |_| {
        if let Some(text) = added.get().filter(|_| !busy.get()) {
            added.set(None);
            match &page_owner {
                Some(o) => o.with(|| submit(text, false)),
                None => submit(text, false),
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
                    <img class="hero-logo" src="/juggernaught.svg" alt="" width="96" height="96"/>
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
                                        view! { <button class="chip" style=format!("--i:{i}") on:click=move |_| submit(label.clone(), true)>{s}</button> }
                                    }).collect_view()}
                                </div>
                            </Show>
                        }
                    })}
                </Suspense>
                <form class="composer" class:dragging=move || dragging.get()
                    on:submit=move |ev| { ev.prevent_default(); submit(draft.get_untracked(), true); }
                    on:dragover=move |ev| { ev.prevent_default(); if !sending.get_untracked() { dragging.set(true); } }
                    on:dragleave=move |_| dragging.set(false)
                    on:drop=move |ev| {
                        ev.prevent_default();
                        dragging.set(false);
                        #[cfg(feature = "hydrate")]
                        if !sending.get_untracked() {
                            let files = crate::client::chat_files(&ev.into());
                            if files.is_empty() {
                                attachment_error.set(Some("Drop an image file or paste the copied image.".into()));
                            } else {
                                add_images(queued, next_image, attachment_error, files);
                            }
                        }
                    }
                    on:paste=move |ev| {
                        #[cfg(feature = "hydrate")]
                        if !sending.get_untracked() {
                            let files = crate::client::chat_files(&leptos::wasm_bindgen::JsValue::from(ev.clone()));
                            if !files.is_empty() {
                                ev.prevent_default();
                                add_images(queued, next_image, attachment_error, files);
                            }
                        }
                        let _ = ev;
                    }>
                    <Show when=move || queued.with(|images| !images.is_empty())>
                        <ul class="composer-images" aria-label="Images to send">
                            <For each=move || queued.with(|images| images.iter().map(|i| (i.id, i.name.clone(), i.preview.clone())).collect::<Vec<_>>())
                                key=|i| i.0 let:item>
                                <li>
                                    <img src=item.2 alt=item.1.clone() title=item.1.clone()/>
                                    <button type="button" aria-label=format!("Remove {}", item.1)
                                        disabled=move || sending.get()
                                        on:click=move |_| {
                                            queued.update(|images| images.retain(|i| i.id != item.0));
                                            attachment_error.set(None);
                                        }>"×"</button>
                                </li>
                            </For>
                        </ul>
                    </Show>
                    <label for="composer-input" class="sr-only">"Message Juggernaught"</label>
                    <textarea
                        id="composer-input"
                        rows="1"
                        readonly=move || sending.get()
                        placeholder="Ask for a logo, a post, a page, a poster…"
                        prop:value=move || draft.get()
                        on:input=move |ev| draft.set(event_target_value(&ev))
                        on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                            if ev.key() == "Enter" && !ev.shift_key() && !ev.is_composing() {
                                ev.prevent_default();
                                submit(draft.get_untracked(), true);
                            }
                        }
                    ></textarea>
                    <div class="composer-bar">
                        <input node_ref=file_input type="file" hidden multiple accept=ACCEPT_IMAGES
                            on:change=move |ev| {
                                #[cfg(feature = "hydrate")]
                                {
                                    let files = crate::client::chat_files(&ev.into());
                                    if let Some(input) = file_input.get() { input.set_value(""); }
                                    if !files.is_empty() { add_images(queued, next_image, attachment_error, files); }
                                }
                                #[cfg(not(feature = "hydrate"))]
                                let _ = ev;
                            }/>
                        <button type="button" class="icon-btn" aria-label="Attach images" title="Attach images"
                            disabled=move || sending.get() on:click=move |_| {
                                if let Some(input) = file_input.get() { input.click(); }
                            }>"+"</button>
                        <span class="composer-status" role="status">{move || if sending.get() && queued.with(|i| !i.is_empty()) { "Uploading images…" } else { "" }}</span>
                        <button type="submit" class="send" aria-label="Send"
                            disabled=move || busy.get() || (draft.with(|d| d.trim().is_empty()) && queued.with(|i| i.is_empty()))>"↑"</button>
                    </div>
                </form>
                {move || attachment_error.get().map(|error| view! { <p class="composer-error" role="alert">{error}</p> })}
                <p class="fineprint">"Drafts are checked against your brand before they are final. Publishing always asks first."</p>
            </div>
        </section>
    }
}

#[component]
fn Message(msg: Msg) -> impl IntoView {
    if msg.user {
        let images = (!msg.images.is_empty()).then(|| view! {
            <div class="message-images">
                {msg.images.into_iter().map(|image| {
                    let url = format!("/files/chat-image/{}", image.id);
                    let preview = url.clone();
                    let title = image.name.clone();
                    view! { <a href=url target="_blank" rel="noopener" title=title>
                        <img src=preview alt=image.name loading="lazy"/>
                    </a> }
                }).collect_view()}
            </div>
        });
        return view! {
            <div class="msg user"><div class="bubble">{images}{move || msg.text.get()}</div></div>
        }.into_any();
    }
    let Msg { text, tools, live, streaming, assets, approvals, handoffs, .. } = msg;
    let waiting = move || streaming.get() && text.with(|t| t.is_empty()) && tools.with(|t| t.is_empty());
    view! {
        <div class="msg assistant" class:streaming=move || streaming.get()>
            <img class="avatar avatar-logo" src="/juggernaught.svg" alt="" width="32" height="32"/>
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
                s.set(Some(format!("The image from ChatGPT for “{name}” is ready. It's in Assets now (asset {aid}), and in this chat's folder as {}.", v.file.clone().unwrap_or_default())));
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
