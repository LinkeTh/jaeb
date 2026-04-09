// SPDX-License-Identifier: MIT
//! Compile-fail tests for the `#[event_listener]` proc macro.
//!
//! These tests verify that incorrect usage produces clear, actionable
//! compiler diagnostics rather than cryptic errors from generated code.

#[test]
fn compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/*.rs");
}
