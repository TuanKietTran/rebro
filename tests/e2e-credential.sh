#!/usr/bin/env bash
# End-to-end on macOS/Linux: real git + the real OS credential store (macOS login Keychain) + rebro as
# credential helper, then `rebro import` against a fake helper. Same checks as e2e-credential.ps1.
# Isolated: own REBRO_HOME, own keychain service name, fake tokens, temp git config, gh/glab untouched.
#   tests/e2e-credential.sh [path/to/rebro]
# bash 3.2 compatible (macOS /bin/bash): no associative arrays.
set -u
EXE=${1:-"$(cd "$(dirname "$0")/.." && pwd)/target/debug/rebro"}
TMP=$(mktemp -d)
export REBRO_HOME="$TMP" REBRO_KEYRING_SERVICE=rebro-e2e REBRO_SKIP_TOOLS=1 REBRO_NO_BROWSER=1
export GIT_CONFIG_GLOBAL="$TMP/gitconfig" GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0
FAIL=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; FAIL=$((FAIL + 1)); }

cat > "$TMP/accounts.toml" <<'EOF'
[[account]]
forge = "github"
host = "github.com"
username = "me"
default = true

[[account]]
forge = "github"
host = "github.com"
username = "work-bot"
owners = ["acme"]

[[account]]
forge = "forgejo"
host = "codeberg.org"
username = "cb-me"
default = true
EOF

KEYS="github.com/me github.com/work-bot codeberg.org/cb-me"
secret_for() {
  case "$1" in
    github.com/me) echo tok-me-1 ;;
    github.com/work-bot) echo tok-work-2 ;;
    codeberg.org/cb-me) echo tok-cb-3 ;;
  esac
}
put() {
  if [ "$(uname)" = Darwin ]; then
    # keyring's macOS backend: generic password, service = REBRO_KEYRING_SERVICE, account = host/user.
    # -A: items made by `security` are otherwise readable only by `security`; rebro reading them would
    # raise a Keychain "allow access?" dialog, which blocks forever on a headless runner. Fake tokens only.
    security add-generic-password -U -A -s rebro-e2e -a "$1" -w "$2" >/dev/null
  else
    printf %s "$2" | secret-tool store --label="$1" service rebro-e2e username "$1"
  fi
}
del() {
  if [ "$(uname)" = Darwin ]; then
    security delete-generic-password -s rebro-e2e -a "$1" >/dev/null 2>&1
  else
    secret-tool clear service rebro-e2e username "$1" 2>/dev/null
  fi
}
cleanup() { for k in $KEYS; do del "$k"; done; rm -rf "$TMP"; }
trap cleanup EXIT
for k in $KEYS; do put "$k" "$(secret_for "$k")"; done

"$EXE" sync
echo "--- git config written by sync:"; cat "$GIT_CONFIG_GLOBAL"

fill() { # host path -> "username=… password=…"
  printf 'protocol=https\nhost=%s\npath=%s\n\n' "$1" "$2" | git credential fill 2>/dev/null | grep -E '^(username|password)=' | tr '\n' ' ' | sed 's/ $//'
}
check() { # host path expected
  got=$(fill "$1" "$2")
  if [ "$got" = "$3" ]; then ok "$1/$2"; else bad "$1/$2 -> $(echo "$got" | sed 's/password=[^ ]*/password=***/')"; fi
}
check github.com TuanKietTran/edge.git "username=me password=tok-me-1"
check github.com acme/app.git "username=work-bot password=tok-work-2"
check github.com me/x.git "username=me password=tok-me-1"
check codeberg.org cb-me/repo.git "username=cb-me password=tok-cb-3"

unk=$(printf 'protocol=https\nhost=example.org\n\n' | "$EXE" git-credential get)
if [ -z "$unk" ]; then ok "unknown host: no answer"; else bad "unknown host answered"; fi

printf 'protocol=https\nhost=github.com\nusername=me\npassword=WRONG\n\n' | "$EXE" git-credential erase
if [ "$(fill github.com me/x.git)" = "username=me password=tok-me-1" ]; then ok "erase ignored, token intact"; else bad "erase changed the token"; fi

# import: a fake helper and a fake env token are found, verified (and rejected by the forge), never printed
git config --global --add credential.helper '!f() { test "$1" = get && echo username=x && echo password=fake-helper-tok; }; f'
out=$(GH_TOKEN=fake-env-tok "$EXE" import --dry-run --json 2>&1)
echo "$out" | grep -q '"source": "git (custom helper)"' && ok "import found the git helper's credential" || bad "import missed the git helper"
echo "$out" | grep -q '"source": "env GH_TOKEN"' && ok "import found GH_TOKEN" || bad "import missed GH_TOKEN"
echo "$out" | grep -q '"result": "failed"' && ok "fake tokens rejected by the forge" || bad "fake tokens not rejected"
if echo "$out" | grep -q 'fake-helper-tok\|fake-env-tok'; then bad "import printed a token"; else ok "no token in import output"; fi
[ "$(grep -c '^\[\[account\]\]' "$TMP/accounts.toml")" = 3 ] && ok "dry run stored nothing" || bad "dry run changed accounts.toml"

"$EXE" doctor
if [ "$FAIL" -ne 0 ]; then echo "$FAIL check(s) failed"; exit 1; fi
echo "ALL PASSED"
