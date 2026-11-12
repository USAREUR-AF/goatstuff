#!/usr/bin/env python3
"""Tiny dependency-free static server for the offline Zenoh sandbox demo.

Serves the app directory as it is in the tree. Compared with
`python3 -m http.server` it binds to localhost only, sends the JavaScript
module MIME type whatever the host's MIME table says (a browser refuses a
module served as anything else), tells the browser not to cache, and falls
back to index.html for an unknown path. stdlib only — works on any machine
with python3, no internet, no Docker.

Usage: python3 static-server.py <port> <dir>
"""
import sys
import os
from functools import partial
from http.server import HTTPServer, SimpleHTTPRequestHandler


class Handler(SimpleHTTPRequestHandler):
    extensions_map = {
        **SimpleHTTPRequestHandler.extensions_map,
        ".js": "text/javascript",
        ".mjs": "text/javascript",
        ".css": "text/css",
        ".svg": "image/svg+xml",
    }

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def send_head(self):
        # Unknown non-asset path → index.html, the same fallback nginx.conf
        # gives the deployed image.
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
