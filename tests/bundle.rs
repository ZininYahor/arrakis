//! `scripts/bundle.sh` and its `scripts/Info.plist`, on macOS. The script runs
//! with `ARRAKIS_BIN` set to the binary that cargo built for this test and
//! `BUNDLE_DIR` in a new folder under `TMPDIR`, so it builds nothing and writes
//! nothing in the repository. The binary is only copied and signed here, never
//! run, and nothing opens the bundle: a launch would tint the screen.

#![cfg(target_os = "macos")]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/bundle.sh");
const TEMPLATE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/Info.plist");
const MASTER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icon.png");
const IDENTIFIER: &str = "io.github.zininyahor.arrakis";

/// A new folder for a test's bundle, removed when it goes out of scope.
struct Sandbox(PathBuf);

impl Sandbox {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("arrakis-bundle-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("a folder for the bundle");
        Self(path)
    }

    /// Where the script writes the bundle.
    fn out(&self) -> PathBuf {
        self.0.join("out")
    }

    fn app(&self) -> PathBuf {
        self.out().join("Arrakis.app")
    }

    /// Runs the script on `bin`.
    fn run(&self, bin: &str) -> Output {
        self.run_script(Path::new(SCRIPT), bin)
    }

    /// Runs `script` on `bin`.
    fn run_script(&self, script: &Path, bin: &str) -> Output {
        Command::new("sh")
            .arg(script)
            .env("ARRAKIS_BIN", bin)
            .env("BUNDLE_DIR", self.out())
            .output()
            .expect("sh runs")
    }

    /// Runs the script on `bin` on a Mac that says it runs macOS `version`.
    /// `sw_vers` is the only stand-in.
    fn run_on_macos(&self, version: &str, bin: &str) -> Output {
        let path = self.stubs(&[("sw_vers", &format!("echo '{version}'"))]);
        self.command()
            .env("PATH", path)
            .env("ARRAKIS_BIN", bin)
            .env("BUNDLE_DIR", self.out())
            .output()
            .expect("sh runs")
    }

    /// The script as a command with neither `ARRAKIS_BIN` nor `BUNDLE_DIR`
    /// set: a test sets what it needs.
    fn command(&self) -> Command {
        let mut command = Command::new("sh");
        command
            .arg(SCRIPT)
            .env_remove("ARRAKIS_BIN")
            .env_remove("BUNDLE_DIR");
        command
    }

    /// A copy of the files the script reads, with `version` as the package's,
    /// in a folder of its own. Returns the copy of the script.
    fn repository_with_version(&self, version: &str) -> PathBuf {
        let repository = self.0.join("repository");
        for (from, to) in [
            (SCRIPT, "scripts/bundle.sh"),
            (TEMPLATE, "scripts/Info.plist"),
            (MASTER, "assets/icon.png"),
        ] {
            let to = repository.join(to);
            fs::create_dir_all(to.parent().expect("a folder")).expect("a folder");
            fs::copy(from, &to).expect("a copy");
        }
        fs::write(
            repository.join("Cargo.toml"),
            format!("[package]\nname = \"arrakis\"\nversion = \"{version}\"\n"),
        )
        .expect("a Cargo.toml");
        repository.join("scripts/bundle.sh")
    }

    /// A folder of programs the script finds before the real ones. Each is a
    /// shell script; `stubs` names them and gives their bodies. The `PATH`
    /// to run the script with is returned.
    fn stubs(&self, stubs: &[(&str, &str)]) -> std::ffi::OsString {
        let dir = self.0.join("stubs");
        fs::create_dir_all(&dir).expect("a folder");
        for (name, body) in stubs {
            let path = dir.join(name);
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("a stub");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("executable");
        }
        let mut dirs = vec![dir];
        dirs.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        std::env::join_paths(dirs).expect("a PATH")
    }

