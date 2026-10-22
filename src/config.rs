// sd config. The bash verbs reach it through lib/config.sh's prc_get and prc_step.

use crate::git;
use serde_json::{json, Map, Value};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

const HELP: &str = r#"sd config - harness and settings for the PR pipeline.

Usage:
  sd config get   [--no-branch] [--json] <path> [<branch>]
  sd config step  [--harness <h>] <step> [<branch>]
  sd config set   [--global | --branch <b>] [--json] <path> <value>
  sd config unset [--global | --branch <b>] <path>
  sd config show  [--no-branch] [<branch>]
  sd config files [--no-branch] [<branch>]
  sd config init  --global [--force]
  sd config init  [--force] [<branch>]

Examples:
  sd config get settings.verify                    one value
  sd config step address                           what a step runs with (JSON)
  sd config set --global harnesses.claude.model opus
  sd config set --json settings.botLogin '["marge"]'
  sd config unset steps.address.harness            inherit it again
  sd config show                                   everything, merged
  sd config files                                  where the files are

Layers (top wins):
  branch    <main repo>/.git/sheepdog/<branch>.json, shared by all worktrees
            (sd config files prints the paths)
  global    ~/.config/sheepdog/config.json ($SHEEPDOG_CONFIG, $XDG_CONFIG_HOME)
  defaults  harness claude; settings.remote origin, stepTimeout 1800,
            stepBudget 5 (USD per claude step; 0 is no cap), branchBudget
            25 (USD a branch's steps may spend before sd state hands them
            to chrismo; 0 is none), forkLimit 100000 (tokens of context a
            step may inherit before it starts a new session; 0 is none),
            reviewRounds 3,
            botTimeout 3600 (seconds to wait on the review bot), interval
            300, vacuumAfter 604800, rateLimitFloor 250 (GraphQL points
            under which sd watch stops a pass), replyPrefix "🤖 ";
            protectedPaths none (unset: .sheepdog/protected-paths in the
            base commit, else the CI config dirs, sd act --help); the other
            settings empty

Notes:
  - null in a file removes the key; a default still applies
  - get prints strings raw, lists one per line, objects as JSON (--json: all)
  - exit 3: no value and no default. exit 1: an error
  - <branch> defaults to the current one; --no-branch skips the branch file
  - options may go anywhere; -- ends them (for a value like --json)
  - set/unset write the current branch's file unless --global or --branch
  - init (no --global) writes empty blocks: a branch file holds only overrides
  - files are typed: an unknown key or a bad value is an error naming the
    file and path; a number may be quoted ("1800", as set writes it without
    --json); forkLimit, reviewRounds, interval and rateLimitFloor are whole
    numbers; get prints a value as written; remote, interval, notify,
    vacuumAfter and rateLimitFloor are global-file only
  - set values are strings; --json takes a number, list, object or null
  - step: the harness's block, with the step's block for it laid over it
"#;

fn defaults() -> Value {
    json!({
        "harness": "claude",
        "settings": {
            "remote": "origin", "stepTimeout": 1800, "stepBudget": 5, "branchBudget": 25,
            "forkLimit": 100000, "reviewRounds": 3, "botTimeout": 3600, "interval": 300,
            "vacuumAfter": 604800, "rateLimitFloor": 250, "replyPrefix": "🤖 ",
            "verify": "", "autofix": "", "localReview": "", "botCommand": "", "notify": "",
            "reviewPrompt": "", "implementPrompt": "", "reviewFixPrompt": "", "describePrompt": "",
            "addressPrompt": "", "resolvePrompt": "", "ciFixPrompt": "",
            "botLogin": [], "botIgnore": [], "botIgnoreBody": [], "implementAllow": [],
            "flags": {
                "suppressions": [r"shellcheck\s+disable", "eslint-disable", r"#\s*noqa", "@ts-ignore",
                                 "@ts-expect-error", r"#\s*type:\s*ignore", r"//\s*nolint",
                                 "rubocop:disable", r"#!?\[allow\("],
                "skips": [r"(^|[;&|])\s*skip(\s|$)", r"\.skip\(", r"\bxit\(", r"\bxdescribe\(",
                          r"\bt\.Skip(f|Now)?\(", r"@pytest\.mark\.skip", r"^\s*pending(\s|\(|$)"],
                "testFiles": ["*_test.*", "*.test.*", "*.bats", "test/", "spec/", "tests/"]
            }
        }
    })
}


