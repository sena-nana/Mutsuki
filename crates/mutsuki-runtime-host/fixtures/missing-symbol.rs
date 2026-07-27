//! Minimal library intentionally lacking the ABI v2 entry symbol.

#[unsafe(no_mangle)]
pub extern "C" fn unrelated_fixture_symbol() {}
