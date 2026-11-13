# Test double for a ready Docker daemon: answers GET /_ping with 200 on a Unix socket. Usage: fake-docker-ping.py <socket path>
import os, socket, sys
path = sys.argv[1]
if os.path.exists(path): os.unlink(path)
s = socket.socket(socket.AF_UNIX); s.bind(path); s.listen(8)
while True:
    c, _ = s.accept()
    try:
        c.recv(1024); c.sendall(b"HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\n\r\nOK")
    finally: c.close()
