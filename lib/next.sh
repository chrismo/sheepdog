# What to do next on a branch, from its sd state JSON. Source it after
# lib/config.sh; needs bash 5 and SuperDB.
#
# A step is {key, who, say, cmd}. who is who does it once it's automated:
#   you    chrismo's judgment     agent  a model step
#   run    mechanical             wait   nothing to do yet
# cmd is what to run by hand meanwhile (null: no command yet).
#
# Ordering of the loop's own steps, after what blocks everything (stale refs,
# human items): restack, agent fixes, one push, then review and wait; the push
# leads because every push restarts CI (#79 Design record).

# Behind the default branch alone is no step: it moves too often to chase.
# A conflict with it is, and so is a restack after the parent merged.
# Only bot items are ever marked i, so owed_bot - info is the blocking bot count.
# on() and n() come from lib/config.sh's PRC_FNS: a null or missing
# field compares to nothing in SuperDB, and they make it false and 0.
#
# Past settings.branchBudget (spend.budget; 0 is none), nothing spends tokens
# on the branch: the first agent step becomes chrismo's, the rest go, and the
# run steps stay (#121). So does open-pr: sd act ship describes the branch
# when its description is stale, which spends. Spend that can't be read, with
# a budget set, holds the same way: a cost guard fails closed. spends() makes
# that swap, agent() for agent steps; the dedupe at the end keeps the first.
# Not guarded: ship-parent, whose describe spends on the parent; this state
# has only this branch's spend, so the parent's budget isn't checked here.
PRN_QUERY='fn plural(k, w): f"{k} {w}{k == 1 ? "" : "s"}"
fn step(key, who, say, cmd): {key: key, who: who, say: say, cmd: cmd}
fn usd(x): f"${round(x * 100)::int64 / 100}.{round(x * 100)::int64 % 100 < 10 ? "0" : ""}{round(x * 100)::int64 % 100}"
fn spends(budget, s): budget is null ? s : budget
fn agent(budget, key, say, cmd): spends(budget, step(key, "agent", say, cmd))
fn covers(p): on(p.status in ["covered", "rewritten-same"])
fn trails(p): on(p.status in ["uncovered", "rewritten-changed", "unknown"])
fn why_blocked(m): m == "BLOCKED" ? "the merge is blocked"
                   : m == "DIRTY" ? "there are conflicts" : m == "DRAFT" ? "it'"'"'s a draft" : lower(m)