    /// Runs the script on the binary cargo built, and it must succeed.
    fn bundle(&self) -> &Self {
        let output = self.run(env!("CARGO_BIN_EXE_arrakis"));
        assert!(output.status.success(), "{}", text(&output));
        self
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The output of a command, for a failure's message.
fn text(output: &Output) -> String {
    format!(
        "{}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Runs `program` and returns its output, which must have succeeded.
fn succeeds(program: &str, args: &[&Path]) -> Output {
    let output = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("{program}: {error}"));
    assert!(
        output.status.success(),
        "{program} {args:?}: {}",
        text(&output)
    );
    output
}

/// A property list as JSON, through `plutil`.
fn plist(path: &Path) -> Value {
    let output = succeeds(
        "plutil",
        &[
            Path::new("-convert"),
            "json".as_ref(),
            "-o".as_ref(),
            "-".as_ref(),
            path,
        ],
    );
    serde_json::from_slice(&output.stdout).expect("plutil writes JSON")
}

/// The files under `dir`, as paths relative to it.
fn files(dir: &Path) -> BTreeSet<String> {
    fn walk(dir: &Path, base: &Path, found: &mut BTreeSet<String>) {
        for entry in fs::read_dir(dir).expect("a folder") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                walk(&path, base, found);
            } else {
                let relative = path.strip_prefix(base).expect("under the base");
                found.insert(relative.display().to_string());
            }
        }
    }
    let mut found = BTreeSet::new();
    walk(dir, dir, &mut found);
    found
}

/// The pixel size of a PNG, from `sips`.
fn pixel_size(path: &Path) -> (u32, u32) {
    let output = succeeds(
        "sips",
        &[
            Path::new("-g"),
            "pixelWidth".as_ref(),
            "-g".as_ref(),
            "pixelHeight".as_ref(),
            path,
        ],
    );
    let output = String::from_utf8_lossy(&output.stdout).into_owned();
    let value = |key: &str| -> u32 {
        output
            .lines()
            .find_map(|line| line.trim().strip_prefix(key)?.strip_prefix(':'))
            .unwrap_or_else(|| panic!("no {key} in {output:?}"))
            .trim()
            .parse()
            .expect("a number")
    };
    (value("pixelWidth"), value("pixelHeight"))
}

/// The repository's state, as `git status` gives it.
fn repository() -> String {
    let output = Command::new("git")
        .args(["-C", ROOT, "status", "--porcelain", "--untracked-files=all"])
        .output()
        .expect("git runs");
    assert!(output.status.success(), "{}", text(&output));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The `name` of the first `[[bin]]` in `Cargo.toml`.
fn bin_name() -> String {
    let manifest = fs::read_to_string(Path::new(ROOT).join("Cargo.toml")).expect("Cargo.toml");
    let mut in_bin = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_bin = line == "[[bin]]";
        } else if let Some(name) = line.strip_prefix("name").filter(|_| in_bin) {
            return name
                .trim_start_matches([' ', '='])
                .trim_matches('"')
                .to_owned();
        }
    }
    panic!("no [[bin]] name in Cargo.toml");
}

// The template.

#[test]
fn the_template_names_the_app_and_keeps_it_out_of_the_dock() {
    let template = plist(Path::new(TEMPLATE));
    for (key, value) in [
        ("CFBundleName", "Arrakis"),
        ("CFBundleIdentifier", IDENTIFIER),
        ("CFBundlePackageType", "APPL"),
        ("CFBundleIconFile", "Arrakis"),
    ] {
        assert_eq!(template[key], value, "{key}");
    }
    // `LSUIElement`: the app hides its own Dock icon and never shows it, and
    // a bundled launch would put the icon in the Dock until that call runs.
    for key in ["NSHighResolutionCapable", "LSUIElement"] {
        assert_eq!(template[key], true, "{key}");
    }
}

#[test]
fn the_template_runs_the_binary_that_cargo_builds() {
    let template = plist(Path::new(TEMPLATE));
    assert_eq!(template["CFBundleExecutable"], "arrakis");
    assert_eq!(bin_name(), "arrakis", "the [[bin]] of Cargo.toml");
    let built = Path::new(env!("CARGO_BIN_EXE_arrakis"));
    assert_eq!(built.file_name().and_then(|n| n.to_str()), Some("arrakis"));
}

#[test]
fn the_template_has_two_tokens_the_script_fills_in() {
    let template = plist(Path::new(TEMPLATE));
    assert_eq!(template["CFBundleShortVersionString"], "@VERSION@");
    assert_eq!(template["CFBundleVersion"], "@VERSION@");
    assert_eq!(template["LSMinimumSystemVersion"], "@MIN_OS@");
    // No other `@...@` token: the script would leave it in the plist.
    let source = fs::read_to_string(TEMPLATE).expect("the template");
    let tokens: BTreeSet<&str> = source
        .split('@')
        .skip(1)
        .step_by(2)
        .filter(|token| token.chars().all(|c| c.is_ascii_uppercase() || c == '_'))
        .collect();
    assert_eq!(tokens, BTreeSet::from(["MIN_OS", "VERSION"]));
    assert_eq!(source.matches('@').count() % 2, 0, "an `@` is unpaired");
}

