#!/bin/sh
set -u
mode=$1 base=$2 stop=$3 log=$4
header='X-Forwarded-Email: prober@example.org'
hold=1
[ "$mode" = hold ] && hold=20
while [ ! -e "$stop" ]; do
	if [ "$mode" = page ]; then
		code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 -H "$header" "$base/tasks" || true)
		[ "$code" = 200 ] && result=ok || result="FAIL($code)"
	else
		got=$(curl -s -N -o /dev/null -w '%{http_code} %{exitcode}' --max-time "$hold" -H "$header" "$base/api/tasks/events" || true)
		case "$got" in
		"200 28") result=ok ;;
		200\ *) result=reconnect ;;
		*) result="FAIL($got)" ;;
		esac
	fi
	echo "$(date +%s) $mode $result" >>"$log"
	sleep 1
done
