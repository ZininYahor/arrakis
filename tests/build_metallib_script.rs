//! `scripts/build-metallib.sh` in a copy, under `$TMPDIR`, of the few files it reads, with stand-ins
//! for `xcrun` and `xcodebuild`: the `DEVELOPER_DIR` it hands the compiler, what it refuses and with
//! which fix, the one form of the minimum it reads, and the config it formats the stamp with. Nothing
//! here needs Xcode, and nothing in the repository is written: the real compile is the one run of the
//! script that rebuilds the compiled file.
#![cfg(target_os = "macos")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/build-metallib.sh");

/// The app's minimum, in the one form the script reads.
const CONFIG: &str = "[env]\nMACOSX_DEPLOYMENT_TARGET = { value = \"14.0\", force = true }\n";

/// The stand-in for `xcrun`. It notes the `DEVELOPER_DIR` it ran in and its arguments, says a version
/// for `--version`, and writes the target it was given to the `-o` path. With `FAKE_NO_METAL` set, it
/// fails as `xcrun` does where `DEVELOPER_DIR` has no `metal`, such as the Command Line Tools.
const XCRUN: &str = r#"
echo "$DEVELOPER_DIR $*" >>"$(dirname "$0")/calls"
if [ -n "${FAKE_NO_METAL-}" ]; then
    echo 'xcrun: error: unable to find utility "metal", not a developer tool or in PATH' >&2
    exit 72
fi
out=
target=
while [ $# -gt 0 ]; do
    case "$1" in
    --version) echo "Apple metal version 0 (a stand-in)"; exit 0 ;;
    -o) out=$2; shift ;;
    -target) target=$2; shift ;;
    esac
    shift
done
echo "a library for $target" >"$out"
"#;

const XCODEBUILD: &str = r#"
echo "Xcode 0"
"#;

/// A folder that holds the script, a shader, a `.cargo/config.toml`, the stand-ins in `bin` and an
/// empty `xcode` to name as `DEVELOPER_DIR`, removed when it goes out of scope.
struct Tree {
    root: PathBuf,
}

impl Tree {
    fn new(name: &str) -> Self {
        Self::with_config(name, CONFIG)
    }