#[derive(Clone, Copy)]
enum Ty {
    Str,
    Num,
    Whole,
    Bool,
    List,
    Enum(&'static [&'static str]),
    Obj(&'static [(&'static str, Ty)]),
    Settings,
}

// A step is a sd act verb that runs an agent: add new ones here.
const HARNESSES: &[&str] = &["claude", "pi"];
const STEPS: &[&str] = &["address", "ci-fix", "describe", "implement", "review"];

const CLAUDE: &[(&str, Ty)] = &[
    ("model", Ty::Str),
    ("effort", Ty::Enum(&["low", "medium", "high", "xhigh", "max"])),
    ("args", Ty::Str),
    ("useApiKey", Ty::Bool),
];
const PI: &[(&str, Ty)] = &[
    ("provider", Ty::Str),
    ("model", Ty::Str),
    ("effort", Ty::Enum(&["off", "minimal", "low", "medium", "high", "xhigh", "max"])),
    ("args", Ty::Str),
];
const HARNESS_BLOCKS: &[(&str, Ty)] = &[("claude", Ty::Obj(CLAUDE)), ("pi", Ty::Obj(PI))];
const STEP: &[(&str, Ty)] = &[
    ("harness", Ty::Enum(HARNESSES)),
    ("claude", Ty::Obj(CLAUDE)),
    ("pi", Ty::Obj(PI)),
];
const STEP_BLOCKS: &[(&str, Ty)] = &[
    ("address", Ty::Obj(STEP)),
    ("ci-fix", Ty::Obj(STEP)),
    ("describe", Ty::Obj(STEP)),
    ("implement", Ty::Obj(STEP)),
    ("review", Ty::Obj(STEP)),
];
const FLAGS: &[(&str, Ty)] = &[("suppressions", Ty::List), ("skips", Ty::List), ("testFiles", Ty::List)];
const SETTINGS: &[(&str, Ty)] = &[
    ("stepTimeout", Ty::Num), ("stepBudget", Ty::Num), ("branchBudget", Ty::Num),
    ("forkLimit", Ty::Whole), ("reviewRounds", Ty::Whole), ("replyPrefix", Ty::Str),
    ("verify", Ty::Str), ("autofix", Ty::Str), ("localReview", Ty::Str), ("botCommand", Ty::Str),
    ("botTimeout", Ty::Num),
    ("reviewPrompt", Ty::Str), ("implementPrompt", Ty::Str), ("reviewFixPrompt", Ty::Str),
    ("describePrompt", Ty::Str), ("addressPrompt", Ty::Str), ("resolvePrompt", Ty::Str),
    ("ciFixPrompt", Ty::Str),
    ("botLogin", Ty::List), ("botIgnore", Ty::List), ("botIgnoreBody", Ty::List),
    ("implementAllow", Ty::List), ("protectedPaths", Ty::List),
    ("flags", Ty::Obj(FLAGS)),
];
// The watcher's settings: it reads no branch file.
const GLOBAL_ONLY: &[(&str, Ty)] = &[
    ("remote", Ty::Str), ("interval", Ty::Whole), ("notify", Ty::Str),
    ("vacuumAfter", Ty::Num), ("rateLimitFloor", Ty::Whole),
];
const CONFIG: &[(&str, Ty)] = &[
    ("harness", Ty::Enum(HARNESSES)),
    ("harnesses", Ty::Obj(HARNESS_BLOCKS)),
    ("steps", Ty::Obj(STEP_BLOCKS)),
    ("settings", Ty::Settings),
];

#[derive(Clone, Copy, PartialEq)]
enum Layer {
    Global,
    Branch,
}

fn problems(doc: &Value, layer: Layer) -> Vec<String> {
    let mut out = Vec::new();
    check(doc, Ty::Obj(CONFIG), "", layer, &mut out);
    out.sort();
    out.into_iter().map(|(p, m)| format!("{p}: {m}")).collect()
}

