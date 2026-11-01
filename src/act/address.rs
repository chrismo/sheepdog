// address (J3, J4): the owed bot feedback, handed to the agent in the
// branch's session; the pipeline commits its edits, runs verify, pushes, then
// posts its replies and resolves the threads.

use super::commands::{command, status, verify_autofix};
use super::hold::{hold_gate, hold_since, pr_gate};
use super::implement::blank;
use super::push::push_branch;
use super::step::{agent_rules, answer_cmd, answer_prompt, answer_store, answer_waits, commit_edits, give_up, guidance_base,
                  question_answered, question_ask, question_gate, question_say, going_on, step_session, step_store, with_stored_answer};
use super::gates::{self, addr};
use super::{cfg, die, enforce, lock_recheck, state_json, Args, Gate, Go};
use crate::git::{git, rev, worktree_for};
use crate::guidance::{lib_dir, step_guidance};
use crate::harness::{self, Ran, Step, PI_TOOLS};
use crate::next::{round_key, yes, Derived};
use crate::state::unbot;
use crate::{decisions, proc, running};
use crate::val::{at, sj, t, text};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::sync::OnceLock;

const NODES_Q: &str = "query($ids: [ID!]!) {
  viewer { login }
  nodes(ids: $ids) {
    __typename
    ... on PullRequestReviewThread { id path line isOutdated
      comments(first: 50) { nodes { author { __typename login } body } } }
    ... on PullRequestReview { id url author { __typename login } body }
    ... on IssueComment { id url author { __typename login } body }
  }
}";

const SCHEMA: &str = r#"{"type": "object", "required": ["outcome", "commit_message", "summary", "items"],
 "properties": {
   "outcome": {"enum": ["done", "needs-input", "gave-up"]},
   "commit_message": {"type": ["string", "null"]},
   "summary": {"type": "string"},
   "items": {"type": "array", "items": {"type": "object", "required": ["id", "action", "reply"],
     "properties": {"id": {"type": "string"},
                    "action": {"enum": ["fixed", "reply", "pushback", "ask"]},
                    "reply": {"type": "string"}}}}}}"#;

// The address step's prompt: guidance (settings.addressPrompt, else
// .sheepdog/address.md in the base commit, else lib/prompts/address.md), then
// this contract, which the schema, the posting and the pipeline rely on.
const ADDRESS_CONTRACT: &str = r#"For each finding below, pick exactly one action:
- fixed:    you changed code to address it
- reply:    no change needed (already handled, or a question you can answer)
- pushback: you disagree; say why
- ask:      it needs a decision from the PR's author; say what the question is

Do not commit, push, or run gh: the pipeline commits your edits, runs checks,
pushes, and posts each `reply` on the PR. A /word or @login in a reply is posted
with a lookalike glyph so it can't trigger a bot or ping anyone; for a path,
use a relative one. Return one item per finding id, a one-line commit_message
if you changed anything (else null), and outcome "done", "needs-input"
(anything you marked ask) or "gave-up" (you couldn't make sense of it; nothing
is posted)."#;

const ACTIONS: [&str; 4] = ["fixed", "reply", "pushback", "ask"];

// An answer that can't reach the agent fails, so NEXT's question step
// doesn't hand it to a loop again.
const UNANSWERED: &str = ". The answer didn't reach the agent; sd act ack --question drops its question";

/// <why>, one of gates::address's reasons, in record <s>'s words.
fn worded(why: &str, s: &Value) -> String {
    let b = t(s, "branch");
    match why {
        addr::NO_PR => format!("no PR for {b}"),
        addr::CLOSED => format!("PR #{} is {}", t(s, "pr.number"), text(at(s, "pr.state")).to_lowercase()),
        addr::UNREAD => match sj(at(s, "errors"), "; ") {
            e if e.is_empty() => why.into(),
            e => format!("{why}: {e}"),
        },
        addr::NOTHING => {
            let human = if at(s, "threads.owed_human").is_null() { "0".into() } else { t(s, "threads.owed_human") };
            format!("nothing to address on {b} ({human} human item(s) are chrismo's)")
        }
        addr::LOCKED => format!("taken over (branch.{b}.sd-lock is set)"),
        addr::SYNC_UNKNOWN if at(s, "sync.target").is_null() => "sync with the parent unknown".into(),
        addr::SYNC_UNKNOWN => format!("sync with {} unknown", t(s, "sync.target")),
        addr::REMOTE_MORE => format!("the remote branch has {} commit(s) you don't", t(s, "push.behind")),
        addr::UNPUSHED if at(s, "push.remote_sha").is_null() => "not pushed".into(),
        addr::UNPUSHED => format!("{} unpushed commit(s)", t(s, "push.ahead")),
        addr::BEHIND => format!("behind {} by {}", t(s, "sync.target"), t(s, "sync.behind")),
        _ => why.into(),
    }
}

