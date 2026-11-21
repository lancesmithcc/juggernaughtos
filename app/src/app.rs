use crate::pages::{chat::ChatPage, login::LoginPage, panels::*, setup::SetupPage};
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
        <Link rel="icon" href="/favicon.svg" type_="image/svg+xml"/>
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
    view! {
        <div class="workspace" class:sidebar-open=open>
            <aside class="sidebar" aria-label="Main">
                <div class="sidebar-top">
                    <A href="/" attr:class="mark-link">
                        <span class="mark" aria-hidden="true">"J"</span>
                        <span class="wordmark">"Juggernaught"</span>
                    </A>
                    <button class="icon-btn only-mobile" aria-label="Close menu" on:click=move |_| set_open.set(false)>"×"</button>
                </div>
                <A href="/" attr:class="new-chat">
                    <span class="plus" aria-hidden="true">"+"</span>"New chat"
                </A>
                <nav class="nav">
                    <NavItem href="/assets" icon="◧" label="Assets"/>
                    <NavItem href="/website" icon="◎" label="Website"/>
                    <NavItem href="/brand" icon="◆" label="Brand"/>
                    <NavItem href="/security" icon="◈" label="Security"/>
                    <NavItem href="/settings" icon="⚙" label="Settings"/>
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
                <button class="icon-btn menu-btn only-mobile" aria-label="Open menu" on:click=move |_| set_open.set(true)>"☰"</button>
                <Outlet/>
            </main>
        </div>
    }
}

#[component]
fn NavItem(href: &'static str, icon: &'static str, label: &'static str) -> impl IntoView {
    view! {
        <A href=href attr:class="nav-item">
            <span class="nav-icon" aria-hidden="true">{icon}</span>
            <span>{label}</span>
        </A>
    }
}
