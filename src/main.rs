// libexec/sd-<verb> links here for each verb ported from bash (#193), so the
// name it runs as picks the verb.

use std::path::Path;
use std::process::ExitCode;

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
        None if rest.is_empty() => String::new(),
        None => rest.remove(0),
    };
    match verb.as_str() {
        "act" => sheepdog::act::run(&rest),
        "config" => sheepdog::config::run(&rest),
        "harness" => sheepdog::harness::run(&rest),
        "state" => sheepdog::state::run(&rest),
        "watch" => sheepdog::watch::run(&rest),
        _ => {
            eprintln!("sd: unknown verb: {verb}");
            ExitCode::from(1)
        }
    }
}
