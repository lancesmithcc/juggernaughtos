//! Asset tiles (in chat and in the library) and the Assets panel.

use crate::api::{decide_asset, list_assets, list_jobs, AssetCard};
use leptos::prelude::*;

fn label(kind: &str) -> String {
    match kind {
        "logo" | "logo_vector" => "Logo".into(),
        "business_card" => "Business card".into(),
        "email_signature" => "Email signature".into(),
        "social_post" => "Social post".into(),
        "icon_set" => "Icons".into(),
        "web_page" => "Web page".into(),
        other => {
            let mut s = other.replace('_', " ");
            if let Some(f) = s.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            s
        }
    }
}

fn status_label(s: &str) -> &'static str {
    match s {
        "final" => "Approved",
        "review" => "Ready for review",
        "rejected" => "Off-brand",
        _ => "Draft",
    }
}

fn icon(mime: &str) -> &'static str {
    if mime.starts_with("video/") {
        "▶"
    } else if mime.starts_with("audio/") {
        "♪"
    } else if mime == "application/pdf" {
        "PDF"
    } else if mime.starts_with("text/html") {
        "</>"
    } else {
        "◧"
    }
}

/// One asset: preview, status, the brand gate's verdict, and the owner's decision.
#[component]
pub fn AssetTile(card: AssetCard) -> impl IntoView {
    let card = RwSignal::new(card);
    let working = RwSignal::new(false);
    let decide = move |approve: bool| {
        working.set(true);
        let id = card.get_untracked().id;
        leptos::task::spawn_local(async move {
            let reason = (approve && card.get_untracked().status == "rejected").then(|| "Owner chose to use it despite the brand check".to_string());
            if let Ok(c) = decide_asset(id, approve, reason).await {
                card.set(c);
            }
            working.set(false);
        });
    };
    view! {
        {move || {
            let c = card.get();
            let preview = c.preview_url.clone();
            let status = c.status.clone();
            let rejected = status == "rejected";
            let undecided = status == "review" || rejected;
            view! {
                <article class="asset" data-status=status.clone()>
                    <a class="asset-media" href=c.url.clone() target="_blank" rel="noopener">
                        {match preview {
                            Some(p) => view! { <img src=p alt=c.title.clone() loading="lazy"/> }.into_any(),
                            None => view! { <span class="asset-icon" aria-hidden="true">{icon(&c.mime)}</span> }.into_any(),
                        }}
                    </a>
                    <div class="asset-body">
                        <p class="asset-kind">{label(&c.kind)}</p>
                        <h3>{c.title.clone()}</h3>
                        <p class=format!("asset-status s-{status}")>{status_label(&status)}</p>
                        {(!c.summary.is_empty()).then(|| view! { <p class="asset-summary">{c.summary.clone()}</p> })}
                        {(!c.findings.is_empty()).then(|| view! {
                            <ul class="asset-findings">{c.findings.iter().take(4).map(|f| view! { <li>{f.clone()}</li> }).collect_view()}</ul>
                        })}
                        <div class="asset-actions">
                            <a class="btn ghost small" href=c.url.clone() target="_blank" rel="noopener" download="">"Download"</a>
                            {undecided.then(|| if rejected {
                                view! { <button class="btn ghost small warn" disabled=move || working.get() on:click=move |_| decide(true)
                                    title="This doesn't match your brand rules. Using it anyway is recorded.">"Use anyway"</button> }.into_any()
                            } else {
                                view! { <button class="btn primary small" disabled=move || working.get() on:click=move |_| decide(true)>"Approve"</button> }.into_any()
                            })}
                        </div>
                    </div>
                </article>
            }
        }}
    }
}

const KINDS: &[(&str, &str)] = &[
    ("", "All"),
    ("logo_vector", "Logos"),
    ("business_card", "Business cards"),
    ("email_signature", "Email signatures"),
    ("social_post", "Social"),
    ("video", "Video"),
    ("image", "Images"),
    ("pdf", "Documents"),
];

#[component]
pub fn AssetsPanel() -> impl IntoView {
    let kind = RwSignal::new(String::new());
    let assets = Resource::new(move || kind.get(), |k| list_assets(Some(k)));
    let jobs = Resource::new(|| (), |_| list_jobs());
    view! {
        <section class="panel">
            <header class="panel-head">
                <h1>"Assets"</h1>
                <p>"Everything we've made. Each piece is checked against your brand first; approve it to make it final."</p>
            </header>
            <Suspense fallback=|| ()>
                {move || Suspend::new(async move {
                    let running = jobs.await.unwrap_or_default();
                    (!running.is_empty()).then(|| view! {
                        <ul class="jobs">{running.into_iter().map(|j| view! {
                            <li class=format!("job j-{}", j.status)>
                                <span class="job-dot" aria-hidden="true"></span>
                                <strong>{j.tool.replace('_', " ")}</strong>
                                <span>{match j.status.as_str() { "queued" => "Waiting".to_string(), "running" => "Working…".to_string(), _ => format!("Failed: {}", j.error.unwrap_or_default()) }}</span>
                            </li>
                        }).collect_view()}</ul>
                    })
                })}
            </Suspense>
            <div class="filters" role="tablist" aria-label="Kinds">
                {KINDS.iter().map(|(k, l)| {
                    let k = k.to_string();
                    let (k2, k3) = (k.clone(), k.clone());
                    view! { <button class="chip" role="tab" class:on=move || kind.with(|c| *c == k2) aria-selected=move || kind.with(|c| *c == k).to_string()
                        on:click=move |_| kind.set(k3.clone())>{*l}</button> }
                }).collect_view()}
            </div>
            <Suspense fallback=|| view! { <div class="skeleton"></div> }>
                {move || Suspend::new(async move {
                    let items = assets.await.unwrap_or_default();
                    if items.is_empty() {
                        view! {
                            <div class="empty-state">
                                <div class="empty-art" aria-hidden="true"><i></i><i></i><i></i></div>
                                <h2>"Nothing here yet"</h2>
                                <p>"Ask for a logo, a business card or an email signature and it will land here."</p>
                            </div>
                        }.into_any()
                    } else {
                        view! { <div class="asset-grid">{items.into_iter().map(|c| view! { <AssetTile card=c/> }).collect_view()}</div> }.into_any()
                    }
                })}
            </Suspense>
        </section>
    }
}
