import json, os, socket, subprocess, sys, time

moor, listed, unlisted = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])


def ip(*args):
    subprocess.run(["ip", *args], check=True)


ip("link", "set", "lo", "up")
for name, addr in [("up0", "10.9.0.2/24"), ("docker0", "172.17.0.1/16")]:
    ip("link", "add", name, "type", "dummy")
    ip("addr", "add", addr, "dev", name)
    ip("link", "set", name, "up")
ip("route", "add", "default", "via", "10.9.0.1")
docker = socket.socket()
docker.bind(("172.17.0.1", 0))
docker.listen(4)

front = r'''
import socket, sys, time
listeners = []
for port in sys.argv[1:3]:
    s = socket.socket()
    s.bind(("127.0.0.1", int(port)))
    s.listen(8)
    listeners.append(s)
out = socket.socket()
out.settimeout(3)
try:
    out.connect(("172.17.0.1", int(sys.argv[3])))
    docker = "connected"
except OSError as e:
    docker = type(e).__name__
print(docker, flush=True)
while True:
    client, _ = listeners[0].accept()
    client.sendall(b"front")
    client.close()
'''
shim = subprocess.Popen(
    [moor, "front-shim", "--parent", str(os.getpid()), "--port", str(listed), "--",
     "python3", "-c", front, str(listed), str(unlisted), str(docker.getsockname()[1])],
    stdout=subprocess.PIPE, text=True, env={"PATH": "/usr/bin:/bin"})
got = {"front_to_docker0": shim.stdout.readline().strip()}


def reach(host, port):
    s = socket.socket()
    s.settimeout(3)
    try:
        s.connect((host, port))
        return s.recv(16).decode() or "connected"
    except OSError as e:
        return type(e).__name__


for host, port in [("127.0.0.1", listed), ("10.9.0.2", listed), ("172.17.0.1", listed),
                   ("127.0.0.1", unlisted)]:
    got[f"{host}:{'listed' if port == listed else 'unlisted'}"] = reach(host, port)
shim.kill()
shim.wait()
print(json.dumps(got))
