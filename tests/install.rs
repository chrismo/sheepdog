// Black-box tests of install.sh (#204): a copy of the script in repo/, run in
// a temp dir (tests/common) beside a fixture repo/bin/sd that records watcher
// installation without touching launchd, and a fake cargo and uname first on
// PATH. Moved from install.bats; each test keeps the bats test's title above it.

mod common;

use common::{assert_has, chomp, Run, Sd};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

const UNAME: &str = "#!/usr/bin/env bash\nprintf '%s\\n' \"${TEST_OS:-Darwin}\"\n";

const CARGO: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "$CARGO_CALLS"
exit "${CARGO_EXIT:-0}"
"#;

const SD: &str = r#"#!/usr/bin/env bash
[[ -L $LOCAL_BIN/sd ]] || exit 99
printf '%s\n' "$*" >> "$WATCH_CALLS"
exit "${WATCH_EXIT:-0}"
"#;

/// install.bats' setup on top of tests/common's (whose bin/sd-install link and
/// config env go unused): repo/install.sh (sd.bin) and the fixture repo/bin/sd,
/// the fakes, and LOCAL_BIN, WATCH_CALLS and CARGO_CALLS exported.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("install");
    fs::create_dir_all(sd.tmp.join("repo/bin")).unwrap();
    sd.env = [
        ("LOCAL_BIN", "local-bin"),
        ("WATCH_CALLS", "watch-calls"),
        ("CARGO_CALLS", "cargo-calls"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), sd.path(v)))
    .collect();
    // cargo test exports CARGO, the real cargo, and install.sh would run it.
    sd.env.push(("CARGO".to_string(), "cargo".to_string()));
    sd.fake("uname", UNAME);
    sd.fake("cargo", CARGO);
    sd.bin = sd.tmp.join("repo/install.sh");
    fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/install.sh"), &sd.bin).unwrap();
    sd.script(&sd.tmp.join("repo/bin/sd"), SD);
    sd
}

impl Sd {
    /// `unset LOCAL_BIN; run "$INSTALLER"`: HOME is already $TEST_DIR/home.
    fn run_unset(&self, env: &[(&str, &str)]) -> Run {
        let installer = self.bin.display().to_string();
        self.exec(Path::new("env"), env, &["-u", "LOCAL_BIN", &installer])
    }

    fn local_bin(&self, name: &str) -> std::path::PathBuf {
        self.tmp.join("local-bin").join(name)
    }

    fn read(&self, name: &str) -> String {
        chomp(&fs::read_to_string(self.tmp.join(name)).unwrap())
    }

    fn readlink(&self, p: &Path) -> String {
        fs::read_link(p).unwrap().display().to_string()
    }
}

fn is_link(p: &Path) -> bool {
    p.symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
}

