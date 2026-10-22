// Black-box tests of sd config (#199): the built binary, run as sd-config
// through a link, in a temp git repo with SHEEPDOG_CONFIG in a temp dir.
// Built-in defaults, the global XDG file and the branch file, merged; `step`
// is what one pipeline step runs with, `get` reads any dotted path.
// lib/config.sh's tests, and the super it pins, stay in bats: lib/config.bats.

mod common;

use common::{chomp, Sd};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const GLOBAL: &str = r#"{"harness":"claude",
  "harnesses":{"claude":{"model":"strong","effort":"high"},
               "pi":{"provider":"openrouter","model":"cheap","effort":"medium"}},
  "steps":{"address":{"harness":"pi","pi":{"model":"cheaper","effort":null}},
           "review":{"claude":{"effort":"max"}}}}"#;

impl Sd {
    /// The harness with a git repo on branch feat (off main) as cwd.
    fn new() -> Sd {
        let mut sd = Sd::for_verb("config");
        sd.git(&["init", "-q", "repo"]);
        sd.cd("repo");
        sd.git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        sd.git(&["switch", "-qc", "feat"]);
        sd
    }

    /// One field of the step view as raw text ("" when absent).
    fn get_field(&self, step: &str, field: &str) -> String {
        let v = json(&self.out(&["step", step]));
        match &v[field] {
            Value::Null | Value::Bool(false) => String::new(),
            f => raw(f),
        }
    }

    fn cfg_s(&self) -> String {
        self.cfg().display().to_string()
    }

    /// Where sd config keeps a branch's overrides.
    fn branch_file(&self, b: &str) -> PathBuf {
        self.tmp.join(format!("repo/.git/sheepdog/{b}.json"))
    }

    fn bf(&self, b: &str) -> String {
        self.branch_file(b).display().to_string()
    }

    fn write_branch(&self, b: &str, s: &str) {
        let f = self.branch_file(b);
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(f, format!("{s}\n")).unwrap();
    }

    /// Where sd config keeps the repo's settings, shared by every branch (#194).
    fn repo_file(&self) -> PathBuf {
        self.tmp.join("repo/.git/sheepdog.json")
    }

    fn rf(&self) -> String {
        self.repo_file().display().to_string()
    }

    fn write_repo(&self, s: &str) {
        fs::write(self.repo_file(), format!("{s}\n")).unwrap();
    }

    /// feat rebasing onto main, stopped on a conflict (detached HEAD, as
    /// restack --resolve leaves it).
    fn rebase_stopped(&self) {
        self.write("f.txt", "ours\n");
        self.git(&["add", "f.txt"]);
        self.git(&["commit", "-qm", "ours"]);
        self.git(&["switch", "-q", "main"]);
        self.write("f.txt", "theirs\n");
        self.git(&["add", "f.txt"]);
        self.git(&["commit", "-qm", "theirs"]);
        self.git(&["switch", "-q", "feat"]);
        assert_ne!(self.git_status(&["rebase", "-q", "main"]), 0);
        assert!(self.cwd.join(".git/rebase-merge").is_dir());
        assert_eq!(self.git(&["branch", "--show-current"]), "");
    }
}

fn json(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|e| panic!("not JSON ({e}): {s}"))
}

fn file_json(p: impl AsRef<Path>) -> Value {
    json(&fs::read_to_string(p).unwrap())
}

/// jq -c .
fn compact(v: &Value) -> String {
    v.to_string()
}

/// jq -cS .
fn sorted(v: &Value) -> String {
    fn sort(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                Value::Object(keys.into_iter().map(|k| (k.clone(), sort(&m[k]))).collect())
            }
            Value::Array(a) => Value::Array(a.iter().map(sort).collect()),
            v => v.clone(),
        }
    }
    sort(v).to_string()
}

/// jq -r: a string unquoted, anything else as JSON.
fn raw(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}

/// jq -c keys
fn keys(v: &Value) -> String {
    let mut k: Vec<&String> = v.as_object().unwrap().keys().collect();
    k.sort();
    serde_json::to_string(&k).unwrap()
}

fn lacks(v: &Value, key: &str) -> bool {
    !v.as_object().unwrap().contains_key(key)
}

#[test]
fn no_verb_usage_on_stderr_nonzero() {
    let sd = Sd::new();
    let r = sd.run(&[]);
    assert_ne!(r.status, 0);
    assert!(r.output.contains("Usage"), "{}", r.output);
}

#[test]
fn unknown_verb_fails() {
    let sd = Sd::new();
    let r = sd.run(&["frobnicate"]);
    assert_ne!(r.status, 0);
    assert!(r.output.contains("unknown verb"), "{}", r.output);
}

// ── step: what one pipeline step runs with ──────────────────────────────────

#[test]
fn step_a_step_name_that_could_carry_query_syntax_is_refused() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    let r = sd.run(&["step", r#"x" | values 1 | values ""#]);
    assert_ne!(r.status, 0);
    assert!(r.output.contains("unknown step"), "{}", r.output);
}

#[test]
fn step_no_config_anywhere_resolves_to_the_claude_default() {
    let sd = Sd::new();
    let r = sd.run(&["step", "address"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, r#"{"harness":"claude"}"#);
}

#[test]
fn step_the_harness_block_supplies_model_and_effort() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    assert_eq!(sd.get_field("describe", "model"), "strong");
    assert_eq!(sd.get_field("describe", "effort"), "high");
}

#[test]
fn step_a_step_block_on_the_same_harness_overrides_the_harness_block() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    assert_eq!(sd.get_field("review", "effort"), "max");
    assert_eq!(sd.get_field("review", "model"), "strong");
}

#[test]
fn step_a_step_can_select_another_harness_and_gets_that_harnesss_block() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    assert_eq!(sd.get_field("address", "harness"), "pi");
    assert_eq!(sd.get_field("address", "provider"), "openrouter");
    assert_eq!(sd.get_field("address", "model"), "cheaper");
}

#[test]
fn step_null_in_a_step_block_clears_the_inherited_key() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    let r = sd.run(&["step", "address"]);
    assert!(lacks(&json(&r.output), "effort"));
}

#[test]
fn step_a_model_override_that_inherits_effort_warns() {
    let sd = Sd::new();
    sd.write_global(
        r#"{"harnesses":{"claude":{"model":"strong","effort":"high"}},
                 "steps":{"review":{"claude":{"model":"other"}}}}"#,
    );
    let r = sd.run_sep(&["step", "review"]);
    assert_eq!(r.status, 0);
    let v = json(&r.output);
    assert_eq!(raw(&v["model"]), "other");
    assert_eq!(raw(&v["effort"]), "high");
    assert!(
        r.stderr.contains(
            "steps.review.claude sets model but inherits effort high from harnesses.claude"
        ),
        "{}",
        r.stderr
    );
}

#[test]
fn step_the_branch_files_model_override_warns_too_and_the_warning_is_once() {
    let sd = Sd::new();
    sd.write_global(r#"{"harnesses":{"claude":{"model":"strong","effort":"high"}}}"#);
    sd.write_branch(
        "feat",
        r#"{"steps":{"review":{"claude":{"model":"other"}}}}"#,
    );
    let r = sd.run_sep(&["step", "review"]);
    assert_eq!(r.status, 0);
    assert_eq!(
        r.stderr
            .lines()
            .filter(|l| l.contains("inherits effort"))
            .count(),
        1
    );
}

#[test]
fn step_no_warning_when_the_override_sets_effort_clears_it_or_sets_no_model() {
    let sd = Sd::new();
    for o in [
        r#"{"model":"other","effort":"low"}"#,
        r#"{"model":"other","effort":null}"#,
        r#"{"effort":"low"}"#,
        "{}",
    ] {
        sd.write_global(&format!(
            r#"{{"harnesses":{{"claude":{{"model":"strong","effort":"high"}}}},"steps":{{"review":{{"claude":{o}}}}}}}"#
        ));
        let r = sd.run_sep(&["step", "review"]);
        assert_eq!(r.status, 0, "for: {o}");
        assert_eq!(r.stderr, "", "for: {o}");
    }
}

#[test]
fn step_no_warning_when_the_harness_block_has_no_effort_to_inherit() {
    let sd = Sd::new();
    sd.write_global(r#"{"harnesses":{"claude":{"model":"strong"}},"steps":{"review":{"claude":{"model":"other"}}}}"#);
    let r = sd.run_sep(&["step", "review"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.stderr, "");
}

#[test]
fn step_the_branch_file_overrides_the_global_step_block() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.write_branch(
        "feat",
        r#"{"steps":{"address":{"pi":{"model":"branch-model"}}}}"#,
    );
    assert_eq!(sd.get_field("address", "model"), "branch-model");
    assert_eq!(sd.get_field("address", "provider"), "openrouter");
}

#[test]
fn step_the_branch_file_overrides_the_global_harness_block() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.write_branch(
        "feat",
        r#"{"harnesses":{"claude":{"model":"branch-strong"}}}"#,
    );
    assert_eq!(sd.get_field("describe", "model"), "branch-strong");
    assert_eq!(sd.get_field("describe", "effort"), "high");
}

#[test]
fn step_the_branch_file_can_switch_a_steps_harness() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.write_branch(
        "feat",
        r#"{"steps":{"review":{"harness":"pi","pi":{"effort":null,"model":"b2"}}}}"#,
    );
    let r = sd.run(&["step", "review"]);
    assert_eq!(
        r.output,
        r#"{"harness":"pi","model":"b2","provider":"openrouter"}"#
    );
}

#[test]
fn step_the_branch_file_can_clear_a_global_value_with_null() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.write_branch("feat", r#"{"harnesses":{"claude":{"effort":null}}}"#);
    let r = sd.run(&["step", "describe"]);
    assert!(lacks(&json(&r.output), "effort"));
}

#[test]
fn step_a_branch_file_with_no_global_file_still_resolves() {
    let sd = Sd::new();
    sd.write_branch(
        "feat",
        r#"{"harnesses":{"claude":{"model":"only-branch"}}}"#,
    );
    assert_eq!(sd.get_field("describe", "model"), "only-branch");
}

