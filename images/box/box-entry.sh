#!/bin/sh
for i in $(seq 1 100); do [ -f /keys/id.pub ] && [ -f /ca/bundle.pem ] && break; sleep 0.2; done
mkdir -p $HOME/.ssh $HOME/.cache/pnpm-store $HOME/.cache/uv
sed -i -e "s#__HPROXY__#$HTTP_PROXY#g" -e "s#__PROXY__#$HTTPS_PROXY#g" $HOME/.docker/config.json
[ -f $HOME/.ssh/hostkey ] || ssh-keygen -q -t ed25519 -N '' -f $HOME/.ssh/hostkey
cp /keys/id.pub $HOME/.ssh/authorized_keys; chmod 600 $HOME/.ssh/authorized_keys
cat > $HOME/.ssh/ns-shell <<'EOF'
#!/bin/sh
for i in $(seq 1 150); do [ -S /run/user/1000/docker.sock ] && break; sleep 0.2; done
exec nsenter -U --preserve-credentials -t "$(cat /run/user/1000/docker.pid)" /bin/bash ${SSH_ORIGINAL_COMMAND:+-c "$SSH_ORIGINAL_COMMAND"}
EOF
chmod 755 $HOME/.ssh/ns-shell
NP=localhost,127.0.0.1,::1
cat > $HOME/.ssh/sshd_config <<EOF
Port 2222
HostKey $HOME/.ssh/hostkey
PidFile none
UsePAM no
StrictModes no
PasswordAuthentication no
AuthorizedKeysFile $HOME/.ssh/authorized_keys
ForceCommand $HOME/.ssh/ns-shell
SetEnv PATH=/opt/mise/shims:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin MISE_DATA_DIR=/opt/mise MISE_CONFIG_DIR=/opt/mise/config MISE_STATE_DIR=/opt/mise/state MISE_CACHE_DIR=$HOME/.cache/mise MISE_NODE_FLAVOR=musl MISE_NODE_MIRROR_URL=https://unofficial-builds.nodejs.org/download/release/ MISE_PYTHON_PRECOMPILED_OS=unknown-linux-musl MISE_YES=1 MISE_ALL_COMPILE=false MISE_TRUSTED_CONFIG_PATHS=/workspace npm_config_store_dir=$HOME/.cache/pnpm-store UV_CACHE_DIR=$HOME/.cache/uv DOCKER_HOST=unix:///run/user/1000/docker.sock DOCKER_BUILDKIT=1 HTTPS_PROXY=$HTTPS_PROXY HTTP_PROXY=$HTTP_PROXY https_proxy=$HTTPS_PROXY http_proxy=$HTTP_PROXY NO_PROXY=$NP no_proxy=$NP AGENT_BROWSER_EXECUTABLE_PATH=/usr/bin/chromium-browser LANG=C.UTF-8 SSL_CERT_FILE=$SSL_CERT_FILE NODE_EXTRA_CA_CERTS=$NODE_EXTRA_CA_CERTS REQUESTS_CA_BUNDLE=$REQUESTS_CA_BUNDLE
EOF
/usr/sbin/sshd -D -e -f $HOME/.ssh/sshd_config &
exec dockerd-entrypoint.sh dockerd ${BOX_DNS:+--dns $BOX_DNS} ${EXTRA_DOCKERD:-} --host=unix:///run/user/1000/docker.sock --mtu=1280 --default-network-opt bridge=com.docker.network.driver.mtu=1280 --bip=192.168.231.1/24 --default-address-pool base=10.231.0.0/16,size=24
