//! dcontroller dashboard shell. The version label is build-injected from the
//! same `buildver` as the server's `/api/version`.

use leptos::prelude::*;

const VERSION: &str = env!("DC_VERSION");

#[component]
fn App() -> impl IntoView {
    view! {
        <header class="bar">
            <h1>"dcontroller"</h1>
            <span class="version" data-testid="version">{VERSION}</span>
        </header>
        <main>
            <p data-testid="status">"Dante devices appear here once discovery is available."</p>
        </main>
    }
}

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}
