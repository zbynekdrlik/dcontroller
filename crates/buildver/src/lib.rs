//! The one version string of a dcontroller build, computed at build time from
//! git so the server's `/api/version` and the dashboard label never drift.
//!
//! Format: `v<pkg>` on a commit tagged `v<pkg>`, else `v<pkg>-dev.<n>`, where
//! `<n>` counts commits since the last `v*` tag (all commits when none),
//! followed by ` (<short sha>)` when git is available.

use std::process::Command;

/// What git says about HEAD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitState {
    /// The last `v*` tag reachable from HEAD, if any.
    pub tag: Option<String>,
    /// Commits since `tag` (since the root when there is no tag).
    pub since: u32,
    /// HEAD's short SHA.
    pub sha: String,
}

/// The version string for package version `pkg` and git state `git`.
pub fn format(pkg: &str, git: Option<&GitState>) -> String {
    let Some(g) = git else {
        return format!("v{pkg}-dev.0");
    };
    let released = g.since == 0 && g.tag.as_deref() == Some(&format!("v{pkg}"));
    if released {
        format!("v{pkg} ({})", g.sha)
    } else {
        format!("v{pkg}-dev.{} ({})", g.since, g.sha)
    }
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8(out.stdout).ok()?.trim().to_owned())
}

/// Reads HEAD's state from the git repository in the current directory.
pub fn read_git() -> Option<GitState> {
    let sha = git(&["rev-parse", "--short=7", "HEAD"])?;
    let tag = git(&["describe", "--tags", "--abbrev=0", "--match", "v*"]);
    let range = match &tag {
        Some(t) => format!("{t}..HEAD"),
        None => "HEAD".to_owned(),
    };
    let since = git(&["rev-list", "--count", &range])?.parse().ok()?;
    Some(GitState { tag, since, sha })
}

/// For a `build.rs`: sets `DC_VERSION` for the crate and reruns on new commits.
pub fn emit() {
    let pkg = std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    let version = format(&pkg, read_git().as_ref());
    println!("cargo:rustc-env=DC_VERSION={version}");
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/refs");
        println!("cargo:rerun-if-changed={dir}/packed-refs");
        if let Some(head) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={dir}/{head}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(tag: Option<&str>, since: u32) -> GitState {
        GitState {
            tag: tag.map(str::to_owned),
            since,
            sha: "abc1234".into(),
        }
    }

    #[test]
    fn release_commit_has_no_dev_suffix() {
        assert_eq!(
            format("1.2.3", Some(&st(Some("v1.2.3"), 0))),
            "v1.2.3 (abc1234)"
        );
    }

    #[test]
    fn commits_after_tag_are_dev() {
        assert_eq!(
            format("1.2.4", Some(&st(Some("v1.2.3"), 5))),
            "v1.2.4-dev.5 (abc1234)"
        );
    }

    #[test]
    fn tagged_commit_of_another_version_is_dev() {
        assert_eq!(
            format("1.2.4", Some(&st(Some("v1.2.3"), 0))),
            "v1.2.4-dev.0 (abc1234)"
        );
    }

    #[test]
    fn no_tag_counts_all_commits() {
        assert_eq!(
            format("0.1.0", Some(&st(None, 7))),
            "v0.1.0-dev.7 (abc1234)"
        );
    }

    #[test]
    fn no_git_still_valid_semver() {
        assert_eq!(format("0.1.0", None), "v0.1.0-dev.0");
    }

    #[test]
    fn reads_this_repo() {
        let g = read_git().expect("tests run inside the git checkout");
        assert_eq!(g.sha.len(), 7);
        assert!(g.sha.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
