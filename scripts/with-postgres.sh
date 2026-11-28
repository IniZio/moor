#!/bin/sh
set -eu
name=""

cleanup() {
	if [ -n "$name" ]; then
		docker rm -f "$name" >/dev/null 2>&1 || true
	fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP

if [ -z "${MOOR_TEST_POSTGRES_DSN:-}" ]; then
	name="moor-test-pg-$(head -c 6 /dev/urandom | od -An -tx1 | tr -d ' \n')"
	docker run -d --name "$name" -e POSTGRES_PASSWORD=moor -e POSTGRES_DB=moor \
		-p 127.0.0.1::5432 --tmpfs /var/lib/postgresql/data postgres:17 -c fsync=off >/dev/null
	port=$(docker port "$name" 5432/tcp | head -n 1 | sed 's/.*://')
	ready=""
	for _ in $(seq 1 60); do
		if docker exec "$name" pg_isready -h 127.0.0.1 -U postgres -d moor >/dev/null 2>&1; then
			ready=yes
			break
		fi
		sleep 1
	done
	[ -n "$ready" ] || { echo "with-postgres: $name did not become ready" >&2; docker logs "$name" >&2; exit 1; }
	MOOR_TEST_POSTGRES_DSN="postgres://postgres:moor@127.0.0.1:$port/moor?sslmode=disable"
	export MOOR_TEST_POSTGRES_DSN
fi
"$@"
