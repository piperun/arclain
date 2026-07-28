// `SecretInput` must not be `serde::Deserialize`: constructing one from an
// arbitrary untyped payload would bypass `SecretInput::new` as the single,
// grep-able construction site. This must fail to compile.

fn main() {
    let _: arclain_app::challenge::SecretInput = serde_json::from_str("\"hunter2\"").unwrap();
}
