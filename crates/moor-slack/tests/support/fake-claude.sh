#!/bin/sh
dir=$(dirname "$0")
if [ "$1" = "--version" ]; then
  if [ -f "$dir/version" ]; then cat "$dir/version"; else echo "2.1.304 (Claude Code)"; fi
  exit 0
fi
n=0
until mkdir "$dir/run-$n" 2>/dev/null; do
  [ -d "$dir/run-$n" ] || exit 1
  n=$((n + 1))
done
out="$dir/run-$n"
echo $$ > "$out/pid"
for arg in "$@"; do printf '%s\n' "$arg"; done > "$out/argv"
tr '\0' '\n' < /proc/$$/environ > "$out/env"
pwd > "$out/cwd"
ls -l /proc/$$/fd > "$out/fds" 2>&1
cat > "$out/stdin"
cat <&3 > "$out/fd3" 2>/dev/null
token=$(cat "$out/fd3")
if [ -z "$token" ]; then
  token=$(printf '%s' "$HTTPS_PROXY" | sed -n 's|^http://lease:\([^@]*\)@.*|\1|p')
fi
if [ -f "$dir/mode" ]; then token=$(cat "$dir/mode"); fi
init() {
  echo "{\"type\":\"system\",\"subtype\":\"init\",\"tools\":[$1],\"mcp_servers\":[],\"skills\":[],\"slash_commands\":[],\"plugins\":[{\"name\":\"cc-plugin-telemetry\"}]}"
}
case "$token" in
  *-good-*)
    init
    echo '{"type":"result","subtype":"success","is_error":false,"result":"OK","total_cost_usd":0.0125}'
    ;;
  *-tool-*)
    init '"Bash"'
    sleep 30
    echo '{"type":"result","subtype":"success","is_error":false,"result":"ran a tool"}'
    ;;
  *-hang-*)
    init
    : > "$out/waiting"
    sleep 60
    ;;
  *-listed-*)
    echo '{"type":"system","subtype":"init","tools":["mcp__moor__list_boxes"],"mcp_servers":[{"name":"moor","status":"connected"}],"skills":[],"slash_commands":[],"plugins":[]}'
    echo '{"type":"result","subtype":"success","is_error":false,"result":"OK"}'
    ;;
  *-server-*)
    echo '{"type":"system","subtype":"init","tools":[],"mcp_servers":[{"name":"github","status":"connected"}],"skills":[],"slash_commands":[],"plugins":[]}'
    sleep 30
    ;;
  *-extra-*)
    init '"mcp__moor__list_boxes","mcp__moor__steer_box"'
    sleep 30
    ;;
  *-mcp-*)
    cfg=""
    prev=""
    for arg in "$@"; do
      if [ "$prev" = "--mcp-config" ]; then cfg=$arg; fi
      prev=$arg
    done
    cmd=$(sed -n 's/^ *"command": "\(.*\)",$/\1/p' "$cfg")
    sed -n '/"args": \[/,/\]/p' "$cfg" | sed '1d;$d' | sed 's/^ *"\(.*\)",\{0,1\}$/\1/' > "$out/server-args"
    set --
    while IFS= read -r a; do set -- "$@" "$a"; done < "$out/server-args"
    calls="$dir/calls-$n"
    [ -f "$calls" ] || calls="$dir/calls"
    hello='{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fake","version":"0"}}}'
    mkfifo "$out/server-in"
    "$cmd" "$@" < "$out/server-in" > "$out/mcp-list" 2> "$out/mcp-err" &
    server=$!
    exec 4> "$out/server-in"
    printf '%s\n%s\n' "$hello" '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' >&4
    i=0
    until grep -q '"id":1,' "$out/mcp-list" || [ $i -ge 500 ]; do sleep 0.02; i=$((i + 1)); done
    tr '\0' '\n' < /proc/$server/environ > "$out/server-env"
    exec 4>&-
    wait $server
    tools=$(grep '"id":1,' "$out/mcp-list" | grep -o '"name":"[a-z_]*"' | sed 's/"name":"\(.*\)"/"mcp__moor__\1"/' | paste -sd, -)
    echo "{\"type\":\"system\",\"subtype\":\"init\",\"tools\":[$tools],\"mcp_servers\":[{\"name\":\"moor\",\"status\":\"connected\"}],\"skills\":[],\"slash_commands\":[],\"plugins\":[]}"
    sleep 0.3
    {
      echo "$hello"
      echo '{"jsonrpc":"2.0","method":"notifications/initialized"}'
      if [ -f "$calls" ]; then cat "$calls"; fi
    } | "$cmd" "$@" > "$out/mcp" 2>> "$out/mcp-err"
    if [ -f "$dir/hang-after-tools" ]; then sleep 60; fi
    echo '{"type":"result","subtype":"success","is_error":false,"result":"OK"}'
    ;;
  *-channel-*)
    init
    echo '{"type":"result","subtype":"success","is_error":false,"result":"<!channel> deploy & <@U9> done"}'
    ;;
  *-bare-*)
    echo '{"type":"result","subtype":"success","is_error":false,"result":"no init"}'
    ;;
  "")
    echo '{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}'
    ;;
  *-crash-*)
    echo "crashed reading $token" >&2
    ;;
  *-echo-*)
    echo "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":true,\"result\":\"Bad token $token\"}"
    ;;
  *)
    init
    echo '{"type":"result","subtype":"success","is_error":true,"api_error_status":401,"result":"Failed to authenticate. API Error: 401 OAuth access token is invalid."}'
    ;;
esac
