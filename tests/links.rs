// bin/sd, the link ~/.local/bin/sd points at (#204). Only the target is
// checked, so no release build is needed.

use std::fs;
use std::path::Path;

#[test]
fn bin_sd_is_a_link_to_the_rust_binary() {
    let link = Path::new(env!("CARGO_MANIFEST_DIR")).join("bin/sd");
    let target = fs::read_link(&link).unwrap_or_else(|e| panic!("{}: {e}", link.display()));
    assert_eq!(target, Path::new("../target/release/sd"));
}
