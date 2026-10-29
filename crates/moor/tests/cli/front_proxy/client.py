import base64, json, os, socket, ssl, sys, time
protocol = int(sys.argv[1])
def ask(request):
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(10)
    s.connect(os.environ["MOOR_FRONT_SOCKET"])
    hello = {"frame": "hello", "protocol": protocol}
    frame = {"frame": "request", "id": "1", "request": request}
    s.sendall((json.dumps(hello) + "\n" + json.dumps(frame) + "\n").encode())
    for line in s.makefile():
        got = json.loads(line)
        if got["frame"] in ("response", "error"):
            return got.get("envelope", got)
who = "probe:u1"
ask({"cmd": "enroll", "identity": who})
ask({"cmd": "secret-set", "identity": who, "name": "claude", "value": "sk-ant-oat01-never-shown"})
lease = ask({"cmd": "proxy-lease", "identity": who})["value"]
print("lease", lease["lease"], flush=True)
token = lease.get("token", "")
port = int(os.environ["MOOR_FRONT_PROXY_PORT"])
ca = os.environ["MOOR_FRONT_PROXY_CA_PEM"]
def through_pid1():
    reached = []
    for n in range(64):
        s = socket.socket(socket.AF_UNIX)
        try:
            s.connect(f"/proc/1/fd/{n}/proxy")
            reached.append(str(n))
        except PermissionError:
            pass
        except OSError as e:
            reached.append(f"{n}:{type(e).__name__}") if e.errno not in (2, 13, 20) else None
        finally:
            s.close()
    try:
        os.listdir("/proc/1/fd")
        listed = "listed"
    except PermissionError:
        listed = "PermissionError"
    return f"{listed} {','.join(reached) or 'none'}"
print("pid1", through_pid1(), flush=True)
def connect(password):
    try:
        s = socket.create_connection(("127.0.0.1", port), timeout=5)
        auth = base64.b64encode(f"lease:{password}".encode()).decode()
        s.sendall(f"CONNECT api.anthropic.com:443 HTTP/1.1\r\nHost: api.anthropic.com:443\r\nProxy-Authorization: Basic {auth}\r\n\r\n".encode())
        head = b""
        while b"\r\n\r\n" not in head:
            chunk = s.recv(1)
            if not chunk:
                return "closed"
            head += chunk
        status = head.split(b" ")[1].decode()
        if status != "200":
            return "status " + status
        tls = ssl.create_default_context(cadata=ca).wrap_socket(s, server_hostname="api.anthropic.com")
        tls.close()
        return "tls ok"
    except Exception as e:
        return type(e).__name__
print("stranger", connect("not-a-lease"), flush=True)
while True:
    print("connect", connect(token), flush=True)
    time.sleep(0.3)