fn check(v: &Value, ty: Ty, path: &str, layer: Layer, out: &mut Vec<(String, String)>) {
    if v.is_null() {
        return;
    }
    let ok = match ty {
        Ty::Str => v.is_string(),
        Ty::Num => match v {
            Value::Number(n) => n.as_f64().is_some_and(|f| f >= 0.0),
            Value::String(s) => s.parse::<f64>().is_ok_and(|f| f.is_finite() && f >= 0.0),
            _ => false,
        },
        Ty::Whole => match v {
            Value::Number(n) => n.is_u64(),
            Value::String(s) => s.parse::<u64>().is_ok(),
            _ => false,
        },
        Ty::Bool => v.is_boolean(),
        Ty::List => v.as_array().is_some_and(|a| a.iter().all(Value::is_string)),
        Ty::Enum(syms) => v.as_str().is_some_and(|s| syms.contains(&s)),
        Ty::Obj(_) | Ty::Settings => match v.as_object() {
            Some(m) => {
                for (k, kv) in m {
                    let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                    match field(ty, k, layer) {
                        Ok(t) => check(kv, t, &p, layer, out),
                        Err(problem) => out.push((p, problem.into())),
                    }
                }
                true
            }
            None => false,
        },
    };
    if !ok {
        out.push((path.to_string(), format!("expected {}, got {v}", expected(ty))));
    }
}

fn field(ty: Ty, k: &str, layer: Layer) -> Result<Ty, &'static str> {
    let find = |fields: &[(&str, Ty)]| fields.iter().find(|(name, _)| *name == k).map(|&(_, t)| t);
    match ty {
        Ty::Obj(fields) => find(fields).ok_or("unknown key"),
        _ => match (find(SETTINGS), find(GLOBAL_ONLY)) {
            (Some(t), _) => Ok(t),
            (None, Some(t)) if layer == Layer::Global => Ok(t),
            (None, Some(_)) => Err("global file only"),
            (None, None) => Err("unknown key"),
        },
    }
}

fn expected(ty: Ty) -> String {
    match ty {
        Ty::Str => "a string".into(),
        Ty::Num => "a number of 0 or more".into(),
        Ty::Whole => "a whole number of 0 or more".into(),
        Ty::Bool => "true or false".into(),
        Ty::List => "a list of strings".into(),
        Ty::Enum(syms) => format!("one of {}", syms.join(", ")),
        Ty::Obj(_) | Ty::Settings => "an object".into(),
    }
}


/// Lists are leaves; an empty object has none.
fn leaves(v: &Value, path: &mut Vec<String>, out: &mut Vec<(Vec<String>, Value)>) {
    match v.as_object() {
        Some(m) => {
            for (k, kv) in m {
                path.push(k.clone());
                leaves(kv, path, out);
                path.pop();
            }
        }
        None => out.push((path.clone(), v.clone())),
    }
}

/// Deep merge, later layers on top: a leaf replaces every leaf at, above or
/// below its path in lower layers. Keys come out sorted. With drop_nulls, a
/// null leaf is dropped before it can shadow anything, so a lower layer's
/// value (a default) shows through it.
fn merge(layers: &[&Value], drop_nulls: bool) -> Value {
    let mut all = Vec::new();
    for (i, layer) in layers.iter().enumerate() {
        let mut ls = Vec::new();
        leaves(layer, &mut Vec::new(), &mut ls);
        all.extend(ls.into_iter().filter(|(_, v)| !(drop_nulls && v.is_null())).map(|(k, v)| (i, k, v)));
    }
    let prefix = |a: &[String], b: &[String]| a.len() <= b.len() && a == &b[..a.len()];
    let mut kept: Vec<_> = all
        .iter()
        .filter(|(i, k, _)| !all.iter().any(|(j, f, _)| j > i && (prefix(f, k) || prefix(k, f))))
        .collect();
    kept.sort_by(|a, b| a.1.cmp(&b.1));
    let mut out = Value::Object(Map::new());
    for (_, k, v) in kept {
        put(&mut out, k, v.clone());
    }
    out
}

/// Missing or null parents become objects.
fn put(doc: &mut Value, path: &[String], v: Value) {
    let mut cur = doc;
    for seg in &path[..path.len() - 1] {
        let m = cur.as_object_mut().expect("parent checked to be an object");
        let next = m.entry(seg.clone()).or_insert(Value::Null);
        if next.is_null() {
            *next = Value::Object(Map::new());
        }
        cur = next;
    }
    cur.as_object_mut()
        .expect("parent checked to be an object")
        .insert(path[path.len() - 1].clone(), v);
}

