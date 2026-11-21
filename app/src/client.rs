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
        pub fn passkey_login() -> js_sys::Promise;
    }
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

pub async fn login_passkey() -> String {
    #[cfg(feature = "hydrate")]
    {
        return wasm_bindgen_futures::JsFuture::from(js::passkey_login()).await.ok().and_then(|v| v.as_string()).unwrap_or_else(|| "Passkey sign-in failed.".into());
    }
    #[allow(unreachable_code)]
    "unavailable".into()
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
