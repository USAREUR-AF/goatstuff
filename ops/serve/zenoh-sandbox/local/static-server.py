#!/usr/bin/env python3
"""Tiny dependency-free static server for the offline Zenoh sandbox demo.

Why not `python3 -m http.server`: it serves .wasm with the wrong MIME type, and
the zenoh-ts client's WebAssembly.instantiateStreaming refuses that. This adds
the application/wasm type and an SPA index fallback. stdlib only — works on any
machine with python3, no internet, no Docker.

Usage: python3 static-server.py <port> <dir>
"""
import sys
import os
from functools import partial
from http.server import HTTPServer, SimpleHTTPRequestHandler


class Handler(SimpleHTTPRequestHandler):
    extensions_map = {
        **SimpleHTTPRequestHandler.extensions_map,
        ".wasm": "application/wasm",
        ".js": "text/javascript",
        ".mjs": "text/javascript",
        ".css": "text/css",
        ".svg": "image/svg+xml",
    }

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def send_head(self):
        # SPA fallback: unknown non-asset path → index.html (the app has no
        # server routes, but this keeps deep links / reloads working).
        path = self.translate_path(self.path)
        if not os.path.exists(path) and "." not in os.path.basename(path):
            self.path = "/index.html"
        return super().send_head()


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8080
    directory = sys.argv[2] if len(sys.argv) > 2 else "."
    handler = partial(Handler, directory=directory)
    httpd = HTTPServer(("127.0.0.1", port), handler)
    print(f"serving {directory} at http://localhost:{port}  (Ctrl-C to stop)")
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        httpd.server_close()


if __name__ == "__main__":
    main()
