#!/usr/bin/env bash
# Tests scripts/connect-agents.sh against a throwaway HOME and a mock Eunomia

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="$HERE/../connect-agents.sh"
PASS=0; FAIL=0
TMP="$(mktemp -d)"; trap 'kill "${MOCK_PID:-0}" 2>/dev/null; rm -rf "$TMP"' EXIT

check() { # check <description> <command...>
  local d="$1"; shift
  if "$@" >/dev/null 2>&1; then PASS=$((PASS + 1)); else FAIL=$((FAIL + 1)); echo "FAIL: $d"; fi
}
new_home() { H="$TMP/home$RANDOM$RANDOM"; mkdir -p "$H"; }
# run <extra env...> -- <args...>; clean env so the real CLAUDECODE etc. never leaks in
run() {
  local envs=(); while [ "$1" != "--" ]; do envs+=("$1"); shift; done; shift
  env -i PATH="$PATH" HOME="$H" CONNECT_JSON="${CONNECT_JSON:-}" ${envs[@]+"${envs[@]}"} bash "$SCRIPT" --url http://localhost:1 "$@"
}
has() { grep -q -- "$2" "$1"; }
jq_ok() { python3 -c 'import json,sys;json.load(open(sys.argv[1]))' "$1"; }
auth_of() { python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["mcpServers"]["eunomia"]["headers"]["Authorization"])' "$1"; }
mode() { [ "$(stat -c %a "$1" 2>/dev/null || stat -f %Lp "$1")" = 600 ]; }

# --- every agent, fixed token -------------------------------------------------
new_home
run -- --token tok_ALL --agents all >/dev/null
for f in .claude.json .gemini/settings.json .cursor/mcp.json .codeium/windsurf/mcp_config.json .config/opencode/opencode.json; do
  check "$f is valid JSON" jq_ok "$H/$f"
  check "$f carries the token" has "$H/$f" "Bearer tok_ALL"
  check "$f points at /mcp" has "$H/$f" "http://localhost:1/mcp"
  check "$f is private (0600)" mode "$H/$f"
done
check "claude uses type http" has "$H/.claude.json" '"type": "http"'
check "gemini uses httpUrl" has "$H/.gemini/settings.json" '"httpUrl"'
check "windsurf uses serverUrl" has "$H/.codeium/windsurf/mcp_config.json" '"serverUrl"'
check "opencode uses the mcp section" has "$H/.config/opencode/opencode.json" '"mcp"'
check "codex toml" has "$H/.codex/config.toml" '^\[mcp_servers.eunomia\]'
check "codex toml headers" has "$H/.codex/config.toml" 'Bearer tok_ALL'
check "hermes yaml" has "$H/.hermes/config.yaml" '^mcp_servers:'
check "hermes yaml entry" has "$H/.hermes/config.yaml" '^  eunomia:'
AS="$H/Library/Application Support"; [ "$(uname -s)" = Darwin ] || AS="$H/.config"
check "vscode mcp.json uses servers" has "$AS/Code/User/mcp.json" '"servers"'
check "vscode carries the token" has "$AS/Code/User/mcp.json" 'Bearer tok_ALL'
check "claude desktop bridges via mcp-remote" has "$AS/Claude/claude_desktop_config.json" 'mcp-remote'
check "claude desktop header has no space (mcp-remote quirk)" has "$AS/Claude/claude_desktop_config.json" 'Authorization:Bearer tok_ALL'
check "claude hooks still installed when the server is unreachable" has "$H/.claude/settings.json" 'eunomia/hook.sh'

# --- idempotent, and other config survives --------------------------------------
new_home
echo '{"theme":"dark","mcpServers":{"other":{"url":"http://x"}}}' > "$H/.claude.json"
mkdir -p "$H/.codex" "$H/.hermes"
printf 'model = "o3"\n\n[mcp_servers.keep]\nurl = "http://k"\n' > "$H/.codex/config.toml"
printf 'model: x\nmcp_servers:\n  keep:\n    url: "http://k"\nlogging: on\n' > "$H/.hermes/config.yaml"
run -- --token tok_ONE --agents claude,codex,hermes >/dev/null
sum1="$(cat "$H/.claude.json" "$H/.codex/config.toml" "$H/.hermes/config.yaml" | cksum)"
run -- --token tok_ONE --agents claude,codex,hermes >/dev/null
check "second run changes nothing" test "$sum1" = "$(cat "$H/.claude.json" "$H/.codex/config.toml" "$H/.hermes/config.yaml" | cksum)"
check "claude keeps other keys" has "$H/.claude.json" '"theme": "dark"'
check "claude keeps other servers" has "$H/.claude.json" '"other"'
check "codex keeps its settings" has "$H/.codex/config.toml" 'model = "o3"'
check "codex keeps other servers" has "$H/.codex/config.toml" 'mcp_servers.keep'
check "codex has one eunomia table" test "$(grep -c '^\[mcp_servers.eunomia\]' "$H/.codex/config.toml")" = 1
check "hermes keeps other servers" has "$H/.hermes/config.yaml" '^  keep:'
check "hermes keeps other keys" has "$H/.hermes/config.yaml" '^logging: on'
check "hermes has one mcp_servers key" test "$(grep -c '^mcp_servers:' "$H/.hermes/config.yaml")" = 1
check "hermes has one eunomia entry" test "$(grep -c '^  eunomia:' "$H/.hermes/config.yaml")" = 1
if python3 -c 'import yaml' 2>/dev/null; then
  check "hermes yaml parses with both servers" python3 -c '
