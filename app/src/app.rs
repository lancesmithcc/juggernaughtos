use crate::pages::{assets::AssetsPanel, chat::ChatPage, login::LoginPage, panels::*, setup::SetupPage};
use leptos::prelude::*;
use leptos_meta::{provide_meta_context, Link, Meta, MetaTags, Stylesheet, Title};
use leptos_router::components::{Outlet, ParentRoute, Route, Router, Routes, A};
use leptos_router::path;

pub fn shell(options: LeptosOptions) -> impl IntoView {
    // Per-request CSP nonce minted by the security middleware; Leptos stamps it
    // on its hydration scripts so the admin CSP needs no 'unsafe-inline'.
    #[cfg(feature = "ssr")]
    if let Some(parts) = use_context::<http::request::Parts>() {
        if let Some(n) = parts.extensions.get::<crate::server::CspNonce>() {
            provide_context(leptos::nonce::Nonce::from_value(n.0.clone()));
        }
    }
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <meta name="color-scheme" content="light dark"/>
                <AutoReload options=options.clone()/>
                <HydrationScripts options/>
                <MetaTags/>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    view! {
        <Stylesheet id="leptos" href="/pkg/juggernaught.css"/>
        <Stylesheet id="brand" href="/brand.css"/>
        <Link rel="preload" href="/fonts/PlusJakartaSans-Variable.ttf" as_="font" type_="font/ttf" crossorigin="anonymous"/>
        <Link rel="icon" href="/juggernaught.svg" type_="image/svg+xml"/>
        <Meta name="robots" content="noindex, nofollow"/>
        <Title text="Juggernaught"/>
        <Router>
            <Routes fallback=|| view! { <div class="empty">"Not found."</div> }>
                <Route path=path!("/setup") view=SetupPage/>
                <Route path=path!("/login") view=LoginPage/>
                <ParentRoute path=path!("") view=Workspace>
                    <Route path=path!("") view=ChatPage/>
                    <Route path=path!("assets") view=AssetsPanel/>
                    <Route path=path!("website") view=WebsitePanel/>
                    <Route path=path!("brand") view=BrandPanel/>
                    <Route path=path!("security") view=SecurityPanel/>
                    <Route path=path!("settings") view=SettingsPanel/>
                    <Route path=path!("go-live") view=crate::pages::golive::GoLivePage/>
                </ParentRoute>
            </Routes>
        </Router>
    }
}

