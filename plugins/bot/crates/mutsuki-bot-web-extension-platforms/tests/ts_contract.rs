//! The committed TypeScript contract must match the Rust markers.
//! Regenerate with `MUTSUKI_BLESS_TS_CONTRACT=1 cargo test -p mutsuki-bot-web-extension-platforms`.

use std::path::Path;

#[test]
fn generated_typescript_contract_is_current() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("web/src/generated/platforms.contract.gen.ts");
    let expected = mutsuki_bot_web_extension_platforms::ts_contract();
    if std::env::var_os("MUTSUKI_BLESS_TS_CONTRACT").is_some() {
        std::fs::write(&path, &expected).unwrap();
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        committed,
        expected,
        "stale {}; regenerate with MUTSUKI_BLESS_TS_CONTRACT=1",
        path.display()
    );
}
