// `SecretInput` must not be `Clone`: duplicating a live secret into a
// second heap allocation zeroize does not know about defeats the point of
// wrapping it. This must fail to compile.

fn main() {
    let secret = arclain_app::challenge::SecretInput::new("hunter2".to_string());
    let _cloned = secret.clone();
}