import sys,yaml
d=yaml.safe_load(open(sys.argv[1]))["mcp_servers"]; assert set(d)=={"keep","eunomia"}' "$H/.hermes/config.yaml"
fi
run -- --token tok_TWO --agents claude >/dev/null
check "a new token replaces the old one" test "$(grep -c tok_ONE "$H/.claude.json")" = 0
check "a new token is written" has "$H/.claude.json" tok_TWO

# --- hermes with an empty `mcp_servers: {}` and with no key at all --------------
new_home; mkdir -p "$H/.hermes"; printf 'mcp_servers: {}\n' > "$H/.hermes/config.yaml"
run -- --token t --agents hermes >/dev/null
check "mcp_servers: {} is replaced, not duplicated" test "$(grep -c '^mcp_servers:' "$H/.hermes/config.yaml")" = 1

# --- which agents ------------------------------------------------------------
new_home
run CLAUDECODE=1 -- --token t >/dev/null
check "inside Claude Code: only claude" test -f "$H/.claude.json" -a ! -e "$H/.codex" -a ! -e "$H/.hermes"
new_home
run CODEX_SANDBOX=seatbelt -- --token t >/dev/null
check "inside Codex: only codex" test -f "$H/.codex/config.toml" -a ! -e "$H/.claude.json"
new_home
run -- --token t --agent hermes >/dev/null
check "--agent picks one" test -f "$H/.hermes/config.yaml" -a ! -e "$H/.claude.json"
new_home
run -- --token t >/dev/null
check "no agent detected: all of them" test -f "$H/.claude.json" -a -f "$H/.codex/config.toml" -a -f "$H/.hermes/config.yaml" -a -f "$H/.cursor/mcp.json"
new_home; mkdir -p "$H/.gemini"
run PATH=/usr/bin:/bin -- --token t --detected-only >/dev/null # no real agent CLIs on PATH
check "--detected-only skips what isn't installed" test -f "$H/.gemini/settings.json" -a ! -e "$H/.claude.json"
new_home
run -- --token t --agents none >/dev/null
check "--agents none writes nothing" test ! -e "$H/.claude.json" -a ! -e "$H/.codex" -a ! -e "$H/.hermes" -a ! -e "$H/.gemini"

# --- never clobbers what it can't parse -------------------------------------
new_home; mkdir -p "$H/.codex"
echo '{ // not json
 "a": 1 }' > "$H/.claude.json"
printf '[mcp_servers.eunomia]\nurl = "mine"\n' > "$H/.codex/config.toml"
run -- --token t --agents claude,codex,gemini >/dev/null 2>&1; rc=$?
check "failures are reported" test "$rc" -ne 0
check "unparseable JSON is left alone" has "$H/.claude.json" '// not json'
check "a hand-written codex entry is left alone" has "$H/.codex/config.toml" 'url = "mine"'
check "other agents still get connected" has "$H/.gemini/settings.json" 'Bearer t'

# --- Claude Code hooks are optional: an uneditable settings.json is a note, not a failure ---
new_home; mkdir -p "$H/.claude"
echo '{ // my settings' > "$H/.claude/settings.json"
out="$(run -- --token t --agents claude 2>"$TMP/err")"; rc=$?
check "settings.json not plain JSON: still succeeds" test "$rc" -eq 0
check "settings.json not plain JSON: left untouched" has "$H/.claude/settings.json" '// my settings'
check "settings.json not plain JSON: one optional note" test "$(printf '%s\n' "$out" | grep -c 'hooks   skipped.*optional')" = 1
check "settings.json not plain JSON: no warning about it" bash -c "! grep -q 'settings.json\|hooks' '$TMP/err'"
check "settings.json not plain JSON: MCP still connected" has "$H/.claude.json" 'Bearer t'
new_home; mkdir -p "$H/.claude"
echo '{"model":"opus"}' > "$H/.claude/settings.json"; chmod 444 "$H/.claude/settings.json"
out="$(run -- --token t --agents claude 2>"$TMP/err")"; rc=$?
chmod 644 "$H/.claude/settings.json"
if [ "$(id -u)" != 0 ]; then # root writes through 0444
  check "read-only settings.json (self-modification blocked): still succeeds" test "$rc" -eq 0
  check "read-only settings.json: unchanged" test "$(cat "$H/.claude/settings.json")" = '{"model":"opus"}'
  check "read-only settings.json: one optional note, no warning" bash -c "[ \"\$(printf '%s\n' \"\$1\" | grep -c 'hooks   skipped')\" = 1 ] && ! grep -q 'settings.json\|hooks\|denied' '$TMP/err'" _ "$out"