/// The owner's choice of 2026-10-31, pinned here and not derived: a
/// launch from the bundle gives the app libmalloc's undocumented
/// `MallocSpaceEfficient=1`, which takes 3 to 10 MiB off the Memory column
/// for more frame CPU in a drag. LaunchServices wants a string, so a number
/// `1` would not do. If
/// the owner drops the flag, this test goes with it; `scripts/measure.sh`
/// reads the same key, and `tests/measure.rs` follows it.
#[test]
fn the_template_sets_the_malloc_flag_the_owner_chose() {
    let template = plist(Path::new(TEMPLATE));
    assert_eq!(template["LSEnvironment"]["MallocSpaceEfficient"], "1");
}

// The bundle.

#[test]
fn the_bundle_holds_exactly_its_files() {
    let sandbox = Sandbox::new("files");
    sandbox.bundle();
    assert_eq!(
        files(&sandbox.app()),
        [
            "Contents/Info.plist",
            "Contents/MacOS/arrakis",
            "Contents/Resources/Arrakis.icns",
            "Contents/_CodeSignature/CodeResources",
        ]
        .map(String::from)
        .into()
    );
    let mode = fs::metadata(sandbox.app().join("Contents/MacOS/arrakis"))
        .expect("the binary")
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0, "the binary is not executable: {mode:o}");
    let beside: BTreeSet<String> = fs::read_dir(sandbox.out())
        .expect("the output folder")
        .map(|entry| entry.expect("an entry").file_name().display().to_string())
        .collect();
    assert_eq!(
        beside,
        ["Arrakis.app", "Arrakis.iconset", "Arrakis.icns"]
            .map(String::from)
            .into()
    );
}

