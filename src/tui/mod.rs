//! Terminal UI: themes, the Following feed view and its runtime.

pub mod actions;
pub mod detail;
pub mod feed;
pub mod links;
pub mod picker;
pub mod render;
pub mod runtime;
pub mod theme;

use std::time::Duration;

use crate::app::App;
use feed::FeedApp;

/// Options for the feed UI.
pub struct FeedOptions {
    pub theme: &'static str,
    /// Background check interval; zero turns checking off.
    pub refresh: Duration,
    pub fancy: bool,
}

/// Shows your Following timeline until the user quits.
pub async fn run_feed(app: App, opts: FeedOptions) -> std::io::Result<()> {
    let feed = FeedApp::new(
        theme::active(opts.theme),
        opts.refresh,
        opts.fancy,
        &app.client.did,
        &app.client.handle,
        app.state,
    );
    runtime::run(feed, app.client, app.audit).await
}
