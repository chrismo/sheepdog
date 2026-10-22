#!/usr/bin/env bats

# Throwaway suites, so ci/test never recurses into the real ones. bats names a
# suite outside the working directory by its basename.

CI_TEST="$BATS_TEST_DIRNAME/test"

setup() {
  TMP="$(mktemp -d "${TMPDIR:-/tmp}/sheepdog-ci.XXXXXX")"
  export SHEEPDOG_TEST_RESULTS="$TMP/results"
}

teardown() {
  rm -rf "$TMP"
}

# suite <name> <body>: a one-test .bats file in $TMP.
suite() {
  printf '@test "%s" {\n  %s\n}\n' "$1" "$2" > "$TMP/$1.bats"
}

@test "every testsuite names its file for Captain" {
  suite one true
  suite two true
  run "$CI_TEST" "$TMP/one.bats" "$TMP/two.bats"
  [ "$status" -eq 0 ]
  grep -q "<testsuite name=\"one.bats\" file=\"one.bats\"" "$SHEEPDOG_TEST_RESULTS/report.xml"
  grep -q "<testsuite name=\"two.bats\" file=\"two.bats\"" "$SHEEPDOG_TEST_RESULTS/report.xml"
}

@test "a failing suite still gets its file, and the failure is the exit status" {
  suite bad false
  run "$CI_TEST" "$TMP/bad.bats"
  [ "$status" -ne 0 ]
  grep -q "<testsuite name=\"bad.bats\" file=\"bad.bats\"" "$SHEEPDOG_TEST_RESULTS/report.xml"
}

@test "runs under bash 5 via the bash5 shebang" {
  run head -1 "$CI_TEST"
  [ "$output" = "#!/usr/bin/env bash5" ]
}

@test "with no args, the default globs cover every suite in the repo" {
  local defaults f
  defaults=$(sed -n 's/^(( \$# )) || set -- //p' "$CI_TEST")
  [ -n "$defaults" ]
  cd "$BATS_TEST_DIRNAME/.."
  # shellcheck disable=SC2086 # the globs are meant to expand
  defaults=" $(echo $defaults) "
  while IFS= read -r f; do
    [[ $defaults == *" ${f#./} "* ]] || { echo "not run by default: $f"; false; }
  done < <(find . -name '*.bats' -not -path './.git/*')
}

@test "with no args, every default glob matches a suite" {
  local defaults g
  defaults=$(sed -n 's/^(( \$# )) || set -- //p' "$CI_TEST")
  [ -n "$defaults" ]
  cd "$BATS_TEST_DIRNAME/.."
  for g in $defaults; do
    compgen -G "$g" >/dev/null || { echo "matches nothing: $g"; false; }
  done
}
