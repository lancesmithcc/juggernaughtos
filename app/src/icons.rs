//! The workspace's icon set: one stroke weight, square caps, drawn on a 24px grid.
use leptos::prelude::*;

#[component]
pub fn Icon(name: &'static str, #[prop(default = 18)] size: u32) -> impl IntoView {
    let d = match name {
        "menu" => "M4 7h16M4 12h16M4 17h16",
        "plus" => "M12 5v14M5 12h14",
        "send" => "M12 19V5M6 11l6-6 6 6",
        "check" => "M5 12.5l4.5 4.5L19 7.5",
        "back" => "M19 12H5M11 6l-6 6 6 6",
        "close" => "M6 6l12 12M18 6L6 18",
        _ => "",
    };
    view! {
        <svg class="icon" width=size height=size viewBox="0 0 24 24" fill="none" stroke="currentColor"
            stroke-width="2" stroke-linecap="square" stroke-linejoin="miter" aria-hidden="true" focusable="false">
            <path d=d/>
        </svg>
    }
}
