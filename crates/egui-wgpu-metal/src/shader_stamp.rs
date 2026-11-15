//! What `shader.metallib` was built from. `scripts/build-metallib.sh` writes this file with it, and
//! nobody edits it: `painter.rs` asserts the length, and its tests check the rest.

/// `shader.metal`'s SHA-256.
#[cfg_attr(not(test), expect(dead_code, reason = "only the tests read it"))]
pub const SOURCE: &str = "9e17698c938d6cfbc96c7acc54ba62afe52752c748c7bd7bb99af667cde349ae";
/// `shader.metallib`'s SHA-256.
#[cfg_attr(not(test), expect(dead_code, reason = "only the tests read it"))]
pub const FILE: &str = "2ebf4e973a3eec3ab5ba5a583358407eae7c0d0fc0ccbf8927540680859b02e2";
/// `shader.metallib`'s length, in bytes.
pub const BYTES: usize = 10704;
/// The first line of `metal --version`.
#[expect(dead_code, reason = "only a review reads the compiler's line")]
pub const COMPILER: &str = "Apple metal version 32023.922 (metalfe-32023.922.4)";
/// The deployment target, from `.cargo/config.toml`: `-target air64-apple-macos14.0`.
#[cfg_attr(not(test), expect(dead_code, reason = "only the tests read it"))]
pub const TARGET: &str = "14.0";