    fn with_config(name: &str, config: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "arrakis-build-metallib-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        for folder in [
            "scripts",
            "crates/egui-wgpu-metal/src",
            ".cargo",
            "bin",
            "xcode",
        ] {
            fs::create_dir_all(root.join(folder)).expect("a folder");
        }
        fs::copy(SCRIPT, root.join("scripts/build-metallib.sh")).expect("the script");
        fs::write(
            root.join("crates/egui-wgpu-metal/src/shader.metal"),
            "// a shader\n",
        )
        .expect("a shader");
        fs::write(root.join(".cargo/config.toml"), config).expect("a config");
        executable(&root.join("bin/xcrun"), XCRUN);
        executable(&root.join("bin/xcodebuild"), XCODEBUILD);
        Self { root }
    }

    /// The `DEVELOPER_DIR` that exists, and holds no Xcode: the stand-ins answer for it.
    fn xcode(&self) -> PathBuf {
        self.root.join("xcode")
    }

    fn src(&self, name: &str) -> PathBuf {
        self.root.join("crates/egui-wgpu-metal/src").join(name)
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.src(name)).unwrap_or_else(|error| panic!("{name}: {error}"))
    }

    /// The script, with `developer_dir` as `DEVELOPER_DIR`, or with none set.
    fn run(&self, developer_dir: Option<&Path>, env: &[(&str, &str)]) -> Output {
        let path = std::env::var("PATH").expect("a PATH");
        let mut command = Command::new("sh");
        command
            .arg(self.root.join("scripts/build-metallib.sh"))
            .env(
                "PATH",
                format!("{}:{path}", self.root.join("bin").display()),
            )
            .env_remove("FAKE_NO_METAL")
            .envs(env.iter().copied());
        match developer_dir {
            Some(dir) => command.env("DEVELOPER_DIR", dir),
            None => command.env_remove("DEVELOPER_DIR"),
        };
        command.output().expect("sh runs")
    }

    /// What the stand-in for `xcrun` was run with, one run to a line.
    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("bin/calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn wrote_nothing(&self) -> bool {
        !self.src("shader.metallib").exists() && !self.src("shader_stamp.rs").exists()
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn executable(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("a stand-in");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("executable");
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn say(out: &Output) -> String {
    format!("{}\n{}", String::from_utf8_lossy(&out.stdout), stderr(out))
}

#[test]
fn the_script_is_valid_shell() {
    let status = Command::new("sh")
        .args(["-n", SCRIPT])
        .status()
        .expect("sh runs");
    assert!(status.success());
}

#[test]
fn a_developer_dir_the_caller_set_is_the_one_the_compiler_runs_in() {
    let tree = Tree::new("caller");
    let out = tree.run(Some(&tree.xcode()), &[]);
    assert!(out.status.success(), "{}", say(&out));
    let calls = tree.calls();
    assert_eq!(calls.len(), 2, "the version and the compile: {calls:?}");
    let xcode = tree.xcode().display().to_string();
    for call in &calls {
        assert!(
            call.starts_with(&format!("{xcode} -sdk macosx metal ")),
            "{call}"
        );
    }
    assert!(
        calls[1].contains(" -target air64-apple-macos14.0 -o "),
        "{}",
        calls[1]
    );
    assert_eq!(
        tree.read("shader.metallib"),
        "a library for air64-apple-macos14.0\n"
    );
    let stamp = tree.read("shader_stamp.rs");
    assert!(
        stamp.contains("pub const TARGET: &str = \"14.0\";"),
        "{stamp}"
    );
    assert!(
        stamp.contains("pub const COMPILER: &str = \"Apple metal version 0 (a stand-in)\";"),
        "{stamp}"
    );
}

/// With none set, Xcode's default place: there, where this Mac has Xcode, or refused by that name.
#[test]
fn with_no_developer_dir_the_compiler_runs_in_xcodes_default_place() {
    let tree = Tree::new("default");
    let out = tree.run(None, &[]);
    let default = "/Applications/Xcode.app/Contents/Developer";
    if Path::new(default).is_dir() {
        assert!(out.status.success(), "{}", say(&out));
        let calls = tree.calls();
        assert!(
            !calls.is_empty()
                && calls
                    .iter()
                    .all(|call| call.starts_with(&format!("{default} "))),
            "{calls:?}"
        );
    } else {
        assert_eq!(out.status.code(), Some(1), "{}", say(&out));
        assert!(stderr(&out).contains(&format!("no Xcode in {default}")));
    }
}

#[test]
fn a_developer_dir_that_is_not_there_is_refused_by_name_with_the_fix() {
    let tree = Tree::new("nowhere");
    let nowhere = tree.root.join("nowhere");
    let out = tree.run(Some(&nowhere), &[]);
    assert_eq!(out.status.code(), Some(1), "{}", say(&out));
    let said = stderr(&out);
    assert!(
        said.contains(&format!("no Xcode in {}.", nowhere.display())),
        "{said}"
    );
    assert!(
        said.contains("xcodebuild -downloadComponent MetalToolchain"),
        "{said}"
    );
    assert!(
        said.contains("set DEVELOPER_DIR to its Contents/Developer"),
        "{said}"
    );
    assert!(tree.calls().is_empty(), "{:?}", tree.calls());
    assert!(tree.wrote_nothing());
}

/// As `DEVELOPER_DIR=/Library/Developer/CommandLineTools` reads: a folder that is there, with no `metal`.
#[test]
fn a_developer_dir_with_no_metal_compiler_is_refused_with_the_fix() {
    let tree = Tree::new("no-metal");
    let out = tree.run(Some(&tree.xcode()), &[("FAKE_NO_METAL", "1")]);
    assert_eq!(out.status.code(), Some(1), "{}", say(&out));
    let said = stderr(&out);
    for says in [
        format!(
            "Xcode's Metal compiler does not run in DEVELOPER_DIR={}",
            tree.xcode().display()
        ),
        "unable to find utility \"metal\"".to_owned(),
        "unset DEVELOPER_DIR or set it to an Xcode's Contents/Developer".to_owned(),
        "xcodebuild -downloadComponent MetalToolchain".to_owned(),
    ] {
        assert!(said.contains(&says), "{says:?} missing from {said}");
    }
    assert!(tree.wrote_nothing());
}

/// Read in the one form that `the_minimum_is_read_in_the_scripts_form_alone` (the painter's tests)
/// reads too.
#[test]
fn a_minimum_in_another_form_or_on_two_lines_is_refused() {
    let line = "MACOSX_DEPLOYMENT_TARGET = { value = \"14.0\", force = true }";
    for (name, config) in [
        ("plain", "MACOSX_DEPLOYMENT_TARGET = \"14.0\"\n".to_owned()),
        ("indented", format!("  {line}\n")),
        ("two", format!("{line}\n{line}\n")),
        ("none", "[env]\n".to_owned()),
    ] {
        let tree = Tree::with_config(name, &config);
        let out = tree.run(Some(&tree.xcode()), &[]);
        assert_eq!(out.status.code(), Some(1), "{name}: {}", say(&out));
        assert!(
            stderr(&out).contains(
                "no MACOSX_DEPLOYMENT_TARGET = { value = \"<version>\" ... } line, or more than one"
            ),
            "{name}: {}",
            stderr(&out)
        );
        assert!(tree.wrote_nothing(), "{name}");
    }
}

/// Each constant that a build leaves unread says so in the stamp, and rustfmt formats the stamp with
/// the repository's config: from stdin it looks for one from the folder it runs in.
#[test]
fn the_stamp_is_formatted_with_the_repositorys_rustfmt_config() {
    let tree = Tree::new("rustfmt");
    let out = tree.run(Some(&tree.xcode()), &[]);
    assert!(out.status.success(), "{}", say(&out));
    let stamp = tree.read("shader_stamp.rs");
    let tests_only =
        "#[cfg_attr(not(test), expect(dead_code, reason = \"only the tests read it\"))]\n";
    assert_eq!(stamp.matches(tests_only).count(), 3, "{stamp}");
    assert!(
        stamp.contains(
            "#[expect(dead_code, reason = \"only a review reads the compiler's line\")]\npub const COMPILER"
        ),
        "{stamp}"
    );
    assert!(
        stamp.contains("in bytes.\npub const BYTES"),
        "the length is read by every build: {stamp}"
    );

    fs::write(tree.root.join("rustfmt.toml"), "max_width = 60\n").expect("a config");
    let out = tree.run(Some(&tree.xcode()), &[]);
    assert!(out.status.success(), "{}", say(&out));
    let stamp = tree.read("shader_stamp.rs");
    assert!(
        stamp.contains("#[cfg_attr(\n    not(test),\n"),
        "the repository's max_width did not reach rustfmt: {stamp}"
    );
}
