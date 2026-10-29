import json, os, socket, sys, time
protocol, root, moor, tmp_sock, home_sock = int(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5]
own = os.environ.get("MOOR_FRONT_STATE_DIR")
mine = os.environ.get("MOOR_FRONT_SOCKET")
tag = os.path.basename(root)
def dial(path):
    try:
        s = socket.socket(socket.AF_UNIX)
        s.settimeout(5)
        s.connect(path)
        hello = {"frame": "hello", "protocol": protocol}
        ask = {"frame": "request", "id": "1", "request": {"cmd": "version"}}
        s.sendall((json.dumps(hello) + "\n" + json.dumps(ask) + "\n").encode())
        for line in s.makefile():
            if json.loads(line)["frame"] == "response":
                return "answers"
        return "closed"
    except OSError as e:
        return type(e).__name__
def read(path):
    try:
        return len(open(path, "rb").read())
    except OSError as e:
        return type(e).__name__
def write(path, mode="w"):
    try:
        open(path, mode).close()
        return "writable"
    except OSError as e:
        return e.strerror
def touch(act):
    try:
        act()
        return "ok"
    except OSError as e:
        return e.strerror
while not os.path.exists(f"{own}/ctl.pid"):
    time.sleep(0.05)
ctl = int(open(f"{own}/ctl.pid").read().strip())
open(f"{own}/mine", "w").close()
got = {
    "pid": os.getpid(),
    "own": dial(mine),
    "operator": dial(f"{root}/c.sock"),
    "other_front": dial(f"{root}/state/fronts/slack/front"),
    "tmp_sock": dial(tmp_sock),
    "home_sock": dial(home_sock),
    "socket_chmod": touch(lambda: os.chmod(mine, 0o600)),
    "config_read": read(f"{root}/cfg/c.yaml"),
    "config": write(f"{root}/cfg/c.yaml", "a"),
    "state_read": read(f"{root}/state/controller.out"),
    "state": write(f"{root}/state/planted"),
    "home": write(f"{root}/home/planted"),
    "home_secret": read(f"{root}/home/secret"),
    "shared_read": read(f"{root}/home/shared/note"),
    "shared_write": write(f"{root}/home/shared/planted"),
    "root": write(f"{root}/planted"),
    "moor": write(moor, "a"),
    "moor_dir": write(f"{os.path.dirname(moor)}/planted-{tag}"),
    "system_tmp": write(f"/tmp/moor-planted-{tag}"),
    "own_dir": write(f"{own}/planted"),
    "tmp": write(f"{os.environ.get('TMPDIR')}/planted"),
    "own_meta": touch(lambda: os.chmod(f"{own}/mine", 0o600)),
    "program_meta": touch(lambda: os.chmod(f"{root}/home/bin/front", 0o755)),
    "dev_null": write("/dev/null"),
    "dev_zero": write("/dev/zero", "a"),
    "resolv": read("/etc/resolv.conf"),
    "ca": read("/etc/ssl/cert.pem"),
    "dns": touch(lambda: socket.getaddrinfo("localhost", 80)),
    "signal_controller": touch(lambda: os.kill(ctl, 0)),
    "cwd": os.getcwd(),
}
print("probe", json.dumps(got), file=sys.stderr, flush=True)
time.sleep(600)
