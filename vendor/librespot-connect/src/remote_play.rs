//! LWSpot: who sent the newest play command, so the app can tell another
//! app driving playback here (such as Discord's Listen Along, which plays
//! through the Web API) apart from its own.

use std::sync::OnceLock;

use tokio::sync::watch;

fn channel() -> &'static watch::Sender<Option<String>> {
    static CHANNEL: OnceLock<watch::Sender<Option<String>>> = OnceLock::new();
    CHANNEL.get_or_init(|| watch::Sender::new(None))
}

/// The sender of each play command this device receives, such as
/// `webapi-<client id>` for one made through the Web API.
pub fn subscribe() -> watch::Receiver<Option<String>> {
    channel().subscribe()
}

pub(crate) fn received(sender: &str) {
    channel().send_replace(Some(sender.to_string()));
}
