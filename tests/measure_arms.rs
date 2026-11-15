//! `scripts/measure/build-arm.sh` and `scripts/measure/bisect-arms.sh`, against a throwaway git repository
//! that holds copies of them, and stand-ins for `cargo` and `otool` first on `PATH`. No build runs, and
//! the binaries that the stand-in cargo "builds" note it if anything runs them: the scripts must not.
#![cfg(target_os = "macos")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BUILD: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/measure/build-arm.sh");
const BISECT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/scripts/measure/bisect-arms.sh"
);

/// The stand-in for `cargo -V`, which says a version that `FAKE_CARGO_VERSION` changes, and for `cargo build`:
/// notes its folder, its `CARGO_TARGET_DIR` and its arguments, fails for the kind `FAKE_CARGO_FAIL` names
/// (`real` or `drive`), and otherwise "builds" `release/arrakis`, which notes in `ran` if anyone runs it.
const CARGO: &str = r#"
here=$(dirname "$0")
if [ "$1" = -V ]; then
    echo "$(pwd -P)" >>"$here/cargo-folders"
    echo "cargo ${FAKE_CARGO_VERSION-1.90.0} (stand-in)"
    exit 0
fi
echo "$(pwd -P)|${CARGO_TARGET_DIR-unset}|$*" >>"$here/calls"
case "$*" in
*"--features demo"*) kind=drive ;;
*) kind=real ;;
esac
if [ "${FAKE_CARGO_FAIL-}" = "$kind" ]; then
    exit 101
fi
mkdir -p "$CARGO_TARGET_DIR/release"
printf '#!/bin/sh\necho ran >>"%s/ran"\n' "$here" >"$CARGO_TARGET_DIR/release/arrakis"
chmod +x "$CARGO_TARGET_DIR/release/arrakis"
"#;

/// The stand-in for `rustc -Vv`: the compiler's lines, with the release that `FAKE_RUSTC_RELEASE` names. The
/// folder it was run in goes to `rustc-folders` beside it, as the cargo's `-V` does to `cargo-folders`.
const RUSTC: &str = r#"
[ "$1" = -Vv ] || exit 1
echo "$(pwd -P)" >>"$(dirname "$0")/rustc-folders"
echo "rustc ${FAKE_RUSTC_RELEASE-1.90.0} (stand-in)"
echo "binary: rustc"
echo "host: aarch64-apple-darwin"
echo "release: ${FAKE_RUSTC_RELEASE-1.90.0}"
"#;

