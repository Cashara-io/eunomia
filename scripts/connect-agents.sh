#!/usr/bin/env bash
# Adds Eunomia's MCP server (<url>/mcp) to AI agents' config files, each with
# its own API token, so the agent can recall/search/write memory with no
# OpenAI key on the server -- the agent is the model. Also installs the
# Eunomia memory skill (SKILL.md for Claude Code, Codex, Hermes) and, for
# Claude Code, optional hooks that inject the skill each session and recall
# memories for every prompt (the MCP server carries the skill without them).
#
#   connect-agents.sh --url http://localhost:8001 --email me@example.com --password '...'
#   connect-agents.sh --url http://localhost:8001 --token <existing-token>
#
# Which agents: --agents auto (default) | all | none | claude,codex,hermes,gemini,
#   cursor,windsurf,opencode,vscode,claude-desktop
#   auto = just the agent that is running this script (detected from its
#          environment, or --agent NAME), or every supported agent when the
#          caller isn't one. Agents that aren't installed yet get their config
#          written anyway, so they are connected the first time they run;
#          --detected-only limits it to ones that look installed.
#
# Credentials can also come from EUNOMIA_EMAIL / EUNOMIA_PASSWORD /
# EUNOMIA_TOKEN (keeps the password out of `ps`). Re-running replaces the
# previous Eunomia entry and revokes the token it minted.
set -euo pipefail

SUPPORTED="claude codex hermes gemini cursor windsurf opencode vscode claude-desktop"
URL="${EUNOMIA_URL:-http://localhost:8001}"
TOKEN="${EUNOMIA_TOKEN:-}"
EMAIL="${EUNOMIA_EMAIL:-}"
PASSWORD="${EUNOMIA_PASSWORD:-}"
AGENTS="auto"
CALLER="${EUNOMIA_AGENT:-}"
DETECTED_ONLY=0

die() { printf '  ✗ %s\n' "$1" >&2; exit 1; }
ok() { printf '  ✓ %s\n' "$1"; }
warn() { printf '  ! %s\n' "$1" >&2; }

while [ $# -gt 0 ]; do
  case "$1" in
    --url) URL="$2"; shift 2 ;;
    --token) TOKEN="$2"; shift 2 ;;
    --email) EMAIL="$2"; shift 2 ;;
    --password) PASSWORD="$2"; shift 2 ;;
    --agents) AGENTS="$2"; shift 2 ;;
    --agent) CALLER="$2"; shift 2 ;;
    --detected-only) DETECTED_ONLY=1; shift ;;
    -h|--help) sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown flag: $1" ;;
  esac
done

URL="${URL%/}"; URL="${URL%/mcp}"
MCP_URL="$URL/mcp"
case "$MCP_URL" in http://*|https://*) ;; *) die "--url must start with http:// or https://" ;; esac
case "$MCP_URL" in *[[:space:]\"\'\\]*) die "--url has unsupported characters" ;; esac
case "$TOKEN" in *[!A-Za-z0-9_-]*) die "--token has unexpected characters" ;; esac

# ---------------------------------------------------------------------------
# JSON helpers: jq (ships with macOS 15+ and most Linux) or python3.
# ---------------------------------------------------------------------------
if [ "${CONNECT_JSON:-}" != py ] && command -v jq >/dev/null 2>&1; then JSON=jq # CONNECT_JSON=py: test hook
elif python3 -c 'import json' >/dev/null 2>&1; then JSON=py
else JSON=""; fi

