use sha2::{Digest, Sha256};

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn generated_body(bytes: &[u8]) -> &[u8] {
    let header_end = bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .expect("generated schema has a header line");
    &bytes[header_end + 1..]
}

#[test]
fn abi_wit_schema_and_package_fixture_are_the_reviewed_extraction_inputs() {
    assert_eq!(wirt::WIRT_ABI_VERSION, "0.3.0");
    assert_eq!(
        sha256(include_bytes!("../../../wirt-sdk/wit/plugin.wit")),
        "71e32b758c7b512d2f0e9f41050f19cc7060123820dd5f2445e0dda01af7957b"
    );
    assert_eq!(
        sha256(generated_body(include_bytes!("../src/wirt_schema.rs"))),
        "6e1193d3062be83a0cd3ffadfd5762f4118d965cc8a38a45258072e71fcf24f1"
    );
    assert_eq!(
        sha256(include_bytes!("fixtures/bundled/facade-test-fixture.wirt")),
        "bd04a0eb2684c8eb6e284f68c59a5804b8876455a0864e42bbc537545df00479"
    );
}
