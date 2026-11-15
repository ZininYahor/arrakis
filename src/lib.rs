//! Arrakis: Night Shift, but it means it. A menu-bar app that
//! turns every display properly red on a daily schedule.

pub mod app;
#[cfg(feature = "demo")]
pub mod app_icon;
pub mod color;
#[cfg(feature = "demo")]
pub mod demo;
#[cfg(feature = "demo")]
pub mod diff;
#[cfg(feature = "demo")]
pub mod drive;
pub mod engine;
pub mod entrypoint;
pub mod gamma;
pub mod haptics;
pub mod icon;
pub mod idle;
pub mod model;
pub mod painter;
mod precomputed;
#[cfg(feature = "demo")]
#[doc(hidden)]
pub mod probe;
pub mod schedule;
pub mod settings;
pub mod theme;
pub mod ui;

// The minimum is macOS 14, the owner's choice of 2026-10-31. Only cargo
// reads `.cargo/config.toml`, and it finds that file from its working directory:
// `cargo build --manifest-path <repository>/Cargo.toml` from another folder
// links for macOS 11.0 with no warning, and `scripts/bundle.sh` would copy that.
// So the build stops here when the variable is not 14.0. Reading it with
// `option_env!` also makes cargo track it, so a change of the value builds this
// crate again.
#[cfg(target_os = "macos")]
const _: () = {
    const fn is_14_0(target: Option<&str>) -> bool {
        match target {
            Some(target) => matches!(target.as_bytes(), b"14.0"),
            None => false,
        }
    }
    assert!(
        is_14_0(option_env!("MACOSX_DEPLOYMENT_TARGET")),
        "Arrakis links for macOS 14.0: build from inside the repository, where cargo reads \
         .cargo/config.toml, which sets MACOSX_DEPLOYMENT_TARGET to 14.0"
    );
};