# jfield <name>: stdin object -> that field ("" if absent)
jfield() {
  if [ "$JSON" = jq ]; then jq -r --arg k "$1" '.[$k] // empty'
  else python3 -c 'import json,sys;print(json.load(sys.stdin).get(sys.argv[1]) or "")' "$1"; fi
}
# jids_named <name>: stdin [{id,name}] -> ids whose name matches
jids_named() {
  if [ "$JSON" = jq ]; then jq -r --arg n "$1" '.[] | select(.name == $n) | .id'
  else python3 -c 'import json,sys;[print(t["id"]) for t in json.load(sys.stdin) if t.get("name")==sys.argv[1]]' "$1"; fi
}
# jmerge <section> <server-json>: stdin file contents ("" ok) -> same JSON with
# <section>.eunomia set. Fails on invalid JSON so a hand-edited file is never clobbered.
jmerge() {
  local input; input="$(cat)"; [ -n "$input" ] || input='{}'
  if [ "$JSON" = jq ]; then
    printf '%s' "$input" | jq --arg s "$1" --argjson v "$2" '.[$s] = ((.[$s] // {}) + {eunomia: $v})'
  else
    printf '%s' "$input" | python3 -c '
import json,sys
d=json.load(sys.stdin); s=sys.argv[1]
d.setdefault(s,{})["eunomia"]=json.loads(sys.argv[2])
print(json.dumps(d,indent=2))' "$1" "$2"
  fi
}

# ---------------------------------------------------------------------------
# API: log in (registering the account if the instance has none yet), mint one
# token per agent.
# ---------------------------------------------------------------------------
JAR=""; LOGGED_IN=0; HTTP_BODY=""; HTTP_CODE=""
cleanup() { [ -z "$JAR" ] || rm -f "$JAR"; }
trap cleanup EXIT

# http <METHOD> <path> [json-body]
http() {
  local out; out="$(mktemp)"
  if [ -n "${3:-}" ]; then
    HTTP_CODE="$(printf '%s' "$3" | curl -sS --max-time 20 -o "$out" -w '%{http_code}' -X "$1" -b "$JAR" -c "$JAR" \
      -H 'Content-Type: application/json' --data-binary @- "$URL$2" 2>/dev/null)" || HTTP_CODE=000
  else
    HTTP_CODE="$(curl -sS --max-time 20 -o "$out" -w '%{http_code}' -X "$1" -b "$JAR" -c "$JAR" "$URL$2" 2>/dev/null)" || HTTP_CODE=000
  fi
  HTTP_BODY="$(cat "$out")"; rm -f "$out"
}

login() {
  [ "$LOGGED_IN" -eq 0 ] || return 0
  [ -n "$EMAIL" ] && [ -n "$PASSWORD" ] || die "need --token, or --email and --password for the Eunomia account"
  JAR="$(mktemp)"
  local body
  if [ "$JSON" = jq ]; then body="$(EMAIL="$EMAIL" PASSWORD="$PASSWORD" jq -n '{email: env.EMAIL, password: env.PASSWORD}')"
  else body="$(EMAIL="$EMAIL" PASSWORD="$PASSWORD" python3 -c 'import json,os;print(json.dumps({"email":os.environ["EMAIL"],"password":os.environ["PASSWORD"]}))')"; fi
  http POST /api/auth/login "$body"
  if [ "$HTTP_CODE" = 401 ]; then
    http POST /api/auth/register "$body"
    [ "$HTTP_CODE" != 409 ] || die "an account for $EMAIL already exists with a different password"
  fi
  [ "$HTTP_CODE" = 200 ] || die "could not log in to $URL (HTTP $HTTP_CODE) -- is Eunomia running?"
  LOGGED_IN=1
}

mint_token() { # <token name>
  login
  http GET /api/auth/tokens
  if [ "$HTTP_CODE" = 200 ]; then
    local id
    for id in $(printf '%s' "$HTTP_BODY" | jids_named "$1"); do http DELETE "/api/auth/tokens/$id"; done
  fi
  http POST /api/auth/tokens "{\"name\":\"$1\"}"
  [ "$HTTP_CODE" = 200 ] || die "could not create an API token (HTTP $HTTP_CODE)"
  printf '%s' "$HTTP_BODY" | jfield token
}