fn lookup<'a, S: AsRef<str>>(doc: &'a Value, path: &[S]) -> Option<&'a Value> {
    path.iter().try_fold(doc, |cur, seg| cur.as_object()?.get(seg.as_ref()))
}


enum Fail {
    Error(Vec<String>),
    Unset,
}

type Res<T> = Result<T, Fail>;

fn err<T>(msg: impl Into<String>) -> Res<T> {
    Err(Fail::Error(vec![msg.into()]))
}

fn global_file() -> String {
    let set = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    set("SHEEPDOG_CONFIG").unwrap_or_else(|| {
        let base = set("XDG_CONFIG_HOME")
            .unwrap_or_else(|| format!("{}/.config", std::env::var("HOME").unwrap_or_default()));
        format!("{base}/sheepdog/config.json")
    })
}

/// Where sheepdog keeps its state: usage.sup, the watcher's glyphs and log.
pub fn state_dir() -> String {
    std::env::var("SHEEPDOG_STATE_DIR")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| format!("{}/.local/state/sheepdog", std::env::var("HOME").unwrap_or_default()))
}

fn branch_file(b: &str) -> Res<String> {
    let Some(dir) = git::common_dir() else { return err("not a git repository") };
    let name = b.replace('%', "%25").replace('/', "%2F");
    Ok(format!("{dir}/sheepdog/{name}.json"))
}

fn branch_arg(b: &str) -> Res<String> {
    if !b.is_empty() {
        return Ok(b.to_string());
    }
    git::current_branch().map_or_else(|| err("detached HEAD; name a branch"), Ok)
}

fn one_value(text: &str) -> Option<Value> {
    let mut it = serde_json::Deserializer::from_str(text).into_iter::<Value>();
    match (it.next(), it.next()) {
        (Some(Ok(v)), None) => Some(v),
        _ => None,
    }
}

fn layer(file: &str, kind: Option<Layer>) -> Res<Value> {
    if !Path::new(file).exists() {
        return Ok(json!({}));
    }
    let doc = fs::read_to_string(file)
        .ok()
        .and_then(|t| one_value(&t))
        .filter(Value::is_object);
    let Some(doc) = doc else { return err(format!("not a JSON object: {file}")) };
    if let Some(kind) = kind {
        checked(&doc, kind, file)?;
    }
    Ok(doc)
}

fn checked(doc: &Value, kind: Layer, file: &str) -> Res<()> {
    let ps = problems(doc, kind);
    if ps.is_empty() {
        return Ok(());
    }
    Err(Fail::Error(ps.into_iter().map(|p| format!("{file}: {p}")).collect()))
}

/// The global file with the branch file merged over it, nulls kept.
fn files(b: &str, no_branch: bool) -> Res<Value> {
    let b = match (no_branch, b) {
        (true, _) => String::new(),
        (false, "") => git::current_branch().unwrap_or_default(),
        (false, b) => b.to_string(),
    };
    let g = layer(&global_file(), Some(Layer::Global))?;
    let bj = if b.is_empty() { json!({}) } else { layer(&branch_file(&b)?, Some(Layer::Branch))? };
    Ok(merge(&[&g, &bj], false))
}

fn with_defaults(files: &Value) -> Value {
    merge(&[&defaults(), files], true)
}

fn segments(path: &str) -> Res<Vec<String>> {
    let ok = !path.is_empty()
        && path.split('.').all(|s| {
            !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        });
    if !ok {
        return err(format!("invalid path: '{path}' (dotted names, e.g. harnesses.claude.model)"));
    }
    Ok(path.split('.').map(str::to_string).collect())
}

fn line(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        _ => v.to_string(),
    }
}


fn get(path: &str, b: &str, no_branch: bool, as_json: bool) -> Res<String> {
    let segs = segments(path)?;
    let all = with_defaults(&files(b, no_branch)?);
    let v = lookup(&all, &segs).ok_or(Fail::Unset)?;
    Ok(match v {
        Value::Array(a) if !as_json => a.iter().map(|e| line(e) + "\n").collect(),
        Value::Object(_) => format!("{v}\n"),
        _ if as_json => format!("{v}\n"),
        _ => line(v) + "\n",
    })
}

/// The lines sd config would print to stderr for a failure; none when unset.
fn stderr_lines<T>(r: Res<T>) -> Result<T, Vec<String>> {
    r.map_err(|f| match f {
        Fail::Unset => vec![],
        Fail::Error(lines) => lines,
    })
}