#[test]
fn the_bundles_plist_is_filled_in() {
    let sandbox = Sandbox::new("plist");
    sandbox.bundle();
    let path = sandbox.app().join("Contents/Info.plist");
    succeeds("plutil", &[Path::new("-lint"), &path]);
    assert!(
        !fs::read_to_string(&path).expect("the plist").contains('@'),
        "a token is left in the plist"
    );
    let filled = plist(&path);
    let template = plist(Path::new(TEMPLATE));
    assert_eq!(
        filled["CFBundleShortVersionString"],
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(filled["CFBundleVersion"], env!("CARGO_PKG_VERSION"));
    // The owner's choice, made on 2026-10-31: the app needs macOS 14, glow and
    // Intel included (`.cargo/config.toml`). The binary cargo built says it,
    // the script copies what `otool` reads from it, and the plist holds it. A
    // change of the minimum changes this line on purpose.
    assert_eq!(filled["LSMinimumSystemVersion"], "14.0");
    // Everything but the three tokens is the template's.
    for key in template.as_object().expect("a dictionary").keys() {
        if ![
            "CFBundleShortVersionString",
            "CFBundleVersion",
            "LSMinimumSystemVersion",
        ]
        .contains(&key.as_str())
        {
            assert_eq!(filled[key], template[key], "{key}");
        }
    }
}

#[test]
fn the_bundle_is_signed_ad_hoc_and_verifies() {
    let sandbox = Sandbox::new("signature");
    sandbox.bundle();
    let app = sandbox.app();
    succeeds(
        "codesign",
        &[Path::new("--verify"), "--strict".as_ref(), &app],
    );
    let described = succeeds("codesign", &[Path::new("-dv"), &app]);
    // `codesign -d` writes to stderr.
    let described = String::from_utf8_lossy(&described.stderr).into_owned();
    assert!(described.contains("Signature=adhoc"), "{described}");
    assert!(
        described.contains(&format!("Identifier={IDENTIFIER}")),
        "{described}"
    );
}

#[test]
fn the_icns_holds_the_ten_sizes_and_the_master_at_the_largest() {
    let sandbox = Sandbox::new("icns");
    sandbox.bundle();
    let icns = sandbox.app().join("Contents/Resources/Arrakis.icns");
    let bytes = fs::read(&icns).expect("the icns");
    assert_eq!(&bytes[..4], b"icns");
    let round_trip = sandbox.0.join("round.iconset");
    succeeds(
        "iconutil",
        &[
            Path::new("-c"),
            "iconset".as_ref(),
            &icns,
            "-o".as_ref(),
            &round_trip,
        ],
    );
    let mut wanted = BTreeSet::new();
    for size in [16, 32, 128, 256, 512] {
        wanted.insert((format!("icon_{size}x{size}.png"), size));
        wanted.insert((format!("icon_{size}x{size}@2x.png"), size * 2));
    }
    let found: BTreeSet<String> = files(&round_trip);
    assert_eq!(
        found,
        wanted
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<BTreeSet<_>>()
    );
    for (name, size) in &wanted {
        assert_eq!(pixel_size(&round_trip.join(name)), (*size, *size), "{name}");
    }
    // The largest is the master's pixels: the script copies the file, so the
    // iconset beside the bundle holds the same bytes, and the `.icns` may be
    // recoded, so the round trip is compared by its decoded pixels.
    assert_eq!(
        fs::read(sandbox.out().join("Arrakis.iconset/icon_512x512@2x.png")).expect("the iconset"),
        fs::read(MASTER).expect("the master")
    );
    let master = arrakis::diff::read(Path::new(MASTER)).expect("the master reads");
    let largest =
        arrakis::diff::read(&round_trip.join("icon_512x512@2x.png")).expect("the round trip reads");
    assert_eq!(
        (largest.width, largest.height),
        (master.width, master.height)
    );
    let (master, largest) = (&master.frames[0].pixels, &largest.frames[0].pixels);
    // Compared premultiplied: a recoding may round the color of a pixel that
    // is nearly clear.
    let premultiplied = |[r, g, b, a]: [u8; 4]| {
        [r, g, b]
            .map(|c| (u32::from(c) * u32::from(a) + 127) / 255)
            .into_iter()
            .chain([u32::from(a)])
    };
    let worst = master
        .iter()
        .zip(largest)
        .flat_map(|(a, b)| {
            premultiplied(*a)
                .zip(premultiplied(*b))
                .map(|(a, b)| a.abs_diff(b))
        })
        .max()
        .expect("pixels");
    assert!(
        worst <= 1,
        "the largest icon differs from the master by {worst}"
    );
}

#[test]
fn the_script_writes_nothing_in_the_repository() {
    let before = repository();
    let sandbox = Sandbox::new("repository");
    sandbox.bundle();
    assert_eq!(repository(), before);
}

#[test]
fn the_script_stops_without_a_bundle_when_the_binary_is_not_an_executable() {
    let sandbox = Sandbox::new("no-binary");
    let plain = sandbox.0.join("plain");
    fs::write(&plain, "not a program").expect("a plain file");
    fs::set_permissions(&plain, fs::Permissions::from_mode(0o644)).expect("not executable");
    for bin in ["/nonexistent", plain.to_str().expect("a path")] {
        let output = sandbox.run(bin);
        assert!(!output.status.success(), "{bin}: {}", text(&output));
        let message = String::from_utf8_lossy(&output.stderr);
        assert!(
            message.contains("bundle.sh:") && message.contains("is not an executable"),
            "{bin}: {message}"
        );
        assert!(!sandbox.app().exists(), "{bin}: a bundle was written");
    }
}

/// The app needs macOS 14, and the README's update
/// steps put the bundle over a working app. On an older Mac the script stops
/// first, with the version it found and the minimum, and writes nothing. The
/// binary given does not exist, so a script that went past the check would fail
/// on that instead (the next test reads that message).
#[test]
fn the_script_stops_on_a_mac_older_than_macos_14_before_it_builds_or_writes() {
    let sandbox = Sandbox::new("old-macos");
    for found in ["13.7.8", "12.0", "11.7.10", "10.15.7"] {
        let output = sandbox.run_on_macos(found, "/nonexistent");
        assert!(!output.status.success(), "{found}: {}", text(&output));
        let message = String::from_utf8_lossy(&output.stderr);
        assert!(
            message.contains("bundle.sh:")
                && message.contains(&format!("macOS {found}"))
                && message.contains("needs macOS 14 or later")
                && !message.contains("is not an executable"),
            "{found}: {message}"
        );
        assert!(!sandbox.out().exists(), "{found}: the script wrote");
    }
}

/// From 14 on, the check lets the script go on: it reaches the binary, which
/// does not exist, and says so.
#[test]
fn the_script_goes_on_from_macos_14() {
    let sandbox = Sandbox::new("new-macos");
    for found in ["14.0", "14.8.1", "15.0", "26.6.2"] {
        let output = sandbox.run_on_macos(found, "/nonexistent");
        assert!(!output.status.success(), "{found}: {}", text(&output));
        let message = String::from_utf8_lossy(&output.stderr);
        assert!(
            message.contains("is not an executable") && !message.contains("needs macOS"),
            "{found}: {message}"
        );
    }
}

/// A version that `sw_vers` does not give as numbers is no version to go on.
#[test]
fn the_script_stops_when_sw_vers_does_not_give_a_version() {
    let sandbox = Sandbox::new("no-macos");
    for found in ["", "beta", ".5"] {
        let output = sandbox.run_on_macos(found, "/nonexistent");
        assert!(!output.status.success(), "{found:?}: {}", text(&output));
        let message = String::from_utf8_lossy(&output.stderr);
        assert!(
            message.contains("bundle.sh:") && message.contains("no usable macOS version"),
            "{found:?}: {message}"
        );
        assert!(!sandbox.out().exists(), "{found:?}: the script wrote");
    }
}

// What the script does with where it is run, and what it is run on.

/// Cargo's output folder comes from its config as well as `CARGO_TARGET_DIR`
/// (`build.target-dir`), so the script asks `cargo metadata` for it. The
/// stand-in `cargo` here names a folder of its own, builds into it, and the
/// folder `CARGO_TARGET_DIR` and `./target` would give holds a binary that is
/// not one: a stale build left there must not be bundled.
#[test]
fn the_script_takes_the_binary_and_the_bundle_folder_from_cargo_metadata() {
    let sandbox = Sandbox::new("metadata");
    let configured = sandbox.0.join("configured-target");
    let stale = sandbox.0.join("stale-target");
    fs::create_dir_all(stale.join("release")).expect("a folder");
    fs::write(stale.join("release/arrakis"), "#!/bin/sh\necho stale\n").expect("a stale binary");
    fs::set_permissions(
        stale.join("release/arrakis"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("executable");
    let path = sandbox.stubs(&[(
        "cargo",
        r#"case "$1" in
metadata) printf '{"packages":[],"target_directory":"%s"}\n' "$FAKE_TARGET" ;;
build) mkdir -p "$FAKE_TARGET/release" && cp "$FAKE_BIN" "$FAKE_TARGET/release/arrakis" ;;
*) echo "unexpected: cargo $*" >&2; exit 2 ;;
esac"#,
    )]);
    let cargo = |command: &mut Command| {
        command
            .env("PATH", &path)
            .env("CARGO_TARGET_DIR", &stale)
            .env("FAKE_TARGET", &configured)
            .env("FAKE_BIN", env!("CARGO_BIN_EXE_arrakis"));
    };

    // Not given a binary, the script builds, and bundles what was built where
    // cargo put it.
    let mut command = sandbox.command();
    cargo(command.env("BUNDLE_DIR", sandbox.out()));
    let output = command.output().expect("sh runs");
    assert!(output.status.success(), "{}", text(&output));
    let bundled = fs::read(sandbox.app().join("Contents/MacOS/arrakis")).expect("the binary");
    assert!(!bundled.starts_with(b"#!"), "the stale binary was bundled");
    assert!(
        configured.join("release/arrakis").is_file(),
        "cargo build did not run"
    );

    // Not given a folder, it writes beside the folder cargo builds into.
    let mut command = sandbox.command();
    cargo(command.env("ARRAKIS_BIN", env!("CARGO_BIN_EXE_arrakis")));
    let output = command.output().expect("sh runs");
    assert!(output.status.success(), "{}", text(&output));
    assert!(configured.join("bundle/Arrakis.app").is_dir());
    assert!(
        !stale.join("bundle").exists(),
        "it wrote into the stale one"
    );
}