#[test]
fn step_another_branchs_file_is_not_read() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.write_branch(
        "other",
        r#"{"harnesses":{"claude":{"model":"other-model"}}}"#,
    );
    assert_eq!(sd.get_field("describe", "model"), "strong");
}

#[test]
fn step_a_named_branch_reads_that_branchs_file() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.write_branch(
        "other",
        r#"{"harnesses":{"claude":{"model":"other-model"}}}"#,
    );
    let r = sd.run(&["step", "describe", "other"]);
    assert_eq!(raw(&json(&r.output)["model"]), "other-model");
}

#[test]
fn step_a_branch_name_with_a_slash_maps_to_one_flat_file() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.git(&["switch", "-qc", "topic/sub"]);
    fs::create_dir_all(sd.cwd.join(".git/sheepdog")).unwrap();
    sd.write(
        ".git/sheepdog/topic%2Fsub.json",
        "{\"harnesses\":{\"claude\":{\"model\":\"slashed\"}}}\n",
    );
    assert_eq!(sd.get_field("describe", "model"), "slashed");
}

#[test]
fn step_a_linked_worktree_reads_the_same_branch_file_as_the_main_one() {
    let mut sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.write_branch("wt", r#"{"harnesses":{"claude":{"model":"from-wt"}}}"#);
    let wt = sd.tmp.join("wt");
    sd.git(&["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "wt"]);
    sd.cd("wt");
    assert_eq!(sd.get_field("describe", "model"), "from-wt");
}

#[test]
fn step_xdg_config_home_locates_the_global_file_when_sheepdog_config_is_unset() {
    let mut sd = Sd::new();
    sd.config = None;
    let xdg = sd.tmp.join("xdg");
    fs::create_dir_all(xdg.join("sheepdog")).unwrap();
    fs::write(xdg.join("sheepdog/config.json"), format!("{GLOBAL}\n")).unwrap();
    sd.xdg = Some(xdg);
    assert_eq!(sd.get_field("describe", "model"), "strong");
}

#[test]
fn step_invalid_json_names_the_file_and_fails() {
    let sd = Sd::new();
    fs::write(sd.cfg(), "{not json\n").unwrap();
    let r = sd.run(&["step", "describe"]);
    assert_ne!(r.status, 0);
    assert!(r.output.contains(&sd.cfg_s()), "{}", r.output);
}

#[test]
fn step_detached_head_with_no_branch_named_reads_the_global_file_only() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.write_branch("feat", r#"{"harnesses":{"claude":{"model":"branch"}}}"#);
    sd.git(&["switch", "-q", "--detach"]);
    assert_eq!(sd.get_field("describe", "model"), "strong");
}

#[test]
fn step_get_set_mid_rebase_the_branch_being_rebased_is_the_current_one() {
    // #179
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.write_branch("feat", r#"{"harnesses":{"claude":{"model":"branch"}}}"#);
    sd.rebase_stopped();
    assert_eq!(sd.get_field("describe", "model"), "branch");
    let r = sd.run(&["get", "harnesses.claude.model"]);
    assert_eq!(r.output, "branch");
    let r = sd.run(&["set", "harnesses.claude.model", "opus"]);
    assert_eq!(r.status, 0);
    assert_eq!(
        compact(&file_json(sd.branch_file("feat"))),
        r#"{"harnesses":{"claude":{"model":"opus"}}}"#
    );
    assert_eq!(sd.get_field("describe", "model"), "opus");
}

// ── init ────────────────────────────────────────────────────────────────────

const SKELETON: &str = r#"{"harnesses":{"claude":{},"pi":{}},"settings":{},"steps":{}}"#;

#[test]
fn init_writes_an_empty_overrides_file_and_says_where() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    let r = sd.run(&["init"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, format!("wrote {}", sd.bf("feat")));
    assert_eq!(sorted(&file_json(sd.branch_file("feat"))), SKELETON);
    assert_eq!(sd.get_field("describe", "model"), "strong");
}

#[test]
fn init_later_edits_to_the_global_file_still_reach_the_branch() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.ok(&["init"]);
    sd.ok(&["set", "--global", "harnesses.claude.model", "newer"]);
    sd.ok(&["set", "--global", "settings.verify", "make check"]);
    assert_eq!(sd.out(&["get", "harnesses.claude.model"]), "newer");
    assert_eq!(sd.out(&["get", "settings.verify"]), "make check");
}

#[test]
fn init_from_a_linked_worktree_names_the_shared_file_in_the_main_repo() {
    let mut sd = Sd::new();
    sd.write_global(GLOBAL);
    let wt = sd.tmp.join("wt");
    sd.git(&["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "wt"]);
    sd.cd("wt");
    let r = sd.run(&["init"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, format!("wrote {}", sd.bf("wt")));
}

#[test]
fn init_a_named_branch() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.git(&["branch", "other"]);
    let r = sd.run(&["init", "other"]);
    assert_eq!(r.status, 0);
    assert!(sd.branch_file("other").is_file());
    assert!(!sd.branch_file("feat").exists());
}

#[test]
fn init_refuses_to_overwrite_an_existing_branch_file() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.write_branch("feat", r#"{"harness":"pi"}"#);
    let r = sd.run(&["init"]);
    assert_ne!(r.status, 0);
    assert!(r.output.contains("exists"), "{}", r.output);
    assert_eq!(raw(&file_json(sd.branch_file("feat"))["harness"]), "pi");
}

#[test]
fn init_force_overwrites_it() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.write_branch("feat", r#"{"harness":"pi"}"#);
    let r = sd.run(&["init", "--force"]);
    assert_eq!(r.status, 0);
    assert_eq!(sorted(&file_json(sd.branch_file("feat"))), SKELETON);
}

#[test]
fn init_needs_no_global_file() {
    let sd = Sd::new();
    let r = sd.run(&["init"]);
    assert_eq!(r.status, 0);
    assert!(sd.branch_file("feat").is_file());
    assert_eq!(sd.out(&["get", "settings.remote"]), "origin");
}

#[test]
fn init_global_writes_a_skeleton_that_resolves_to_the_defaults() {
    let mut sd = Sd::new();
    sd.config = Some(sd.tmp.join("new/dir/config.json"));
    let r = sd.run(&["init", "--global"]);
    assert_eq!(r.status, 0);
    assert!(r.output.contains(&sd.cfg_s()), "{}", r.output);
    let v = file_json(sd.cfg());
    assert_eq!(keys(&v), r#"["harness","harnesses","settings","steps"]"#);
    assert_eq!(keys(&v["harnesses"]), r#"["claude","pi"]"#);
    let r = sd.run(&["step", "address"]);
    assert_eq!(r.output, r#"{"harness":"claude"}"#);
    let r = sd.run(&["get", "settings.remote"]);
    assert_eq!(r.output, "origin");
}

#[test]
fn init_global_refuses_to_overwrite_force_overwrites() {
    let sd = Sd::new();
    sd.write_global(r#"{"harness":"pi"}"#);
    let r = sd.run(&["init", "--global"]);
    assert_ne!(r.status, 0);
    assert!(r.output.contains("exists"), "{}", r.output);
    assert_eq!(raw(&file_json(sd.cfg())["harness"]), "pi");
    let r = sd.run(&["init", "--global", "--force"]);
    assert_eq!(r.status, 0);
    assert_eq!(raw(&file_json(sd.cfg())["harness"]), "claude");
}

#[test]
fn init_global_works_outside_a_repo() {
    let mut sd = Sd::new();
    sd.cd("");
    let r = sd.run(&["init", "--global"]);
    assert_eq!(r.status, 0);
    assert!(sd.cfg().is_file());
}

/// [[ $output =~ init\ +--global ]]
fn has_init_global(s: &str) -> bool {
    s.match_indices("init").any(|(i, _)| {
        let rest = &s[i + 4..];
        let t = rest.trim_start_matches(' ');
        t.len() < rest.len() && t.starts_with("--global")
    })
}

#[test]
fn help_every_verb_is_listed() {
    let sd = Sd::new();
    let r = sd.run(&["--help"]);
    assert_eq!(r.status, 0);
    assert!(has_init_global(&r.output), "{}", r.output);
    for v in ["get", "step", "files", "show"] {
        assert!(
            r.output.contains(&format!("sd config {v}")),
            "{v}: {}",
            r.output
        );
    }
    assert!(!r.output.contains("sd config setting"));
}

#[test]
fn init_a_missing_branch_fails() {
    let sd = Sd::new();
    let r = sd.run(&["init", "nope"]);
    assert_ne!(r.status, 0);
    assert!(r.output.contains("no such branch"), "{}", r.output);
}

// ── files: where the layers live ────────────────────────────────────────────

#[test]
fn files_lists_the_global_repo_and_branch_paths_marking_missing_ones() {
    let sd = Sd::new();
    let r = sd.run(&["files"]);
    assert_eq!(r.status, 0);
    assert_eq!(
        r.output,
        format!(
            "global  {}  (missing)\nrepo    {}  (missing)\nbranch  {}  (missing)",
            sd.cfg_s(),
            sd.rf(),
            sd.bf("feat")
        )
    );
}

#[test]
fn files_existing_files_are_listed_without_a_mark() {
    let sd = Sd::new();
    sd.write_global("{}");
    sd.write_repo("{}");
    sd.write_branch("feat", "{}");
    let r = sd.run(&["files"]);
    assert_eq!(
        r.output,
        format!(
            "global  {}\nrepo    {}\nbranch  {}",
            sd.cfg_s(),
            sd.rf(),
            sd.bf("feat")
        )
    );
}

#[test]
fn files_a_named_branch_and_a_slash_in_its_name() {
    let sd = Sd::new();
    let r = sd.run(&["files", "topic/sub"]);
    assert_eq!(
        r.line(3),
        format!(
            "branch  {}/repo/.git/sheepdog/topic%2Fsub.json  (missing)",
            sd.tmp.display()
        )
    );
}

#[test]
fn files_from_a_linked_worktree_the_repo_and_branch_files_are_in_the_main_repo() {
    let mut sd = Sd::new();
    let wt = sd.tmp.join("wt");
    sd.git(&["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "wt"]);
    sd.cd("wt");
    let r = sd.run(&["files"]);
    assert_eq!(r.line(2), format!("repo    {}  (missing)", sd.rf()));
    assert_eq!(r.line(3), format!("branch  {}  (missing)", sd.bf("wt")));
}

#[test]
fn files_detached_head_or_outside_a_repo_says_why_theres_no_branch_file() {
    let mut sd = Sd::new();
    sd.git(&["switch", "-q", "--detach"]);
    let r = sd.run(&["files"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.line(2), format!("repo    {}  (missing)", sd.rf()));
    assert_eq!(r.line(3), "branch  (none: detached HEAD)");
    sd.cd("");
    let r = sd.run(&["files"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.line(2), "repo    (none: not a git repository)");
    assert_eq!(r.line(3), "branch  (none: not a git repository)");
}

#[test]
fn files_mid_rebase_the_branch_being_rebaseds_file() {
    // #179
    let sd = Sd::new();
    sd.rebase_stopped();
    let r = sd.run(&["files"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.line(3), format!("branch  {}  (missing)", sd.bf("feat")));
}

#[test]
fn files_no_branch_the_global_and_repo_files() {
    let mut sd = Sd::new();
    let r = sd.run(&["files", "--no-branch"]);
    assert_eq!(
        r.output,
        format!("global  {}  (missing)\nrepo    {}  (missing)", sd.cfg_s(), sd.rf())
    );
    sd.cd("");
    let r = sd.run(&["files", "--no-branch"]);
    assert_eq!(
        r.output,
        format!("global  {}  (missing)\nrepo    (none: not a git repository)", sd.cfg_s())
    );
}

#[test]
fn files_an_explicit_branch_outside_a_repo_fails() {
    let mut sd = Sd::new();
    sd.cd("");
    let r = sd.run(&["files", "feat"]);
    assert_eq!(r.status, 1);
    assert!(r.output.contains("not a git repository"), "{}", r.output);
}

// ── show: the whole merged config ───────────────────────────────────────────

#[test]
fn show_with_no_files_the_built_in_defaults_pretty_printed() {
    let sd = Sd::new();
    let r = sd.run(&["show"]);
    assert_eq!(r.status, 0);
    let v = json(&r.output);
    assert_eq!(raw(&v["harness"]), "claude");
    assert_eq!(raw(&v["settings"]["remote"]), "origin");
    assert!(r.output.lines().count() > 1);
}

#[test]
fn show_the_branch_file_over_the_global_file_over_the_defaults_nulls_applied() {
    let sd = Sd::new();
    sd.write_global(r#"{"harnesses":{"claude":{"model":"opus"}},"settings":{"stepTimeout":900,"verify":"make"}}"#);
    sd.write_branch(
        "feat",
        r#"{"harnesses":{"claude":{"effort":"max"}},"settings":{"stepTimeout":null}}"#,
    );
    let r = sd.run(&["show"]);
    assert_eq!(r.status, 0);
    let v = json(&r.output);
    assert_eq!(
        compact(&v["harnesses"]["claude"]),
        r#"{"effort":"max","model":"opus"}"#
    );
    assert_eq!(raw(&v["settings"]["stepTimeout"]), "1800");
    assert_eq!(raw(&v["settings"]["verify"]), "make");
}

#[test]
fn show_a_named_branch_and_no_branch() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"verify":"global"}}"#);
    sd.write_branch("other", r#"{"settings":{"verify":"other"}}"#);
    sd.write_branch("feat", r#"{"settings":{"verify":"feat"}}"#);
    assert_eq!(
        raw(&json(&sd.out(&["show", "other"]))["settings"]["verify"]),
        "other"
    );
    assert_eq!(
        raw(&json(&sd.out(&["show", "--no-branch"]))["settings"]["verify"]),
        "global"
    );
}

#[test]
fn show_an_unreadable_file_fails_and_names_it() {
    let sd = Sd::new();
    fs::write(sd.cfg(), "{not json\n").unwrap();
    let r = sd.run(&["show"]);
    assert_eq!(r.status, 1);
    assert!(r.output.contains(&sd.cfg_s()), "{}", r.output);
}

// ── get: any dotted path; built-in defaults under the files ────────────────

#[test]
fn get_a_number_from_the_global_file() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"stepTimeout":900}}"#);
    let r = sd.run(&["get", "settings.stepTimeout"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "900");
}

#[test]
fn get_a_string_keeps_its_emoji_and_trailing_space() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"replyPrefix":"🤖 "}}"#);
    assert_eq!(sd.stdout(&["get", "settings.replyPrefix"]), "🤖 \n");
}

#[test]
fn get_a_list_prints_one_element_per_line() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"botLogin":["alice","bob"]}}"#);
    let r = sd.run(&["get", "settings.botLogin"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "alice\nbob");
}

#[test]
fn get_an_object_prints_as_json() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    let r = sd.run(&["get", "harnesses.pi"]);
    assert_eq!(r.status, 0);
    assert_eq!(
        r.output,
        r#"{"effort":"medium","model":"cheap","provider":"openrouter"}"#
    );
}

#[test]
fn get_json_lists_strings_and_numbers_as_json() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"botLogin":["alice","bob"]}}"#);
    assert_eq!(
        sd.out(&["get", "--json", "settings.botLogin"]),
        r#"["alice","bob"]"#
    );
    assert_eq!(
        sd.out(&["get", "--json", "settings.replyPrefix"]),
        r#""🤖 ""#
    );
    assert_eq!(sd.out(&["get", "--json", "settings.stepTimeout"]), "1800");
    assert_eq!(sd.out(&["get", "--json", "settings.botIgnore"]), "[]");
}

#[test]
fn get_the_branch_file_overrides_the_global_file() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"verify":"make test"}}"#);
    sd.write_branch("feat", r#"{"settings":{"verify":"make quick"}}"#);
    let r = sd.run(&["get", "settings.verify"]);
    assert_eq!(r.output, "make quick");
}

#[test]
fn get_a_named_branch_reads_that_branchs_file() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"verify":"make test"}}"#);
    sd.write_branch("other", r#"{"settings":{"verify":"make other"}}"#);
    let r = sd.run(&["get", "settings.verify", "other"]);
    assert_eq!(r.output, "make other");
}

