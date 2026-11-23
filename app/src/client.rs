//! Browser-only glue: passkey ceremonies (js/passkey.js) and the chat event stream.
//! On the server these are no-ops so components compile for SSR too.

use crate::api::StreamEvent;

#[cfg(feature = "hydrate")]
mod js {
    use wasm_bindgen::prelude::*;
    #[wasm_bindgen(module = "/js/passkey.js")]
    extern "C" {
        #[wasm_bindgen(js_name = passkeyRegister)]
        pub fn passkey_register(label: &str) -> js_sys::Promise;
        #[wasm_bindgen(js_name = passkeyLogin)]
        pub fn passkey_login(any_device: bool) -> js_sys::Promise;
        #[wasm_bindgen(js_name = passkeyHere)]
        pub fn passkey_here() -> bool;
    }
    #[wasm_bindgen(module = "/js/handoff.js")]
    extern "C" {
        #[wasm_bindgen(js_name = imageFromEvent)]
        pub fn image_from_event(ev: &JsValue) -> JsValue;
        #[wasm_bindgen(js_name = droppedUrl)]
        pub fn dropped_url(ev: &JsValue) -> String;
        #[wasm_bindgen(js_name = uploadHandoff)]
        pub fn upload_handoff(id: &str, file: &JsValue) -> js_sys::Promise;
        #[wasm_bindgen(js_name = copyText)]
        pub fn copy_text(text: &str) -> js_sys::Promise;
    }
}

/// An image file the owner dropped, pasted or picked (`None` if there isn't one).
/// When a drop carried only a web address, `Err` holds it.
#[cfg(feature = "hydrate")]
pub fn image_in(ev: &wasm_bindgen::JsValue) -> Result<Option<wasm_bindgen::JsValue>, String> {
    let f = js::image_from_event(ev);
    if f.is_null() || f.is_undefined() {
        let url = js::dropped_url(ev);
        return if url.is_empty() { Ok(None) } else { Err(url) };
    }
    Ok(Some(f))
}

/// Wait in the browser without blocking (for polling).
#[cfg(feature = "hydrate")]
pub async fn sleep_ms(ms: i32) {
    let p = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(p).await;
}

/// Upload the owner's image for a hand-off. Ok(view) or a message to show.
#[cfg(feature = "hydrate")]
pub async fn upload_handoff(id: &str, file: &wasm_bindgen::JsValue) -> Result<crate::api::HandoffView, String> {
    let raw = wasm_bindgen_futures::JsFuture::from(js::upload_handoff(id, file)).await.ok().and_then(|v| v.as_string()).unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&raw).map_err(|_| "Upload failed.".to_string())?;
    if v["ok"] == true {
        serde_json::from_str(v["json"].as_str().unwrap_or_default()).map_err(|_| "Upload failed.".to_string())
    } else {
        Err(v["error"].as_str().unwrap_or("Upload failed.").to_string())
    }
}

/// Copy text to the clipboard (best effort).
pub fn copy(text: &str) {
    #[cfg(feature = "hydrate")]
    {
        let p = js::copy_text(text);
        leptos::task::spawn_local(async move {
            let _ = wasm_bindgen_futures::JsFuture::from(p).await;
        });
    }
    let _ = text;
}

/// Returns "ok" or a message to show.
pub async fn register_passkey(label: String) -> String {
    #[cfg(feature = "hydrate")]
    {
        return wasm_bindgen_futures::JsFuture::from(js::passkey_register(&label)).await.ok().and_then(|v| v.as_string()).unwrap_or_else(|| "Passkey setup failed.".into());
    }
    #[allow(unreachable_code)]
    {
        let _ = label;
        "unavailable".into()
    }
}

/// `any_device`: also offer passkeys this browser doesn't hold (phone QR code).
pub async fn login_passkey(any_device: bool) -> String {
    #[cfg(feature = "hydrate")]
    {
        return wasm_bindgen_futures::JsFuture::from(js::passkey_login(any_device)).await.ok().and_then(|v| v.as_string()).unwrap_or_else(|| "Passkey sign-in failed.".into());
    }
    #[allow(unreachable_code)]
    {
        let _ = any_device;
        "unavailable".into()
    }
}

/// This browser made or used a Juggernaught passkey before.
pub fn passkey_here() -> bool {
    #[cfg(feature = "hydrate")]
    {
        return js::passkey_here();
    }
    #[allow(unreachable_code)]
    true
}

/// Full page load to `path` (picks up a fresh session cookie everywhere).
pub fn go(path: &str) {
    #[cfg(feature = "hydrate")]
    if let Some(w) = web_sys::window() {
        let _ = w.location().assign(path);
    }
    let _ = path;
}

/// Open the SSE stream for a chat turn. `on_event` runs for every event;
/// the stream closes itself on `Done` or on a connection error.
pub fn open_stream(turn_id: &str, on_event: impl Fn(StreamEvent) + 'static) {
    #[cfg(feature = "hydrate")]
    {
        use std::rc::Rc;
        use wasm_bindgen::{closure::Closure, JsCast};
        let Ok(es) = web_sys::EventSource::new(&format!("/api/chat/stream/{turn_id}")) else {
            on_event(StreamEvent::Error { message: "Couldn't open the connection.".into() });
            on_event(StreamEvent::Done);
            return;
        };
        let es = Rc::new(es);
        let on_event = Rc::new(on_event);
        let (es_m, cb_m) = (es.clone(), on_event.clone());
        let onmessage = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
            let Some(data) = e.data().as_string() else { return };
            let ev: StreamEvent = serde_json::from_str(&data).unwrap_or(StreamEvent::Usage {});
            let done = matches!(ev, StreamEvent::Done);
            cb_m(ev);
            if done {
                es_m.close();
            }
        });
        let (es_e, cb_e) = (es.clone(), on_event.clone());
        let onerror = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
            // EventSource would auto-reconnect; a turn is single-use, so stop instead.
            if es_e.ready_state() != web_sys::EventSource::CLOSED {
                es_e.close();
                cb_e(StreamEvent::Error { message: "Connection lost.".into() });
                cb_e(StreamEvent::Done);
            }
        });
        es.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        es.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        // The EventSource owns these callbacks for its (short) lifetime.
        onmessage.forget();
        onerror.forget();
    }
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (turn_id, on_event);
    }
}
