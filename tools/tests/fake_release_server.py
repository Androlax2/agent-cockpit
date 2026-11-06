"""A loopback stand-in for the GitHub routes install.sh reads (TORLEK_INSTALL_ORIGIN).

ROOT/releases.json lists the published releases, newest first, as GitHub's unauthenticated
release list does (drafts are absent). ROOT/<tag>/<asset> are their assets.

  GET /api/releases?per_page=N     the list, as one line of JSON like GitHub's
  GET /api/releases/tags/<tag>     one listed release, or 404
  GET /download/<tag>/<asset>      an asset of a listed release, or 404

Every request path is appended to ROOT/requests.log. Run as a program, it serves ROOT until
stopped and prints its origin first: python3 fake_release_server.py ROOT
"""

import json
import sys
import threading
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit


def handler_for(root):
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            with (root / "requests.log").open("a") as log:
                log.write(self.path + "\n")
            releases = json.loads((root / "releases.json").read_text())
            published = {release["tag_name"]: release for release in releases}
            parts = urlsplit(self.path).path.strip("/").split("/")
            if parts == ["api", "releases"]:
                return self.send(json.dumps(releases, separators=(",", ":")).encode())
            if parts[:3] == ["api", "releases", "tags"] and len(parts) == 4:
                if parts[3] in published:
                    return self.send(
                        json.dumps(published[parts[3]], separators=(",", ":")).encode()
                    )
            if parts[0] == "download" and len(parts) == 3 and parts[1] in published:
                asset = root / parts[1] / parts[2]
                if asset.is_file():
                    return self.send(asset.read_bytes())
            self.send_error(HTTPStatus.NOT_FOUND)

        def send(self, body):
            self.send_response(HTTPStatus.OK)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_arguments):
            pass

    return Handler


def start(root):
    """Serves ROOT on a free loopback port in a thread; returns the server and its origin."""
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(Path(root)))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_address[1]}"


def release_entry(tag, prerelease=True):
    return {"tag_name": tag, "draft": False, "prerelease": prerelease, "body": "notes, with commas"}


def main():
    if len(sys.argv) != 2:
        sys.exit("usage: fake_release_server.py ROOT")
    _server, origin = start(sys.argv[1])
    print(origin, flush=True)
    threading.Event().wait()


if __name__ == "__main__":
    main()