fi

# --- bad input ---------------------------------------------------------------
new_home
check "unknown agent is rejected" bash -c "! run -- --token t --agents nope 2>/dev/null" 
check "bad url is rejected" bash -c "! env -i PATH=\"$PATH\" HOME=\"$H\" bash \"$SCRIPT\" --url 'ftp://x' --token t 2>/dev/null"
check "token with junk is rejected" bash -c "! env -i PATH=\"$PATH\" HOME=\"$H\" bash \"$SCRIPT\" --token 'a\"b' 2>/dev/null"

# --- minting tokens through the API ---------------------------------------
python3 "$HERE/mock-eunomia.py" "$TMP/api.log" > "$TMP/port" & MOCK_PID=$!; disown
for _ in $(seq 1 50); do [ -s "$TMP/port" ] && break; sleep 0.1; done
PORT="$(cat "$TMP/port")"
api() { env -i PATH="$PATH" HOME="$H" CONNECT_JSON="${CONNECT_JSON:-}" EUNOMIA_PASSWORD="${PW:-correct-horse}" bash "$SCRIPT" --url "http://127.0.0.1:$PORT" --email a@b.co "$@"; }

new_home
api --agents claude,codex >/dev/null; rc=$?
check "first run registers the account and mints tokens" test "$rc" -eq 0
check "register was called" has "$TMP/api.log" 'POST /api/auth/register'
tok_claude="$(auth_of "$H/.claude.json")"
tok_codex="$(sed -n 's/.*Bearer \([^"]*\)".*/\1/p' "$H/.codex/config.toml")"
check "each agent gets its own token" test -n "$tok_claude" -a "$tok_claude" != "Bearer $tok_codex"
api --agents claude,codex >/dev/null
check "re-running revokes the previous tokens" has "$TMP/api.log" 'DELETE /api/auth/tokens/api_token:t1'
check "the config holds the fresh token" test "$(auth_of "$H/.claude.json")" != "$tok_claude"
# --- memory skill + Claude Code hooks ------------------------------------------
check "skill installed for claude" has "$H/.claude/skills/eunomia-memory/SKILL.md" '^name: eunomia-memory'
check "skill installed for codex" has "$H/.codex/skills/eunomia-memory/SKILL.md" '# Eunomia memory'
check "hook env is private" mode "$H/.config/eunomia/env"
check "hook env has the claude token" grep -q "EUNOMIA_TOKEN=$(auth_of "$H/.claude.json" | sed 's/Bearer //')" "$H/.config/eunomia/env"
HOOK() { env -i PATH="$PATH" HOME="$H" bash "$H/.config/eunomia/hook.sh" "$@"; }
export H
check "session hook prints the skill" bash -c "$(declare -f HOOK); HOOK session | grep -q '^# Eunomia memory'"
check "session hook strips frontmatter" bash -c "$(declare -f HOOK); ! HOOK session | grep -q '^name:'"
check "prompt hook injects recalled memories" bash -c "$(declare -f HOOK); echo '{\"prompt\":\"what does Ada like?\"}' | HOOK prompt | grep -q '^- Ada prefers async updates. (memory:1)'"
check "prompt hook is silent when nothing is recalled" bash -c "$(declare -f HOOK); test -z \"\$(echo '{\"prompt\":\"weather\"}' | HOOK prompt)\""
check "prompt hook is silent on an empty prompt" bash -c "$(declare -f HOOK); test -z \"\$(echo '{}' | HOOK prompt)\""
python3 -c '
import json,sys
d=json.load(open(sys.argv[1]))["hooks"]
assert [len(d[e]) for e in ("SessionStart","UserPromptSubmit")]==[1,1], d' "$H/.claude/settings.json" && check "one hook per event after two runs" true || check "one hook per event after two runs" false
new_home
mkdir -p "$H/.claude"; echo '{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"my-own-hook"}]}]},"model":"opus"}' > "$H/.claude/settings.json"
api --agents claude >/dev/null 2>&1
check "existing hooks survive" has "$H/.claude/settings.json" 'my-own-hook'
check "other settings survive" has "$H/.claude/settings.json" '"model": "opus"'
check "eunomia hook added beside them" has "$H/.claude/settings.json" 'eunomia/hook.sh.* prompt'

PW=wrong api --agents claude >/dev/null 2>&1; rc=$?
check "a wrong password for an existing account fails" test "$rc" -ne 0
kill "$MOCK_PID" 2>/dev/null; sleep 0.3
check "hooks never fail when Eunomia is down" bash -c "$(declare -f HOOK); echo '{\"prompt\":\"Ada\"}' | HOOK prompt; HOOK session"

echo "connect-agents: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