/// prc_get for the verbs ported to Rust: what `sd config get` prints, less
/// the last newline, or the lines it would print to stderr (none when unset).
pub fn get_text(path: &str, b: &str, as_json: bool) -> Result<String, Vec<String>> {
    get_line(path, b, false, as_json)
}

/// get_text with no branch file: prc_get <path> '' true.
pub fn get_global(path: &str, as_json: bool) -> Result<String, Vec<String>> {
    get_line(path, "", true, as_json)
}

fn get_line(path: &str, b: &str, no_branch: bool, as_json: bool) -> Result<String, Vec<String>> {
    stderr_lines(get(path, b, no_branch, as_json)).map(|out| out.strip_suffix('\n').unwrap_or(&out).to_string())
}

fn show(b: &str, no_branch: bool) -> Res<String> {
    let all = with_defaults(&files(b, no_branch)?);
    Ok(serde_json::to_string_pretty(&all).expect("a Value serializes") + "\n")
}

fn paths(b: &str, no_branch: bool) -> Res<String> {
    let path_line = |kind: &str, f: &str| {
        let missing = if Path::new(f).exists() { "" } else { "  (missing)" };
        format!("{kind}  {f}{missing}\n")
    };
    let mut out = path_line("global", &global_file());
    if no_branch {
        return Ok(out);
    }
    let mut b = b.to_string();
    if b.is_empty() {
        if !git::in_repo() {
            return Ok(out + "branch  (none: not a git repository)\n");
        }
        match git::current_branch() {
            Some(cur) => b = cur,
            None => return Ok(out + "branch  (none: detached HEAD)\n"),
        }
    }
    out += &path_line("branch", &branch_file(&b)?);
    Ok(out)
}

/// prc_step for the verbs ported to Rust: the JSON value `sd config step`
/// prints, or the lines it would print to stderr.
pub fn step(name: &str, b: &str, forced: &str) -> Result<Value, Vec<String>> {
    stderr_lines(step_block(name, b, forced))
}

/// The harness <name> runs on, its block, and the step's block for it.
fn step_block(name: &str, b: &str, forced: &str) -> Res<Value> {
    if !STEPS.contains(&name) {
        return err(format!("unknown step: {name} ({})", STEPS.join(" ")));
    }
    if !forced.is_empty() && !HARNESSES.contains(&forced) {
        return err(format!("unknown harness: {forced} ({})", HARNESSES.join(" ")));
    }
    let f = files(b, false)?;
    let at = |path: &[&str]| lookup(&f, path).cloned();
    let harness = if forced.is_empty() {
        [at(&["steps", name, "harness"]), at(&["harness"])]
            .into_iter()
            .flatten()
            .find_map(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "claude".into())
    } else {
        forced.to_string()
    };
    let hb = at(&["harnesses", &harness]).unwrap_or(Value::Null);
    let sb = at(&["steps", name, &harness]).unwrap_or(Value::Null);
    warn_inherited_effort(name, &harness, &hb, &sb);

    let mut out = Map::new();
    out.insert("harness".into(), Value::String(harness));
    for block in [&hb, &sb] {
        if let Some(m) = block.as_object() {
            for (k, v) in m {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    out.retain(|_, v| !v.is_null());
    Ok(Value::Object(out))
}

/// Effort level names depend on the model. A step block that writes effort,
/// even null, means it.
fn warn_inherited_effort(step: &str, h: &str, hb: &Value, sb: &Value) {
    let sets_model = sb.get("model").is_some_and(|m| !m.is_null());
    let writes_effort = sb.get("effort").is_some();
    match hb.get("effort") {
        Some(e) if sets_model && !writes_effort && !e.is_null() => eprintln!(
            "sd config: steps.{step}.{h} sets model but inherits effort {} from harnesses.{h}; \
             effort levels depend on the model, so set steps.{step}.{h}.effort (null clears it)",
            line(e)
        ),
        _ => {}
    }
}

fn target_file(global: bool, b: &str) -> Res<String> {
    if global {
        return Ok(global_file());
    }
    let b = branch_arg(b)?;
    if !git::branch_exists(&b) {
        return err(format!("no such branch: {b}"));
    }
    branch_file(&b)
}

fn write(file: &str, doc: &Value) -> Res<()> {
    let kind = if file == global_file() { Layer::Global } else { Layer::Branch };
    checked(doc, kind, file)?;
    write_file(file, doc)
}

fn write_file(file: &str, doc: &Value) -> Res<()> {
    let fail = |e: std::io::Error| Fail::Error(vec![format!("couldn't update {file}: {e}")]);
    let p = Path::new(file);
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir).map_err(fail)?;
    }
    let tmp = format!("{file}.{}", std::process::id());
    let text = serde_json::to_string_pretty(doc).expect("a Value serializes") + "\n";
    fs::File::create(&tmp)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .and_then(|_| fs::rename(&tmp, p))
        .map_err(|e| {
            let _ = fs::remove_file(&tmp);
            fail(e)
        })
}

fn set(file: &str, path: &str, value: &str, as_json: bool) -> Res<()> {
    let segs = segments(path)?;
    let v = if as_json {
        match one_value(value) {
            Some(v) => v,
            None => return err(format!("not one JSON value: {value}")),
        }
    } else {
        if value.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\t' | '\r')) {
            return err("control characters aren't supported in a value");
        }
        Value::String(value.to_string())
    };
    let mut doc = layer(file, None)?;
    for i in 1..segs.len() {
        if lookup(&doc, &segs[..i]).is_some_and(|p| !p.is_null() && !p.is_object()) {
            return err(format!("couldn't set {path} in {file}: a parent isn't an object"));
        }
    }
    put(&mut doc, &segs, v);
    write(file, &doc)
}

