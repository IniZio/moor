#!/bin/sh
# Installs the chart on a throwaway kind cluster and proves the deploy rules. Needs docker, kind, kubectl, helm and a built moor-fresh-portal:dev.
# Every object is named moor-fresh-chart or lives inside it. The kubeconfig is a file in the scratch dir, never ~/.kube/config.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cluster=moor-fresh-chart
dir=$(mktemp -d)
export KUBECONFIG="$dir/kubeconfig"
chart="$root/deploy/chart"
ns=portal-test
hdr='X-Forwarded-Email: tester@example.org'
pids=""
probes=""

say() { echo "cluster-test: $*"; }
die() { echo "cluster-test: FAIL $*" >&2; exit 1; }

teardown() {
	status=$?
	touch "$dir/stop"
	for p in $pids; do kill "$p" 2>/dev/null || true; done
	kind delete cluster --name "$cluster" --kubeconfig "$KUBECONFIG" >/dev/null 2>&1 || true
	left=$({ kind get clusters 2>/dev/null | grep -x "$cluster" || true
		docker ps -a --filter "name=$cluster" --format '{{.Names}}'
		docker volume ls --filter "name=$cluster" --format '{{.Name}}'
		docker network ls --filter "name=$cluster" --format '{{.Name}}'; } | tr '\n' ' ')
	[ -z "$left" ] && say "teardown: nothing named $cluster remains" || { say "teardown: left behind: $left" >&2; status=1; }
	rm -rf "$dir"
	exit "$status"
}
trap teardown EXIT

freeport() { while :; do p=$((20000 + $(od -An -N2 -tu2 /dev/urandom | tr -d ' ') % 20000)); ss -ltn | grep -q ":$p " || { echo "$p"; return; }; done; }

create_cluster() {
	hostport=$(freeport)
	cat >"$dir/kind.yaml" <<EOF
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
nodes:
  - role: control-plane
    extraPortMappings:
      - {containerPort: 30080, hostPort: $hostport, listenAddress: 127.0.0.1}
  - role: worker
  - role: worker
EOF
	kind delete cluster --name "$cluster" --kubeconfig "$KUBECONFIG" >/dev/null 2>&1 || true
	kind create cluster --name "$cluster" --kubeconfig "$KUBECONFIG" --config "$dir/kind.yaml" \
		--image kindest/node:v1.31.2@sha256:18fbefc20a7113353c7b75b5c869d7145a6abd6269154825872dc59c1329912e --wait 180s >/dev/null
	docker save --platform "$(docker version --format '{{.Server.Os}}/{{.Server.Arch}}')" -o "$dir/images.tar" moor-fresh-portal:dev
	kind load image-archive "$dir/images.tar" --name "$cluster" >/dev/null
	say "cluster up, node port on 127.0.0.1:$hostport"
}

install_all() {
	kubectl create namespace "$ns" >/dev/null
	kubectl config set-context --current --namespace "$ns" >/dev/null
	pgpass=$(openssl rand -hex 12)
	kubectl create secret generic postgres-auth --from-literal=password="$pgpass" >/dev/null
	kubectl create secret generic portal-secrets \
		--from-literal=database-url="postgres://postgres:$pgpass@postgres:5432/postgres?sslmode=disable" \
		--from-literal=secret-key="$(openssl rand -base64 32)" \
		--from-literal=internal-secret="$(openssl rand -hex 16)" >/dev/null
	kubectl apply -f "$root/deploy/cluster-test/postgres.yaml" >/dev/null
	kubectl wait --for=condition=Ready pod/postgres --timeout=120s >/dev/null
	helm install portal "$chart" -n "$ns" -f "$chart/ci/values.yaml" --set image.repository=moor-fresh-portal --set image.tag=dev \
		--set image.pullPolicy=Never --set service.type=NodePort --set service.nodePort=30080 \
		--set portal.publicURL=http://portal.example.org --wait --timeout 3m >/dev/null
	say "installed, both replicas ready from an empty database"
}