# ---------------------------------------------------------------------------
# Agents
# ---------------------------------------------------------------------------
label() {
  case "$1" in
    claude) echo "Claude Code" ;; codex) echo "Codex" ;; hermes) echo "Hermes" ;; gemini) echo "Gemini CLI" ;;
    cursor) echo "Cursor" ;; windsurf) echo "Windsurf" ;; opencode) echo "OpenCode" ;;
    vscode) echo "VS Code" ;; claude-desktop) echo "Claude Desktop" ;;
  esac
}
config_path() {
  case "$1" in
    claude) echo "$HOME/.claude.json" ;;
    codex) echo "${CODEX_HOME:-$HOME/.codex}/config.toml" ;;
    hermes) echo "${HERMES_HOME:-$HOME/.hermes}/config.yaml" ;;
    gemini) echo "$HOME/.gemini/settings.json" ;;
    cursor) echo "$HOME/.cursor/mcp.json" ;;
    windsurf) echo "$HOME/.codeium/windsurf/mcp_config.json" ;;
    opencode) echo "${XDG_CONFIG_HOME:-$HOME/.config}/opencode/opencode.json" ;;
    vscode) echo "$(app_support)/Code/User/mcp.json" ;;
    claude-desktop) echo "$(app_support)/Claude/claude_desktop_config.json" ;;
  esac
}
app_support() { if [ "$(uname -s)" = Darwin ]; then echo "$HOME/Library/Application Support"; else echo "${XDG_CONFIG_HOME:-$HOME/.config}"; fi; }
installed() {
  case "$1" in
    claude) command -v claude >/dev/null 2>&1 || [ -d "$HOME/.claude" ] ;;
    codex) command -v codex >/dev/null 2>&1 || [ -d "${CODEX_HOME:-$HOME/.codex}" ] ;;
    hermes) command -v hermes >/dev/null 2>&1 || [ -d "${HERMES_HOME:-$HOME/.hermes}" ] ;;
    gemini) command -v gemini >/dev/null 2>&1 || [ -d "$HOME/.gemini" ] ;;
    cursor) command -v cursor >/dev/null 2>&1 || command -v cursor-agent >/dev/null 2>&1 || [ -d "$HOME/.cursor" ] ;;
    windsurf) command -v windsurf >/dev/null 2>&1 || [ -d "$HOME/.codeium" ] ;;
    opencode) command -v opencode >/dev/null 2>&1 || [ -d "${XDG_CONFIG_HOME:-$HOME/.config}/opencode" ] ;;
    vscode) command -v code >/dev/null 2>&1 || [ -d "$(app_support)/Code" ] ;;
    claude-desktop) [ -d "$(app_support)/Claude" ] ;;
  esac
}

# The agent running this script, from the environment it exports to its tools.
detect_caller() {
  [ -z "$CALLER" ] || { echo "$CALLER"; return; }
  if [ -n "${CLAUDECODE:-}" ]; then echo claude
  elif [ -n "${CODEX_SANDBOX:-}${CODEX_SANDBOX_NETWORK_DISABLED:-}${CODEX_CI:-}${CODEX_THREAD_ID:-}" ]; then echo codex
  elif [ -n "${GEMINI_CLI:-}" ]; then echo gemini
  elif [ -n "${CURSOR_AGENT:-}" ]; then echo cursor
  elif [ -n "${OPENCODE:-}" ]; then echo opencode
  elif [ -n "${HERMES_SESSION_ID:-}${HERMES_AGENT:-}" ]; then echo hermes
  fi
}

targets() {
  case "$AGENTS" in
    auto) local c; c="$(detect_caller)"; echo "${c:-$SUPPORTED}" ;;
    all) echo "$SUPPORTED" ;;
    none) ;;
    *) echo "${AGENTS//,/ }" ;;
  esac
}

# Writes stdin to <file> without losing its mode; new files are 0600 (they hold a token).
write_file() {
  mkdir -p "$(dirname "$1")"
  if [ -e "$1" ]; then cat > "$1"; else (umask 077; cat > "$1"); fi
}
strip_block() { awk '/# >>> eunomia >>>/{skip=1;next} /# <<< eunomia <<</{skip=0;next} !skip' "$1" 2>/dev/null || true; }

