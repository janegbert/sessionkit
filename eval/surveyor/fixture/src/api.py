import json
from http.server import BaseHTTPRequestHandler, HTTPServer
from .service import create_item
from .storage import initialize, rename_direct


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path == "/admin/rename":
            rename_direct(body["id"], body["text"])
            result = {"renamed": body["id"]}
        else:
            result = {"created": create_item(body["text"])}
        self.send_response(200)
        self.end_headers()
        self.wfile.write(json.dumps(result).encode())


def main():
    initialize()
    HTTPServer(("", 8080), Handler).serve_forever()


if __name__ == "__main__":
    main()