#[test]
fn get_null_in_the_branch_file_skips_the_global_file_and_takes_the_built_in_default() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"stepTimeout":900}}"#);
    sd.write_branch("feat", r#"{"settings":{"stepTimeout":null}}"#);
    let r = sd.run(&["get", "settings.stepTimeout"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "1800");
}

#[test]
fn get_null_on_a_whole_block_clears_everything_under_it() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.write_branch("feat", r#"{"steps":{"address":null}}"#);
    let r = sd.run(&["get", "steps.address"]);
    assert_eq!(r.status, 3);
    assert_eq!(sd.get_field("address", "harness"), "claude");
}

#[test]
fn get_null_on_a_key_with_no_default_is_unset_exit_3() {
    let sd = Sd::new();
    sd.write_global(r#"{"harnesses":{"claude":{"model":"global"}}}"#);
    sd.write_branch("feat", r#"{"harnesses":{"claude":{"model":null}}}"#);
    let r = sd.run(&["get", "harnesses.claude.model"]);
    assert_eq!(r.status, 3);
    assert_eq!(r.output, "");
}

#[test]
fn get_a_plain_value_where_a_block_belongs_is_an_error() {
    let sd = Sd::new();
    sd.write_global(r#"{"harnesses":{"pi":"oops"}}"#);
    let r = sd.run(&["get", "harnesses.pi.model"]);
    assert_eq!(r.status, 1);
    assert!(
        r.output
            .ends_with(r#"harnesses.pi: expected an object, got "oops""#),
        "{}",
        r.output
    );
}

#[test]
fn get_a_path_set_nowhere_exits_3_with_no_output() {
    let sd = Sd::new();
    let r = sd.run(&["get", "settings.foo"]);
    assert_eq!(r.status, 3);
    assert_eq!(r.output, "");
    let r = sd.run(&["get", "settings.remote.deeper"]);
    assert_eq!(r.status, 3);
}

#[test]
fn get_git_config_sheepdog_key_is_not_read() {
    let sd = Sd::new();
    sd.git(&["config", "sheepdog.verify", "from git"]);
    sd.git(&["config", "--global", "sheepdog.remote", "upstream"]);
    let r = sd.run(&["get", "settings.verify"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "");
    assert_eq!(sd.out(&["get", "settings.remote"]), "origin");
}

#[test]
fn get_works_outside_a_repo_from_the_global_file() {
    let mut sd = Sd::new();
    sd.write_global(r#"{"settings":{"interval":120}}"#);
    sd.cd("");
    let r = sd.run(&["get", "settings.interval"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "120");
}

#[test]
fn get_a_path_that_isnt_plain_dotted_names_is_refused() {
    let sd = Sd::new();
    for p in [
        "a..b",
        ".a",
        "a.",
        "a b",
        r#"a"]|values 1|values this[""#,
        "",
        "a[0]",
    ] {
        let r = sd.run(&["get", p]);
        assert_eq!(r.status, 1, "accepted: [{p}]");
        assert!(
            r.output.contains("invalid path") || r.output.contains("Usage"),
            "[{p}]: {}",
            r.output
        );
    }
}

#[test]
fn get_invalid_json_names_the_file_and_fails() {
    let sd = Sd::new();
    fs::write(sd.cfg(), "{not json\n").unwrap();
    let r = sd.run(&["get", "settings.verify"]);
    assert_eq!(r.status, 1);
    assert!(r.output.contains(&sd.cfg_s()), "{}", r.output);
}

#[test]
fn get_a_file_that_holds_more_than_one_json_value_is_refused() {
    let sd = Sd::new();
    fs::write(sd.cfg(), "{}\n{}\n").unwrap();
    let r = sd.run(&["get", "settings.verify"]);
    assert_eq!(r.status, 1);
    assert!(r.output.contains(&sd.cfg_s()), "{}", r.output);
}

#[test]
fn get_no_path_is_a_usage_error() {
    let sd = Sd::new();
    let r = sd.run(&["get"]);
    assert_ne!(r.status, 0);
    assert!(r.output.contains("Usage"), "{}", r.output);
}

#[test]
fn setting_the_old_verb_is_gone() {
    let sd = Sd::new();
    let r = sd.run(&["setting", "verify"]);
    assert_ne!(r.status, 0);
    assert!(r.output.contains("unknown verb"), "{}", r.output);
}

// ── step --harness: a forced harness picks its own blocks ───────────────────

#[test]
fn step_harness_uses_that_harnesss_blocks_instead_of_the_configured_one() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    let r = sd.run(&["step", "--harness", "pi", "describe"]);
    assert_eq!(r.status, 0);
    assert_eq!(
        r.output,
        r#"{"harness":"pi","effort":"medium","model":"cheap","provider":"openrouter"}"#
    );
}

#[test]
fn step_harness_the_steps_block_for_that_harness_still_applies() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    let r = sd.run(&["step", "--harness", "claude", "address"]);
    assert_eq!(
        r.output,
        r#"{"harness":"claude","effort":"high","model":"strong"}"#
    );
    let r = sd.run(&["step", "--harness", "pi", "address"]);
    assert_eq!(raw(&json(&r.output)["model"]), "cheaper");
}

#[test]
fn step_harness_an_empty_value_means_not_forced() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    let r = sd.run(&["step", "--harness", "", "address"]);
    assert_eq!(raw(&json(&r.output)["harness"]), "pi");
}

#[test]
fn step_harness_a_name_that_could_carry_query_syntax_is_refused() {
    let sd = Sd::new();
    let r = sd.run(&[
        "step",
        "--harness",
        r#"x" | values 1 | values ""#,
        "describe",
    ]);
    assert_ne!(r.status, 0);
    assert!(r.output.contains("unknown harness"), "{}", r.output);
}

#[test]
fn step_harness_needs_a_value() {
    let sd = Sd::new();
    let r = sd.run(&["step", "--harness"]);
    assert_ne!(r.status, 0);
    assert!(r.output.contains("Usage"), "{}", r.output);
}

// ── get --no-branch: no branch context (the watcher) ────────────────────────

#[test]
fn get_no_branch_ignores_the_branch_file() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"verify":"global"}}"#);
    sd.write_branch("feat", r#"{"settings":{"verify":"branch"}}"#);
    let r = sd.run(&["get", "--no-branch", "settings.verify"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "global");
}

#[test]
fn get_no_branch_a_null_in_the_branch_file_does_not_unset_it() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"verify":"global"}}"#);
    sd.write_branch("feat", r#"{"settings":{"verify":null}}"#);
    let r = sd.run(&["get", "--no-branch", "settings.verify"]);
    assert_eq!(r.output, "global");
}

#[test]
fn get_no_branch_needs_a_path() {
    let sd = Sd::new();
    let r = sd.run(&["get", "--no-branch"]);
    assert_ne!(r.status, 0);
    assert!(r.output.contains("Usage"), "{}", r.output);
}

#[test]
fn get_a_config_error_exits_1_never_the_unset_code() {
    let sd = Sd::new();
    fs::write(sd.cfg(), "{not json\n").unwrap();
    assert_eq!(sd.run(&["get", "settings.verify"]).status, 1);
    assert_eq!(sd.run(&["get", "a..b"]).status, 1);
}

#[test]
fn step_an_explicit_branch_outside_a_repo_fails_instead_of_using_the_global_file_alone() {
    let mut sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.cd("");
    let r = sd.run(&["step", "describe", "somebranch"]);
    assert_ne!(r.status, 0);
    assert!(r.output.contains("not a git repository"), "{}", r.output);
    assert_eq!(sd.run(&["get", "settings.verify", "somebranch"]).status, 1);
}

#[test]
fn step_an_unreadable_global_file_fails() {
    let sd = Sd::new();
    fs::write(sd.cfg(), "{not json\n").unwrap();
    let r = sd.run(&["step", "describe"]);
    assert_ne!(r.status, 0);
    assert!(r.output.contains(&sd.cfg_s()), "{}", r.output);
}

// ── set / unset ─────────────────────────────────────────────────────────────

#[test]
fn set_writes_the_current_branchs_file_and_get_sees_it() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    let r = sd.run(&["set", "harnesses.claude.model", "opus"]);
    assert_eq!(r.status, 0);
    assert_eq!(
        compact(&file_json(sd.branch_file("feat"))),
        r#"{"harnesses":{"claude":{"model":"opus"}}}"#
    );
    assert_eq!(sd.get_field("describe", "model"), "opus");
    assert_eq!(sd.get_field("describe", "effort"), "high");
}

#[test]
fn set_global_updates_the_global_file_and_keeps_its_other_keys() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    let r = sd.run(&["set", "--global", "settings.verify", "make test"]);
    assert_eq!(r.status, 0);
    let v = file_json(sd.cfg());
    assert_eq!(raw(&v["settings"]["verify"]), "make test");
    assert_eq!(raw(&v["harnesses"]["pi"]["provider"]), "openrouter");
    assert!(!sd.branch_file("feat").exists());
}

