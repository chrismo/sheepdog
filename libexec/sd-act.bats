#!/usr/bin/env bats

bats_require_minimum_version 1.5.0

# Test suite for libexec/sd-act's dispatcher: the shebang, --help, no verb,
# an unknown verb, and a flag on a verb it isn't for (#204). The verbs ported
# to Rust are tested in tests/act_*.rs (#202).

SD_ACT="$BATS_TEST_DIRNAME/sd-act"

setup() {
  TMP="$(cd "$BATS_TEST_TMPDIR" && pwd -P)"
  export GIT_CONFIG_GLOBAL="$TMP/gitconfig-global"
  export GIT_CONFIG_SYSTEM=/dev/null
  printf '[user]\n\temail = t@example.com\n\tname = Tester\n[init]\n\tdefaultBranch = main\n' > "$GIT_CONFIG_GLOBAL"
  git init -q "$TMP/repo"
  cd "$TMP/repo"
  git commit -q --allow-empty -m init
  git switch -qc feat
}

@test "runs under bash 5 via the bash5 shebang" {
  run head -1 "$SD_ACT"
  [ "$output" = "#!/usr/bin/env bash5" ]
}

@test "no verb: usage on stderr, nonzero" {
  run "$SD_ACT"
  [ "$status" -ne 0 ]
  [[ "$output" == *"Usage"* ]]
}

@test "unknown verb fails" {
  run "$SD_ACT" frobnicate
  [ "$status" -ne 0 ]
  [[ "$output" == *"unknown verb"* ]]
}

@test "--remove is for sd act forget only" {
  run "$SD_ACT" unenroll --remove
  [ "$status" -ne 0 ]
  [[ "$output" == *"--remove is for sd act forget only"* ]]
}

@test "--help: every verb has a usage line" {
  run "$SD_ACT" --help
  [ "$status" -eq 0 ]
  verbs=$(sed -n '/^case \$verb in/,/^esac/s/^  \([a-z-]*\)).*/\1/p' "$SD_ACT")
  [ -n "$verbs" ]
  for v in $verbs; do
    [[ "$output" =~ sd\ act\ $v\  ]] || { echo "missing: $v"; false; }
  done
}

@test "--help works outside a repo" {
  cd "$TMP"
  run "$SD_ACT" --help
  [[ "$output" == *"Usage"* ]]
}