# connect <agent> <token>; prints one status line, returns non-zero on failure.
connect() {
  local a="$1" tok="$2" file name server section hdr tmp rest block
  file="$(config_path "$a")"; name="$(label "$a")"
  hdr="{\"Authorization\":\"Bearer $tok\"}"
  case "$a" in
    claude|gemini|cursor|windsurf|opencode|vscode|claude-desktop)
      [ -n "$JSON" ] || { warn "$name: needs jq or python3 to edit $file"; return 1; }
      section=mcpServers
      case "$a" in
        claude) server="{\"type\":\"http\",\"url\":\"$MCP_URL\",\"headers\":$hdr}" ;;
        gemini) server="{\"httpUrl\":\"$MCP_URL\",\"headers\":$hdr}" ;;
        cursor) server="{\"url\":\"$MCP_URL\",\"headers\":$hdr}" ;;
        windsurf) server="{\"serverUrl\":\"$MCP_URL\",\"headers\":$hdr}" ;;
        opencode) section=mcp; server="{\"type\":\"remote\",\"url\":\"$MCP_URL\",\"headers\":$hdr,\"enabled\":true}" ;;
        vscode) section=servers; server="{\"type\":\"http\",\"url\":\"$MCP_URL\",\"headers\":$hdr}" ;;
        # Claude Desktop's config only launches local commands; mcp-remote bridges to HTTP (needs Node)
        claude-desktop) server="{\"command\":\"npx\",\"args\":[\"-y\",\"mcp-remote\",\"$MCP_URL\",\"--header\",\"Authorization:Bearer $tok\"]}" ;;
      esac
      if ! tmp="$( { [ -e "$file" ] && cat "$file" || true; } | jmerge "$section" "$server" 2>/dev/null)" || [ -z "$tmp" ]; then
        warn "$name: $file isn't plain JSON (or has a non-object '$section') -- left untouched"; return 1
      fi
      printf '%s\n' "$tmp" | write_file "$file" ;;
    codex)
      rest="$(strip_block "$file")"
      if printf '%s\n' "$rest" | grep -q '^\[mcp_servers\.eunomia\]'; then
        warn "$name: $file already has its own [mcp_servers.eunomia] -- left untouched"; return 1
      fi
      block="$(printf '# >>> eunomia >>>\n[mcp_servers.eunomia]\nurl = "%s"\nhttp_headers = { Authorization = "Bearer %s" }\n# <<< eunomia <<<' "$MCP_URL" "$tok")"
      printf '%s\n%s\n' "$rest" "$block" | write_file "$file" ;;
    hermes)
      rest="$(strip_block "$file")"
      if printf '%s\n' "$rest" | grep -q '^  eunomia:'; then
        warn "$name: $file already has its own eunomia server -- left untouched"; return 1
      fi
      block="$(printf '  # >>> eunomia >>>\n  eunomia:\n    url: "%s"\n    headers:\n      Authorization: "Bearer %s"\n  # <<< eunomia <<<' "$MCP_URL" "$tok")"
      # ENVIRON, not `awk -v`: BSD awk rejects newlines in -v values
      printf '%s\n' "$rest" | BLK="$block" awk '
        /^mcp_servers:[[:space:]]*(\{\})?[[:space:]]*$/ && !done { print "mcp_servers:"; print ENVIRON["BLK"]; done=1; next }
        { print }
        END { if (!done) { print "mcp_servers:"; print ENVIRON["BLK"] } }' | write_file "$file" ;;
  esac
  ok "$(printf '%-14s %s' "$name" "$file")"
  case "$a" in
    claude) install_skill "$tok" "$HOME/.claude/skills/eunomia-memory/SKILL.md"; install_claude_hooks "$tok" ;;
    codex) install_skill "$tok" "${CODEX_HOME:-$HOME/.codex}/skills/eunomia-memory/SKILL.md" ;;
    hermes) install_skill "$tok" "${HERMES_HOME:-$HOME/.hermes}/skills/eunomia-memory/SKILL.md" ;;
  esac
  return 0
}