/// address's gate <g>, in record <s>'s words; NEXT says them too.
pub fn said(g: Gate, s: &Value) -> Gate {
    let all = |ws: Vec<String>| ws.iter().map(|w| worded(w, s)).collect();
    match g {
        Gate::Hard(why) => Gate::Hard(worded(&why, s)),
        Gate::Done(why) => Gate::Done(worded(&why, s)),
        Gate::Soft(ws) => Gate::Soft(all(ws)),
        Gate::Warn(ws) => Gate::Warn(all(ws)),
        Gate::Go => Gate::Go,
    }
}

/// The soft reasons <why> nothing sd watch runs clears: all but the last,
/// the push's, when NEXT offers the push that clears it.
fn yours(why: &[String], d: &Derived, s: &Value) -> Vec<String> {
    let pushed = usize::from(gates::address_pushes(d) && crate::next::push_offered(s));
    why[..why.len() - pushed.min(why.len())].to_vec()
}

/// address's gate on <s>, as <a> asks: Go, or None when there's nothing to
/// address or chrismo's answer is stored to run once a soft reason clears.
fn gated(b: &str, s: &Value, a: &Args) -> Option<Go> {
    let answering = !a.answer.is_empty();
    let d = Derived::new(s);
    match said(gates::address(&d), s) {
        Gate::Done(line) if answering => die(format!("{line}{UNANSWERED}")),
        // not for a restack: it moves HEAD, dropping the question (#326)
        Gate::Soft(why) if answering && !a.force && !yes(d.sync_conflict) => {
            answer_store(b, "address", a);
            println!("{}", answer_waits(b, "address", &why.join("; "), "that clears", &yours(&why, &d, s), a));
            None
        }
        Gate::Soft(why) if answering && !a.force => {
            eprintln!("sd act: not addressing {b}:");
            for w in &why {
                eprintln!("  - {w}");
            }
            if a.stored {
                eprintln!("a restack would drop the question: sd act address --force runs your stored answer now, or sd act ack --question drops both");
            } else {
                eprintln!("a restack would drop the question, so the answer can't wait for one: sd act answer --force answers now");
            }
            eprintln!("(--force to run anyway)");
            std::process::exit(1)
        }
        Gate::Soft(why) if a.force => {
            let warn = gates::address_warnings(&d).iter().map(|w| worded(w, s)).collect();
            enforce(Gate::Soft([why, warn].concat()), true, &format!("addressing {b}"))
        }
        g => enforce(g, a.force, &format!("addressing {b}")),
    }
}

/// gh api graphql with these -f fields: its answer, or None (gh said why).
fn gql(fields: &[(&str, &str)]) -> Option<String> {
    let fields: Vec<String> = fields.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let mut args = vec!["api", "graphql"];
    for f in &fields {
        args.extend(["-f", f.as_str()]);
    }
    let o = proc::run("gh", &args, None, None);
    eprint!("{}", o.stderr);
    o.ok.then_some(o.stdout)
}

/// The owed items' text, from the nodes(ids:) answer <n>, each with who else
/// is in it. Anyone but a bot, the viewer or a settings.botLogin user is
/// untrusted: their text would steer edits that verify runs and the push
/// publishes.
fn findings(n: &Value, bot_logins: &[String]) -> Vec<(Map<String, Value>, Vec<String>)> {
    let me = at(n, "data.viewer.login");
    let trusted = |c: &Value| {
        let author = at(c, "author.login");
        t(c, "author.__typename") == "Bot"
            || (!author.is_null() && author == me)
            || author.as_str().is_some_and(|a| bot_logins.iter().any(|b| unbot(b) == unbot(a)))
    };
    let finding = |x: &Value| {
        let ty = match t(x, "__typename").as_str() {
            "PullRequestReviewThread" => json!("thread"),
            "PullRequestReview" => json!("review"),
            "IssueComment" => json!("comment"),
            _ => Value::Null,
        };
        let said = at(x, "comments.nodes").as_array().map_or(vec![x], |cs| cs.iter().collect());
        let mut untrusted: Vec<String> = said.iter().filter(|c| !trusted(c)).map(|c| t(c, "author.login")).collect();
        untrusted.sort();
        untrusted.dedup();
        let conversation: Vec<Value> = said.iter().map(|c| json!({"author": at(c, "author.login"), "body": at(c, "body")})).collect();
        let f = json!({
            "id": at(x, "id"), "type": ty, "url": at(x, "url"), "path": at(x, "path"), "line": at(x, "line"),
            "outdated": at(x, "isOutdated"), "conversation": conversation,
        });
        (f.as_object().cloned().unwrap_or_default(), untrusted)
    };
    at(n, "data.nodes").as_array().into_iter().flatten().filter(|x| !x.is_null()).map(finding).collect()
}

fn with(f: &Map<String, Value>, action: &str, reply: String) -> Map<String, Value> {
    let mut f = f.clone();
    f.insert("action".into(), Value::String(action.into()));
    f.insert("reply".into(), Value::String(reply));
    f
}

/// What sharing a finding with someone untrusted makes it: chrismo's.
fn held(f: &Map<String, Value>, untrusted: &[String]) -> Map<String, Value> {
    with(f, "ask", format!("joined by {}: yours to answer", untrusted.join(", ")))
}

