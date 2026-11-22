//! Juggernaught admin app: chat-first, Leptos SSR + hydration.
#![recursion_limit = "512"]

pub mod api;
pub mod app;
pub mod client;
pub mod markdown;
pub mod pages;
#[cfg(feature = "ssr")]
pub mod assets;
#[cfg(feature = "ssr")]
pub mod server;
#[cfg(feature = "ssr")]
pub mod site;
#[cfg(feature = "ssr")]
pub mod web;

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(app::App);
}