# ---------------------------------------------------------------------------
# Memory skill + Claude Code hooks
# ---------------------------------------------------------------------------
SKILL_FRONTMATTER='---
name: eunomia-memory
description: Use Eunomia, the user'"'"'s long-term memory, on every task. Recall what'"'"'s relevant before answering or acting, and save durable new facts, preferences and decisions as you learn them.
---
'

# install_skill <token> <path>: the user's skill (Memory skill page) as SKILL.md
install_skill() {
  local body
  body="$(curl -fsS --max-time 10 -H "Authorization: Bearer $1" "$URL/api/settings" 2>/dev/null | jfield memory_skill)"
  if [ -z "$body" ]; then warn "    couldn't fetch the memory skill -- skipped $2"; return 1; fi
  case "$body" in ---*) ;; *) body="$SKILL_FRONTMATTER
$body" ;; esac
  printf '%s\n' "$body" | write_file "$2"
  printf '      + skill   %s\n' "$2"
}

HOOK_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/eunomia"

# install_claude_hooks <token>: optional (the MCP server already carries the skill). SessionStart
# injects the skill, UserPromptSubmit injects memories recalled for the prompt; existing hooks are
# kept. When ~/.claude/settings.json can't be read or written (not plain JSON, or the agent running
# this may not change its own settings), skip with one note and carry on.
install_claude_hooks() {
  local file="$HOME/.claude/settings.json" tmp cmd="bash \"$HOOK_DIR/hook.sh\""
  if tmp="$( { [ ! -e "$file" ] || cat "$file"; } 2>/dev/null | jhooks "$cmd" 2>/dev/null)" && [ -n "$tmp" ] \
    && { mkdir -p "$HOOK_DIR" \
      && (umask 077; printf 'EUNOMIA_URL=%s\nEUNOMIA_TOKEN=%s\n' "$URL" "$1" > "$HOOK_DIR/env") \
      && write_hook_script > "$HOOK_DIR/hook.sh" && chmod 755 "$HOOK_DIR/hook.sh" \
      && printf '%s\n' "$tmp" | write_file "$file"; } 2>/dev/null; then
    printf '      + hooks   %s (skill each session, recall on every prompt)\n' "$file"
  else
    printf '      - hooks   skipped: %s is not editable here (optional, the MCP server carries the skill)\n' "$file"
  fi
}