#[test]
fn the_script_stops_when_cargo_does_not_say_where_it_builds() {
    let sandbox = Sandbox::new("no-metadata");
    let path = sandbox.stubs(&[("cargo", "echo 'no manifest' >&2; exit 101")]);
    let output = sandbox
        .command()
        .env("PATH", path)
        .env("BUNDLE_DIR", sandbox.out())
        .output()
        .expect("sh runs");
    assert!(!output.status.success(), "{}", text(&output));
    assert!(!sandbox.app().exists(), "a bundle was written");
}

/// A relative `ARRAKIS_BIN` or `BUNDLE_DIR` means the folder the script is run
/// from, not the repository, which the script moves to.
#[test]
fn relative_paths_are_taken_from_where_the_script_is_run() {
    let before = repository();
    let sandbox = Sandbox::new("relative");
    std::os::unix::fs::symlink(
        env!("CARGO_BIN_EXE_arrakis"),
        sandbox.0.join("arrakis-built"),
    )
    .expect("a link to the binary");
    let output = sandbox
        .command()
        .current_dir(&sandbox.0)
        .env("ARRAKIS_BIN", "arrakis-built")
        .env("BUNDLE_DIR", "out/nested")
        .output()
        .expect("sh runs");
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        sandbox
            .0
            .join("out/nested/Arrakis.app/Contents/MacOS/arrakis")
            .is_file()
    );
    assert!(
        !Path::new(ROOT).join("out").exists(),
        "the bundle went into the repository"
    );
    assert_eq!(repository(), before);
}