check_migrations() { # check_migrations <round>
	want=$(( $(ls "$root"/internal/adapter/store/migrations/*.sql | wc -l) + 1 ))
	got=$(kubectl exec postgres -- psql -U postgres -Atc "SELECT count(*) || ' ' || count(DISTINCT version_id) FROM goose_db_version")
	restarts=$(kubectl get pods -l app.kubernetes.io/name=portal -o jsonpath='{range .items[*]}{.status.containerStatuses[0].restartCount}{" "}{end}')
	started=$(kubectl get pods -l app.kubernetes.io/name=portal -o jsonpath='{range .items[*]}{.status.containerStatuses[0].state.running.startedAt}{" "}{end}')
	nodes=$(kubectl get pods -l app.kubernetes.io/name=portal -o jsonpath='{range .items[*]}{.spec.nodeName}{" "}{end}')
	[ "$got" = "$want $want" ] || die "round $1: goose_db_version rows and distinct versions are '$got', want '$want $want'"
	[ "$restarts" = "0 0 " ] || die "round $1: restart counts '$restarts', want '0 0 '"
	say "round $1: $want migration rows, each applied once; restarts 0 0; started $started on $nodes"
}

race_from_empty() {
	for round in 2 3; do
		kubectl scale deployment/portal --replicas=0 >/dev/null
		kubectl wait --for=delete pod -l app.kubernetes.io/name=portal --timeout=120s >/dev/null
		kubectl exec postgres -- env PGOPTIONS='-c client_min_messages=warning' psql -U postgres -qc 'DROP SCHEMA public CASCADE; CREATE SCHEMA public' >/dev/null
		kubectl scale deployment/portal --replicas=2 >/dev/null
		kubectl rollout status deployment/portal --timeout=120s >/dev/null
		check_migrations "$round"
	done
}

by_port_forward() {
	kubectl port-forward svc/portal :80 >"$dir/pf.log" 2>&1 &
	pids="$pids $!"
	for _ in 1 2 3 4 5 6 7 8 9 10; do grep -q Forwarding "$dir/pf.log" && break; sleep 1; done
	pf="http://127.0.0.1:$(sed -n 's/.*127.0.0.1:\([0-9]*\) ->.*/\1/p' "$dir/pf.log" | head -n 1)"
	[ "$(curl -s "$pf/healthz")" = ok ] || die "/healthz"
	[ "$(curl -s "$pf/readyz")" = ok ] || die "/readyz"
	[ "$(curl -s -o /dev/null -w '%{http_code}' "$pf/tasks")" = 401 ] || die "/tasks without the header is not 401"
	curl -sf -H "$hdr" "$pf/tasks" | grep -qi '<html' || die "/tasks page with the header"
	doc='repositories:\n  - example-org/shop-web\n'
	saved=$(curl -s -X PUT -H "$hdr" -H 'Content-Type: application/json' -d "{\"base\":0,\"text\":\"$doc\"}" "$pf/api/settings")
	echo "$saved" | grep -q '"version":1' || die "settings save answered: $saved"
	say "port-forward: /healthz, /readyz, /tasks with the header, 401 without it, settings saved as v1"
}

roll_under_load() {
	node="http://127.0.0.1:$hostport"
	for mode in page stream hold; do
		sh "$root/deploy/cluster-test/probe.sh" "$mode" "$node" "$dir/stop" "$dir/probe.log" &
		probes="$probes $!"
	done
	sleep 5
	old=$(kubectl get pods -l app.kubernetes.io/name=portal -o name)
	kubectl rollout restart deployment/portal >/dev/null
	kubectl rollout status deployment/portal --timeout=240s >/dev/null
	kubectl wait --for=delete $old --timeout=120s >/dev/null
	sleep 5
	touch "$dir/stop"
	wait $probes
	awk '{n[$2" "($3 ~ /^FAIL/ ? "FAIL" : $3)]++} END {for (k in n) print "  " k ": " n[k]}' "$dir/probe.log" | sort
	fails=$(grep -c FAIL "$dir/probe.log" || true)
	[ "$fails" = 0 ] || { grep FAIL "$dir/probe.log" >&2; die "$fails requests failed during the roll"; }
	[ "$(curl -s -H "$hdr" "$node/api/settings" | grep -c '"version":1')" = 1 ] || die "settings gone after the roll"
	[ "$(grep -c 'hold reconnect' "$dir/probe.log" || true)" -ge 1 ] || die "the held stream never saw an old replica stop, so the roll was not under test"
	say "rolling restart: zero failed requests apart from stream reconnects, settings v1 still served"
}

create_cluster
install_all
check_migrations 1
race_from_empty
kubectl get pods -o wide
by_port_forward
roll_under_load
say "all passed"