#[test]
fn set_global_creates_the_global_file_and_works_outside_a_repo() {
    let mut sd = Sd::new();
    sd.cd("");
    let r = sd.run(&["set", "--global", "settings.interval", "120"]);
    assert_eq!(r.status, 0);
    assert_eq!(raw(&file_json(sd.cfg())["settings"]["interval"]), "120");
}

#[test]
fn set_branch_writes_that_branchs_file() {
    let sd = Sd::new();
    sd.git(&["branch", "other"]);
    let r = sd.run(&["set", "--branch", "other", "steps.address.harness", "pi"]);
    assert_eq!(r.status, 0);
    assert_eq!(
        raw(&file_json(sd.branch_file("other"))["steps"]["address"]["harness"]),
        "pi"
    );
    assert!(!sd.branch_file("feat").exists());
}

#[test]
fn set_a_missing_branch_fails() {
    let sd = Sd::new();
    let r = sd.run(&["set", "--branch", "nope", "settings.verify", "x"]);
    assert_ne!(r.status, 0);
    assert!(r.output.contains("no such branch"), "{}", r.output);
}

#[test]
fn set_values_are_strings_by_default_keeping_spaces_and_emoji() {
    let sd = Sd::new();
    sd.run(&["set", "settings.replyPrefix", "🤖 "]);
    assert_eq!(
        compact(&file_json(sd.branch_file("feat"))["settings"]["replyPrefix"]),
        r#""🤖 ""#
    );
    sd.run(&["set", "settings.stepTimeout", "900"]);
    assert_eq!(
        compact(&file_json(sd.branch_file("feat"))["settings"]["stepTimeout"]),
        r#""900""#
    );
}

