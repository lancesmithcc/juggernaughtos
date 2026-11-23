//! Asset tiles (in chat and in the library) and the Assets panel.

use crate::api::{decide_approval, decide_asset, finish_logo, list_assets, list_jobs, ApprovalView, AssetCard};
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

/// The file format as a spec sheet names it: SVG, PNG, PDF, EPS…
fn format_of(mime: &str) -> String {
    let sub = mime.split(';').next().unwrap_or(mime).rsplit('/').next().unwrap_or(mime);
    match sub {
        "svg+xml" => "SVG".into(),
        "jpeg" => "JPEG".into(),
        "postscript" => "EPS".into(),
        "html" => "HTML".into(),
        "mpeg" => "MP3".into(),
        "plain" => "Text".into(),
        other => other.to_ascii_uppercase(),
    }
}

fn icon(mime: &str) -> String {
    format_of(mime)
}

/// `1795442384` → `23 Nov 2026` (UTC), without a date library in the browser.
fn day(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{d} {} {y}", MONTHS[(m - 1) as usize])
}

/// One asset: preview, status, the brand gate's verdict, and the owner's decision.
#[component]
pub fn AssetTile(card: AssetCard) -> impl IntoView {
    let card = RwSignal::new(card);
    let working = RwSignal::new(false);
    // Approving a logo image makes the whole file set (vector, print, screen).
    let pack = RwSignal::new(None::<Result<Vec<AssetCard>, String>>);
    let making = RwSignal::new(false);
    let decide = move |approve: bool| {
        working.set(true);
        let id = card.get_untracked().id;
        leptos::task::spawn_local(async move {
            let reason = (approve && card.get_untracked().status == "rejected").then(|| "Owner chose to use it despite the brand check".to_string());
            if let Ok(c) = decide_asset(id.clone(), approve, reason).await {
                let raster_logo = approve && c.kind == "logo" && c.mime.starts_with("image/") && !c.mime.contains("svg");
                card.set(c);
                if raster_logo {
                    making.set(true);
                    let r = finish_logo(id).await.map_err(|e| e.to_string().replace("error running server function: ", ""));
                    making.set(false);
                    pack.set(Some(r));
                }
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
                        <h3>{c.title.clone()}</h3>
                        <dl class="spec">
                            <dt>"Kind"</dt><dd>{label(&c.kind)}</dd>
                            <dt>"Format"</dt><dd>{format_of(&c.mime)}</dd>
                            <dt>"Made"</dt><dd>{day(c.created_at)}</dd>
                            <dt>"Status"</dt><dd class=format!("asset-status s-{status}")>{status_label(&status)}</dd>
                            {(!c.summary.is_empty()).then(|| view! { <dt>"Brand check"</dt><dd>{c.summary.clone()}</dd> })}
                        </dl>
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
                    {move || making.get().then(|| view! {
                        <p class="logo-pack-status" role="status"><span class="spinner"></span>"Vectorizing and making your logo files…"</p>
                    })}
                    {move || pack.get().map(|r| match r {
                        Ok(files) => view! {
                            <div class="logo-pack">
                                <p class="ok-line">"Your logo files are ready, and it's now your brand logo."</p>
                                <ul>
                                    {files.into_iter().map(|f| {
                                        let label = f.title.rsplit(" · ").next().unwrap_or(&f.title).to_string();
                                        view! { <li><a class="btn ghost small" href=f.url.clone() download="">{label}</a></li> }
                                    }).collect_view()}
                                </ul>
                            </div>
                        }.into_any(),
                        Err(e) => view! { <p class="hint err" role="alert">{format!("Couldn't make the logo files: {e}")}</p> }.into_any(),
                    })}
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
            <crate::pages::panels::SectionPlate no="2" title="Assets" lede="Everything we've made. Each piece is checked against your brand first; approve it to make it final."/>
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

/// An action that waits for the owner (publish, roll back, …).
#[component]
pub fn ApprovalCard(item: ApprovalView) -> impl IntoView {
    let item = RwSignal::new(item);
    let working = RwSignal::new(false);
    let decide = move |approve: bool| {
        working.set(true);
        let id = item.get_untracked().id;
        leptos::task::spawn_local(async move {
            match decide_approval(id, approve).await {
                Ok(v) => item.set(v),
                Err(e) => item.update(|i| i.result = Some(e.to_string().replace("error running server function: ", ""))),
            }
            working.set(false);
        });
    };
    view! {
        {move || {
            let i = item.get();
            let pending = i.status == "pending";
            view! {
                <div class="approval" data-status=i.status.clone()>
                    <div class="approval-text">
                        <p class="approval-summary">{i.summary.clone()}</p>
                        <p class="note" data-tone=if pending { "needs" } else { "done" }>{if pending { "Needs your OK before it goes out." } else if i.status == "approved" { "Approved." } else { "Declined." }}</p>
                        {i.result.clone().map(|r| view! { <p class="approval-result">{r}</p> })}
                    </div>
                    {pending.then(|| view! {
                        <div class="approval-actions">
                            <button class="btn ghost small" disabled=move || working.get() on:click=move |_| decide(false)>"Not now"</button>
                            <button class="btn primary small" disabled=move || working.get() on:click=move |_| decide(true)>
                                {move || if working.get() { "Working…" } else { "Approve" }}
                            </button>
                        </div>
                    })}
                </div>
            }
        }}
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn spec_values() {
        assert_eq!(super::day(1_795_442_384), "23 Nov 2026");
        assert_eq!(super::day(0), "1 Jan 1970");
        assert_eq!(super::format_of("image/svg+xml"), "SVG");
        assert_eq!(super::format_of("application/postscript"), "EPS");
        assert_eq!(super::format_of("text/html; charset=utf-8"), "HTML");
    }
}
