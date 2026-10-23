import json, os, socket, struct, subprocess, sys, threading, time

moor = sys.argv[1]
REMOTE = "198.51.100.7"
PRIVATE = "172.17.0.1"
HOST_ADDRESSES = [PRIVATE, "100.64.0.156", "10.235.225.1", "fc00::1", "203.0.113.5"]


def ip(*args):
    subprocess.run(["ip", *args], check=True)


def serve(listener, banner):
    def run():
        for _ in iter(int, 1):
            conn, _ = listener.accept()
            conn.sendall(banner)
            conn.close()
    threading.Thread(target=run, daemon=True).start()


ip("link", "set", "lo", "up")
for name, addr in [("docker0", PRIVATE + "/16"), ("tailscale0", "100.64.0.156/32"),
                   ("lxdbr0", "10.235.225.1/24"), ("public0", "203.0.113.5/32"), ("ula0", "fc00::1/64")]:
    ip("link", "add", name, "type", "dummy")
    ip("addr", "add", addr, "dev", name, *(["nodad"] if ":" in addr else []))
    ip("link", "set", name, "up")
host = socket.socket(socket.AF_INET6)
host.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 0)
host.bind(("::", 443))
host.listen(16)
serve(host, b"host")

remote_script = r'''
import socket, subprocess, sys, threading, time
for _ in range(200):
    if subprocess.run(["ip", "link", "show", "v1"], capture_output=True).returncode == 0:
        break
    time.sleep(0.05)
for c in ["ip link set lo up", "ip link set v1 up", f"ip addr add {sys.argv[1]}/24 dev v1"]:
    subprocess.run(c.split(), check=True)
def run(listener):
    for _ in iter(int, 1):
        conn, _ = listener.accept()
        conn.sendall(b"remote")
        conn.close()
for port in [443, 80, 8443]:
    s = socket.socket()
    s.bind((sys.argv[1], port))
    s.listen(16)
    threading.Thread(target=run, args=(s,), daemon=True).start()
print("ready", flush=True)
time.sleep(120)
'''
remote = subprocess.Popen(["unshare", "--net", "--", sys.executable, "-c", remote_script, REMOTE],
                          stdout=subprocess.PIPE, text=True)
time.sleep(0.2)
ip("link", "add", "v0", "type", "veth", "peer", "name", "v1", "netns", str(remote.pid))
ip("addr", "add", "198.51.100.1/24", "dev", "v0")
ip("link", "set", "v0", "up")
remote.stdout.readline()

dns = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
dns.bind(("127.0.0.53", 53))
rebinds = []


def answer():
    for _ in iter(int, 1):
        q, addr = dns.recvfrom(512)
        i, labels = 12, []
        while q[i]:
            labels.append(q[i + 1:i + 1 + q[i]].decode())
            i += 1 + q[i]
        hit = struct.unpack(">H", q[i + 1:i + 3])[0] == 1
        to = REMOTE
        if labels[0] == "private":
            to = PRIVATE
        elif labels[0] == "rebind" and hit:
            to = REMOTE if not rebinds else PRIVATE
            rebinds.append(to)
        head = q[:2] + struct.pack(">HHHHH", 0x8180, 1, int(hit), 0, 0)
        rr = b"\xc0\x0c" + struct.pack(">HHIH", 1, 1, 60, 4) + socket.inet_aton(to)
        dns.sendto(head + q[12:i + 5] + (rr if hit else b""), addr)


threading.Thread(target=answer, daemon=True).start()


def banner(host, port):
    try:
        with socket.create_connection((host, port), timeout=2) as s:
            return s.recv(16).decode()
    except OSError as e:
        return type(e).__name__


control = {h: banner(h, 443) for h in HOST_ADDRESSES + [REMOTE]}

front = r'''
import json, os, socket, sys
port = int(os.environ["HTTPS_PROXY"].rsplit(":", 1)[1])
def via(target):
    s = socket.create_connection(("127.0.0.1", port), timeout=5)
    s.sendall(f"CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n".encode())
    head = b""
    while b"\r\n\r\n" not in head:
        got = s.recv(1)
        if not got:
            break
        head += got
    status = int(head.split()[1]) if head else 0
    return [status, s.recv(16).decode() if status == 200 else ""]
got = {t: via(t) for t in sys.argv[1:]}
direct = socket.socket()
direct.settimeout(3)
try:
    direct.connect(("198.51.100.7", 443))
    got["direct"] = "connected"
except OSError as e:
    got["direct"] = e.strerror
got["caps"] = [l.split()[1] for l in open("/proc/self/status") if l.startswith(("CapEff", "CapAmb"))]
print(json.dumps(got), flush=True)
'''
targets = [f"[{h}]:443" if ":" in h else f"{h}:443" for h in HOST_ADDRESSES]
targets += ["127.0.0.1:443", "[::1]:443", "localhost:443", f"{REMOTE}:443", f"{REMOTE}:80",
            f"{REMOTE}:8443", "remote.test:443", "private.test:443", "rebind.test:443"]
out = subprocess.run(
    [moor, "front-shim", "--parent", str(os.getpid()), "--", "python3", "-c", front, *targets],
    capture_output=True, text=True, env={"PATH": "/usr/bin:/bin"}, timeout=60)
remote.kill()
print(json.dumps({"control": control, "front": out.stdout.strip(), "log": out.stderr.strip(),
                  "rebind_answers": rebinds}))