# jhooks <command>: stdin settings.json -> with one Eunomia entry per hook event
jhooks() {
  local input; input="$(cat)"; [ -n "$input" ] || input='{}'
  if [ "$JSON" = jq ]; then
    printf '%s' "$input" | jq --arg c "$1" '
      .hooks = (.hooks // {})
      | reduce (["SessionStart", "session"], ["UserPromptSubmit", "prompt"]) as [$e, $m] (.;
          .hooks[$e] = ([(.hooks[$e] // [])[] | select(([.hooks[]?.command] | map(test("eunomia/hook.sh")) | any) | not)]
                        + [{hooks: [{type: "command", command: ($c + " " + $m), timeout: 5}]}]))'
  else
    printf '%s' "$input" | python3 -c '
import json,sys
d=json.load(sys.stdin); c=sys.argv[1]; h=d.setdefault("hooks",{})
for e,m in (("SessionStart","session"),("UserPromptSubmit","prompt")):
    keep=[x for x in h.get(e,[]) if not any("eunomia/hook.sh" in (y.get("command") or "") for y in x.get("hooks",[]))]
    h[e]=keep+[{"hooks":[{"type":"command","command":c+" "+m,"timeout":5}]}]
print(json.dumps(d,indent=2))' "$1"
  fi
}

write_hook_script() {
  cat <<'HOOK'
#!/usr/bin/env bash
# Claude Code hook installed by Eunomia's connect-agents.sh (re-run it to update).
#   session: print your Eunomia memory skill, live from the server
#   prompt:  print memories Eunomia recalls for the prompt Claude Code is about to see
# Prints nothing and exits 0 on any failure, so it can never block a prompt.
. "${EUNOMIA_ENV:-${XDG_CONFIG_HOME:-$HOME/.config}/eunomia/env}" 2>/dev/null || exit 0
[ -n "${EUNOMIA_URL:-}" ] && [ -n "${EUNOMIA_TOKEN:-}" ] || exit 0
api() { curl -fsS --max-time 4 -H "Authorization: Bearer $EUNOMIA_TOKEN" -H 'Content-Type: application/json' "$@" 2>/dev/null; }
if command -v jq >/dev/null 2>&1; then J=jq; else J=py; fi

case "${1:-}" in
  session)
    if [ $J = jq ]; then skill="$(api "$EUNOMIA_URL/api/settings" | jq -r '.memory_skill // empty')"
    else skill="$(api "$EUNOMIA_URL/api/settings" | python3 -c 'import json,sys;print(json.load(sys.stdin).get("memory_skill",""))' 2>/dev/null)"; fi
    [ -n "$skill" ] || exit 0
    # drop the SKILL.md frontmatter
    printf '%s
' "$skill" | awk 'NR==1 && /^---$/ {fm=1; next} fm && /^---$/ {fm=0; next} !fm && (seen || NF) {seen=1; print}'
    ;;
  prompt)
    input="$(cat)"
    if [ $J = jq ]; then
      body="$(printf '%s' "$input" | jq -c '{query: ((.prompt // "")[0:500]), limit: 6, max_tokens: 600}')"
    else
      body="$(printf '%s' "$input" | python3 -c 'import json,sys;print(json.dumps({"query":(json.load(sys.stdin).get("prompt") or "")[:500],"limit":6,"max_tokens":600}))' 2>/dev/null)"
    fi
    case "$body" in *'"query":""'*|"") exit 0 ;; esac
    res="$(api -X POST --data-binary @- "$EUNOMIA_URL/api/tools/recall" <<<"$body")" || exit 0
    if [ $J = jq ]; then
      lines="$(printf '%s' "$res" | jq -r '.results[]? | "- " + (.text | gsub("[[:space:]]+"; " ") | .[0:400]) + " (" + .id + ")"')"
    else
      lines="$(printf '%s' "$res" | python3 -c '
import json,sys
for r in json.load(sys.stdin).get("results",[]): print("- "+" ".join(r["text"].split())[:400]+" ("+r["id"]+")")' 2>/dev/null)"
    fi
    [ -n "$lines" ] || exit 0
    printf 'Relevant memories from Eunomia, recalled for this message (use them if they help; cite naturally):
%s
' "$lines"
    ;;
esac
exit 0
HOOK
}

main() {
  [ -n "$TOKEN" ] || [ -n "$JSON" ] || die "need jq or python3 to talk to the Eunomia API"
  local list a failed=0 tok host n=0
  list="$(targets)"
  for a in $list; do
    case " $SUPPORTED " in *" $a "*) ;; *) die "unknown agent '$a' (supported: ${SUPPORTED// /, })" ;; esac
  done
  host="$(hostname -s 2>/dev/null || echo host)"
  [ -n "$TOKEN" ] || [ -z "$list" ] || login # in this shell, so the session is reused for every agent
  for a in $list; do
    if [ "$DETECTED_ONLY" -eq 1 ] && ! installed "$a"; then continue; fi
    n=$((n + 1))
    tok="${TOKEN:-$(mint_token "$(label "$a") ($host)")}"
    connect "$a" "$tok" || failed=1
  done
  [ "$n" -gt 0 ] || { warn "no agents to connect"; return 0; }
  echo
  printf '  Eunomia MCP: %s\n' "$MCP_URL"
  printf '  Restart each agent to pick it up (Claude Code: exit and run claude --continue). ChatGPT and Claude.ai need a public https URL: add it by hand under Connectors.\n'
  return "$failed"
}

main
