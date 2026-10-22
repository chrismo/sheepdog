// The libexec/sd-<verb> links bin/sd dispatches to. Moved from
// lib/config.bats; each test keeps the bats test's title above it. Only the
// target is checked, so no release build is needed.

use std::fs;
use std::path::Path;

#[track_caller]
fn assert_links_to_the_binary(verb: &str) {
    let link = Path::new(env!("CARGO_MANIFEST_DIR")).join("libexec").join(format!("sd-{verb}"));
    let target = fs::read_link(&link).unwrap_or_else(|e| panic!("{}: {e}", link.display()));
    assert_eq!(target, Path::new("../target/release/sd"), "{}", link.display());
}

// @test "is a link to the Rust binary, which runs as the verb it's named for"
#[test]
fn sd_config_is_a_link_to_the_rust_binary() {
    assert_links_to_the_binary("config");
}

// @test "sd-state: is a link to the Rust binary, which runs as the verb it's named for"
#[test]
fn sd_state_is_a_link_to_the_rust_binary() {
    assert_links_to_the_binary("state");
}

// @test "sd-watch: is a link to the Rust binary, which runs as the verb it's named for"
#[test]
fn sd_watch_is_a_link_to_the_rust_binary() {
    assert_links_to_the_binary("watch");
}

// @test "sd-review-local: is a link to the Rust binary, which runs as the verb it's named for"
#[test]
fn sd_review_local_is_a_link_to_the_rust_binary() {
    assert_links_to_the_binary("review-local");
}

// @test "sd-act: is a link to the Rust binary, which runs as the verb it's named for"
#[test]
fn sd_act_is_a_link_to_the_rust_binary() {
    assert_links_to_the_binary("act");
}
