//! `CHANGELOG.md` follows the package. Its newest entry, the first `## `
//! heading, is the version that `Cargo.toml` gives the package, then ` - ` and
//! the release's date. A release's notes on GitHub are that entry, so a version
//! that moves without its entry, or an entry without its version, would put out
//! the wrong notes.

const CHANGELOG: &str = include_str!("../CHANGELOG.md");
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Whether `text` is a date written `YYYY-MM-DD`, with a month from 01 to 12
/// and a day from 01 to 31. It does not know how long each month is.
fn is_date(text: &str) -> bool {
    let mut parts = text.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let digits = |part: &str, length: usize| {
        part.len() == length && part.bytes().all(|byte| byte.is_ascii_digit())
    };
    digits(year, 4)
        && digits(month, 2)
        && digits(day, 2)
        && matches!(month.parse::<u8>(), Ok(1..=12))
        && matches!(day.parse::<u8>(), Ok(1..=31))
}

/// What is wrong with the newest entry of `changelog` for a package at
/// `package`, or `None` when it is `## <package> - YYYY-MM-DD`. Every message
/// names `package`, so a failure says which version was expected.
fn newest_entry_problem(changelog: &str, package: &str) -> Option<String> {
    let Some(heading) = changelog.lines().find_map(|line| line.strip_prefix("## ")) else {
        return Some(format!(
            "CHANGELOG.md has no `## ` heading for a release: Cargo.toml's package is \
             {package}, so expected `## {package} - YYYY-MM-DD`"
        ));
    };
    let Some((version, date)) = heading.split_once(" - ") else {
        return Some(format!(
            "CHANGELOG.md's newest heading is `## {heading}`: Cargo.toml's package is \
             {package}, so expected `## {package} - YYYY-MM-DD`"
        ));
    };
    if version != package {
        return Some(format!(
            "CHANGELOG.md's newest entry is for {version}, but Cargo.toml's package is \
             {package}: a release changes the version and adds its entry together"
        ));
    }
    if !is_date(date) {
        return Some(format!(
            "CHANGELOG.md's entry for {version} is dated `{date}`, expected YYYY-MM-DD \
             (Cargo.toml's package is {package})"
        ));
    }
    None
}

#[test]
fn the_changelogs_newest_entry_is_the_packages_version_and_a_date() {
    if let Some(problem) = newest_entry_problem(CHANGELOG, VERSION) {
        panic!("{problem}");
    }
}

#[test]
fn every_failure_names_the_version_the_package_has() {
    // A stand-in for the package's version: any text that cannot be a
    // heading's own, so a message that names it must have taken it from the
    // package.
    let package = "7.8.9";
    for (what, changelog) in [
        ("no heading", "# Changelog\n\nSome notes.\n"),
        ("no version and date", "# Changelog\n\n## Unreleased\n"),
        ("another version", "## 1.2.3 - 2026-01-02\n"),
        ("a bad date", "## 7.8.9 - soon\n"),
        ("an impossible month", "## 7.8.9 - 2026-13-01\n"),
        ("an impossible day", "## 7.8.9 - 2026-11-00\n"),
    ] {
        let problem = newest_entry_problem(changelog, package)
            .unwrap_or_else(|| panic!("a changelog with {what} passed"));
        assert!(
            problem.contains(package),
            "the failure for a changelog with {what} does not name the package's version: {problem}"
        );
    }
}

#[test]
fn a_date_needs_a_month_from_01_to_12_and_a_day_from_01_to_31() {
    for date in ["2026-11-16", "2026-01-01", "2026-12-31"] {
        assert!(is_date(date), "`{date}` was refused");
    }
    for date in [
        "2026-13-01",
        "2026-00-10",
        "2026-11-00",
        "2026-11-32",
        "2026-99-99",
        "2026-1-16",
        "2026-11-1",
        "26-11-16",
        "2026/11/16",
        "2026-11-1a",
        "2026-11-16-01",
        "+026-11-16",
        "soon",
        "",
    ] {
        assert!(!is_date(date), "`{date}` was taken");
    }
}

#[test]
fn a_matching_version_and_a_date_pass() {
    assert_eq!(
        newest_entry_problem(
            "# Changelog\n\n## 7.8.9 - 2026-11-16\n\n- A change.\n",
            "7.8.9"
        ),
        None
    );
}