/// The two-column workspace: sidebar + whatever route is active.
#[component]
fn Workspace() -> impl IntoView {
    let (open, set_open) = signal(false);
    let overview = Resource::new(|| (), |_| crate::api::get_overview());
    // Panels (e.g. Settings → Intelligence) refetch this to update the sidebar pill.
    provide_context(overview);
    let navigate = leptos_router::hooks::use_navigate();
    Effect::new(move |prev: Option<()>| {
        if prev.is_none() {
            section_shortcuts(navigate.clone());
        }
    });
    view! {
        <div class="workspace" class:sidebar-open=open>
            <aside class="sidebar" aria-label="Main">
                <div class="sidebar-top">
                    <A href="/" attr:class="mark-link">
                        <Suspense fallback=|| view! { <img class="mark-logo" src="/juggernaught.svg" alt="" width="30" height="30"/><span class="wordmark">"Juggernaught"</span> }>
                            {move || Suspend::new(async move {
                                match overview.await {
                                    Ok(o) if o.has_logo => view! {
                                        <img class="mark-img" src="/brand-mark.svg" alt="" width="30" height="30"/>
                                        <span class="wordmark">{o.business}</span>
                                    }.into_any(),
                                    _ => view! { <img class="mark-logo" src="/juggernaught.svg" alt="" width="30" height="30"/><span class="wordmark">"Juggernaught"</span> }.into_any(),
                                }
                            })}
                        </Suspense>
                    </A>
                    <button class="icon-btn only-mobile" aria-label="Close menu" on:click=move |_| set_open.set(false)><crate::icons::Icon name="close"/></button>
                </div>
                <A href="/" attr:class="new-chat">
                    <span class="plus"><crate::icons::Icon name="plus" size=14/></span>"New chat"
                </A>
                <nav class="nav">
                    <NavItem href="/" no="1" label="Chat"/>
                    <NavItem href="/assets" no="2" label="Assets"/>
                    <NavItem href="/website" no="3" label="Website"/>
                    <NavItem href="/brand" no="4" label="Brand"/>
                    <NavItem href="/security" no="5" label="Security"/>
                    <NavItem href="/settings" no="6" label="Settings"/>
                    <A href="/go-live" attr:class="nav-item" attr:aria-keyshortcuts="7">
                        <span class="nav-no" aria-hidden="true">"7"</span>
                        <span>"Go live"</span>
                        <Suspense fallback=|| ()>
                            {move || Suspend::new(async move {
                                overview.await.ok().filter(|o| o.go_live_needs_you).map(|_| view! { <span class="nav-note">"needs you"</span> })
                            })}
                        </Suspense>
                    </A>
                </nav>
                <div class="sidebar-foot">
                    <Suspense fallback=|| view! { <span class="pill">"…"</span> }>
                        {move || Suspend::new(async move {
                            match overview.await {
                                Ok(o) => view! {
                                    <A href="/settings" attr:class="pill" attr:data-ready=o.provider_ready.to_string()>
                                        <span class="dot" aria-hidden="true"></span>{o.provider}
                                    </A>
                                }.into_any(),
                                Err(_) => view! { <span class="pill">"Offline"</span> }.into_any(),
                            }
                        })}
                    </Suspense>
                </div>
            </aside>
            <div class="scrim only-mobile" on:click=move |_| set_open.set(false)></div>
            <main class="main">
                <button class="icon-btn menu-btn only-mobile" aria-label="Open menu" on:click=move |_| set_open.set(true)><crate::icons::Icon name="menu" size=20/></button>
                <Outlet/>
            </main>
            <crate::pages::account::PasskeyNudge/>
        </div>
    }
}

/// One manual section. Its number is also its keyboard shortcut (press 1–7).
#[component]
fn NavItem(href: &'static str, no: &'static str, label: &'static str) -> impl IntoView {
    view! {
        <A href=href attr:class="nav-item" attr:aria-keyshortcuts=no exact=true>
            <span class="nav-no" aria-hidden="true">{no}</span>
            <span>{label}</span>
        </A>
    }
}

/// Section numbers double as shortcuts, like a manual's tabs: 1 Chat … 7 Go live.
/// Ignored while typing, and with any modifier held.
fn section_shortcuts(navigate: impl Fn(&str, leptos_router::NavigateOptions) + Clone + 'static) {
    #[cfg(not(feature = "hydrate"))]
    let _ = navigate;
    #[cfg(feature = "hydrate")]
    {
        use leptos::wasm_bindgen::{closure::Closure, JsCast};
        let Some(win) = web_sys::window() else { return };
        let handler = Closure::<dyn Fn(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
            if ev.ctrl_key() || ev.meta_key() || ev.alt_key() || ev.default_prevented() {
                return;
            }
            let typing = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).is_some_and(|el| {
                matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT") || el.closest("[contenteditable]").ok().flatten().is_some()
            });
            if typing {
                return;
            }
            let to = match ev.key().as_str() {
                "1" => "/",
                "2" => "/assets",
                "3" => "/website",
                "4" => "/brand",
                "5" => "/security",
                "6" => "/settings",
                "7" => "/go-live",
                _ => return,
            };
            ev.prevent_default();
            navigate(to, Default::default());
        });
        let _ = win.add_event_listener_with_callback("keydown", handler.as_ref().unchecked_ref());
        handler.forget();
    }
}
