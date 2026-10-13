#!/bin/sh
# Minimal gh for a credential-less sandbox: pr create, pr view, pr list only.
# Sends no credential and no Authorization header; the proxy adds credentials.
set -u

die() { printf '%s\n' "$1" >&2; exit "${2:-2}"; }
usage() { die 'gh: moor supports only: gh pr create, gh pr view, gh pr list'; }

API=${MOOR_GH_API:-}
REPO=${MOOR_GH_REPO:-}
[ -n "$API" ] && [ -n "$REPO" ] || die 'gh: MOOR_GH_API and MOOR_GH_REPO must be set'
API=${API%/}
OWNER=${REPO%%/*}

[ $# -ge 2 ] || usage
[ "$1" = pr ] || usage
CMD=$2
case $CMD in create | view | list) ;; *) usage ;; esac
shift 2

lc() { printf '%s' "$1" | tr 'A-Z' 'a-z'; }

TITLE='' HAVE_TITLE=0 BODY='' BASE='' HEAD='' DRAFT=false
STATE=open LIMIT=30 NUM='' ARG_REPO='' JSON=0

while [ $# -gt 0 ]; do
  flag=$1
  shift
  val=''
  case $flag in
    --*=*) val=${flag#*=}; flag=${flag%%=*}; hasval=1 ;;
    *) hasval=0 ;;
  esac
  # Flags that take a value: fetch it from the next argument if not inline.
  case $flag in
    -R | --repo | -H | --head | --json) need=1 ;;
    -t | --title | -b | --body | -F | --body-file | -B | --base) need=$([ "$CMD" = create ] && echo 1 || echo 0) ;;
    -s | --state | -L | --limit) need=$([ "$CMD" = list ] && echo 1 || echo 0) ;;
    *) need=0 ;;
  esac
  if [ "$need" = 1 ] && [ "$hasval" = 0 ]; then
    [ $# -gt 0 ] || die "gh: flag needs an argument: $flag"
    val=$1
    shift
  fi
  case $flag in
    -R | --repo) ARG_REPO=$val ;;
    -H | --head) [ "$CMD" = view ] && die "gh: unsupported flag: $flag"; HEAD=$val ;;
    --json) JSON=1 ;;
    -t | --title) [ "$CMD" = create ] || die "gh: unsupported flag: $flag"; TITLE=$val; HAVE_TITLE=1 ;;
    -b | --body) [ "$CMD" = create ] || die "gh: unsupported flag: $flag"; BODY=$val ;;
    -F | --body-file)
      [ "$CMD" = create ] || die "gh: unsupported flag: $flag"
      if [ "$val" = - ]; then BODY=$(cat); else BODY=$(cat "$val") || die "gh: cannot read $val" 1; fi ;;
    -B | --base) [ "$CMD" = create ] || die "gh: unsupported flag: $flag"; BASE=$val ;;
    -d | --draft) [ "$CMD" = create ] || die "gh: unsupported flag: $flag"; DRAFT=true ;;
    -s | --state) [ "$CMD" = list ] || die "gh: unsupported flag: $flag"; STATE=$val ;;
    -L | --limit) [ "$CMD" = list ] || die "gh: unsupported flag: $flag"; LIMIT=$val ;;
    -*) die "gh: unsupported flag: $flag" ;;
    *) if [ "$CMD" = view ] && [ -z "$NUM" ]; then NUM=$flag; else die "gh: unsupported flag: $flag"; fi ;;
  esac
done

mismatch() { die "gh: moor only reaches $REPO"; }
if [ -n "$ARG_REPO" ] && [ "$(lc "$ARG_REPO")" != "$(lc "$REPO")" ]; then mismatch; fi
case $HEAD in
  *:*) [ "$(lc "${HEAD%%:*}")" = "$(lc "$OWNER")" ] || mismatch ;;
esac
case $NUM in
  *[!0-9]*) die "gh: pull request must be a number: $NUM" ;;
esac
case $LIMIT in
  *[!0-9]* | '') die 'gh: --limit must be a number' ;;
esac
case $STATE in
  open | closed | all) ;;
  *) die 'gh: --state must be open, closed or all' ;;
esac
# Branch names go into a query value unescaped; refuse anything that could add parameters.
check_branch() {
  case $1 in
    *[' &#?%+=']*) die "gh: unsupported branch name: $1" ;;
  esac
}
check_branch "$HEAD"
if [ "$CMD" = create ] && [ "$HAVE_TITLE" = 0 ]; then die 'gh: --title is required'; fi

TMP=$(mktemp -d) || die 'gh: cannot create temp dir' 1
trap 'rm -rf "$TMP"' EXIT
trap 'exit 1' INT TERM HUP

# http METHOD URL [DATAFILE]: response body in $TMP/out; fails on non-2xx.
http() {
  m=$1 u=$2 d=${3:-}
  if [ -n "$d" ]; then
    status=$(curl -q -sS --noproxy '*' -X "$m" -H 'Accept: application/vnd.github+json' \
      -H 'Content-Type: application/json' --data-binary "@$d" \
      -o "$TMP/out" -w '%{http_code}' "$u") || status=curl
  else
    status=$(curl -q -sS --noproxy '*' -X "$m" -H 'Accept: application/vnd.github+json' \
      -o "$TMP/out" -w '%{http_code}' "$u") || status=curl
  fi
  case $status in
    2??) ;;
    *) printf 'gh: HTTP %s\n' "$status" >&2; cat "$TMP/out" >&2 2>/dev/null; exit 1 ;;
  esac
}

# Escapes backslash, quote and every control byte (0x01-0x1F, 0x7F) for a JSON
# string. The byte table uses only POSIX awk (no gawk extensions).
jesc() {
  printf '%s' "$1" | awk 'BEGIN{ORS=""
    for (i = 1; i < 32; i++) if (i != 10) { c[i] = sprintf("%c", i); r[i] = sprintf("\\u%04x", i) }
    c[127] = sprintf("%c", 127); r[127] = "\\u007f"
    r[9] = "\\t"; r[13] = "\\r"
  } {
    gsub(/\\/, "\\\\"); gsub(/"/, "\\\"")
    for (i in c) gsub(c[i], r[i])
    if (NR > 1) print "\\n"
    print
  }'
}

cur_branch() {
  git rev-parse --abbrev-ref HEAD 2>/dev/null || die 'gh: cannot determine current branch' 1
}

first_val() { grep -o "\"$1\": *\"[^\"]*\"" "$TMP/out" | head -n 1 | sed 's/^[^:]*: *"\(.*\)"$/\1/'; }

case $CMD in
  create)
    [ -n "$HEAD" ] || HEAD=$(cur_branch) || exit 1
    if [ -z "$BASE" ]; then
      http GET "$API/repos/$REPO"
      BASE=$(first_val default_branch)
      [ -n "$BASE" ] || die 'gh: cannot determine default branch' 1
    fi
    printf '{"title":"%s","body":"%s","head":"%s","base":"%s","draft":%s}' \
      "$(jesc "$TITLE")" "$(jesc "$BODY")" "$(jesc "$HEAD")" "$(jesc "$BASE")" "$DRAFT" >"$TMP/req"
    http POST "$API/repos/$REPO/pulls" "$TMP/req"
    url=$(first_val html_url)
    printf '%s\n' "$url"
    ;;
  view)
    if [ -n "$NUM" ]; then
      http GET "$API/repos/$REPO/pulls/$NUM"
    else
      b=$(cur_branch) || exit 1
      check_branch "$b"
      http GET "$API/repos/$REPO/pulls?head=$OWNER:$b&state=open"
      if [ -z "$(tr -d ' \t\r\n' <"$TMP/out")" ] || [ "$(tr -d ' \t\r\n' <"$TMP/out")" = '[]' ]; then
        die "gh: no open pull request for branch $b" 1
      fi
    fi
    if [ "$JSON" = 1 ]; then
      cat "$TMP/out"
    else
      printf 'title:\t%s\nstate:\t%s\nurl:\t%s\n' "$(first_val title)" "$(first_val state)" "$(first_val html_url)"
    fi
    ;;
  list)
    q="state=$STATE&per_page=$LIMIT"
    [ -z "$HEAD" ] || q="$q&head=$HEAD"
    http GET "$API/repos/$REPO/pulls?$q"
    if [ "$JSON" = 1 ]; then
      cat "$TMP/out"
    else
      grep -o '"html_url": *"[^"]*/pull/[0-9]*"' "$TMP/out" | sed 's/.*: *"\(.*\)"/\1/'
    fi
    ;;
esac