/// `CFBundleVersion` and `CFBundleShortVersionString` are integers separated
/// by periods; a pre-release or build suffix of the package's version is cut.
#[test]
fn a_suffix_of_the_version_is_cut_from_the_bundles() {
    let sandbox = Sandbox::new("version");
    for (version, wanted) in [
        ("0.2.0", "0.2.0"),
        ("0.2.0-beta.1", "0.2.0"),
        ("1.0.0+build.5", "1.0.0"),
        ("1.2.3-rc.1+abc", "1.2.3"),
    ] {
        let script = sandbox.repository_with_version(version);
        let output = sandbox.run_script(&script, env!("CARGO_BIN_EXE_arrakis"));
        assert!(output.status.success(), "{version}: {}", text(&output));
        let filled = plist(&sandbox.app().join("Contents/Info.plist"));
        for key in ["CFBundleShortVersionString", "CFBundleVersion"] {
            assert_eq!(filled[key], wanted, "{version}: {key}");
        }
    }
}

#[test]
fn a_version_that_is_not_numbers_is_refused_without_a_bundle() {
    let sandbox = Sandbox::new("bad-version");
    for version in ["", "-beta.1", "one.two", "1.0 beta"] {
        let script = sandbox.repository_with_version(version);
        let output = sandbox.run_script(&script, env!("CARGO_BIN_EXE_arrakis"));
        assert!(!output.status.success(), "{version:?}: {}", text(&output));
        let message = String::from_utf8_lossy(&output.stderr);
        assert!(
            message.contains("bundle.sh:") && message.contains("no usable version"),
            "{version:?}: {message}"
        );
        assert!(!sandbox.app().exists(), "{version:?}: a bundle was written");
    }
}

/// The 1024 pixel size is the master itself, copied: the script does not
/// resample the master to 1024 pixels first and then overwrite that.
#[test]
fn the_script_resamples_nine_sizes_and_never_the_largest() {
    let sandbox = Sandbox::new("sips");
    let log = sandbox.0.join("sips.log");
    let path = sandbox.stubs(&[(
        "sips",
        r#"echo "$*" >> "$SIPS_LOG"; exec /usr/bin/sips "$@""#,
    )]);
    let output = sandbox
        .command()
        .env("PATH", path)
        .env("SIPS_LOG", &log)
        .env("ARRAKIS_BIN", env!("CARGO_BIN_EXE_arrakis"))
        .env("BUNDLE_DIR", sandbox.out())
        .output()
        .expect("sh runs");
    assert!(output.status.success(), "{}", text(&output));
    let calls = fs::read_to_string(&log).expect("the stub logged its calls");
    let resamples: Vec<&str> = calls.lines().filter(|line| line.contains(" -z ")).collect();
    assert_eq!(resamples.len(), 9, "{calls}");
    for line in resamples {
        assert!(!line.contains("-z 1024 1024"), "{line}");
    }
}
