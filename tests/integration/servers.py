"""Servers on the host, recording what reaches them: the far side of
celily's proxy.

On loopback: the instance cannot reach them by itself, only through the
proxy, which runs on the host.
"""

import contextlib
import dataclasses
import http.client
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# What the HTTP server answers to every GET.
BODY = "celily-it\n"


@dataclasses.dataclass(frozen=True)
class Request:
    method: str
    path: str
    # Case-insensitive, and get_all() shows a header sent twice.
    headers: http.client.HTTPMessage


class _Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.server.requests.append(Request(self.command, self.path, self.headers))
        body = BODY.encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


class HttpServer(ThreadingHTTPServer):
    """Answers every GET with BODY. Records every connection, HTTP or not
    (a raw tunnel reaching it is a connection, never a request), and every
    GET."""

    def __init__(self):
        super().__init__(("127.0.0.1", 0), _Handler)
        self.connections = []
        self.requests = []

    @property
    def host(self):
        return self.server_address[0]

    @property
    def port(self):
        return self.server_address[1]

    def url(self, path):
        return f"http://{self.host}:{self.port}{path}"

    def verify_request(self, request, client_address):
        # Called for every accepted connection, before anything is read.
        self.connections.append(client_address)
        return True


@contextlib.contextmanager
def serve_http():
    with HttpServer() as server:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield server
        finally:
            server.shutdown()
            thread.join()
