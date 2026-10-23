import ctypes, json, os, socket, stat, sys, time
protocol, root, moor, tmp_sock, run_sock = int(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5]
own = os.environ.get("MOOR_FRONT_STATE_DIR")
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
def metadata(path):
    try:
        os.stat(path)
    except OSError as e:
        return e.strerror
    def fchmod():
        fd = os.open(path, os.O_RDONLY)
        try:
            os.fchmod(fd, os.stat(path).st_mode & 0o7777)
        finally:
            os.close(fd)
    mode = os.stat(path).st_mode & 0o7777
    return {
        "chmod": touch(lambda: os.chmod(path, mode)),
        "chown": touch(lambda: os.chown(path, os.getuid(), os.getgid())),
        "utime": touch(lambda: os.utime(path)),
        "setxattr": touch(lambda: os.setxattr(path, "user.moor", b"1")),
        "fchmod": touch(fchmod),
    }
def kind(fd):
    mode = os.fstat(fd).st_mode
    return "pipe" if stat.S_ISFIFO(mode) else "file" if stat.S_ISREG(mode) else "other"
def listdir(path):
    try:
        return len(os.listdir(path))
    except OSError as e:
        return type(e).__name__
while not os.path.exists(f"{own}/ctl.pid"):
    time.sleep(0.05)
ctl = open(f"{own}/ctl.pid").read().strip()
open(f"{own}/mine", "w").close()
libc = ctypes.CDLL(None, use_errno=True)
unmounted = libc.umount2(f"{root}/state".encode(), 2) == 0
got = {
    "own": dial("/proc/self/fd/3"),
    "operator": dial(f"{root}/c.sock"),
    "other_front": dial(f"{root}/state/fronts/slack.sock"),
    "vault_key": read(f"{root}/cfg/vault.key"),
    "controller_fds": listdir(f"/proc/{ctl}/fd"),
    "controller_root": dial(f"/proc/{ctl}/root{root}/c.sock"),
    "unmount": "allowed" if unmounted else os.strerror(ctypes.get_errno()),
    "cover_write": write(f"{root}/state/planted"),
    "own_dir": write(f"{os.environ.get('MOOR_FRONT_STATE_DIR')}/planted"),
    "tmp": write(f"{os.environ.get('TMPDIR')}/planted"),
    "dev_null": write("/dev/null"),
    "dev_zero": write("/dev/zero", "a"),
    "config": write(f"{root}/cfg/c.yaml", "a"),
    "home": write(f"{root}/home/planted"),
    "home_secret": read(f"{root}/home/secret"),
    "shared_read": read(f"{root}/home/shared/note"),
    "shared_write": write(f"{root}/home/shared/planted"),
    "root": write(f"{root}/planted"),
    "moor_dir": write(f"{os.path.dirname(moor)}/planted-{os.path.basename(root)}"),
    "tmp_sock": dial(tmp_sock),
    "run_sock": dial(run_sock),
    "bus": dial(f"/run/user/{os.getuid()}/bus"),
    "docker": dial("/var/run/docker.sock"),
    "shm": write("/dev/shm/planted"),
    "shared_meta": metadata(f"{root}/home/shared/note"),
    "program_meta": metadata(f"{root}/home/bin/front"),
    "fd3_chmod": touch(lambda: os.chmod("/proc/self/fd/3", 0o600)),
    "lxd": dial(f"{root}/var/lib/x/unix.socket"),
    "resolv": read("/etc/resolv.conf"),
    "ca": len(__import__("ssl").create_default_context().get_ca_certs()) > 0 or bool(__import__("ssl").create_default_context().cert_store_stats()["x509_ca"]),
    "dns": touch(lambda: socket.getaddrinfo("localhost", 80)),
    "own_meta": metadata(f"{own}/mine"),
    "rw_mounts": sorted({l.split(" ")[4] for l in open("/proc/self/mountinfo") if l.split(" ")[5].split(",")[0] == "rw"}),
    "cwd": os.getcwd(),
    "init_cwd": os.readlink("/proc/1/cwd"),
    "stdout": kind(1),
    "stderr": kind(2),
}
print("front says hello", flush=True)
print("probe", json.dumps(got), file=sys.stderr, flush=True)
time.sleep(600)