fn unset(file: &str, path: &str) -> Res<()> {
    let segs = segments(path)?;
    let mut doc = layer(file, None)?;
    let (last, parent) = segs.split_last().expect("a path has a segment");
    let mut cur = Some(&mut doc);
    for seg in parent {
        cur = cur.and_then(|c| c.as_object_mut()?.get_mut(seg));
    }
    if let Some(m) = cur.and_then(Value::as_object_mut) {
        m.shift_remove(last);
    }
    write(file, &doc)
}

/// A branch file holds only overrides, so its skeleton has no harness.
fn init(dest: &str, force: bool, with_harness: bool) -> Res<String> {
    if !force && Path::new(dest).exists() {
        return err(format!("{dest} exists; --force overwrites it"));
    }
    let mut doc = json!({"harness": "claude", "harnesses": {"claude": {}, "pi": {}}, "steps": {}, "settings": {}});
    if !with_harness {
        doc.as_object_mut().expect("an object").shift_remove("harness");
    }
    write_file(dest, &doc)?;
    Ok(format!("wrote {dest}\n"))
}


pub fn run(argv: &[String]) -> ExitCode {
    match cli(argv) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(Fail::Unset) => ExitCode::from(3),
        Err(Fail::Error(lines)) => {
            for l in lines {
                eprintln!("sd config: {l}");
            }
            ExitCode::from(1)
        }
    }
}

fn usage_exit(text: &str) -> ! {
    eprint!("{text}");
    std::process::exit(1)
}

fn needs(verb: &str, what: &str) -> ! {
    let lines: String = HELP
        .lines()
        .filter(|l| l.starts_with(&format!("  sd config {verb} ")))
        .map(|l| format!("{l}\n"))
        .collect();
    usage_exit(&format!("sd config: {verb} needs {what}\nUsage:\n{lines}"))
}

