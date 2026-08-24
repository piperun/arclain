use arclain_app::{analyze_url, plugins};

fn main() {
    let _ = analyze_url("https://plugins.example.test/path");
    let _ = std::mem::size_of::<plugins::PluginSummary>();
}
