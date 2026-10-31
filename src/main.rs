// sd <verb> [<args>] (src/sd_help.txt): bin/sd links here. Run through a
// link named sd-<verb>, as tests/common's harness runs it, the name picks
// the verb.

use std::path::Path;
use std::process::ExitCode;

const HELP: &str = include_str!("sd_help.txt");

fn main() -> ExitCode {
    let mut args = std::env::args_os().map(|a| a.to_string_lossy().into_owned());
    let argv0 = args.next().unwrap_or_default();
    let name = Path::new(&argv0)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut rest: Vec<String> = args.collect();
    let verb = match name.strip_prefix("sd-") {
        Some(v) => v.to_string(),
        None if rest.first().is_none_or(String::is_empty) => {
            eprint!("{HELP}");
            return ExitCode::from(1);
        }
        None => rest.remove(0),
    };
    match verb.as_str() {
        "act" => sheepdog::act::run(&rest),
        "config" => sheepdog::config::run(&rest),
        "harness" => sheepdog::harness::run(&rest),
        "review-local" => sheepdog::review_local::run(&rest),
        "state" => sheepdog::state::run(&rest),
        "view" => sheepdog::view::run(&rest),
        "watch" => sheepdog::watch::run(&rest),
        "help" | "-h" | "--help" => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("sd: unknown verb: {verb}");
            ExitCode::from(1)
        }
    }
}