/// The stand-in for `otool -l <binary>`: the load commands of a binary whose sdk is `FAKE_SDK_REAL` or
/// `FAKE_SDK_DRIVE` (27.2 by default), with other commands around `LC_BUILD_VERSION` that also hold an
/// `sdk`, or no `LC_BUILD_VERSION` at all under `FAKE_NO_BUILD_VERSION`.
const OTOOL: &str = r#"
[ "$1" = -l ] && [ -x "$2" ] || exit 1
case "$2" in
*/drive/*) sdk=${FAKE_SDK_DRIVE-27.2} ;;
*) sdk=${FAKE_SDK_REAL-27.2} ;;
esac
echo "$2:"
echo "Load command 8"
echo "      cmd LC_VERSION_MIN_MACOSX"
echo "  cmdsize 16"
echo "  version 10.9"
echo "      sdk 10.9"
if [ -z "${FAKE_NO_BUILD_VERSION-}" ]; then
    echo "Load command 9"
    echo "      cmd LC_BUILD_VERSION"
    echo "  cmdsize 32"
    echo " platform 1"
    echo "    minos 14.0"
    echo "      sdk $sdk"
    echo "   ntools 1"
    echo "     tool 3"
    echo "  version 1234.5"
fi
echo "Load command 10"
echo "      cmd LC_SOURCE_VERSION"
echo "  cmdsize 16"
echo "  version 0.0"
echo "      sdk 1.1"
"#;

/// The variables, beside `CARGO_PROFILE_RELEASE_*`, that `build-arm.sh` records in an arm's `toolchain`.
const BUILD_VARIABLES: [&str; 5] = [
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "RUSTUP_TOOLCHAIN",
    "SDKROOT",
    "DEVELOPER_DIR",
];

/// A new folder with a `bin` of stand-ins, a git repository `repo` that holds copies of the scripts in
/// `scripts/measure/`, and an `arms` folder for roots, removed when it goes out of scope.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("arrakis-arms-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("bin")).expect("a bin folder");
        fs::create_dir_all(root.join("arms")).expect("an arms folder");
        // /var is a link to /private/var here: the paths a script prints are the physical ones.
        let root = fs::canonicalize(&root).expect("the sandbox");
        let sandbox = Self { root };
        sandbox.stand_in("cargo", CARGO);
        sandbox.stand_in("rustc", RUSTC);
        sandbox.stand_in("otool", OTOOL);
        let scripts = sandbox.repo().join("scripts/measure");
        fs::create_dir_all(&scripts).expect("the scripts folder");
        for (from, to) in [(BUILD, "build-arm.sh"), (BISECT, "bisect-arms.sh")] {
            fs::copy(from, scripts.join(to)).expect("a copy of the script");
        }
        sandbox.git(&["init", "-q", "-b", "main"]);
        fs::write(sandbox.repo().join("README"), "start").expect("a file");
        sandbox.git(&["add", "README"]);
        sandbox.git(&["commit", "-q", "-m", "The start"]);
        sandbox
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    /// The root for an arm named `name`: not created.
    fn arm(&self, name: &str) -> PathBuf {
        self.root.join("arms").join(name)
    }

    fn stand_in(&self, name: &str, body: &str) {
        let path = self.root.join("bin").join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("a stand-in");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("executable");
    }

    /// What the stand-in cargo noted, one build to a line.
    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("bin/calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Whether anything that a stand-in "built" was run.
    fn something_built_ran(&self) -> bool {
        self.root.join("bin/ran").exists()
    }

    fn git_command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(self.repo())
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com");
        command
    }

    fn git(&self, args: &[&str]) -> String {
        let output = self.git_command(args).output().expect("git runs");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// A task: a branch with a commit, merged into `main` with a merge commit. Returns the merge.
    fn task(&self, name: &str) -> String {
        self.git(&["checkout", "-q", "-b", name]);
        fs::write(self.repo().join(format!("{name}.txt")), name).expect("a file");
        self.git(&["add", &format!("{name}.txt")]);
        self.git(&["commit", "-q", "-m", &format!("Work of {name}")]);
        self.git(&["checkout", "-q", "main"]);
        self.git(&[
            "merge",
            "--no-ff",
            "-q",
            "-m",
            &format!("Merge pull request from x/{name}"),
            name,
        ]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// A commit made straight on `main`. Returns it.
    fn direct(&self, name: &str) -> String {
        fs::write(self.repo().join(format!("{name}.txt")), name).expect("a file");
        self.git(&["add", &format!("{name}.txt")]);
        self.git(&["commit", "-q", "-m", &format!("Direct {name}")]);
        self.git(&["rev-parse", "HEAD"])
    }

    fn run(&self, script: &str, args: &[&str], env: &[(&str, &str)]) -> Run {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        // The scripts' own `git` gets the isolation `git_command` gives the test's: no config of the
        // owner's, and no repository, tree or index of the environment's.
        let mut command = Command::new("sh");
        command
            .arg(self.repo().join("scripts/measure").join(script))
            .args(args)
            .env("PATH", path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("FAKE_CARGO_FAIL")
            .env_remove("FAKE_CARGO_VERSION")
            .env_remove("FAKE_RUSTC_RELEASE")
            .env_remove("FAKE_NO_BUILD_VERSION")
            .env_remove("FAKE_SDK_REAL")
            .env_remove("FAKE_SDK_DRIVE");
        // What shapes a build is the environment's: a variable of the owner's must not leak into the file.
        for (name, _) in std::env::vars() {
            if BUILD_VARIABLES.contains(&name.as_str())
                || name.starts_with("CARGO_PROFILE_RELEASE_")
            {
                command.env_remove(name);
            }
        }
        let output = command.envs(env.iter().copied()).output().expect("sh runs");
        Run(output)
    }

    fn build(&self, commit: &str, root: &Path) -> Run {
        self.run("build-arm.sh", &[commit, &root.to_string_lossy()], &[])
    }

    fn worktrees(&self) -> usize {
        self.git(&["worktree", "list"]).lines().count()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Run(Output);

impl Run {
    fn code(&self) -> Option<i32> {
        self.0.status.code()
    }

    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.0.stdout).into_owned()
    }

    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.0.stderr).into_owned()
    }

    fn say(&self) -> String {
        format!("{}{}", self.stdout(), self.stderr())
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

#[test]
fn the_scripts_are_valid_shell() {
    for script in [BUILD, BISECT] {
        let status = Command::new("sh")
            .args(["-n", script])
            .status()
            .expect("sh runs");
        assert!(status.success(), "{script}");
    }
}

#[test]
fn arguments_that_are_not_a_commit_and_a_root_exit_two_before_anything_runs() {
    let sandbox = Sandbox::new("usage");
    let commit = sandbox.git(&["rev-parse", "HEAD"]);
    let arm = sandbox.arm("usage");
    let arm = arm.to_string_lossy().into_owned();
    let cases: [(&str, Vec<&str>, &str); 7] = [
        ("no arguments", vec![], "usage: build-arm.sh"),
        ("one argument", vec![&commit], "usage: build-arm.sh"),
        (
            "three arguments",
            vec![&commit, &arm, "x"],
            "usage: build-arm.sh",
        ),
        ("an empty root", vec![&commit, ""], "the root is empty"),
        (
            "a relative root",
            vec![&commit, "arms/relative"],
            "must be an absolute path",
        ),
        ("the root /", vec![&commit, "/"], "the root is /"),
        (
            "a commit that does not resolve",
            vec!["no-such-ref", &arm],
            "does not resolve to a commit",
        ),
    ];
    for (what, args, said) in cases {
        let run = sandbox.run("build-arm.sh", &args, &[]);
        assert_eq!(run.code(), Some(2), "{what}: {}", run.say());
        assert!(run.stderr().contains(said), "{what}: {}", run.say());
    }
    // A tree is not a commit either.
    let tree = sandbox.git(&["rev-parse", "HEAD^{tree}"]);
    let run = sandbox.run("build-arm.sh", &[&tree, &arm], &[]);
    assert_eq!(run.code(), Some(2), "a tree: {}", run.say());
    // Nothing ran, nothing was made, and no worktree was added.
    assert!(sandbox.calls().is_empty());
    assert!(!sandbox.arm("usage").exists());
    assert_eq!(sandbox.worktrees(), 1);
}

#[test]
fn a_build_makes_the_layout_every_measuring_script_reads_and_writes_the_commit_and_the_sdks() {
    let sandbox = Sandbox::new("layout");
    sandbox.task("one");
    let commit = sandbox.task("two");
    sandbox.task("three");
    let root = sandbox.arm("two");
    // A short name resolves to the full SHA, and a CARGO_TARGET_DIR of the caller's does not leak in.
    let short = &commit[..9];
    let run = sandbox.run(
        "build-arm.sh",
        &[short, &root.to_string_lossy()],
        &[
            ("CARGO_TARGET_DIR", "/elsewhere/target"),
            ("FAKE_SDK_REAL", "27.2"),
            ("FAKE_SDK_DRIVE", "26.5"),
        ],
    );
    assert_eq!(run.code(), Some(0), "{}", run.say());

    // The source is a detached worktree of that commit, under the root.
    let src = root.join("src");
    assert_eq!(
        sandbox.git(&["-C", &src.to_string_lossy(), "rev-parse", "HEAD"]),
        commit
    );
    let head = Command::new("git")
        .arg("-C")
        .arg(&src)
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
        .expect("git runs");
    assert!(!head.status.success(), "the worktree is on a branch");
    assert_eq!(sandbox.worktrees(), 2);
    assert!(src.join("two.txt").exists() && !src.join("three.txt").exists());

    // Two release builds in the worktree, each into a target folder of its own under the root.
    let real = root.join("arrakis-measure/real");
    let drive = root.join("arrakis-measure/drive");
    assert_eq!(
        sandbox.calls(),
        [
            format!(
                "{}|{}|build --release --locked",
                src.display(),
                real.display()
            ),
            format!(
                "{}|{}|build --release --locked --features demo",
                src.display(),
                drive.display()
            ),
        ]
    );
    for binary in [real.join("release/arrakis"), drive.join("release/arrakis")] {
        assert!(binary.is_file(), "{}", binary.display());
    }

    // The commit is in full, the sdk and the minos are each binary's own, and the decoys are not read.
    assert_eq!(read(&root.join("commit")), format!("{commit}\n"));
    assert_eq!(
        read(&root.join("sdk")),
        "real sdk=27.2 minos=14.0\ndrive sdk=26.5 minos=14.0\n"
    );
    // It says how to remove the worktree once, last and on stderr, with the command that works.
    let removal = format!("worktree remove --force {}", src.display());
    assert_eq!(run.say().matches(&removal).count(), 1, "{}", run.say());
    assert!(run.stderr().trim_end().ends_with(&removal), "{}", run.say());
    // Nothing that was built ran.
    assert!(!sandbox.something_built_ran());
}

#[test]
fn a_build_records_the_compiler_cargo_and_the_variables_that_shape_it_beside_the_sdk() {
    let sandbox = Sandbox::new("toolchain");
    let commit = sandbox.task("one");
    let root = sandbox.arm("a");
    // A DEVELOPER_DIR and an SDKROOT that exist: the system's `git` is a shim that finds the real one through
    // them, and fails when they name nothing.
    let say = |program: &str, arg: &str| {
        let output = Command::new(program).arg(arg).output().expect("a tool");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    let (developer, sdk_root) = (say("xcode-select", "-p"), say("xcrun", "--show-sdk-path"));
    let run = sandbox.run(
        "build-arm.sh",
        &[&commit, &root.to_string_lossy()],
        &[
            ("RUSTFLAGS", "-C target-cpu=native"),
            ("CARGO_PROFILE_RELEASE_LTO", "fat"),
            ("CARGO_PROFILE_RELEASE_OPT_LEVEL", "3"),
            ("RUSTUP_TOOLCHAIN", "stable-aarch64-apple-darwin"),
            ("SDKROOT", &sdk_root),
            ("DEVELOPER_DIR", &developer),
            // Not what shapes a build.
            ("CARGO_PROFILE_DEV_LTO", "off"),
            ("RUSTDOCFLAGS", "--cfg docs"),
        ],
    );
    assert_eq!(run.code(), Some(0), "{}", run.say());
    // The compiler and cargo as the source's folder gives them, and each variable that is set, by name.
    assert_eq!(
        read(&root.join("toolchain")),
        format!(
            "rustc: rustc 1.90.0 (stand-in)\nrustc: binary: rustc\nrustc: host: aarch64-apple-darwin\n\
             rustc: release: 1.90.0\ncargo: cargo 1.90.0 (stand-in)\n\
             env: CARGO_PROFILE_RELEASE_LTO=fat\nenv: CARGO_PROFILE_RELEASE_OPT_LEVEL=3\n\
             env: DEVELOPER_DIR={developer}\n\
             env: RUSTFLAGS=-C target-cpu=native\nenv: RUSTUP_TOOLCHAIN=stable-aarch64-apple-darwin\n\
             env: SDKROOT={sdk_root}\n"
        )
    );
    // Both ran in the source's folder, where a rust-toolchain.toml picks the compiler. The stand-in
    // cargo's `-V` is no build.
    let src = format!("{}\n", root.join("src").display());
    for tool in ["rustc-folders", "cargo-folders"] {
        assert_eq!(read(&sandbox.root.join("bin").join(tool)), src, "{tool}");
    }
    assert_eq!(sandbox.calls().len(), 2);

    // With none of them set, only the compiler and cargo are there; another release reads differently, so a
    // base's arm and a step's can differ.
    let bare = sandbox.arm("bare");
    let run = sandbox.run(
        "build-arm.sh",
        &[&commit, &bare.to_string_lossy()],
        &[
            ("FAKE_RUSTC_RELEASE", "1.91.0"),
            ("FAKE_CARGO_VERSION", "1.91.0"),
        ],
    );
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let text = read(&bare.join("toolchain"));
    assert!(!text.contains("env:"), "{text}");
    assert!(
        text.contains("rustc: release: 1.91.0\n") && text.contains("cargo: cargo 1.91.0 "),
        "{text}"
    );
}

#[test]
fn a_toolchain_that_cannot_run_ends_the_script_before_any_build() {
    let sandbox = Sandbox::new("no-rustc");
    let commit = sandbox.task("one");
    let root = sandbox.arm("a");
    sandbox.stand_in("rustc", "exit 1");
    let run = sandbox.build(&commit, &root);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(run.stderr().contains("rustc -Vv failed"), "{}", run.say());
    assert!(sandbox.calls().is_empty());
    assert!(!root.join("commit").exists() && !root.join("sdk").exists());
}

#[test]
fn two_arms_never_share_a_target_folder_or_a_source() {
    let sandbox = Sandbox::new("two-arms");
    let first = sandbox.task("one");
    let second = sandbox.task("two");
    let (a, b) = (sandbox.arm("a"), sandbox.arm("b"));
    assert_eq!(sandbox.build(&first, &a).code(), Some(0));
    assert_eq!(sandbox.build(&second, &b).code(), Some(0));
    let targets: Vec<String> = sandbox
        .calls()
        .iter()
        .map(|call| call.split('|').nth(1).expect("a target").to_owned())
        .collect();
    assert_eq!(targets.len(), 4);
    let mut distinct = targets.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), 4, "{targets:?}");
    for target in &targets {
        assert!(
            target.starts_with(&a.to_string_lossy().into_owned())
                || target.starts_with(&b.to_string_lossy().into_owned()),
            "{target}"
        );
    }
    assert_eq!(read(&a.join("commit")).trim(), first);
    assert_eq!(read(&b.join("commit")).trim(), second);
}

#[test]
fn a_root_that_holds_another_commits_arm_is_refused_and_left_as_it_was() {
    let sandbox = Sandbox::new("other-commit");
    let first = sandbox.task("one");
    let second = sandbox.task("two");
    let root = sandbox.arm("a");
    assert_eq!(sandbox.build(&first, &root).code(), Some(0));
    let calls = sandbox.calls();

    let run = sandbox.build(&second, &root);
    assert_eq!(run.code(), Some(2), "{}", run.say());
    assert!(
        run.stderr().contains(&format!("holds the arm of {first}")),
        "{}",
        run.say()
    );
    assert_eq!(sandbox.calls(), calls, "a build ran");
    assert_eq!(read(&root.join("commit")).trim(), first);
    assert_eq!(
        sandbox.git(&[
            "-C",
            &root.join("src").to_string_lossy(),
            "rev-parse",
            "HEAD"
        ]),
        first
    );
    assert_eq!(sandbox.worktrees(), 2);
}

#[test]
fn a_root_with_builds_that_the_script_did_not_make_is_refused_untouched() {
    let sandbox = Sandbox::new("by-hand");
    let commit = sandbox.task("one");
    // As arms built by hand are: builds and no commit file and no source.
    let root = sandbox.arm("by-hand");
    let binary = root.join("arrakis-measure/drive/release/arrakis");
    fs::create_dir_all(binary.parent().expect("a folder")).expect("a folder");
    fs::write(&binary, "the old arm").expect("a binary");

    let run = sandbox.build(&commit, &root);
    assert_eq!(run.code(), Some(2), "{}", run.say());
    assert!(run.stderr().contains("did not make"), "{}", run.say());
    assert_eq!(read(&binary), "the old arm");
    assert!(sandbox.calls().is_empty());
    assert!(!root.join("src").exists() && !root.join("commit").exists());
}

#[test]
fn a_failed_build_leaves_no_commit_file_and_a_run_for_another_commit_is_still_refused() {
    let sandbox = Sandbox::new("failed");
    let first = sandbox.task("one");
    let second = sandbox.task("two");
    let root = sandbox.arm("a");

    let run = sandbox.run(
        "build-arm.sh",
        &[&first, &root.to_string_lossy()],
        &[("FAKE_CARGO_FAIL", "drive")],
    );
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(
        run.stderr().contains("the drive build failed"),
        "{}",
        run.say()
    );
    // The commit file says a finished arm, and this is none. The worktree stays, for a retry.
    assert!(!root.join("commit").exists());
    assert!(!root.join("sdk").exists());
    assert!(root.join("src/.git").exists());
    assert_eq!(sandbox.calls().len(), 2);

    // Another commit would build over the first's folders: refused, before a build runs.
    let run = sandbox.build(&second, &root);
    assert_eq!(run.code(), Some(2), "{}", run.say());
    assert_eq!(sandbox.calls().len(), 2);

    // The same commit again reuses the worktree and finishes.
    let run = sandbox.build(&first, &root);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    assert_eq!(sandbox.calls().len(), 4);
    assert_eq!(read(&root.join("commit")).trim(), first);
    assert_eq!(sandbox.worktrees(), 2);
}

#[test]
fn a_binary_with_no_build_version_is_no_arm() {
    let sandbox = Sandbox::new("no-version");
    let commit = sandbox.task("one");
    let root = sandbox.arm("a");
    let run = sandbox.run(
        "build-arm.sh",
        &[&commit, &root.to_string_lossy()],
        &[("FAKE_NO_BUILD_VERSION", "1")],
    );
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(
        run.stderr().contains("no LC_BUILD_VERSION"),
        "{}",
        run.say()
    );
    assert!(!root.join("commit").exists() && !root.join("sdk").exists());
}

#[test]
fn a_rerun_over_a_source_with_changes_is_refused_and_left_as_it_was() {
    let sandbox = Sandbox::new("dirty");
    let commit = sandbox.task("one");
    let root = sandbox.arm("a");
    assert_eq!(sandbox.build(&commit, &root).code(), Some(0));
    let calls = sandbox.calls();
    let src = root.join("src");

    // A tracked file edited, then one added: each is a tree that is not the commit, and `git status` shows
    // both (an untracked file counts, which `git diff` would miss).
    for (what, path, named) in [
        ("an edited file", src.join("one.txt"), "one.txt"),
        ("an added file", src.join("added.rs"), "added.rs"),
    ] {
        fs::write(&path, "changed").expect("a change");
        let run = sandbox.build(&commit, &root);
        assert_eq!(run.code(), Some(2), "{what}: {}", run.say());
        assert!(
            run.stderr().contains("has changes") && run.stderr().contains(named),
            "{what}: {}",
            run.say()
        );
        assert_eq!(sandbox.calls(), calls, "{what}: a build ran");
        // Nothing was cleared: the arm it refused is still a finished one.
        assert_eq!(read(&root.join("commit")).trim(), commit, "{what}");
        assert!(root.join("sdk").exists() && root.join("toolchain").exists());
        if path.ends_with("one.txt") {
            sandbox.git(&["-C", &src.to_string_lossy(), "checkout", "--", "one.txt"]);
        } else {
            fs::remove_file(&path).expect("clean");
        }
    }

    // The tree as it was: the same commit builds again over it.
    let run = sandbox.build(&commit, &root);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    assert_eq!(sandbox.calls().len(), calls.len() + 2);
}

#[test]
fn a_rerun_that_fails_part_way_leaves_no_mark_of_a_finished_arm_and_names_the_removal_once() {
    let sandbox = Sandbox::new("rerun-fails");
    let commit = sandbox.task("one");
    let root = sandbox.arm("a");
    assert_eq!(sandbox.build(&commit, &root).code(), Some(0));
    assert!(root.join("commit").exists() && root.join("sdk").exists());

    // The second build of the rerun fails: the first one's binaries are there, and no mark says finished.
    let run = sandbox.run(
        "build-arm.sh",
        &[&commit, &root.to_string_lossy()],
        &[("FAKE_CARGO_FAIL", "drive")],
    );
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(
        !root.join("commit").exists(),
        "a finished-arm mark of the run before"
    );
    assert!(!root.join("sdk").exists(), "the sdk of the run before");
    assert!(root.join("arrakis-measure/real/release/arrakis").exists());
    // The worktree is still there, so the one line about its removal comes after the failure too.
    let removal = format!("worktree remove --force {}", root.join("src").display());
    assert_eq!(run.say().matches(&removal).count(), 1, "{}", run.say());
    assert!(run.stderr().trim_end().ends_with(&removal), "{}", run.say());
}

/// Builds `commit` as an arm in the root named `name`, under the environment `env`.
fn arm_with(sandbox: &Sandbox, commit: &str, name: &str, env: &[(&str, &str)]) -> PathBuf {
    let root = sandbox.arm(name);
    let run = sandbox.run("build-arm.sh", &[commit, &root.to_string_lossy()], env);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    root
}

#[test]
fn a_step_whose_toolchain_differs_from_the_bases_is_warned_about_with_the_lines_that_differ() {
    let sandbox = Sandbox::new("bisect-toolchain");
    let (base, merges, _) = history(&sandbox);
    let base_root = arm_with(&sandbox, &base, "base", &[]);
    let bisect = |name: &str, base_root: &Path, env: &[(&str, &str)]| {
        let root = sandbox.arm(name);
        let run = sandbox.run(
            "bisect-arms.sh",
            &[
                &base,
                &merges[2],
                &root.to_string_lossy(),
                &base_root.to_string_lossy(),
            ],
            env,
        );
        assert_eq!(
            run.code(),
            Some(0),
            "{name}: a warning is no failure: {}",
            run.say()
        );
        run
    };

    // The same toolchain and the same sdk: nothing to warn about.
    let run = bisect("same", &base_root, &[]);
    assert!(!run.say().contains("WARNING"), "{}", run.say());

    // Another compiler: the warning names the file and the lines of both arms.
    let run = bisect("compiler", &base_root, &[("FAKE_RUSTC_RELEASE", "1.91.0")]);
    let said = run.stderr();
    assert!(
        said.contains("WARNING") && said.contains("differ in their toolchain"),
        "{said}"
    );
    assert!(
        said.contains("< rustc: release: 1.90.0") && said.contains("> rustc: release: 1.91.0"),
        "{said}"
    );
    assert!(!said.contains("differ in their sdk"), "{said}");

    // A variable only the step was built with.
    let run = bisect(
        "flags",
        &base_root,
        &[("RUSTFLAGS", "-C target-cpu=native")],
    );
    assert!(
        run.stderr()
            .contains("> env: RUSTFLAGS=-C target-cpu=native"),
        "{}",
        run.say()
    );

    // Another sdk is the other file's warning.
    let run = bisect("sdk", &base_root, &[("FAKE_SDK_REAL", "26.5")]);
    let said = run.stderr();
    assert!(said.contains("differ in their sdk"), "{said}");
    assert!(
        said.contains("< real sdk=27.2 minos=14.0") && said.contains("> real sdk=26.5 minos=14.0"),
        "{said}"
    );
    assert!(!said.contains("differ in their toolchain"), "{said}");

    // The step is built either way, and the way on is still said.
    assert_eq!(read(&sandbox.arm("sdk").join("commit")).trim(), merges[0]);
    assert!(
        run.say().contains("Bad: run again with the tip"),
        "{}",
        run.say()
    );

    // Without the base's root there is nothing to compare, and nothing is said.
    let root = sandbox.arm("alone");
    let run = sandbox.run(
        "bisect-arms.sh",
        &[&base, &merges[2], &root.to_string_lossy()],
        &[("FAKE_RUSTC_RELEASE", "1.91.0")],
    );
    assert_eq!(run.code(), Some(0), "{}", run.say());
    assert!(!run.say().contains("WARNING"), "{}", run.say());
}

#[test]
fn a_base_with_no_finished_arm_is_warned_about_and_the_step_is_still_built() {
    let sandbox = Sandbox::new("bisect-no-base");
    let (base, merges, _) = history(&sandbox);
    let root = sandbox.arm("step");
    // A root that does not exist, one that holds a build that failed, and one without the toolchain file.
    let missing = sandbox.arm("missing");
    let failed = sandbox.arm("failed");
    let run = sandbox.run(
        "build-arm.sh",
        &[&base, &failed.to_string_lossy()],
        &[("FAKE_CARGO_FAIL", "drive")],
    );
    assert_eq!(run.code(), Some(1), "{}", run.say());
    let bare = arm_with(&sandbox, &base, "bare", &[]);
    fs::remove_file(bare.join("toolchain")).expect("an arm from before the file");
    for (what, base_root) in [("missing", &missing), ("failed", &failed), ("bare", &bare)] {
        let root = root.with_file_name(format!("step-{what}"));
        let run = sandbox.run(
            "bisect-arms.sh",
            &[
                &base,
                &merges[2],
                &root.to_string_lossy(),
                &base_root.to_string_lossy(),
            ],
            &[],
        );
        assert_eq!(run.code(), Some(0), "{what}: {}", run.say());
        assert!(
            run.stderr().contains("WARNING")
                && run.stderr().contains("holds no finished arm")
                && run.stderr().contains("cannot be compared"),
            "{what}: {}",
            run.say()
        );
        assert_eq!(read(&root.join("commit")).trim(), merges[0], "{what}");
    }
}

/// The merges of a history of three tasks, with a commit made straight on `main` after the second, and
/// another after the third: `(base, [one, two, three], the commit after three)`.
fn history(sandbox: &Sandbox) -> (String, Vec<String>, String) {
    let base = sandbox.git(&["rev-parse", "HEAD"]);
    let one = sandbox.task("one");
    let two = sandbox.task("two");
    sandbox.direct("between");
    let three = sandbox.task("three");
    let last = sandbox.direct("after");
    (base, vec![one, two, three], last)
}

#[test]
fn the_merges_since_a_base_are_listed_oldest_first_and_the_middle_is_named() {
    let sandbox = Sandbox::new("list");
    let (base, merges, _) = history(&sandbox);
    let run = sandbox.run("bisect-arms.sh", &[&base, &merges[2]], &[]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let said = run.stdout() + &run.stderr();
    let lines: Vec<&str> = said.lines().collect();
    // Three merges, the commits made straight on the branch left out, the tip last.
    assert!(lines[0].contains("3 candidate(s)"), "{said}");
    for (index, (name, sha)) in ["one", "two", "three"].iter().zip(&merges).enumerate() {
        let expected = format!(
            "  {} {} Merge pull request from x/{name}",
            index + 1,
            &sha[..7]
        );
        assert_eq!(lines[1 + index], expected, "{said}");
    }
    // The tip is known bad, so the middle is candidate k/2 of k: the first of three.
    assert!(
        lines[4].contains("the middle is 1 of 3") && lines[4].contains(&merges[0]),
        "{said}"
    );
    // With no root, nothing is built.
    assert_eq!(lines.len(), 5, "{said}");
    assert!(sandbox.calls().is_empty());
    assert_eq!(sandbox.worktrees(), 1);
}

#[test]
fn a_tip_that_is_no_merge_is_the_last_candidate() {
    let sandbox = Sandbox::new("direct-tip");
    let (base, merges, last) = history(&sandbox);
    let run = sandbox.run("bisect-arms.sh", &[&base, &last], &[]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let said = run.stdout() + &run.stderr();
    assert!(said.contains("4 candidate(s)"), "{said}");
    assert!(said.contains(&format!("  4 {}", &last[..7])), "{said}");
    // Of four, the middle is the second.
    assert!(
        said.contains(&format!("the middle is 2 of 4, {}", merges[1])),
        "{said}"
    );
}

#[test]
fn with_a_root_the_middle_merge_is_built_as_an_arm() {
    let sandbox = Sandbox::new("build-middle");
    let (base, merges, _) = history(&sandbox);
    let root = sandbox.arm("middle");
    let run = sandbox.run(
        "bisect-arms.sh",
        &[&base, &merges[2], &root.to_string_lossy()],
        &[],
    );
    assert_eq!(run.code(), Some(0), "{}", run.say());
    // Three candidates: the first is built, as `build-arm.sh` builds any commit.
    assert_eq!(read(&root.join("commit")).trim(), merges[0]);
    assert_eq!(sandbox.calls().len(), 2);
    assert!(root.join("sdk").exists());
    let said = run.say();
    assert!(said.contains("Bad: run again with the tip"), "{said}");
    assert!(!sandbox.something_built_ran());

    // Narrowed to the merges after the first (it read good), three candidates become two: the first of
    // them is built in a root of its own.
    let second = sandbox.arm("second");
    let run = sandbox.run(
        "bisect-arms.sh",
        &[&merges[0], &merges[2], &second.to_string_lossy()],
        &[],
    );
    assert_eq!(run.code(), Some(0), "{}", run.say());
    assert!(run.say().contains("2 candidate(s)"), "{}", run.say());
    assert_eq!(read(&second.join("commit")).trim(), merges[1]);
}

#[test]
fn one_candidate_left_is_the_first_bad_merge_and_nothing_is_built() {
    let sandbox = Sandbox::new("one-left");
    let (_, merges, _) = history(&sandbox);
    let root = sandbox.arm("none");
    let run = sandbox.run(
        "bisect-arms.sh",
        &[&merges[1], &merges[2], &root.to_string_lossy()],
        &[],
    );
    assert_eq!(run.code(), Some(0), "{}", run.say());
    assert!(
        run.say()
            .contains(&format!("the first bad merge is {}", merges[2])),
        "{}",
        run.say()
    );
    assert!(sandbox.calls().is_empty());
    assert!(!root.exists());
}

#[test]
fn a_range_that_is_no_range_exits_two_before_anything_is_built() {
    let sandbox = Sandbox::new("bad-range");
    let (base, merges, _) = history(&sandbox);
    let root = sandbox.arm("none");
    let root = root.to_string_lossy().into_owned();
    let cases: [(&str, Vec<&str>, &str); 8] = [
        ("one argument", vec![&base], "usage: bisect-arms.sh"),
        (
            "five arguments",
            vec![&base, &merges[2], &root, &root, "x"],
            "usage: bisect-arms.sh",
        ),
        (
            "an empty base root",
            vec![&base, &merges[2], &root, ""],
            "the base's root is empty",
        ),
        (
            "a base that is not an ancestor",
            vec![&merges[2], &merges[0], &root],
            "is no ancestor",
        ),
        (
            "one commit for both",
            vec![&merges[1], &merges[1], &root],
            "are one commit",
        ),
        (
            "a base that does not resolve",
            vec!["no-such-ref", &merges[2], &root],
            "does not resolve",
        ),
        (
            "a tip that does not resolve",
            vec![&base, "no-such-ref", &root],
            "does not resolve",
        ),
        (
            "an empty root",
            vec![&base, &merges[2], ""],
            "the root is empty",
        ),
    ];
    for (what, args, said) in cases {
        let run = sandbox.run("bisect-arms.sh", &args, &[]);
        assert_eq!(run.code(), Some(2), "{what}: {}", run.say());
        assert!(run.stderr().contains(said), "{what}: {}", run.say());
    }
    assert!(sandbox.calls().is_empty());
    assert_eq!(sandbox.worktrees(), 1);
}

#[test]
fn a_root_the_build_refuses_ends_the_bisection_step_with_its_exit_code() {
    let sandbox = Sandbox::new("refused-root");
    let (base, merges, _) = history(&sandbox);
    let run = sandbox.run("bisect-arms.sh", &[&base, &merges[2], "relative/root"], &[]);
    assert_eq!(run.code(), Some(2), "{}", run.say());
    assert!(
        run.stderr().contains("must be an absolute path"),
        "{}",
        run.say()
    );
    assert!(sandbox.calls().is_empty());
}
