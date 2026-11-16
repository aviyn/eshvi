"""One VNC connection to the QA VM, kept open (scripts/qa-vm.sh uses it).

Apple's VNC server in Virtualization.framework (`tart run
--vnc-experimental`) brings the VM down after a number of client
connections, so commands go through this one instead of a vncdo each.

  python qa-vnc.py serve URL SOCKET   connect, then take commands on SOCKET
  python qa-vnc.py send SOCKET CMD... run vncdo-style commands, in order:
      capture FILE | move X Y | click B | mousedown B | mouseup B |
      key K | keydown K | keyup K | pause S | type TEXT
"""
import os
import socket
import sys
import time
from urllib.parse import urlparse

# The VNC server drops shifted characters sent as their own keysyms ("E",
# ":"): send the unshifted key with shift held.
SHIFTED = dict(zip('~!@#$%^&*()_+{}|:"<>?', "`1234567890-=[]\\;',./"))
NAMES = {"\n": "enter", "\t": "tab"}


def press(client, key, shift):
    if shift:
        client.keyDown("shift")
        time.sleep(0.03)
    client.keyPress(key)
    if shift:
        time.sleep(0.03)
        client.keyUp("shift")


def type_text(client, text):
    for c in text:
        if c.isupper():
            press(client, c.lower(), True)
        elif c in SHIFTED:
            base = SHIFTED[c]
            press(client, NAMES.get(base, base), True)
        else:
            press(client, NAMES.get(c, c), False)
        time.sleep(0.02)


def run(client, args):
    i = 0
    while i < len(args):
        cmd = args[i]
        if cmd == "capture":
            client.captureScreen(args[i + 1]); i += 2
        elif cmd == "move":
            client.mouseMove(int(args[i + 1]), int(args[i + 2])); i += 3
        elif cmd == "click":
            client.mousePress(int(args[i + 1])); i += 2
        elif cmd == "mousedown":
            client.mouseDown(int(args[i + 1])); i += 2
        elif cmd == "mouseup":
            client.mouseUp(int(args[i + 1])); i += 2
        elif cmd == "key":
            k = args[i + 1]
            if k.startswith("shift-"):
                press(client, k[len("shift-"):], True)
            else:
                client.keyPress(k)
            i += 2
        elif cmd == "keydown":
            client.keyDown(args[i + 1]); i += 2
        elif cmd == "keyup":
            client.keyUp(args[i + 1]); i += 2
        elif cmd == "pause":
            time.sleep(float(args[i + 1])); i += 2
        elif cmd == "type":
            type_text(client, args[i + 1]); i += 2
        else:
            raise ValueError(f"unknown command {cmd!r}")


def serve(url, path):
    from vncdotool import api

    u = urlparse(url)

    def connect():
        return api.connect(f"{u.hostname}::{u.port}", password=u.password)

    client = connect()
    if os.path.exists(path):
        os.unlink(path)
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(path)
    srv.listen(1)
    print("ready", flush=True)
    try:
        while True:
            conn, _ = srv.accept()
            with conn:
                data = b""
                while not data.endswith(b"\0\0"):
                    chunk = conn.recv(65536)
                    if not chunk:
                        break
                    data += chunk
                args = [a.decode() for a in data.rstrip(b"\0").split(b"\0")]
                try:
                    run(client, args)
                    conn.sendall(b"ok\n")
                except Exception as e:
                    # vncdotool chains every call on the connection: after a
                    # failure each later one fails the same way. A new one.
                    conn.sendall(f"error: {e}\n".encode())
                    try:
                        client.disconnect()
                    except Exception:
                        pass
                    client = connect()
    finally:
        api.shutdown()


def send(path, args):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(path)
    s.sendall(b"\0".join(a.encode() for a in args) + b"\0\0")
    reply = s.makefile().readline().strip()
    if reply != "ok":
        sys.exit(reply)


if __name__ == "__main__":
    if sys.argv[1] == "serve":
        serve(sys.argv[2], sys.argv[3])
    else:
        send(sys.argv[2], sys.argv[3:])