// @test "links sd into LOCAL_BIN"
#[test]
fn links_sd_into_local_bin() {
    let sd = sd();
    let r = sd.run(&[]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert!(is_link(&sd.local_bin("sd")));
    assert_eq!(sd.readlink(&sd.local_bin("sd")), sd.path("repo/bin/sd"));
}

// @test "builds the Rust verbs in the repo before linking (#193)"
#[test]
fn builds_the_rust_verbs_in_the_repo_before_linking() {
    let sd = sd();
    let r = sd.run(&[]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(
        sd.read("cargo-calls"),
        format!(
            "build --release --quiet --manifest-path {}",
            sd.path("repo/Cargo.toml")
        )
    );
    assert!(is_link(&sd.local_bin("sd")));
}

// @test "a failed build links nothing and installs no watcher"
#[test]
fn a_failed_build_links_nothing_and_installs_no_watcher() {
    let sd = sd();
    let r = sd.run_env(&[("CARGO_EXIT", "101")], &[]);
    assert_eq!(r.status, 1, "{}", r.output);
    assert!(!sd.local_bin("sd").exists());
    assert!(!sd.tmp.join("watch-calls").exists());
    assert_has(&r.output, &["cargo build failed"]);
}

// @test "no cargo: says what's needed and links nothing"
#[test]
fn no_cargo_says_whats_needed_and_links_nothing() {
    let sd = sd();
    let r = sd.run_env(&[("CARGO", &sd.path("no-such-cargo"))], &[]);
    assert_eq!(r.status, 1, "{}", r.output);
    assert!(!sd.local_bin("sd").exists());
    assert_has(&r.output, &["needs cargo", "rustup.rs"]);
}

// @test "links nothing else: the verbs are reached through sd"
#[test]
fn links_nothing_else_the_verbs_are_reached_through_sd() {
    let sd = sd();
    let r = sd.run(&[]);
    assert_eq!(r.status, 0, "{}", r.output);
    let names: Vec<_> = fs::read_dir(sd.tmp.join("local-bin"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, ["sd"]);
}

// @test "replaces a link left by another checkout"
#[test]
fn replaces_a_link_left_by_another_checkout() {
    let sd = sd();
    fs::create_dir(sd.tmp.join("local-bin")).unwrap();
    symlink("/somewhere/else/bin/sd", sd.local_bin("sd")).unwrap();
    let r = sd.run(&[]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(sd.readlink(&sd.local_bin("sd")), sd.path("repo/bin/sd"));
}

// @test "a regular file blocks installation without being overwritten or installing the watcher"
#[test]
fn a_regular_file_blocks_installation_without_being_overwritten_or_installing_the_watcher() {
    let sd = sd();
    fs::create_dir(sd.tmp.join("local-bin")).unwrap();
    fs::write(sd.local_bin("sd"), "mine\n").unwrap();
    let r = sd.run(&[]);
    assert_ne!(r.status, 0, "{}", r.output);
    assert!(!sd.tmp.join("watch-calls").exists());
    assert!(!is_link(&sd.local_bin("sd")));
    assert_eq!(sd.read("local-bin/sd"), "mine");
    assert_has(&r.output, &["sd", "not a symlink"]);
}

// @test "installs the watcher by default after linking sd"
#[test]
fn installs_the_watcher_by_default_after_linking_sd() {
    let sd = sd();
    let r = sd.run(&[]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(sd.read("watch-calls"), "watch install");
}

// @test "reinstalling sd reinstalls the watcher"
#[test]
fn reinstalling_sd_reinstalls_the_watcher() {
    let sd = sd();
    sd.ok(&[]);
    let r = sd.run(&[]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(sd.read("watch-calls").lines().count(), 2);
}

// @test "the watcher is installed for the link the installer made"
#[test]
fn the_watcher_is_installed_for_the_link_the_installer_made() {
    let sd = sd();
    let r = sd.run_unset(&[]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert!(is_link(&sd.tmp.join("home/.local/bin/sd")));
    assert_eq!(sd.read("watch-calls"), "watch install");
}

// @test "a relative LOCAL_BIN is linked and handed on as an absolute path"
#[test]
fn a_relative_local_bin_is_linked_and_handed_on_as_an_absolute_path() {
    let sd = sd(); // cwd is the temp dir
    let r = sd.run_env(&[("LOCAL_BIN", "local-bin"), ("WATCH_EXIT", "1")], &[]);
    assert_eq!(r.status, 1, "{}", r.output);
    assert!(is_link(&sd.local_bin("sd")));
    let lb = sd.path("local-bin");
    assert!(
        r.output
            .ends_with(&format!("retry with: LOCAL_BIN={lb} {lb}/sd watch install")),
        "{}",
        r.output
    );
}

// @test "a watcher installation failure fails the installer"
#[test]
fn a_watcher_installation_failure_fails_the_installer() {
    let sd = sd();
    let r = sd.run_env(&[("WATCH_EXIT", "42")], &[]);
    assert_eq!(r.status, 42, "{}", r.output);
    assert_eq!(sd.read("watch-calls"), "watch install");
    let lb = sd.path("local-bin");
    assert_has(
        &r.output,
        &[
            "linked",
            "watcher installation failed",
            &format!("LOCAL_BIN={lb} {lb}/sd watch install"),
        ],
    );
}

// @test "the retry hint for the default LOCAL_BIN is the plain command"
#[test]
fn the_retry_hint_for_the_default_local_bin_is_the_plain_command() {
    let sd = sd();
    let r = sd.run_unset(&[("WATCH_EXIT", "1")]);
    assert_eq!(r.status, 1, "{}", r.output);
    let home = sd.path("home");
    assert!(
        r.output
            .ends_with(&format!("retry with: {home}/.local/bin/sd watch install")),
        "{}",
        r.output
    );
}

// @test "on Linux links sd and explains that the background watcher requires macOS"
#[test]
fn on_linux_links_sd_and_explains_that_the_background_watcher_requires_macos() {
    let sd = sd();
    let r = sd.run_env(&[("TEST_OS", "Linux")], &[]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert!(is_link(&sd.local_bin("sd")));
    assert!(!sd.tmp.join("watch-calls").exists());
    assert_has(&r.output, &["watcher requires macOS"]);
}