/// The findings with the agent's answers, then the <held> ones. A finding
/// takes its item's action and reply; no answer, no reply text, or fixed
/// with nothing <changed> is ask, so chrismo sees it.
fn plan(fs: &[Map<String, Value>], result: &Value, changed: bool, held: &[Map<String, Value>]) -> Vec<Map<String, Value>> {
    let no_items = vec![];
    let items = at(result, "items").as_array().unwrap_or(&no_items);
    let answer = |f: &Map<String, Value>| {
        let a = items.iter().find(|i| f.get("id").is_some_and(|id| !id.is_null() && at(i, "id") == id)).unwrap_or(&Value::Null);
        let act = at(a, "action").as_str().unwrap_or("");
        let (action, reply) = match at(a, "reply").as_str() {
            _ if !ACTIONS.contains(&act) => ("ask", "(the agent did not answer this one)".to_string()),
            Some(rep) if !blank(rep) => match act {
                "fixed" if !changed => ("ask", format!("(said fixed but changed nothing): {rep}")),
                _ => (act, rep.to_string()),
            },
            _ => ("ask", "(the agent gave no reply text)".to_string()),
        };
        with(f, action, reply)
    };
    fs.iter().map(answer).chain(held.iter().cloned()).collect()
}

/// A reply is posted as written, so a /command or @mention in it would run a
/// bot or ping a person. Lookalike glyphs keep the reading: a /command
/// written as a word (a /word at a line start or after whitespace, a bracket
/// or a quote, not followed by more path) gets U+2215 for its slash, which
/// defeats the line-start regexes and whitespace-delimited substring checks
/// bots use; an @login after anything but a letter, a digit or a slash gets
/// U+FF20, which GitHub doesn't read as a mention (t@e.com and
/// medium.com/@who stay; _@who_ is scrubbed, since markdown runs before
/// mention detection). Not covered: a /word glued to other text (foo/marge,
/// **/marge**, a URL ending in /marge), which stays as written, and a
/// one-segment absolute path (/tmp, /README.md), which reads as a command,
/// code spans included (so does an @ in code, like `npm i @types/node`), and
/// an @ right after a slash outside a URL (a/@who), which GitHub may still
/// read as a mention (unverified). Two passes, because a match eats the
/// character after the command, which may be the boundary of the next one
/// ("/a /b").
fn scrub(s: &str) -> String {
    static CMD: OnceLock<Regex> = OnceLock::new();
    static AT: OnceLock<Regex> = OnceLock::new();
    let cmd = CMD.get_or_init(|| {
        Regex::new(r#"(^|[\t\n\x0C\r ("'`\[])/([A-Za-z][A-Za-z0-9_-]*)($|[^A-Za-z0-9_/-])"#).expect("a fixed pattern compiles")
    });
    let at = AT.get_or_init(|| Regex::new(r"(^|[^A-Za-z0-9/])@([A-Za-z0-9])").expect("a fixed pattern compiles"));
    let once = cmd.replace_all(s, "${1}∕${2}${3}");
    let twice = cmd.replace_all(&once, "${1}∕${2}${3}");
    at.replace_all(&twice, "${1}＠${2}").into_owned()
}

/// Ask items stay as the agent wrote them, since chrismo reads those; the
/// terminal report shows the posted text, glyphs included, on purpose.
fn scrubbed(plan: Vec<Map<String, Value>>) -> Vec<Map<String, Value>> {
    let scrub_one = |mut it: Map<String, Value>| {
        if !is_ask(&it) {
            let r = scrub(&field(&it, "reply"));
            it.insert("reply".into(), Value::String(r));
        }
        it
    };
    plan.into_iter().map(scrub_one).collect()
}

fn field(it: &Map<String, Value>, k: &str) -> String {
    it.get(k).map_or(String::new(), text)
}

fn place(it: &Map<String, Value>) -> String {
    match field(it, "type").as_str() {
        "thread" => format!("{}:{}", field(it, "path"), field(it, "line")),
        "review" => "review body".into(),
        _ => "PR comment".into(),
    }
}

fn is_ask(it: &Map<String, Value>) -> bool {
    field(it, "action") == "ask"
}

/// The plan, as printed: the agent's answers, then the ask items under
/// "needs chrismo:" when there are any.
fn report(plan: &[Map<String, Value>]) -> Vec<String> {
    let (asks, rest): (Vec<_>, Vec<_>) = plan.iter().partition(|it| is_ask(it));
    let mut lines: Vec<String> =
        rest.iter().map(|it| format!("  {:<9} {:<20} {}", field(it, "action"), place(it), field(it, "reply"))).collect();
    if !asks.is_empty() {
        lines.push("needs chrismo:".into());
    }
    lines.extend(asks.iter().map(|it| format!("  {:<20} {}", place(it), field(it, "reply"))));
    lines
}

fn print_report(b: &str, summary: &str, lines: &[String]) {
    println!("{b}: {summary}");
    for l in lines {
        println!("{l}");
    }
}

/// Review bodies and PR comments can't be replied to or resolved: the ones
/// answered go in one PR comment.
fn in_comment(it: &Map<String, Value>) -> bool {
    !is_ask(it) && matches!(field(it, "type").as_str(), "review" | "comment")
}

/// The PR comment. Link, don't quote: quoted text can carry what other bots
/// act on (/commands, @mentions).
fn pr_comment(prefix: &str, plan: &[Map<String, Value>]) -> Option<String> {
    let parts: Vec<String> = plan
        .iter()
        .filter(|it| in_comment(it))
        .map(|it| {
            let what = if field(it, "type") == "review" { "Review" } else { "Comment" };
            format!("[{what}]({})\n\n{}", field(it, "url"), field(it, "reply"))
        })
        .collect();
    (!parts.is_empty()).then(|| format!("{prefix}Addressed review feedback:\n\n{}", parts.join("\n\n")))
}

/// The decision on record (#343) an item answered on the PR at <head> is:
/// what the bot said first, and the reply.
fn decision(it: &Map<String, Value>, head: &str) -> Value {
    let said = it.get("conversation").and_then(|c| c.get(0)).map_or(String::new(), |c| t(c, "body"));
    decisions::entry(&format!("bot-{}", field(it, "action")), head, &place(it), &said, &field(it, "reply"))
}

/// post, and each item it answered on the PR goes on the decisions record,
/// before exiting when marking items handled failed.
fn post_and_record(b: &str, pr_url: &str, plan: &[Map<String, Value>]) -> bool {
    let mut answered = vec![];
    let ok = post(b, pr_url, plan, &mut answered);
    let head = rev(".", &format!("refs/heads/{b}"));
    decisions::append(b, &answered.iter().map(|it| decision(it, &head)).collect::<Vec<_>>());
    ok.unwrap_or_else(|code| std::process::exit(code))
}

/// The replies posted and threads resolved (all but pushback), then the PR
/// comment, whose items go in branch.<b>.sd-handled; each item answered
/// goes in <answered>. False when any failed; Err(<git's exit code>) when
/// writing sd-handled failed.
fn post<'p>(
    b: &str,
    pr_url: &str,
    plan: &'p [Map<String, Value>],
    answered: &mut Vec<&'p Map<String, Value>>,
) -> Result<bool, i32> {
    let prefix = cfg("settings.replyPrefix", b).trim_end_matches('\n').to_string();
    let mut ok = true;
    for it in plan.iter().filter(|it| !is_ask(it) && field(it, "type") == "thread") {
        let (id, at) = (field(it, "id"), place(it));
        let reply = format!("{prefix}{}", field(it, "reply"));
        let q = "mutation($thread: ID!, $body: String!) {
          addPullRequestReviewThreadReply(input: {pullRequestReviewThreadId: $thread, body: $body}) { comment { id } } }";
        if gql(&[("query", q), ("thread", &id), ("body", &reply)]).is_none() {
            eprintln!("sd act: replying to {at} failed");
            ok = false;
            continue;
        }
        answered.push(it);
        let q = "mutation($thread: ID!) { resolveReviewThread(input: {threadId: $thread}) { thread { id } } }";
        if field(it, "action") != "pushback" && gql(&[("query", q), ("thread", &id)]).is_none() {
            eprintln!("sd act: resolving {at} failed");
            ok = false;
        }
    }
    let Some(body) = pr_comment(&prefix, plan) else { return Ok(ok) };
    // By URL, not {owner}/{repo}: gh's default repo may be a fork.
    let q = "query($url: URI!) { resource(url: $url) { ... on PullRequest { id } } }";
    let pr_id = gql(&[("url", pr_url), ("query", q)])
        .and_then(|o| crate::config::one_value(&o))
        .map(|v| t(&v, "data.resource.id"))
        .filter(|id| !id.is_empty());
    let q = "mutation($subject: ID!, $body: String!) { addComment(input: {subjectId: $subject, body: $body}) { clientMutationId } }";
    let posted = pr_id.is_some_and(|id| gql(&[("query", q), ("subject", &id), ("body", &body)]).is_some());
    if !posted {
        eprintln!("sd act: posting the PR comment failed");
        return Ok(false);
    }
    let in_it: Vec<_> = plan.iter().filter(|it| in_comment(it)).collect();
    answered.extend(&in_it);
    let handled = format!("branch.{b}.sd-handled");
    for it in in_it {
        let o = git(".", &["config", "--add", &handled, &field(it, "id")]);
        if !o.ok {
            eprint!("{}", o.stderr);
            return Err(o.code);
        }
    }
    Ok(ok)
}

