//! Nothing in the app sets its icon at run time. The icon reaches macOS through
//! the bundle's `Info.plist` and `.icns` alone (`scripts/bundle.sh`), and
//! the plist's `LSUIElement` keeps the app out of the Dock, since the app
//! hides its own Dock icon and never shows it. Each test scans the source for
//! what would break that. The window options carry the empty icon on macOS:
//! `entrypoint`'s own tests check that.

use std::fs;
use std::path::{Path, PathBuf};

/// Every `.rs` file under `dir`, with its path.
fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
        .map(|entry| entry.expect("an entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

/// One line of code under `src/`, with where it is.
struct Line {
    file: String,
    number: usize,
    text: String,
}

impl Line {
    fn at(&self) -> String {
        format!("{}:{}: {}", self.file, self.number, self.text.trim())
    }
}

/// The lines of `src/**/*.rs` that are not comments: those that start with
/// `//`, doc comments included.
fn code() -> Vec<Line> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    assert!(files.len() > 10, "the scan finds the source: {files:?}");
    let mut lines = Vec::new();
    for path in files {
        let file = path
            .strip_prefix(root)
            .expect("under the root")
            .display()
            .to_string();
        let source = fs::read_to_string(&path).expect("a source file");
        for (i, text) in source.lines().enumerate() {
            if !text.trim_start().starts_with("//") {
                lines.push(Line {
                    file: file.clone(),
                    number: i + 1,
                    text: text.to_owned(),
                });
            }
        }
    }
    lines
}

/// The code lines that hold `needle`.
fn hits<'a>(code: &'a [Line], needle: &str) -> Vec<&'a Line> {
    code.iter()
        .filter(|line| line.text.contains(needle))
        .collect()
}

#[test]
fn nothing_hands_appkit_an_icon() {
    let code = code();
    for needle in [
        // AppKit's own call, and `fastframe_dock::set_icon`.
        "setApplicationIconImage",
        "set_icon(",
        "NSImage",
        // An icon built for eframe or winit.
        "IconData {",
        "IconData::from",
        "from_rgba(",
        "from_png_bytes",
    ] {
        let found: Vec<String> = hits(&code, needle).iter().map(|l| l.at()).collect();
        assert!(found.is_empty(), "`{needle}` in the app: {found:#?}");
    }
}

/// The one icon option is the empty icon, on macOS, in `sized_options`, which
/// the app's window and the plate's both open through: an empty icon sets
/// none, where egui's own makes AppKit hold about 4.6 MB for each window
/// opened.
#[test]
fn the_one_icon_option_is_the_empty_icon_in_sized_options() {
    let code = code();
    let found = hits(&code, "with_icon(");
    let places: Vec<String> = found.iter().map(|l| l.at()).collect();
    assert_eq!(found.len(), 1, "`with_icon(` more than once: {places:#?}");
    let line = found[0];
    assert_eq!(line.file, "src/entrypoint.rs");
    assert!(
        line.text.contains("with_icon(egui::IconData::default())"),
        "not the empty icon: {}",
        line.at()
    );
    let function = code
        .iter()
        .filter(|l| l.file == line.file && l.number < line.number)
        .rfind(|l| l.text.trim_start().contains("fn ") && !l.text.starts_with(' '))
        .expect("a function holds the option");
    assert!(
        function.text.contains("fn sized_options("),
        "the option is in {}",
        function.at()
    );
}

#[test]
fn the_app_embeds_no_icon() {
    let code = code();
    for (i, line) in code.iter().enumerate() {
        if line.text.contains("include_bytes!") {
            // The macro may take its path on the lines after it.
            let call: String = code[i..]
                .iter()
                .take(4)
                .filter(|next| next.file == line.file)
                .map(|next| next.text.as_str())
                .collect();
            for ext in [".png", ".icns", ".ico", ".jpg"] {
                assert!(!call.contains(ext), "{} embeds a `{ext}`", line.at());
            }
        }
    }
}

/// The plist is the one source of the bundle's icon: no `cargo-bundle` or
/// `cargo-packager` table in `Cargo.toml` may name another.
#[test]
fn cargo_toml_has_no_bundling_table() {
    let manifest = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("Cargo.toml");
    assert!(manifest.contains("[package]"), "the manifest is read");
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            for word in ["metadata", "bundle", "packager"] {
                assert!(
                    !line.contains(word),
                    "a bundling table in Cargo.toml: {line}"
                );
            }
        }
    }
}

/// `scripts/Info.plist` sets `LSUIElement`, on the strength of the app hiding
/// its own Dock icon and never showing it. The calls are `wire`'s and
/// `App::update`'s, and they all hide it.
#[test]
fn the_dock_icon_is_only_ever_hidden() {
    let code = code();
    let calls = hits(&code, "set_dock_icon_visible(");
    assert!(!calls.is_empty(), "the app no longer hides its Dock icon");
    for call in &calls {
        assert!(
            call.text.contains("set_dock_icon_visible(false)"),
            "the Dock icon is not hidden: {}",
            call.at()
        );
    }
    let mut files: Vec<&str> = calls.iter().map(|l| l.file.as_str()).collect();
    files.dedup();
    assert_eq!(
        files,
        ["src/app.rs", "src/entrypoint.rs"],
        "{calls:?}",
        calls = calls.iter().map(|l| l.at()).collect::<Vec<_>>()
    );
    assert!(hits(&code, "set_dock_icon_visible(true)").is_empty());
}
