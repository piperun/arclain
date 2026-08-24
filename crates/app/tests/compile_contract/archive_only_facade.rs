use arclain_app::archive::{ArchiveSnapshot, OpenArchiveRequest};
use arclain_app::challenge::ChallengeResponse;
use arclain_app::ids::ArchiveSessionId;
use arclain_app::settings::SettingsSnapshot;
use arclain_app::{
    ArclainApp, BootstrapConfig, APPLICATION_API_VERSION,
};

async fn host_image_facade_remains_available(app: &ArclainApp) {
    let _ = app.read_host_image(String::new()).await;
    let _ = app
        .fetch_host_image(String::new(), String::new(), None)
        .await;
    let _ = app.discard_host_image(String::new()).await;
}

fn main() {
    let _ = APPLICATION_API_VERSION;
    let _ = std::mem::size_of::<ArclainApp>();
    let _ = std::mem::size_of::<BootstrapConfig>();
    let _ = std::mem::size_of::<ArchiveSnapshot>();
    let _ = std::mem::size_of::<OpenArchiveRequest>();
    let _ = std::mem::size_of::<ArchiveSessionId>();
    let _ = std::mem::size_of::<ChallengeResponse>();
    let _ = std::mem::size_of::<SettingsSnapshot>();
    let _ = host_image_facade_remains_available;
}