#[test]
fn set_json_numbers_lists_and_null() {
    let sd = Sd::new();
    sd.ok(&["set", "--json", "settings.stepTimeout", "900"]);
    sd.ok(&["set", "--json", "settings.botLogin", r#"["a","b"]"#]);
    sd.ok(&["set", "--json", "steps.address.pi.effort", "null"]);
    assert_eq!(
        compact(&file_json(sd.branch_file("feat"))),
        r#"{"settings":{"stepTimeout":900,"botLogin":["a","b"]},"steps":{"address":{"pi":{"effort":null}}}}"#
    );
}

#[test]
fn set_json_more_than_one_value_or_one_that_adds_keys_is_refused() {
    let sd = Sd::new();
    sd.write_branch("feat", r#"{"harness":"pi"}"#);
    for v in ["1 2", r#"1, "harness": "claude""#, ""] {
        let r = sd.run(&["set", "--json", "settings.x", v]);
        assert_ne!(r.status, 0, "accepted: [{v}]");
    }
    assert_eq!(
        compact(&file_json(sd.branch_file("feat"))),
        r#"{"harness":"pi"}"#
    );
}

#[test]
fn set_a_string_value_with_quotes_backslashes_and_a_newline_round_trips() {
    let sd = Sd::new();
    let v = "say \"hi\" \\ there\nline two";
    sd.ok(&["set", "settings.verify", v]);
    assert_eq!(sd.out(&["get", "settings.verify"]), v);
}

#[test]
fn set_a_value_with_other_control_characters_is_refused() {
    let sd = Sd::new();
    let r = sd.run(&["set", "settings.verify", "a\x01b"]);
    assert_eq!(r.status, 1);
    assert!(r.output.contains("control characters"), "{}", r.output);
    assert!(!sd.branch_file("feat").exists());
}

#[test]
fn set_keeps_empty_blocks_and_the_other_keys_in_the_file() {
    let sd = Sd::new();
    sd.ok(&["init", "--global"]);
    sd.ok(&["set", "--global", "settings.verify", "make"]);
    let v = file_json(sd.cfg());
    assert_eq!(compact(&v["harnesses"]), r#"{"claude":{},"pi":{}}"#);
    assert_eq!(compact(&v["steps"]), "{}");
}

#[test]
fn set_json_invalid_json_is_refused_and_the_file_is_untouched() {
    let sd = Sd::new();
    sd.write_branch("feat", r#"{"harness":"pi"}"#);
    let r = sd.run(&["set", "--json", "settings.botLogin", "[a"]);
    assert_ne!(r.status, 0);
    assert_eq!(
        compact(&file_json(sd.branch_file("feat"))),
        r#"{"harness":"pi"}"#
    );
}

#[test]
fn set_a_null_set_this_way_clears_the_inherited_value() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.ok(&["set", "--json", "harnesses.claude.effort", "null"]);
    let r = sd.run(&["step", "describe"]);
    assert!(lacks(&json(&r.output), "effort"));
}

#[test]
fn set_a_path_that_isnt_plain_dotted_names_is_refused() {
    let sd = Sd::new();
    for p in ["a..b", ".a", "a.", "a b", "a.$x", "", "a[0]"] {
        let r = sd.run(&["set", p, "v"]);
        assert_ne!(r.status, 0, "accepted: [{p}]");
    }
    assert!(!sd.branch_file("feat").exists());
}

#[test]
fn set_through_a_value_that_isnt_an_object_is_refused_file_untouched() {
    let sd = Sd::new();
    sd.write_branch("feat", r#"{"settings":{"verify":"make"}}"#);
    let r = sd.run(&["set", "settings.verify.x", "y"]);
    assert_ne!(r.status, 0);
    assert_eq!(
        compact(&file_json(sd.branch_file("feat"))),
        r#"{"settings":{"verify":"make"}}"#
    );
}

#[test]
fn set_a_malformed_file_is_refused_not_overwritten() {
    let sd = Sd::new();
    let f = sd.branch_file("feat");
    fs::create_dir_all(f.parent().unwrap()).unwrap();
    fs::write(&f, "{not json\n").unwrap();
    let r = sd.run(&["set", "settings.verify", "x"]);
    assert_ne!(r.status, 0);
    assert_eq!(chomp(&fs::read_to_string(&f).unwrap()), "{not json");
}

#[test]
fn set_needs_a_path_and_a_value() {
    let sd = Sd::new();
    let r = sd.run(&["set", "settings.verify"]);
    assert_ne!(r.status, 0);
    assert!(r.output.contains("Usage"), "{}", r.output);
}

#[test]
fn set_options_may_follow_the_path_and_value() {
    let sd = Sd::new();
    let r = sd.run(&[
        "set",
        "--global",
        "settings.botIgnore",
        "--json",
        r#"["linear-code"]"#,
    ]);
    assert_eq!(r.status, 0);
    assert_eq!(
        compact(&file_json(sd.cfg())["settings"]["botIgnore"]),
        r#"["linear-code"]"#
    );
    let r = sd.run(&["set", "settings.verify", "make", "--branch", "feat"]);
    assert_eq!(r.status, 0);
    assert_eq!(
        raw(&file_json(sd.branch_file("feat"))["settings"]["verify"]),
        "make"
    );
}

#[test]
fn get_show_and_step_options_may_follow_the_arguments() {
    let sd = Sd::new();
    sd.ok(&["set", "--global", "--json", "settings.botLogin", r#"["a"]"#]);
    let r = sd.run(&["get", "settings.botLogin", "--json", "--no-branch"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, r#"["a"]"#);
    let r = sd.run(&["show", "feat", "--no-branch"]);
    assert_eq!(r.status, 0);
    let r = sd.run(&["step", "address", "--harness", "pi"]);
    assert_eq!(r.status, 0);
    assert_eq!(raw(&json(&r.output)["harness"]), "pi");
}

#[test]
fn a_value_starting_with_dashes_that_isnt_an_option_is_a_value() {
    let sd = Sd::new();
    assert_eq!(
        sd.run(&["set", "harnesses.claude.args", "--model sonnet"])
            .status,
        0
    );
    assert_eq!(sd.run(&["set", "harnesses.pi.args", "--verbose"]).status, 0);
    let v = file_json(sd.branch_file("feat"));
    assert_eq!(
        compact(&Value::Array(vec![
            v["harnesses"]["claude"]["args"].clone(),
            v["harnesses"]["pi"]["args"].clone()
        ])),
        r#"["--model sonnet","--verbose"]"#
    );
}

#[test]
fn double_dash_ends_options_so_a_value_can_be_an_options_name() {
    let sd = Sd::new();
    let r = sd.run(&["set", "harnesses.claude.args", "--", "--json"]);
    assert_eq!(r.status, 0);
    assert_eq!(
        raw(&file_json(sd.branch_file("feat"))["harnesses"]["claude"]["args"]),
        "--json"
    );
}

#[test]
fn an_option_the_verb_doesnt_take_is_named() {
    let sd = Sd::new();
    let r = sd.run(&["set", "settings.verify", "make", "--no-branch"]);
    assert_eq!(r.status, 1);
    assert!(
        r.output.contains("set doesn't take --no-branch"),
        "{}",
        r.output
    );
}

#[test]
fn a_wrong_argument_count_shows_that_verbs_usage_not_the_whole_help() {
    let sd = Sd::new();
    let r = sd.run(&["set", "settings.verify"]);
    assert_eq!(r.status, 1);
    assert_eq!(r.lines()[0], "sd config: set needs <path> <value>");
    assert!(
        r.output.contains("sd config set   [--global"),
        "{}",
        r.output
    );
    assert!(!r.output.contains("Layers"));
}

#[test]
fn unset_removes_the_key_so_it_inherits_again() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.write_branch(
        "feat",
        r#"{"harnesses":{"claude":{"model":"opus","effort":"low"}}}"#,
    );
    let r = sd.run(&["unset", "harnesses.claude.model"]);
    assert_eq!(r.status, 0);
    assert_eq!(
        compact(&file_json(sd.branch_file("feat"))),
        r#"{"harnesses":{"claude":{"effort":"low"}}}"#
    );
    assert_eq!(sd.get_field("describe", "model"), "strong");
}

#[test]
fn unset_global_removes_from_the_global_file() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    let r = sd.run(&["unset", "--global", "steps.review"]);
    assert_eq!(r.status, 0);
    assert_eq!(keys(&file_json(sd.cfg())["steps"]), r#"["address"]"#);
}

#[test]
fn unset_a_key_that_isnt_there_is_fine() {
    let sd = Sd::new();
    sd.write_branch("feat", "{}");
    assert_eq!(sd.run(&["unset", "settings.verify"]).status, 0);
}

#[test]
fn help_set_and_unset_are_listed() {
    let sd = Sd::new();
    let r = sd.run(&["--help"]);
    assert!(r.output.contains("sd config set"), "{}", r.output);
    assert!(r.output.contains("sd config unset"), "{}", r.output);
}

#[test]
fn set_json_a_number_and_a_list_are_values() {
    let sd = Sd::new();
    sd.ok(&["set", "--json", "settings.stepTimeout", "900"]);
    sd.ok(&["set", "--json", "settings.botLogin", r#"["marge"]"#]);
    assert_eq!(
        compact(&file_json(sd.branch_file("feat"))["settings"]),
        r#"{"stepTimeout":900,"botLogin":["marge"]}"#
    );
}

// ── validation: typed files ─────────────────────────────────────────────────

#[test]
fn validate_an_unknown_key_in_the_global_file_is_an_error_that_names_it() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"verfy":"make test"}}"#);
    let r = sd.run(&["get", "settings.verify"]);
    assert_eq!(r.status, 1);
    assert_eq!(
        r.output,
        format!("sd config: {}: settings.verfy: unknown key", sd.cfg_s())
    );
}

#[test]
fn validate_an_unknown_key_holding_an_empty_block_is_an_unknown_key() {
    let sd = Sd::new();
    sd.write_global(r#"{"settnigs":{}}"#);
    let r = sd.run(&["show"]);
    assert_eq!(r.status, 1);
    assert_eq!(
        r.output,
        format!("sd config: {}: settnigs: unknown key", sd.cfg_s())
    );
}

#[test]
fn validate_an_empty_block_under_a_misspelled_step_or_harness_is_named_by_its_path() {
    let sd = Sd::new();
    sd.write_global(r#"{"steps":{"adress":{"claude":{}}},"harnesses":{"claud":{}}}"#);
    let r = sd.run(&["show"]);
    assert_eq!(r.status, 1);
    let lines = r.lines();
    assert!(
        lines[0].ends_with(": harnesses.claud: unknown key"),
        "{}",
        r.output
    );
    assert!(
        lines[1].ends_with(": steps.adress: unknown key"),
        "{}",
        r.output
    );
    assert_eq!(lines.len(), 2);
}

#[test]
fn validate_an_empty_block_where_a_value_belongs_is_still_a_cast_error() {
    let sd = Sd::new();
    sd.write_global(r#"{"harness":{},"harnesses":{"claude":{"effort":{}}}}"#);
    let r = sd.run(&["show"]);
    assert_eq!(r.status, 1);
    let lines = r.lines();
    assert!(
        lines[0].ends_with(": harness: expected one of claude, pi, got {}"),
        "{}",
        r.output
    );
    assert!(
        lines[1].ends_with(
            ": harnesses.claude.effort: expected one of low, medium, high, xhigh, max, got {}"
        ),
        "{}",
        r.output
    );
    assert_eq!(lines.len(), 2);
}

#[test]
fn validate_known_empty_blocks_are_fine() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{},"harnesses":{"claude":{},"pi":{}},"steps":{"address":{},"review":{"pi":{}}}}"#);
    assert_eq!(sd.run(&["show"]).status, 0);
}

#[test]
fn validate_an_empty_block_in_a_branch_file_is_checked_the_same_way() {
    let sd = Sd::new();
    sd.ok(&["init"]);
    fs::write(sd.branch_file("feat"), "{\"setings\":{}}\n").unwrap();
    let r = sd.run(&["show"]);
    assert_eq!(r.status, 1);
    assert!(r.output.contains("setings: unknown key"), "{}", r.output);
}

#[test]
fn settings_step_budget_usd_per_step_5_by_default() {
    let sd = Sd::new();
    let r = sd.run(&["get", "settings.stepBudget"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "5");
    sd.ok(&["set", "--global", "--json", "settings.stepBudget", "2.5"]);
    assert_eq!(sd.out(&["get", "settings.stepBudget"]), "2.5");
    sd.write_global(r#"{"settings":{"stepBudget":"abc"}}"#);
    let r = sd.run(&["show"]);
    assert_eq!(r.status, 1);
    assert!(
        r.output
            .ends_with(r#"settings.stepBudget: expected a number of 0 or more, got "abc""#),
        "{}",
        r.output
    );
}

#[test]
fn settings_branch_budget_and_fork_limit_defaults_per_branch_typed() {
    // #121
    let sd = Sd::new();
    assert_eq!(sd.out(&["get", "settings.branchBudget"]), "25");
    assert_eq!(sd.out(&["get", "settings.forkLimit"]), "100000");
    sd.ok(&["set", "--json", "settings.branchBudget", "40.5"]);
    sd.ok(&["set", "--json", "settings.forkLimit", "50000"]);
    assert_eq!(sd.out(&["get", "settings.branchBudget"]), "40.5");
    assert_eq!(sd.out(&["get", "settings.forkLimit"]), "50000");
    sd.write_global(r#"{"settings":{"forkLimit":"lots"}}"#);
    let r = sd.run(&["show"]);
    assert_eq!(r.status, 1);
    assert!(
        r.output
            .ends_with(r#"settings.forkLimit: expected a whole number of 0 or more, got "lots""#),
        "{}",
        r.output
    );
}

#[test]
fn validate_counts_and_interval_refuse_a_fraction_other_seconds_and_the_budgets_take_one() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"reviewRounds":1.5,"interval":1.5,"stepBudget":2.5}}"#);
    let r = sd.run(&["show"]);
    assert_eq!(r.status, 1);
    let lines = r.lines();
    assert_eq!(
        lines[0],
        format!(
            "sd config: {}: settings.interval: expected a whole number of 0 or more, got 1.5",
            sd.cfg_s()
        )
    );
    assert_eq!(
        lines[1],
        format!(
            "sd config: {}: settings.reviewRounds: expected a whole number of 0 or more, got 1.5",
            sd.cfg_s()
        )
    );
    assert_eq!(lines.len(), 2);
    sd.write_global("{}");
    assert_eq!(
        sd.run(&["set", "--global", "settings.botTimeout", "0.5"])
            .status,
        0
    );
    assert_eq!(
        sd.run(&["set", "--global", "--json", "settings.stepBudget", "-1"])
            .status,
        1
    );
}

#[test]
fn settings_bot_timeout_seconds_to_wait_for_the_review_bot_3600_by_default() {
    let sd = Sd::new();
    assert_eq!(sd.out(&["get", "settings.botTimeout"]), "3600");
    sd.ok(&["set", "--global", "--json", "settings.botTimeout", "600"]);
    assert_eq!(sd.out(&["get", "settings.botTimeout"]), "600");
    sd.write_global(r#"{"settings":{"botTimeout":"soon"}}"#);
    let r = sd.run(&["show"]);
    assert_eq!(r.status, 1);
    assert!(
        r.output
            .ends_with(r#"settings.botTimeout: expected a number of 0 or more, got "soon""#),
        "{}",
        r.output
    );
}

/// A typed string setting, empty by default, then <value> from the global file.
fn string_setting_empty_by_default(key: &str, value: &str) -> Sd {
    let sd = Sd::new();
    let path = format!("settings.{key}");
    let r = sd.run(&["get", &path]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "");
    sd.write_global(&format!(r#"{{"settings":{{"{key}":"{value}"}}}}"#));
    let r = sd.run(&["get", &path]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, value);
    sd
}

#[test]
fn settings_bot_command_a_typed_string_setting_empty_by_default() {
    string_setting_empty_by_default("botCommand", "/review-bot");
}

#[test]
fn settings_autofix_a_typed_string_setting_empty_by_default() {
    string_setting_empty_by_default("autofix", "make fmt");
}

#[test]
fn settings_ci_fix_prompt_a_typed_string_setting_empty_by_default() {
    string_setting_empty_by_default("ciFixPrompt", "docs/ci-fix.md");
}

#[test]
fn settings_local_review_a_typed_string_setting_empty_by_default() {
    let sd = string_setting_empty_by_default("localReview", "/opt/review");
    sd.write_branch("feat", r#"{"settings":{"localReview":"./review"}}"#);
    assert_eq!(sd.out(&["get", "settings.localReview"]), "./review");
}

#[test]
fn settings_review_prompt_a_typed_string_setting_empty_by_default() {
    let sd = string_setting_empty_by_default("reviewPrompt", "/opt/prompt.md");
    sd.write_branch(
        "feat",
        r#"{"settings":{"reviewPrompt":".sheepdog/review-prompt.md"}}"#,
    );
    assert_eq!(
        sd.out(&["get", "settings.reviewPrompt"]),
        ".sheepdog/review-prompt.md"
    );
}

#[test]
fn settings_review_rounds_a_typed_count_3_by_default() {
    let sd = Sd::new();
    assert_eq!(sd.out(&["get", "settings.reviewRounds"]), "3");
    sd.write_branch("feat", r#"{"settings":{"reviewRounds":5}}"#);
    assert_eq!(sd.out(&["get", "settings.reviewRounds"]), "5");
    sd.write_global(r#"{"settings":{"reviewRounds":"many"}}"#);
    assert_eq!(sd.run(&["get", "settings.remote"]).status, 1);
}

#[test]
fn settings_step_prompt_typed_string_settings_empty_by_default_per_branch() {
    let sd = Sd::new();
    for k in [
        "implementPrompt",
        "reviewFixPrompt",
        "describePrompt",
        "addressPrompt",
        "resolvePrompt",
    ] {
        let path = format!("settings.{k}");
        assert_eq!(sd.out(&["get", &path]), "", "{k} not empty");
        sd.write_branch("feat", &format!(r#"{{"settings":{{"{k}":"p/{k}.md"}}}}"#));
        assert_eq!(sd.out(&["get", &path]), format!("p/{k}.md"), "{k} not set");
    }
}

#[test]
fn settings_implement_allow_a_typed_list_setting_empty_by_default() {
    let sd = Sd::new();
    let r = sd.run(&["get", "settings.implementAllow"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "");
    sd.write_global(r#"{"settings":{"implementAllow":["Bash(bats *)"]}}"#);
    assert_eq!(sd.out(&["get", "settings.implementAllow"]), "Bash(bats *)");
    sd.write_branch("feat", r#"{"settings":{"implementAllow":"Bash"}}"#);
    let r = sd.run(&["get", "settings.implementAllow"]);
    assert_eq!(r.status, 1);
    assert!(r.output.contains("settings.implementAllow"), "{}", r.output);
}

#[test]
fn settings_flags_typed_lists_with_defaults_a_files_list_replaces_only_that_one() {
    // #125
    let sd = Sd::new();
    assert!(sd
        .out(&["get", "settings.flags.suppressions"])
        .contains("shellcheck"));
    assert!(sd
        .out(&["get", "settings.flags.testFiles"])
        .contains("*.bats"));
    sd.write_global(r#"{"settings":{"flags":{"skips":[]}}}"#);
    assert_eq!(sd.out(&["get", "settings.flags.skips"]), "");
    assert!(sd
        .out(&["get", "settings.flags.suppressions"])
        .contains("shellcheck"));
    sd.write_branch("feat", r#"{"settings":{"flags":{"skips":"skip"}}}"#);
    let r = sd.run(&["get", "settings.flags.skips"]);
    assert_eq!(r.status, 1);
    assert!(r.output.contains("settings.flags.skips"), "{}", r.output);
    sd.write_branch("feat", r#"{"settings":{"flags":{"skip":[]}}}"#);
    let r = sd.run(&["get", "settings.flags.skips"]);
    assert_eq!(r.status, 1);
    assert!(
        r.output.ends_with("settings.flags.skip: unknown key"),
        "{}",
        r.output
    );
}

#[test]
fn settings_protected_paths_a_typed_list_setting_with_no_default_so_unset_reads_as_no_value() {
    // #124
    let sd = Sd::new();
    assert_eq!(sd.run(&["get", "settings.protectedPaths"]).status, 3);
    sd.write_branch("feat", r#"{"settings":{"protectedPaths":[]}}"#);
    let r = sd.run(&["get", "settings.protectedPaths"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "");
    sd.write_global(r#"{"settings":{"protectedPaths":["ci/"]}}"#);
    sd.write_branch("feat", r#"{"settings":{"protectedPaths":"ci/"}}"#);
    let r = sd.run(&["get", "settings.protectedPaths"]);
    assert_eq!(r.status, 1);
    assert!(r.output.contains("settings.protectedPaths"), "{}", r.output);
}

#[test]
fn validate_a_wrong_type_is_an_error_with_the_bad_value() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"stepTimeout":"30m"}}"#);
    let r = sd.run(&["get", "settings.remote"]);
    assert_eq!(r.status, 1);
    assert!(
        r.output
            .ends_with(r#"settings.stepTimeout: expected a number of 0 or more, got "30m""#),
        "{}",
        r.output
    );
}

#[test]
fn validate_a_bad_effort_level_in_the_branch_file_names_the_branch_file() {
    let sd = Sd::new();
    sd.write_branch("feat", r#"{"harnesses":{"claude":{"effort":"hihg"}}}"#);
    let r = sd.run(&["step", "address"]);
    assert_eq!(r.status, 1);
    let want = format!(
        r#"{}: harnesses.claude.effort: expected one of low, medium, high, xhigh, max, got "hihg""#,
        sd.bf("feat")
    );
    assert!(r.output.contains(&want), "{}", r.output);
}

#[test]
fn validate_harnesses_claude_use_api_key_is_a_bool_and_claudes_only() {
    let sd = Sd::new();
    sd.write_global(r#"{"harnesses":{"claude":{"useApiKey":true}}}"#);
    assert_eq!(sd.run(&["show"]).status, 0);
    sd.write_global(r#"{"harnesses":{"claude":{"useApiKey":"yes"},"pi":{"useApiKey":true}}}"#);
    let r = sd.run(&["show"]);
    assert_eq!(r.status, 1);
    let lines = r.lines();
    assert!(
        lines[0].ends_with(r#": harnesses.claude.useApiKey: expected true or false, got "yes""#),
        "{}",
        r.output
    );
    assert!(
        lines[1].ends_with(": harnesses.pi.useApiKey: unknown key"),
        "{}",
        r.output
    );
}

#[test]
fn validate_each_harness_takes_its_own_keys_and_effort_levels() {
    let sd = Sd::new();
    sd.write_global(
        r#"{"harnesses":{"pi":{"provider":"p","effort":"minimal"},"claude":{"provider":"p"}}}"#,
    );
    let r = sd.run(&["show"]);
    assert_eq!(r.status, 1);
    assert_eq!(
        r.output,
        format!(
            "sd config: {}: harnesses.claude.provider: unknown key",
            sd.cfg_s()
        )
    );
}

#[test]
fn validate_a_misspelled_step_or_harness_name_is_an_error() {
    let sd = Sd::new();
    sd.write_global(r#"{"steps":{"adress":{"harness":"pi"}},"harness":"claud"}"#);
    let r = sd.run(&["show"]);
    assert_eq!(r.status, 1);
    let lines = r.lines();
    assert!(
        lines[0].ends_with(r#": harness: expected one of claude, pi, got "claud""#),
        "{}",
        r.output
    );
    assert!(
        lines[1].ends_with(": steps.adress: unknown key"),
        "{}",
        r.output
    );
}

#[test]
fn validate_every_problem_is_listed_not_just_the_first() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"verfy":"x","stepTimeout":"30m","botLogin":"marge"}}"#);
    let r = sd.run(&["get", "settings.remote"]);
    assert_eq!(r.status, 1);
    assert_eq!(r.lines().len(), 3, "{}", r.output);
}

/// <key> is read from the global file and refused in a repo or branch file.
fn global_file_only(key: &str, global: &str) {
    let sd = Sd::new();
    let path = format!("settings.{key}");
    sd.write_global(global);
    let want = raw(&json(global)["settings"][key]);
    assert_eq!(sd.out(&["get", &path]), want);
    sd.write_branch("feat", &format!(r#"{{"settings":{{"{key}":5}}}}"#));
    let r = sd.run(&["get", &path]);
    assert_eq!(r.status, 1);
    assert_eq!(
        r.output,
        format!(
            "sd config: {}: settings.{key}: global file only",
            sd.bf("feat")
        )
    );
    fs::remove_file(sd.branch_file("feat")).unwrap();
    sd.write_repo(&format!(r#"{{"settings":{{"{key}":5}}}}"#));
    let r = sd.run(&["get", "--no-branch", &path]);
    assert_eq!(r.status, 1);
    assert_eq!(
        r.output,
        format!("sd config: {}: settings.{key}: global file only", sd.rf())
    );
}

#[test]
fn validate_remote_interval_and_notify_are_refused_in_a_branch_file() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"remote":"upstream","interval":60,"notify":"say"}}"#);
    assert_eq!(sd.out(&["get", "settings.remote"]), "upstream");
    sd.write_branch("feat", r#"{"settings":{"remote":"fork"}}"#);
    let r = sd.run(&["get", "settings.remote"]);
    assert_eq!(r.status, 1);
    assert_eq!(
        r.output,
        format!(
            "sd config: {}: settings.remote: global or repo file only",
            sd.bf("feat")
        )
    );
}

#[test]
fn validate_interval_and_notify_can_only_be_set_in_the_global_file() {
    global_file_only("interval", r#"{"settings":{"interval":60}}"#);
    global_file_only("notify", r#"{"settings":{"notify":"say"}}"#);
}

#[test]
fn validate_vacuum_after_can_only_be_set_in_the_global_file_as_whole_seconds() {
    global_file_only("vacuumAfter", r#"{"settings":{"vacuumAfter":60}}"#);
}

#[test]
fn validate_rate_limit_floor_can_only_be_set_in_the_global_file_as_whole_points() {
    global_file_only("rateLimitFloor", r#"{"settings":{"rateLimitFloor":50}}"#);
}

#[test]
fn validate_null_is_allowed_anywhere_to_clear_an_inherited_key() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.write_branch(
        "feat",
        r#"{"harness":null,"steps":{"address":null},"settings":{"verify":null},"harnesses":{"pi":{"effort":null}}}"#,
    );
    assert_eq!(sd.run(&["show"]).status, 0);
}

#[test]
fn validate_the_built_in_defaults_pass_as_a_global_file() {
    let sd = Sd::new();
    let defaults = sd.ok(&["show", "--no-branch"]);
    fs::write(sd.cfg(), defaults).unwrap();
    assert_eq!(sd.run(&["show"]).status, 0);
}

#[test]
fn validate_set_refuses_an_unknown_key_and_leaves_the_file_alone() {
    let sd = Sd::new();
    sd.write_branch("feat", r#"{"settings":{"verify":"make"}}"#);
    let r = sd.run(&["set", "settings.verfy", "make test"]);
    assert_eq!(r.status, 1);
    assert!(
        r.output.contains("settings.verfy: unknown key"),
        "{}",
        r.output
    );
    assert_eq!(
        compact(&file_json(sd.branch_file("feat"))),
        r#"{"settings":{"verify":"make"}}"#
    );
    let r = sd.run(&["set", "--global", "settings.verfy", "x"]);
    assert_eq!(r.status, 1);
    assert!(!sd.cfg().exists());
}

#[test]
fn validate_set_refuses_a_global_only_key_in_a_branch_file() {
    let sd = Sd::new();
    let r = sd.run(&["set", "settings.remote", "fork"]);
    assert_eq!(r.status, 1);
    assert!(
        r.output.contains("settings.remote: global or repo file only"),
        "{}",
        r.output
    );
    assert!(!sd.branch_file("feat").exists());
    sd.ok(&["set", "--global", "settings.remote", "upstream"]);
    assert_eq!(sd.out(&["get", "settings.remote"]), "upstream");
}

#[test]
fn validate_unset_can_remove_a_bad_key_from_an_invalid_file() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"verfy":"x","verify":"make"}}"#);
    assert_eq!(sd.run(&["unset", "--global", "settings.verfy"]).status, 0);
    assert_eq!(sd.out(&["get", "settings.verify"]), "make");
}

#[test]
fn validate_step_refuses_a_step_name_it_doesnt_know() {
    let sd = Sd::new();
    let r = sd.run(&["step", "adress"]);
    assert_eq!(r.status, 1);
    assert!(r.output.contains("unknown step: adress"), "{}", r.output);
}

// ── built-in defaults ───────────────────────────────────────────────────────

#[test]
fn defaults_every_pipeline_setting_has_one_when_nothing_is_set() {
    let sd = Sd::new();
    assert_eq!(sd.out(&["get", "settings.remote"]), "origin");
    assert_eq!(sd.out(&["get", "settings.stepTimeout"]), "1800");
    assert_eq!(sd.out(&["get", "settings.interval"]), "300");
    assert_eq!(sd.out(&["get", "settings.vacuumAfter"]), "604800");
    assert_eq!(sd.out(&["get", "settings.rateLimitFloor"]), "250");
    assert_eq!(sd.stdout(&["get", "settings.replyPrefix"]), "🤖 \n");
}

#[test]
fn defaults_verify_notify_and_the_bot_lists_default_to_nothing_exit_0() {
    let sd = Sd::new();
    for k in ["verify", "notify", "botLogin", "botIgnore", "botIgnoreBody"] {
        let r = sd.run(&["get", &format!("settings.{k}")]);
        assert_eq!(r.status, 0, "{k}");
        assert_eq!(r.output, "", "{k}");
    }
}

#[test]
fn defaults_an_empty_list_in_a_file_is_a_value() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"botLogin":["marge"]}}"#);
    sd.write_branch("feat", r#"{"settings":{"botLogin":[]}}"#);
    let r = sd.run(&["get", "settings.botLogin"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "");
}

#[test]
fn defaults_steps_harness_default_is_claude_and_a_null_harness_falls_back_to_it() {
    let sd = Sd::new();
    assert_eq!(
        sd.run(&["step", "address"]).output,
        r#"{"harness":"claude"}"#
    );
    sd.write_global(r#"{"harness":"pi"}"#);
    sd.write_branch("feat", r#"{"harness":null}"#);
    assert_eq!(sd.get_field("address", "harness"), "claude");
}

#[test]
fn step_git_config_sheepdog_harness_args_is_not_read() {
    let sd = Sd::new();
    sd.git(&["config", "sheepdog.claude.args", "--max-turns 3"]);
    assert_eq!(
        sd.run(&["step", "describe"]).output,
        r#"{"harness":"claude"}"#
    );
}

#[test]
fn step_args_come_from_the_files_like_any_other_field() {
    let sd = Sd::new();
    sd.write_global(r#"{"harnesses":{"claude":{"args":"--global"}}}"#);
    sd.write_branch(
        "feat",
        r#"{"steps":{"describe":{"claude":{"args":"--max-turns 3"}}}}"#,
    );
    assert_eq!(sd.get_field("describe", "args"), "--max-turns 3");
}

// ── repo: the layer between global and branch (#194) ────────────────────────

#[test]
fn repo_set_repo_writes_the_main_repos_sheepdog_json_and_every_branch_reads_it() {
    let sd = Sd::new();
    sd.git(&["branch", "other"]);
    let r = sd.run(&["set", "--repo", "settings.verify", "make repo"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(
        compact(&file_json(sd.repo_file())),
        r#"{"settings":{"verify":"make repo"}}"#
    );
    assert!(!sd.branch_file("feat").exists());
    assert_eq!(sd.out(&["get", "settings.verify"]), "make repo");
    assert_eq!(sd.out(&["get", "settings.verify", "other"]), "make repo");
    sd.git(&["switch", "-q", "--detach"]);
    assert_eq!(sd.out(&["get", "settings.verify"]), "make repo");
}

#[test]
fn repo_a_linked_worktree_reads_and_writes_the_main_repos_file() {
    let mut sd = Sd::new();
    sd.write_repo(r#"{"settings":{"verify":"make repo"}}"#);
    let wt = sd.tmp.join("wt");
    sd.git(&["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "wt"]);
    sd.cd("wt");
    assert_eq!(sd.out(&["get", "settings.verify"]), "make repo");
    sd.ok(&["set", "--repo", "settings.autofix", "make fmt"]);
    assert_eq!(
        compact(&file_json(sd.repo_file())),
        r#"{"settings":{"verify":"make repo","autofix":"make fmt"}}"#
    );
    assert!(!sd.tmp.join("repo/.git/worktrees/wt/sheepdog.json").exists());
}

#[test]
fn repo_branch_over_repo_over_global_over_defaults() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"stepTimeout":100}}"#);
    sd.write_repo(r#"{"settings":{"stepTimeout":200}}"#);
    sd.write_branch("feat", r#"{"settings":{"stepTimeout":300}}"#);
    assert_eq!(sd.out(&["get", "settings.stepTimeout"]), "300");
    sd.ok(&["unset", "settings.stepTimeout"]);
    assert_eq!(sd.out(&["get", "settings.stepTimeout"]), "200");
    sd.ok(&["unset", "--repo", "settings.stepTimeout"]);
    assert_eq!(compact(&file_json(sd.repo_file())), r#"{"settings":{}}"#);
    assert_eq!(sd.out(&["get", "settings.stepTimeout"]), "100");
    sd.ok(&["unset", "--global", "settings.stepTimeout"]);
    assert_eq!(sd.out(&["get", "settings.stepTimeout"]), "1800");
}

#[test]
fn repo_null_in_the_repo_file_clears_the_global_value_and_in_the_branch_file_the_repos() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"stepTimeout":100,"verify":"make g"}}"#);
    sd.write_repo(r#"{"settings":{"stepTimeout":null,"verify":"make r"}}"#);
    assert_eq!(sd.out(&["get", "settings.stepTimeout"]), "1800");
    assert_eq!(sd.out(&["get", "settings.verify"]), "make r");
    sd.write_branch("feat", r#"{"settings":{"verify":null}}"#);
    let r = sd.run(&["get", "settings.verify"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "");
}

#[test]
fn repo_show_merges_the_repo_file_between_global_and_branch() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"verify":"g","autofix":"g","botTimeout":1}}"#);
    sd.write_repo(r#"{"settings":{"verify":"r","autofix":"r"}}"#);
    sd.write_branch("feat", r#"{"settings":{"verify":"b"}}"#);
    let v = json(&sd.ok(&["show"]));
    assert_eq!(raw(&v["settings"]["verify"]), "b");
    assert_eq!(raw(&v["settings"]["autofix"]), "r");
    assert_eq!(raw(&v["settings"]["botTimeout"]), "1");
}

#[test]
fn repo_the_harness_and_step_blocks_come_from_the_repo_file_too() {
    let sd = Sd::new();
    sd.write_global(GLOBAL);
    sd.write_repo(
        r#"{"steps":{"describe":{"claude":{"model":"from-repo"}},"review":{"harness":"pi"}}}"#,
    );
    assert_eq!(sd.get_field("describe", "model"), "from-repo");
    assert_eq!(sd.get_field("describe", "effort"), "high");
    assert_eq!(sd.get_field("review", "harness"), "pi");
    sd.write_branch(
        "feat",
        r#"{"steps":{"describe":{"claude":{"model":"from-branch"}}}}"#,
    );
    assert_eq!(sd.get_field("describe", "model"), "from-branch");
}

#[test]
fn repo_no_branch_still_reads_the_repo_file_inside_a_repo_and_only_the_global_outside() {
    let mut sd = Sd::new();
    sd.write_global(r#"{"settings":{"remote":"upstream","verify":"g"}}"#);
    sd.write_repo(r#"{"settings":{"remote":"fork"}}"#);
    sd.write_branch("feat", r#"{"settings":{"verify":"b"}}"#);
    assert_eq!(sd.out(&["get", "--no-branch", "settings.remote"]), "fork");
    assert_eq!(sd.out(&["get", "--no-branch", "settings.verify"]), "g");
    let v = json(&sd.ok(&["show", "--no-branch"]));
    assert_eq!(raw(&v["settings"]["remote"]), "fork");
    sd.cd("");
    assert_eq!(sd.out(&["get", "--no-branch", "settings.remote"]), "upstream");
    assert_eq!(sd.out(&["get", "settings.remote"]), "upstream");
}

#[test]
fn repo_settings_never_leak_into_another_repository() {
    let mut sd = Sd::new();
    sd.write_repo(r#"{"settings":{"remote":"fork","verify":"make repo"}}"#);
    sd.cd("");
    sd.git(&["init", "-q", "two"]);
    sd.cd("two");
    sd.git(&["commit", "-q", "--allow-empty", "-m", "init"]);
    sd.git(&["switch", "-qc", "feat"]);
    assert_eq!(sd.out(&["get", "--no-branch", "settings.remote"]), "origin");
    assert_eq!(sd.out(&["get", "settings.remote"]), "origin");
    assert_eq!(sd.run(&["get", "settings.verify"]).output, "");
    sd.ok(&["set", "--repo", "settings.remote", "mine"]);
    assert!(sd.tmp.join("two/.git/sheepdog.json").exists());
    assert_eq!(sd.out(&["get", "--no-branch", "settings.remote"]), "mine");
    sd.cd("repo");
    assert_eq!(sd.out(&["get", "--no-branch", "settings.remote"]), "fork");
}

#[test]
fn repo_remote_may_be_set_in_the_repo_file_and_beats_the_global_one() {
    let sd = Sd::new();
    sd.write_global(r#"{"settings":{"remote":"upstream"}}"#);
    sd.ok(&["set", "--repo", "settings.remote", "fork"]);
    assert_eq!(sd.out(&["get", "settings.remote"]), "fork");
    let r = sd.run(&["set", "settings.remote", "x"]);
    assert_eq!(r.status, 1);
    assert_eq!(
        r.output,
        format!(
            "sd config: {}: settings.remote: global or repo file only",
            sd.bf("feat")
        )
    );
    assert!(!sd.branch_file("feat").exists());
}

#[test]
fn repo_set_repo_refuses_a_global_only_key_and_leaves_the_file_alone() {
    let sd = Sd::new();
    sd.write_repo(r#"{"settings":{"verify":"make"}}"#);
    for key in ["interval", "notify", "vacuumAfter", "rateLimitFloor"] {
        let r = sd.run(&["set", "--repo", &format!("settings.{key}"), "60"]);
        assert_eq!(r.status, 1, "{key}");
        assert_eq!(
            r.output,
            format!("sd config: {}: settings.{key}: global file only", sd.rf())
        );
    }
    assert_eq!(
        compact(&file_json(sd.repo_file())),
        r#"{"settings":{"verify":"make"}}"#
    );
}

#[test]
fn repo_the_repo_file_takes_everything_a_branch_file_takes() {
    let sd = Sd::new();
    let doc = r#"{"harness":"pi",
      "harnesses":{"claude":{"model":"m","effort":"low","args":"","useApiKey":true},
                   "pi":{"provider":"p","model":"m","effort":"off","args":""}},
      "steps":{"address":{"harness":"claude","claude":{"effort":"max"},"pi":{}},
               "ci-fix":{},"describe":{},"implement":{},"review":{}},
      "settings":{"stepTimeout":60,"stepBudget":1,"branchBudget":2,"forkLimit":3,"reviewRounds":1,
        "replyPrefix":"x","verify":"v","autofix":"a","localReview":"l","botCommand":"c","botTimeout":5,
        "reviewPrompt":"p","implementPrompt":"p","reviewFixPrompt":"p","describePrompt":"p",
        "addressPrompt":"p","resolvePrompt":"p","ciFixPrompt":"p","botLogin":["b"],"botIgnore":[],
        "botIgnoreBody":[],"implementAllow":[],"protectedPaths":[],
        "flags":{"suppressions":[],"skips":[],"testFiles":[]}}}"#;
    sd.write_branch("feat", doc);
    let r = sd.run(&["show"]);
    assert_eq!(r.status, 0, "{}", r.output);
    fs::remove_file(sd.branch_file("feat")).unwrap();
    sd.write_repo(doc);
    let r = sd.run(&["show"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(sd.out(&["get", "settings.describePrompt"]), "p");
    assert_eq!(sd.get_field("address", "effort"), "max");
}

#[test]
fn repo_a_bad_value_or_unknown_key_in_the_repo_file_names_the_repo_file() {
    let sd = Sd::new();
    sd.write_repo(r#"{"harness":"claud","settings":{"verfy":"x"}}"#);
    let r = sd.run(&["get", "--no-branch", "settings.verify"]);
    assert_eq!(r.status, 1);
    assert_eq!(
        r.output,
        format!(
            "sd config: {f}: harness: expected one of claude, pi, got \"claud\"\n\
             sd config: {f}: settings.verfy: unknown key",
            f = sd.rf()
        )
    );
    fs::write(sd.repo_file(), "[]\n").unwrap();
    let r = sd.run(&["step", "address"]);
    assert_eq!(r.status, 1);
    assert_eq!(r.output, format!("sd config: not a JSON object: {}", sd.rf()));
}

#[test]
fn repo_set_and_unset_refuse_more_than_one_of_global_repo_and_branch() {
    let sd = Sd::new();
    for flags in [
        &["--global", "--repo"][..],
        &["--repo", "--branch", "feat"],
        &["--global", "--branch", "feat"],
        &["--global", "--repo", "--branch", "feat"],
    ] {
        for verb in ["set", "unset"] {
            let mut args = vec![verb];
            args.extend_from_slice(flags);
            args.push("settings.verify");
            if verb == "set" {
                args.push("x");
            }
            let r = sd.run(&args);
            assert_eq!(r.status, 1, "{args:?}");
            assert_eq!(
                r.output,
                format!("sd config: {verb} takes at most one of --global, --repo and --branch")
            );
        }
    }
    assert!(!sd.cfg().exists());
    assert!(!sd.repo_file().exists());
    assert!(!sd.branch_file("feat").exists());
}

#[test]
fn repo_set_repo_outside_a_repo_fails() {
    let mut sd = Sd::new();
    sd.cd("");
    let r = sd.run(&["set", "--repo", "settings.verify", "x"]);
    assert_eq!(r.status, 1);
    assert_eq!(r.output, "sd config: not a git repository");
}

#[test]
fn repo_only_set_and_unset_take_repo() {
    let sd = Sd::new();
    for verb in ["get", "show", "files", "step", "init"] {
        let r = sd.run(&[verb, "--repo", "x"]);
        assert_eq!(r.status, 1, "{verb}");
        assert_eq!(r.output, format!("sd config: {verb} doesn't take --repo"));
    }
}

#[test]
fn repo_help_documents_the_layer_its_path_and_precedence() {
    let sd = Sd::new();
    let help = sd.ok(&["--help"]);
    assert!(help.contains("sd config set   [--global | --repo | --branch <b>]"), "{help}");
    assert!(help.contains("sd config unset [--global | --repo | --branch <b>]"), "{help}");
    let layer = |name: &str| {
        help.lines()
            .position(|l| l.starts_with(&format!("  {name} ")))
            .unwrap_or_else(|| panic!("no {name} layer: {help}"))
    };
    assert!(layer("branch") < layer("repo"));
    assert!(layer("repo") < layer("global"));
    assert!(layer("global") < layer("defaults"));
    assert!(
        help.lines().nth(layer("repo")).unwrap().contains("<main repo>/.git/sheepdog.json"),
        "{help}"
    );
}
