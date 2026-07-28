// `SecretInput` must not be `serde::Serialize`: serializing a live secret
// into a log or bridge payload by accident would defeat the point of
// wrapping it. This must fail to compile.

fn main() {
    let secret = arclain_app::challenge::SecretInput::new("hunter2".to_string());
    let _ = serde_json::to_string(&secret);
}