values {...s, remote}
| put open:=(pr is null or on(pr.state == "OPEN")), has_pr:=on(pr.state == "OPEN"),
      foreign:=(on(push.remote_ours == false) and n(push.theirs) > 0),
      bot_blocking:=(n(threads.owed_bot) - n(threads.info)),
      dropping:=(on(push.remote_ours) and n(push.behind) > 0 and n(push.ahead) == 0 and on(push.remote_made)),
      taken:=(on(push.remote_ours) and n(push.behind) > 0 and n(push.ahead) == 0 and on(push.remote_made == false)),
      held:=(on(push.remote_ours) and n(push.behind) > 0 and n(push.ahead) == 0
             and (on(push.remote_made == false) or (on(push.remote_made) and on(push.dirty)))),
      to_push:=(push.remote_sha is null or n(push.ahead) > 0
                or (on(push.remote_ours) and n(push.behind) > 0 and n(push.ahead) == 0 and on(push.remote_made) and not on(push.dirty))),
      ci_held:=not nul(push.held),
      chase:=(on(sync.target != sync.default) or on(parent.merged)),
      undone:=(n(sync.undone) > 0),
      merge_state:=coalesce(pr.merge_state, "UNKNOWN"),
      gh_behind:=on(pr.merge_state == "BEHIND"),
      draft_ready:=(on(pr.draft) and not (push.remote_sha is null or n(push.ahead) > 0
                                           or (on(push.remote_ours) and n(push.behind) > 0 and n(push.ahead) == 0))
                    and not (on(push.remote_ours == false) and n(push.theirs) > 0)
                    and (on(ci.verdict == "green") or on(ci.verdict == "none"))
                    -- sd state reads the required checks of every draft: null is a failed read
                    and not on(ci.required is null)),
      review_required:=on(pr.review_decision == "REVIEW_REQUIRED"),
      stacked:=(on(pr.base != sync.default) and (on(parent.pr.state == "OPEN") or on(parent.merged))),
      parent_closed:=(on(pr.base != sync.default) and on(parent.pr.state == "CLOSED")),
      parent_unpushed:=on(parent.unpushed),
      no_base:=(sync.old_base is null and (on(sync.target != sync.default) or on(parent.merged))),
      restack:=(sync.old_base is null ? f"git rebase {remote}/{sync.target}"
                : f"git rebase --onto {remote}/{sync.target} {sync.old_base[0:12]}"),
      need_review:=((on(pr.state == "OPEN") or trails(review.local) or trails(review.bot) or trails(review.accepted)
                     or on(review.findings.at_head == false))
                    and not (covers(review.local) or covers(review.bot) or covers(review.accepted))),
      fixing:=(on(review.findings.at_head) and not (covers(review.local) or covers(review.bot) or covers(review.accepted))),
      bot_due:=(coalesce(bot.command, null) is not null and covers(review.local)
                and not covers(review.bot) and not covers(review.accepted)),
      bot_waiting:=(covers(review.bot) and on(review.bot.replied == false) and not covers(review.accepted)),
      budget:=(nul(spend) or n(spend.budget) <= 0 ? null
               : nul(spend.usd)
               ? step("budget", "you", "can'"'"'t tell what this branch has spent (sd state'"'"'s errors say why), so nothing spends: fix the usage record, or take over",
                      "sd act attach")
               : spend.usd > spend.budget
               ? step("budget", "you", f"spent {usd(spend.usd)} on this branch, past settings.branchBudget ({usd(spend.budget)}): raise it, or take over",
                      "sd act attach") : null)
| unnest [
    on(pr.state == "MERGED") ? step("merged", "run", "merged: drop the branch'"'"'s pins and enrollment, then remove this worktree", "sd act forget") : null,
    on(pr.state == "CLOSED") ? step("closed", "you", "PR closed without merging", null) : null,
    open and len(errors) > 0
      ? step("errors", "you", f"{plural(len(errors), "error")}: the steps below may be wrong", "sd state --json") : null,
    open and on(fresh == false)
      ? step("stale", "run", "fetch: GitHub is ahead of your remote refs, so the steps below may be wrong",
           f"git fetch --prune {remote}") : null,
    open and on(session.locked)
      ? step("attached", "wait", "attached: the branch'"'"'s session is open interactively; agent steps wait until it exits", null) : null,
    open and foreign
      ? step("foreign", "you", f"{remote}/{branch} has {plural(push.theirs, "commit")} this copy has never had",
             f"git log HEAD..{remote}/{branch}") : null,
    open and n(threads.owed_human) > 0
      ? step("human", "you", f"reply to {plural(threads.owed_human, "human item")}", "sd state --long") : null,
    open and undone
      ? step("restack-undone", "you",
             f"the restack was undone ({sync.undone} of {sync.target}'"'"'s commits {sync.undone == 1 ? "is a copy" : "are copies"} on {branch}; a plain git push after a restack?): redo it, and don'"'"'t sd act push before that, which would publish the copies{no_base ? f"; no base to redo it from on {sync.target}: pin one by hand (git update-ref refs/sheepdog/{branch}/base <the commit {branch} left {parent.name} at>), then sd act restack" : ""}",
             no_base ? null : f"{restack} && sd act push") : null,
    open and parent_unpushed
      ? step("ship-parent", "run", f"ship the parent first: {parent.name} isn'"'"'t on {remote} yet", f"sd act ship {parent.name}") : null,
    open and not undone and not parent_closed and on(sync.up_to_date == false) and on(sync.conflict)
      ? (on(sync.by_tree)
         ? step("conflict", "you", f"restack onto {sync.target} by tree conflicts, which --resolve can'"'"'t do: resolve it by hand (sd act restack says how)", "sd act restack")
         : agent(budget, "conflict", f"restack onto {sync.target} conflicts: resolve it", "sd act restack --resolve")) : null,
    open and not undone and not parent_closed and on(sync.up_to_date == false) and not on(sync.conflict) and chase
      ? (sync.old_base is null
         ? step("behind", "you", f"restack onto {sync.target} ({sync.behind} behind): no base to restack from; see sd state'"'"'s errors", "sd state --json")
         : step("behind", "run", on(sync.by_tree) ? f"restack onto {sync.target} by tree ({sync.behind} behind): its commits become one"
                                                  : f"restack onto {sync.target} ({sync.behind} behind)",
                "sd act restack"))
      : has_pr and not undone and not parent_closed and not on(sync.conflict) and gh_behind
      ? step("behind", "run", f"restack onto {sync.default} ({n(sync.behind)} behind): GitHub requires branches to be up to date",
             "sd act restack") : null,
    has_pr and on(ci.verdict == "red") and on(ci.for_head) and not to_push
      ? agent(budget, "ci-red", f"fix CI: {join(ci.failing, ", ")}", "sd act ci-fix") : null,
    has_pr and on(ci.verdict == "blocked") and on(ci.for_head) and not to_push
      ? step("ci-blocked", "you", f"CI blocked: {sj(coalesce(ci.blocked, []), ", ")} didn'"'"'t start or want approval; an agent can'"'"'t fix that",
             "gh pr checks") : null,
    open and bot_blocking > 0
      ? agent(budget, "bot", f"address {plural(threads.owed_bot, "bot item")}", "sd act address") : null,
    open and bot_blocking == 0 and n(threads.info) > 0
      ? step("read", "you", f"read {plural(threads.info, "bot item")} in sd state --long, then ack", "sd act ack") : null,
    open and taken
      ? step("take", "you", f"{remote}/{branch} has {plural(push.behind, "commit")} this branch took but didn'"'"'t make, reset away here: take {n(push.behind) == 1 ? "it" : "them"} back (git merge --ff-only {remote}/{branch}), or drop {n(push.behind) == 1 ? "it" : "them"} by hand", null) : null,
    open and dropping and on(push.dirty)
      ? step("reset-dirty", "you", f"{remote}/{branch} has {plural(push.behind, "commit")} reset away here, and the worktree has uncommitted changes: commit them (then sd act push replaces the remote {n(push.behind) == 1 ? "commit" : "commits"}), or remove them (git reset --hard; git clean for untracked files)", null) : null,
    open and ci_held
      ? (foreign or undone or parent_unpushed
         ? step("ci-config", "you", f"the agent changed CI config ({join(push.held.paths, ", ")}): review it; push once the steps above are done", null)
         : step("ci-config", "you", f"the agent changed CI config ({join(push.held.paths, ", ")}): review, then sd act push", "sd act push")) : null,
    open and not ci_held and not foreign and not undone and not parent_unpushed and to_push
      ? step("push", "run", push.remote_sha is null ? f"push: not on {remote} yet"
             : on(push.diverged) ? f"force-push {plural(coalesce(sync.ahead, push.ahead), "commit")} (rewritten); a plain git push is rejected, and pulling would undo the rewrite"
             : n(push.ahead) == 0 ? f"force-push: drop {plural(push.behind, "commit")} the remote has, reset away here"
             : f"push {plural(push.ahead, "commit")}", "sd act push") : null,
    pr is null and len(errors) == 0 and not parent_unpushed and not ci_held
      ? spends(budget, step("open-pr", "run", "ship: push, open a draft PR", "sd act ship")) : null,
    has_pr and draft_ready and not need_review and not bot_due and not bot_waiting
      ? step("draft", "run", f"{on(ci.verdict == "none") ? "reviewed, no CI checks" : "CI green and reviewed"}: mark the PR ready for review",
             "gh pr ready") : null,
    open and fixing and not on(review.findings.declined) and review.findings.round < review.findings.cap
      ? agent(budget, "review-fix", f"fix {plural(review.findings.n, "review finding")} (round {review.findings.round}/{review.findings.cap})",
             "sd act implement")
      : open and fixing
      ? step("review-decide", "you",
             f"{on(review.findings.declined) ? f"the agent declined {plural(review.findings.n, "review finding")}"
                : f"{plural(review.findings.n, "review finding")} after {review.findings.round} rounds"}: fix {review.findings.n == 1 ? "it" : "them"}, or accept HEAD as reviewed (sd state --long lists them)",
             "sd act review --accept")
      : open and need_review ? agent(budget, "review", "review the changes since the last review", "sd act review") : null,
    has_pr and not to_push and not held and not foreign and bot_due
      ? step("bot-trigger", "run", f"trigger the review bot ({bot.command})", "sd act trigger") : null,
    has_pr and on(ci.verdict == "inconclusive") ? step("ci-rerun", "run", "rerun cancelled CI", "gh pr checks") : null,
    has_pr and bot_waiting and n(review.bot.waited) <= n(bot.timeout)
      ? step("bot-wait", "wait", f"waiting on the review bot ({bot.command})", null) : null,
    has_pr and bot_waiting and n(review.bot.waited) > n(bot.timeout)
      ? step("bot-silent", "you", f"the review bot hasn'"'"'t answered in {n(review.bot.waited) / 60} min: check it, trigger it again, or accept HEAD as reviewed (sd act review --accept)",
             "sd act trigger --force") : null,
    has_pr and not to_push and not held and (on(ci.verdict == "pending") or on(ci.for_head == false))
      ? step("ci-wait", "wait", on(ci.verdict == "pending")
             ? (len(coalesce(ci.pending, [])) == 0 and len(coalesce(ci.missing, [])) > 0
                ? f"waiting on required check{len(ci.missing) == 1 ? "" : "s"} {sj(ci.missing, ", ")} to report{on(pr.draft)
                    ? f"; if {len(ci.missing) == 1 ? "it never runs" : "they never run"} on a draft, undraft by hand (gh pr ready)" : ""}"
                : "CI running")
             : "CI hasn'"'"'t reported on HEAD yet",
             "gh pr checks --watch") : null,
    has_pr and parent_closed
      ? step("parent-closed", "you", f"{parent.name} (#{parent.pr.number}) was closed without merging: decide what this PR targets", null) : null,
    has_pr and stacked and on(parent.merged)
      ? step("retarget", "run", f"{pr.base} already merged: point this PR at {sync.default}", "sd act retarget") : null,
    has_pr and review_required
      ? step("review-wait", "wait", len(coalesce(pr.review_requests, [])) > 0
             ? f"waiting on review from {join(pr.review_requests, ", ")}" : "waiting on a required review", null) : null,
    has_pr and stacked and not on(parent.merged)
      ? step("parent-wait", "wait", f"waiting on {parent.name} (#{parent.pr.number}) to merge first", null) : null,
    has_pr and not review_required and not stacked and not parent_closed and not gh_behind and not on(pr.draft) and not bot_waiting
      ? (merge_state in ["CLEAN", "UNSTABLE", "HAS_HOOKS"] ? step("ready", "you", "ready to merge", "gh pr merge")
         : merge_state == "UNKNOWN" ? step("merge-wait", "wait", "GitHub hasn'"'"'t decided whether it can merge yet", null)
         : step("blocked", "you", f"GitHub says {why_blocked(merge_state)}: see why", "gh pr view --web")) : null
  ]
| where this is not null
| aggregate steps:=collect(this)
| values coalesce(steps, [])
| values {first: coalesce(coalesce([unnest this | count {s: this, j} | where s.key == "budget" | values j], [])[0], 0), all: this}
| values coalesce([unnest {first, s: all} | count {r: this, j} | where r.s.key != "budget" or j == r.first | values r.s], [])
| values (len(this) > 1 and this[len(this)-1].key in ["ready", "merge-wait", "blocked"]) ? this[0:len(this)-1] : this'

# prn_next <remote>: sd state JSON on stdin; prints the steps as a JSON array, first is next.
prn_next() {
  local state remote kind
  state=$(cat)
  remote=$(prc_json_string "$1") || return
  kind=$(prc_line 'values kind(this)' "$state") || return
  [[ $kind == record ]] || { echo "lib/next.sh: sd state JSON must be one record" >&2; return 1; }
  prc_json "$PRN_QUERY" "{\"remote\": $remote, \"s\": $state}"
}

# prn_lines [one|all]: steps on stdin; one adds a count of the rest, all lists them as "then".
prn_lines() {
  local mode=${1:-one} out more=""
  local -a ls
  out=$(prc_line 'unnest this
    | values f"{who}{"     "[0:5-len(who)]}  {say}{cmd is null ? "" : f"  → {cmd}"}"') || return
  [[ -n $out ]] || return 0
  mapfile -t ls <<<"$out"
  if [[ $mode == all ]]; then
    printf 'NEXT  %s\n' "${ls[0]}"
    (( ${#ls[@]} == 1 )) || printf 'then  %s\n' "${ls[@]:1}"
  else
    (( ${#ls[@]} == 1 )) || more="  (+$(( ${#ls[@]} - 1 )) more)"
    printf 'NEXT  %s%s\n' "${ls[0]}" "$more"
  fi
}