/// The commit message the agent gave, else a default.
fn commit_message(result: &Value) -> String {
    match at(result, "commit_message").as_str() {
        Some(m) if !blank(m) => m.to_string(),
        _ => "Address review feedback".into(),
    }
}

/// address in <wt>; false when it ran but a reply or the PR comment failed.
fn address_run(b: &str, wt: &str, remote: &str, a: &Args) -> bool {
    let answering = !a.answer.is_empty();
    if !answering && !status(wt).is_empty() {
        die(format!("{wt} has uncommitted changes"));
    }
    let f = git(wt, &["fetch", "--quiet", remote]);
    eprint!("{}", f.stderr);
    if !f.ok {
        die(format!("fetch from {remote} failed"));
    }
    let s = state_json(wt, b).unwrap_or_else(|e| {
        eprint!("{e}");
        die("sd state failed")
    });
    let Some(_go) = gated(b, &s, a) else { return true };
    let no_items = vec![];
    let owed: Vec<&Value> =
        at(&s, "threads.items").as_array().unwrap_or(&no_items).iter().filter(|it| t(it, "kind") == "bot").collect();
    question_gate(b, "address", a.force, answering);
    going_on("address", a);

    if owed.len() > 100 {
        eprintln!("sd act: warning: over 100 owed items; taking the first 100");
    }
    let ids: Vec<String> = owed.iter().take(100).map(|it| t(it, "id")).collect();
    let mut fields = vec![("query", NODES_Q)];
    fields.extend(ids.iter().map(|id| ("ids[]", id.as_str())));
    let nodes = gql(&fields).unwrap_or_else(|| die("couldn't fetch the findings' text"));
    let nodes = crate::config::one_value(&nodes).unwrap_or_else(|| die("couldn't read the findings' text"));
    let bots: Vec<String> = cfg("settings.botLogin", b).lines().map(str::to_string).collect();
    let all = findings(&nodes, &bots);
    if all.is_empty() {
        die("none of the owed items could be fetched");
    }
    let theirs: Vec<Map<String, Value>> = all.iter().filter(|(_, u)| !u.is_empty()).map(|(f, u)| held(f, u)).collect();
    let fs: Vec<Map<String, Value>> = all.into_iter().filter(|(_, u)| u.is_empty()).map(|(f, _)| f).collect();
    if fs.is_empty() {
        if answering {
            die(format!("no bot feedback the agent can take on {b}{UNANSWERED}"));
        }
        print_report(b, "no bot feedback the agent can take", &report(&theirs));
        return true;
    }

    let sess = step_session(b, "address", &a.harness, a.force, answering);
    let verify = command("verify", b);
    let gbase = guidance_base(b);
    let guidance = step_guidance("addressPrompt", b, &gbase, &lib_dir().join("prompts/address.md")).unwrap_or_else(|e| {
        eprintln!("{e}");
        die("can't read the address guidance")
    });
    let rules = agent_rules(b);
    let list: Vec<Value> = fs.iter().cloned().map(Value::Object).collect();
    let listed = format!("Findings:\n{}", serde_json::to_string_pretty(&list).expect("a Value serializes"));
    let mut prompt = format!("{guidance}\n\n{ADDRESS_CONTRACT}\n\nPR {}\n{listed}", t(&s, "pr.url"));
    if answering {
        prompt = answer_prompt(&a.answer, &listed);
    }
    if !verify.is_empty() {
        prompt += &format!("\n\nThe repo's checks, which the pipeline runs after you: {verify}");
    }
    let (allow, said) = harness::tool_rules(&sess.harness, &rules);
    prompt += &format!("\n\n{said}");

    let (h, sid) = (sess.harness.as_str(), sess.id.as_str());
    eprintln!("sd act: {h} ({} session {sid}) is addressing {} item(s) on {b}…", sess.mode, fs.len());
    let head = format!("refs/heads/{b}");
    let before = rev(".", &head);
    let step = Step {
        harness: h,
        session: sid,
        mode: sess.mode,
        parent: &sess.parent,
        timeout: sess.timeout,
        prompt: &prompt,
        schema: SCHEMA,
        settings: &sess.cfg,
        name: "address",
        branch: b,
        claude_allow: &allow,
        pi_tools: PI_TOOLS,
    };
    let Some((result, ran)) = harness::run_step(&step).ok().and_then(Ran::last) else {
        hold_since(b, &gbase, &before);
        die(format!("the {h} step failed or returned no usable result (session {sid})"))
    };
    step_store(b, &sess, &ran);
    question_answered(b, &a.answer);
    hold_since(b, &gbase, &before);
    let agent_head = rev(".", &head);

    // pi has no schema enforcement, so check the shape here too.
    let outcome = t(&result, "outcome");
    let summary = t(&result, "summary");
    match outcome.as_str() {
        "done" | "needs-input" => {}
        "gave-up" => give_up(b, wt, "address", &summary, format!("the agent gave up: {summary}. Any edits are left uncommitted in {wt}")),
        _ => give_up(b, wt, "address", &format!("returned outcome \"{outcome}\": {summary}"), format!("the agent returned outcome \"{outcome}\"; edits (if any) are left uncommitted in {wt}")),
    }
    let changed = !status(wt).is_empty();
    let plan = scrubbed(plan(&fs, &result, changed, &theirs));
    if changed {
        commit_edits(b, "address", wt, &commit_message(&result));
    }
    hold_since(b, &gbase, &agent_head);
    let verified = verify_autofix(wt, b, "address");
    // The asks stay owed, so the bot step would hand them over again: the
    // question replaces it, recorded on HEAD as verify left it (a push keeps it).
    let lines = report(&plan);
    let asks = lines.iter().skip_while(|l| *l != "needs chrismo:").cloned().collect::<Vec<_>>().join("\n");
    let mut qmsg = String::new();
    if outcome == "needs-input" || !asks.is_empty() {
        let text = if asks.is_empty() { summary.clone() } else { format!("{summary}\n{asks}") };
        question_ask(b, wt, "address", &text);
        qmsg = format!(
            "{}: answer with {}; or answer on GitHub or in its session (sd act attach {b}), then sd act ack --question",
            question_say(b),
            answer_cmd(b)
        );
    }
    if verified.is_err() {
        let q = if qmsg.is_empty() { qmsg } else { format!(". {qmsg}") };
        die(format!("verify failed ({verify}); the commit is kept in {wt}, nothing pushed or posted{q}"));
    }

    print_report(b, &summary, &lines);
    if !qmsg.is_empty() {
        println!("{qmsg}");
    }
    if a.no_publish {
        println!("not published: nothing pushed or posted");
        return true;
    }

    // The plan isn't kept: after the push, the items are still owed, and a
    // second address finds the fix already in (it spends tokens again).
    hold_gate(b, &format!("pushing {b}"), "The replies weren't posted, so the items are still owed: after sd act push, sd act address answers them again, or answer them on GitHub");
    // A reply-only round commits nothing, unless autofix did.
    let left = if rev(".", &head) == before { "nothing was committed".to_string() } else { format!("the commit is kept in {wt}") };
    pr_gate(&t(&s, "pr.number"), &t(&s, "pr.url"), "address", "pushing or posting", &left);
    lock_recheck(b, "address", "pushing", &left);
    if let Err(e) = push_branch(b, remote) {
        eprintln!("sd act: {e}");
        die("not pushed, so nothing posted");
    }
    let posted = post_and_record(b, &t(&s, "pr.url"), &plan);
    unposted(b, if posted { &[] } else { &owed[..ids.len()] });
    posted
}