fn cli(argv: &[String]) -> Res<String> {
    let Some(verb) = argv.first() else { usage_exit(HELP) };
    let takes: &[&str] = match verb.as_str() {
        "-h" | "--help" => return Ok(HELP.into()),
        "get" => &["--no-branch", "--json"],
        "files" | "show" => &["--no-branch"],
        "step" => &["--harness"],
        "init" => &["--force", "--global"],
        "set" => &["--global", "--branch", "--json"],
        "unset" => &["--global", "--branch"],
        _ => return err(format!("unknown verb: {verb}")),
    };

    let mut flags: Vec<&str> = Vec::new();
    let (mut branch_opt, mut harness_opt) = (String::new(), String::new());
    let mut args: Vec<&str> = Vec::new();
    let mut it = argv[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--" => {
                args.extend(it.by_ref().map(String::as_str));
            }
            "-h" | "--help" => return Ok(HELP.into()),
            o @ ("--no-branch" | "--json" | "--harness" | "--force" | "--global" | "--branch") => {
                if !takes.contains(&o) {
                    return err(format!("{verb} doesn't take {o}"));
                }
                if o == "--branch" || o == "--harness" {
                    let Some(v) = it.next() else { needs(verb, &format!("a value for {o}")) };
                    if o == "--branch" {
                        branch_opt = v.clone();
                    } else {
                        harness_opt = v.clone();
                    }
                } else {
                    flags.push(o);
                }
            }
            v => args.push(v),
        }
    }
    let has = |f: &str| flags.contains(&f);
    let arg = |i: usize| args.get(i).copied().unwrap_or("");
    let n = args.len();

    match verb.as_str() {
        "get" => {
            if !(n == 1 || n == 2) {
                needs(verb, "<path> [<branch>]");
            }
            get(arg(0), arg(1), has("--no-branch"), has("--json"))
        }
        "files" | "show" => {
            if n > 1 {
                needs(verb, "at most a <branch>");
            }
            if verb == "files" { paths(arg(0), has("--no-branch")) } else { show(arg(0), has("--no-branch")) }
        }
        "step" => {
            if !(n == 1 || n == 2) {
                needs(verb, "<step> [<branch>]");
            }
            step_block(arg(0), arg(1), &harness_opt).map(|v| format!("{v}\n"))
        }
        "init" if has("--global") => {
            if n != 0 {
                needs(verb, "no <branch> with --global");
            }
            init(&global_file(), has("--force"), true)
        }
        "init" => {
            if n > 1 {
                needs(verb, "at most a <branch>");
            }
            let b = branch_arg(arg(0))?;
            if !git::branch_exists(&b) {
                return err(format!("no such branch: {b}"));
            }
            init(&branch_file(&b)?, has("--force"), false)
        }
        _ => {
            let file = target_file(has("--global"), &branch_opt)?;
            if verb == "unset" {
                if n != 1 {
                    needs(verb, "<path>");
                }
                unset(&file, arg(0))?;
            } else {
                if n != 2 {
                    needs(verb, "<path> <value>");
                }
                set(&file, arg(0), arg(1), has("--json"))?;
            }
            Ok(String::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probs(doc: Value, layer: Layer) -> Vec<String> {
        problems(&doc, layer)
    }

    #[test]
    fn merge_a_higher_leaf_shadows_lower_leaves_at_above_and_below_its_path() {
        let low = json!({"a": {"b": 1, "c": 2}, "d": "x", "e": {"f": 1}});
        let high = json!({"a": {"b": 9}, "d": {"g": 3}, "e": null});
        assert_eq!(merge(&[&low, &high], false), json!({"a": {"b": 9, "c": 2}, "d": {"g": 3}, "e": null}));
    }

    #[test]
    fn merge_dropping_nulls_lets_the_lower_layer_show_through() {
        let low = json!({"s": {"t": 1800}});
        let high = json!({"s": {"t": null}});
        assert_eq!(merge(&[&low, &high], true), json!({"s": {"t": 1800}}));
    }

    #[test]
    fn merge_sorts_keys_and_drops_empty_blocks() {
        let m = merge(&[&json!({"z": 1, "a": {}, "m": {"y": 1, "b": 2}})], false);
        assert_eq!(m.to_string(), r#"{"m":{"b":2,"y":1},"z":1}"#);
    }

    #[test]
    fn merge_lists_are_leaves() {
        let m = merge(&[&json!({"l": ["a", "b"]}), &json!({"l": []})], false);
        assert_eq!(m, json!({"l": []}));
    }

    #[test]
    fn put_keeps_an_existing_keys_place_and_appends_a_new_one() {
        let mut d = json!({"a": 1, "b": 2});
        put(&mut d, &["a".into()], json!(3));
        put(&mut d, &["c".into(), "d".into()], json!(4));
        assert_eq!(d.to_string(), r#"{"a":3,"b":2,"c":{"d":4}}"#);
    }

    #[test]
    fn the_defaults_pass_as_a_global_file() {
        assert!(probs(defaults(), Layer::Global).is_empty());
    }

    #[test]
    fn an_unknown_key_is_named_at_its_own_path() {
        assert_eq!(
            probs(json!({"settnigs": {}, "steps": {"adress": {"claude": {}}}}), Layer::Global),
            ["settnigs: unknown key", "steps.adress: unknown key"]
        );
    }

    #[test]
    fn a_global_only_setting_in_a_branch_file_says_so() {
        assert_eq!(probs(json!({"settings": {"remote": "x"}}), Layer::Branch), ["settings.remote: global file only"]);
        assert!(probs(json!({"settings": {"remote": "x"}}), Layer::Global).is_empty());
    }

    #[test]
    fn a_bad_value_names_what_was_expected_and_what_was_there() {
        assert_eq!(
            probs(json!({"harness": "claud", "settings": {"stepTimeout": "30m"}}), Layer::Global),
            [
                r#"harness: expected one of claude, pi, got "claud""#,
                r#"settings.stepTimeout: expected a number of 0 or more, got "30m""#,
            ]
        );
    }

    #[test]
    fn whole_settings_refuse_fractions_others_take_them_quoted_or_not_and_none_negative() {
        let ok = json!({"settings": {"stepTimeout": "1800", "botTimeout": "0.5", "vacuumAfter": 0.5,
                                     "forkLimit": "100000", "stepBudget": "2.5", "branchBudget": 40.5}});
        assert!(probs(ok, Layer::Global).is_empty());
        let bad = json!({"settings": {"stepTimeout": -1, "interval": 1.5, "reviewRounds": "2.5",
                                      "stepBudget": true, "branchBudget": -5}});
        assert_eq!(
            probs(bad, Layer::Global),
            [
                "settings.branchBudget: expected a number of 0 or more, got -5",
                "settings.interval: expected a whole number of 0 or more, got 1.5",
                r#"settings.reviewRounds: expected a whole number of 0 or more, got "2.5""#,
                "settings.stepBudget: expected a number of 0 or more, got true",
                "settings.stepTimeout: expected a number of 0 or more, got -1",
            ]
        );
    }

    #[test]
    fn strings_bools_and_lists_are_strict() {
        let bad = json!({"settings": {"verify": ["a"], "botLogin": "marge"},
                         "harnesses": {"claude": {"useApiKey": "yes"}}});
        assert_eq!(probs(bad, Layer::Global).len(), 3);
    }

    #[test]
    fn null_is_allowed_wherever_a_key_is_known() {
        let doc = json!({"harness": null, "steps": {"address": null}, "settings": {"verify": null}});
        assert!(probs(doc, Layer::Branch).is_empty());
    }

    #[test]
    fn a_path_is_plain_dotted_names() {
        for bad in ["a..b", ".a", "a.", "a b", "a[0]", "", r#"a"]"#] {
            assert!(segments(bad).is_err(), "accepted {bad:?}");
        }
        assert!(segments("steps.ci-fix.claude_x").is_ok());
    }

    #[test]
    fn one_value_refuses_none_or_several() {
        assert_eq!(one_value(" 900 "), Some(json!(900)));
        for bad in ["", "1 2", r#"1, "harness": "claude""#, "[a"] {
            assert!(one_value(bad).is_none(), "accepted {bad:?}");
        }
    }

    // The only unit test that reads a config file: no other sets SHEEPDOG_CONFIG.
    #[test]
    fn get_global_reads_the_global_file_over_the_defaults_and_no_branch_file() {
        let dir = std::env::temp_dir().join(format!("sd-config-unit-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.json");
        fs::write(&file, r#"{"settings": {"botTimeout": 900, "botLogin": ["x"]}}"#).unwrap();
        std::env::set_var("SHEEPDOG_CONFIG", &file);
        assert_eq!(get_global("settings.botTimeout", false), Ok("900".into()));
        assert_eq!(get_global("settings.botLogin", false), Ok("x".into()));
        assert_eq!(get_global("settings.botLogin", true), Ok(r#"["x"]"#.into()));
        assert_eq!(get_global("settings.remote", false), Ok("origin".into()));
        assert_eq!(get_global("settings.nope", false), Err(vec![]));
        fs::write(&file, "[]").unwrap();
        assert_eq!(get_global("settings.remote", false), Err(vec![format!("not a JSON object: {}", file.display())]));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn step_refuses_an_unknown_step_or_harness_with_sd_configs_lines() {
        assert_eq!(step("deploy", "", ""), Err(vec!["unknown step: deploy (address ci-fix describe implement review)".into()]));
        assert_eq!(step("review", "", "codex"), Err(vec!["unknown harness: codex (claude pi)".into()]));
    }
}