/// branch.<b>.sd-unposted := <items>' keys, the owed set of a round whose
/// replies didn't all post; address refuses it till a new item comes (#357).
fn unposted(b: &str, items: &[&Value]) {
    let key = format!("branch.{b}.sd-unposted");
    git(".", &["config", "--unset-all", &key]);
    if !items.iter().all(|it| git(".", &["config", "--add", &key, &round_key(it)]).ok) {
        eprintln!("sd act: warning: couldn't record {key}, so NEXT may offer address on these items again");
    }
}

/// address, marked running for all of it, commit, verify and posting
/// included, so NEXT doesn't hand the worktree on mid-step (#170). It runs
/// in the branch's worktree.
pub fn address(b: &str, remote: &str, a: &Args) {
    let stored = with_stored_answer(b, "address", a);
    let a = stored.as_ref().unwrap_or(a);
    let wt = worktree_for(".", b);
    if wt.is_empty() {
        die(format!("{b} isn't checked out in any worktree"));
    }
    if let Err(e) = std::env::set_current_dir(&wt) {
        die(format!("can't work in {wt}: {e}"));
    }
    if running::holding(b, "address", || address_run(b, &wt, remote, a)) != Some(true) {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    /// address's gate on an open PR owing one bot item, in sync and pushed,
    /// with <edits> (a.b: v) applied, in the record's words.
    fn gate_on(edits: Value) -> Gate {
        let mut s = json!({
            "branch": "feat", "pr": {"number": 7, "state": "OPEN"},
            "sync": {"target": "main", "behind": 0, "up_to_date": true, "conflict": false},
            "push": {"remote_sha": "abc", "ahead": 0, "behind": 0}, "ci": {"verdict": "green"},
            "threads": {"owed_bot": 1, "owed_human": 0}, "session": {"locked": false}, "errors": []
        });
        for (k, v) in edits.as_object().unwrap() {
            match k.split_once('.') {
                Some((top, field)) => s[top][field] = v.clone(),
                None => s[k] = v.clone(),
            }
        }
        said(gates::address(&Derived::new(&s)), &s)
    }

    #[test]
    fn said_words_each_reason_from_the_record() {
        assert_eq!(gate_on(json!({})), Gate::Go);
        assert_eq!(gate_on(json!({"pr": null})), Gate::Hard("no PR for feat".into()));
        assert_eq!(gate_on(json!({"pr.state": "MERGED"})), Gate::Hard("PR #7 is merged".into()));
        let unread = json!({"threads.owed_bot": null, "errors": ["gh (feat): boom", "again"]});
        assert_eq!(gate_on(unread), Gate::Hard("can't read the PR from GitHub: gh (feat): boom; again".into()));
        assert_eq!(gate_on(json!({"threads.owed_bot": 0, "threads.owed_human": 2})),
                   Gate::Done("nothing to address on feat (2 human item(s) are chrismo's)".into()));
        let s = json!({"session.locked": true, "sync.conflict": true, "push.ahead": 2, "push.behind": 1});
        let want = ["taken over (branch.feat.sd-lock is set)", "restack due, and it would conflict", "diverged from the remote branch"];
        assert_eq!(gate_on(s), Gate::Soft(strings(&want)));
        assert_eq!(gate_on(json!({"sync.up_to_date": null, "sync.target": null})), Gate::Soft(strings(&["sync with the parent unknown"])));
        assert_eq!(gate_on(json!({"push.remote_sha": null, "push.ahead": 1})), Gate::Soft(strings(&["not pushed"])));
        assert_eq!(gate_on(json!({"push.ahead": 1})), Gate::Soft(strings(&["1 unpushed commit(s)"])));
        assert_eq!(gate_on(json!({"push.behind": 2})), Gate::Soft(strings(&["the remote branch has 2 commit(s) you don't"])));
        let s = json!({"sync.up_to_date": false, "sync.behind": 10, "ci.verdict": "pending"});
        assert_eq!(gate_on(s), Gate::Warn(strings(&["behind main by 10", "CI is still running"])));
    }

    #[test]
    fn yours_leaves_out_the_push_reason_only_when_next_offers_the_push() {
        let rec = |held: bool| {
            let mut s = json!({"branch": "feat", "pr": {"state": "OPEN"}, "sync": {"ahead": 1}, "push": {"remote_sha": "abc", "ahead": 1}});
            if held {
                s["push"]["held"] = json!({"paths": [".rwx/tests.yml"]});
            }
            s
        };
        let why = strings(&["taken over", "1 unpushed commit(s)"]);
        let s = rec(false);
        assert_eq!(yours(&why, &Derived::new(&s), &s), strings(&["taken over"]));
        let s = rec(true);
        assert_eq!(yours(&why, &Derived::new(&s), &s), why);
    }

    fn nodes(said: &[Value]) -> Value {
        let mut thread = json!({"__typename": "PullRequestReviewThread", "id": "T1", "path": "file.txt", "line": 3, "isOutdated": false,
            "comments": {"nodes": [{"author": {"__typename": "Bot", "login": "review-bot"}, "body": "x"}]}});
        thread["comments"]["nodes"].as_array_mut().unwrap().extend(said.iter().cloned());
        json!({"data": {"viewer": {"login": "me"}, "nodes": [
            thread,
            null,
            {"__typename": "PullRequestReview", "id": "R1", "url": "u/r", "author": {"__typename": "Bot", "login": "b"}, "body": "r"},
            {"__typename": "IssueComment", "id": "C1", "url": "u/c", "author": {"__typename": "User", "login": "pal"}, "body": "c"}
        ]}})
    }

    #[test]
    fn findings_are_each_nodes_text_typed_with_who_else_is_in_it() {
        let fs = findings(&nodes(&[]), &[]);
        let got: Vec<(String, String, Vec<String>)> = fs.iter().map(|(f, u)| (field(f, "id"), field(f, "type"), u.clone())).collect();
        let want = [("T1", "thread", vec![]), ("R1", "review", vec![]), ("C1", "comment", strings(&["pal"]))];
        assert_eq!(got, want.map(|(id, ty, u)| (id.to_string(), ty.to_string(), u)));
        assert_eq!(
            Value::Object(fs[0].0.clone()),
            json!({"id": "T1", "type": "thread", "url": null, "path": "file.txt", "line": 3, "outdated": false,
                   "conversation": [{"author": "review-bot", "body": "x"}]})
        );
        assert_eq!(fs[2].0["conversation"], json!([{"author": "pal", "body": "c"}]));
    }

    #[test]
    fn findings_trust_bots_the_viewer_and_bot_logins_and_list_the_rest_once_sorted() {
        let said = |login: &str| json!({"author": {"__typename": "User", "login": login}, "body": "hi"});
        let n = nodes(&[said("zed"), said("me"), said("marge"), said("amy"), said("zed"), said("helper")]);
        let fs = findings(&n, &strings(&["marge", "helper[bot]"]));
        assert_eq!(fs[0].1, strings(&["amy", "zed"]));
        assert_eq!(fs[0].0["conversation"].as_array().unwrap().len(), 7);
        assert!(findings(&json!({"data": {"nodes": null}}), &[]).is_empty());
    }

    #[test]
    fn held_is_chrismos_naming_who_joined() {
        let (f, _) = &findings(&nodes(&[]), &[])[2];
        let h = held(f, &strings(&["pal", "zed"]));
        assert_eq!((field(&h, "action"), field(&h, "reply")), ("ask".into(), "joined by pal, zed: yours to answer".into()));
    }

    fn items(fs: &[Map<String, Value>]) -> Vec<(String, String)> {
        fs.iter().map(|f| (field(f, "action"), field(f, "reply"))).collect()
    }

    fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
        p.iter().map(|(a, r)| (a.to_string(), r.to_string())).collect()
    }

    fn ided(ids: &[&str]) -> Vec<Map<String, Value>> {
        ids.iter().map(|id| json!({"id": id, "type": "thread"}).as_object().cloned().unwrap()).collect()
    }

    #[test]
    fn plan_takes_each_items_action_and_reply_and_what_isnt_one_is_ask() {
        let fs = ided(&["1", "2", "3", "4", "5", "6", "7"]);
        let r = json!({"items": [
            {"id": "1", "action": "fixed", "reply": "Done."},
            {"id": "2", "action": "pushback", "reply": "No."},
            {"id": "3", "action": "reply", "reply": " \n"},
            {"id": "4", "action": "declined", "reply": "hm"},
            {"id": "5", "action": "ask"},
            {"id": 6, "action": "reply", "reply": "a number isn't the id"},
            {"id": "7", "action": "ask", "reply": "Which?"}
        ]});
        let held = [held(&ided(&["8"])[0], &strings(&["pal"]))];
        assert_eq!(
            items(&plan(&fs, &r, true, &held)),
            pairs(&[
                ("fixed", "Done."),
                ("pushback", "No."),
                ("ask", "(the agent gave no reply text)"),
                ("ask", "(the agent did not answer this one)"),
                ("ask", "(the agent gave no reply text)"),
                ("ask", "(the agent did not answer this one)"),
                ("ask", "Which?"),
                ("ask", "joined by pal: yours to answer"),
            ])
        );
    }

    #[test]
    fn plan_fixed_with_nothing_changed_is_ask() {
        let r = json!({"items": [{"id": "1", "action": "fixed", "reply": "Done."}]});
        assert_eq!(items(&plan(&ided(&["1"]), &r, false, &[])), pairs(&[("ask", "(said fixed but changed nothing): Done.")]));
        let none = items(&plan(&ided(&["1"]), &json!({"items": "no"}), true, &[]));
        assert_eq!(none, pairs(&[("ask", "(the agent did not answer this one)")]));
    }

    #[test]
    fn scrub_defangs_commands_and_mentions_and_keeps_paths_urls_and_addresses() {
        assert_eq!(
            scrub("Re-run /review-bot /a /b and ask @someone (@other); see src/x.sh, /tmp/out, https://x/y/marge and https://m.com/@who/p; t@e.com"),
            "Re-run ∕review-bot ∕a ∕b and ask ＠someone (＠other); see src/x.sh, /tmp/out, https://x/y/marge and https://m.com/@who/p; t@e.com"
        );
        assert_eq!(scrub("Done.\n/marge again? cc:@carol _@dave_"), "Done.\n∕marge again? cc:＠carol _＠dave_");
        assert_eq!(scrub("/a\t\"/b\" `/c` [/d] '/e'"), "∕a\t\"∕b\" `∕c` [∕d] '∕e'");
        assert_eq!(scrub("foo/marge **/marge** a/@who @ x"), "foo/marge **/marge** a/@who @ x");
    }

    #[test]
    fn scrub_reads_whitespace_as_re2_does() {
        // RE2's \s is ASCII only: a no-break space is no boundary.
        assert_eq!(scrub("a\u{a0}/marge"), "a\u{a0}/marge");
        assert_eq!(scrub("a\x0c/marge"), "a\x0c∕marge");
    }

    #[test]
    fn scrubbed_leaves_ask_items_as_the_agent_wrote_them() {
        let p = ided(&["1", "2"]);
        let p = vec![with(&p[0], "reply", "ping @carol".into()), with(&p[1], "ask", "ping @carol".into())];
        assert_eq!(items(&scrubbed(p)), pairs(&[("reply", "ping ＠carol"), ("ask", "ping @carol")]));
    }

    fn item(ty: &str, path: Value, action: &str, reply: &str) -> Map<String, Value> {
        let f = json!({"id": "x", "type": ty, "url": format!("u/{ty}"), "path": path, "line": 3});
        with(f.as_object().unwrap(), action, reply.into())
    }

    #[test]
    fn report_is_the_answers_then_the_asks_for_chrismo() {
        let p = [
            item("thread", json!("file.txt"), "ask", "Rename?"),
            item("thread", json!("file.txt"), "fixed", "Done."),
            item("review", json!(null), "pushback", "No."),
            item("comment", json!(null), "ask", "Changelog?"),
        ];
        assert_eq!(
            report(&p),
            strings(&[
                "  fixed     file.txt:3           Done.",
                "  pushback  review body          No.",
                "needs chrismo:",
                "  file.txt:3           Rename?",
                "  PR comment           Changelog?",
            ])
        );
        assert!(report(&[]).is_empty());
    }

    #[test]
    fn pr_comment_links_the_review_bodies_and_comments_answered() {
        let p = [
            item("thread", json!("file.txt"), "fixed", "Done."),
            item("review", json!(null), "fixed", "Added the line."),
            item("comment", json!(null), "ask", "Changelog?"),
            item("comment", json!(null), "reply", "Nothing to do."),
        ];
        assert_eq!(
            pr_comment("🤖 ", &p).unwrap(),
            "🤖 Addressed review feedback:\n\n[Review](u/review)\n\nAdded the line.\n\n[Comment](u/comment)\n\nNothing to do."
        );
        assert_eq!(pr_comment("", &p[..1]), None);
    }

    #[test]
    fn commit_message_is_the_agents_else_a_default() {
        assert_eq!(commit_message(&json!({"commit_message": "Uppercase three"})), "Uppercase three");
        assert_eq!(commit_message(&json!({"commit_message": " "})), "Address review feedback");
        assert_eq!(commit_message(&json!({"commit_message": null})), "Address review feedback");
    }
}
