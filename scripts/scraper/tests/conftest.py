"""Shared fixtures. Everything here is local: no test reaches the internet.

The guard runs at import of this file, before any test can import the
library, exactly as ``cli.main`` runs it before anything else. A test that
needs to watch the import itself (the telemetry switch, the config file)
does so in a fresh interpreter instead; see ``test_guard.py``.
"""

from __future__ import annotations

import dataclasses
import http.server
import ipaddress
import json
import ssl
import subprocess
import sys
import threading
import time
from pathlib import Path
from types import SimpleNamespace

import pytest

SCRAPER = Path(__file__).resolve().parents[1]
if str(SCRAPER) not in sys.path:
    sys.path.insert(0, str(SCRAPER))

from qip_scraper import guard

GUARD_REPORT = guard.prepare()

# Imported after the guard on purpose: everything below may import the library.
from langchain_core.language_models.fake_chat_models import GenericFakeChatModel
from langchain_core.messages import AIMessage

from qip_scraper import cli, fetch

LOCAL_SERVER_ADDRESS = ipaddress.ip_address("127.0.0.1")


def local_servers_only(address) -> str | None:
    """The address policy the tests fetch under: the tool's own, except that
    127.0.0.1, where every local server here listens, is admitted. Nothing
    else is: ::1, 127.0.0.2 and every other non-global address are still
    refused, so a test that strays from the local servers fails."""
    if address == LOCAL_SERVER_ADDRESS:
        return None
    return fetch.address_refusal(address)


ENVIRONMENT_THE_TOOL_READS = (
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "http_proxy",
    "https_proxy",
    "NO_PROXY",
    "no_proxy",
    "ALL_PROXY",
    "all_proxy",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_API_KEY_FILE",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_PROXY",
    "ANTHROPIC_AUTH_TOKEN",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "QIP_SCRAPER_MODEL",
)

#: A model id that matches the id pattern and names no real model. The
#: repository names no model; tests use this one throughout.
FIXTURE_MODEL = "claude-fixture"


@pytest.fixture(autouse=True)
def isolated_environment(monkeypatch):
    """Nothing from the shell running the tests reaches the tool."""
    for name in ENVIRONMENT_THE_TOOL_READS:
        monkeypatch.delenv(name, raising=False)


# --------------------------------------------------------------------------
# A local web server
# --------------------------------------------------------------------------


@dataclasses.dataclass
class Reply:
    status: int = 200
    body: bytes = b""
    headers: dict = dataclasses.field(default_factory=dict)
    #: Seconds to wait before answering at all.
    delay: float = 0.0
    #: If set, the body is sent one byte at a time with this pause, and with
    #: no Content-Length.
    drip: float | None = None
    #: Send no Content-Length; the body ends when the connection closes.
    undeclared: bool = False


class Site:
    """A local HTTP(S) server whose pages a test writes and whose requests it reads."""

    def __init__(self, tls: ssl.SSLContext | None = None) -> None:
        self.routes: dict[str, Reply] = {}
        self.requests: list[dict] = []
        site = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                site.requests.append(
                    {"method": "GET", "path": self.path, "headers": dict(self.headers)}
                )
                self.answer()

            def do_POST(self):
                body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                site.requests.append(
                    {
                        "method": "POST",
                        "path": self.path,
                        "headers": dict(self.headers),
                        "body": body,
                    }
                )
                self.answer()

            def do_CONNECT(self):  # a proxy that records the tunnel and refuses it
                site.requests.append(
                    {
                        "method": "CONNECT",
                        "path": self.path,
                        "headers": dict(self.headers),
                    }
                )
                self.send_response(403)
                self.send_header("Content-Length", "0")
                self.end_headers()

            def answer(self):
                reply = site.routes.get(self.path) or Reply(
                    404, b"not found", {"Content-Type": "text/plain"}
                )
                if reply.delay:
                    time.sleep(reply.delay)
                self.send_response(reply.status)
                for name, value in reply.headers.items():
                    self.send_header(name, value)
                if not (reply.drip or reply.undeclared):
                    self.send_header("Content-Length", str(len(reply.body)))
                self.end_headers()
                try:
                    if reply.drip:
                        for index in range(len(reply.body)):
                            self.wfile.write(reply.body[index : index + 1])
                            self.wfile.flush()
                            time.sleep(reply.drip)
                    else:
                        self.wfile.write(reply.body)
                except (BrokenPipeError, ConnectionResetError):
                    pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self.tls = tls is not None
        if tls is not None:
            self.server.socket = tls.wrap_socket(self.server.socket, server_side=True)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    @property
    def port(self) -> int:
        return self.server.server_address[1]

    def url(self, path: str = "/") -> str:
        return f"{'https' if self.tls else 'http'}://127.0.0.1:{self.port}{path}"

    def page(
        self,
        path: str,
        body: str | bytes,
        content_type: str = "text/html; charset=utf-8",
        status: int = 200,
        **reply,
    ) -> str:
        data = body.encode() if isinstance(body, str) else body
        headers = {"Content-Type": content_type} if content_type else {}
        headers.update(reply.pop("headers", {}))
        self.routes[path] = Reply(status, data, headers, **reply)
        return self.url(path)

    def robots(self, text: str, status: int = 200, **reply) -> None:
        self.page("/robots.txt", text, "text/plain; charset=utf-8", status, **reply)

    def paths(self) -> list[str]:
        return [request["path"] for request in self.requests]

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()


@pytest.fixture
def site():
    server = Site()
    yield server
    server.close()


@pytest.fixture
def certificate(tmp_path):
    """A throwaway self-signed certificate for 127.0.0.1, minted for this test.

    Minted rather than committed: a private key in the tree, even a test one,
    is a key in the tree. The openssl command line is the one dependency this
    adds, and it is on every machine that has Python's ssl module built.
    """
    key, cert = tmp_path / "key.pem", tmp_path / "cert.pem"
    subprocess.run(
        [
            "openssl",
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            str(key),
            "-out",
            str(cert),
            "-days",
            "1",
            "-subj",
            "/CN=127.0.0.1",
            "-addext",
            "subjectAltName=IP:127.0.0.1,DNS:localhost",
        ],
        check=True,
        capture_output=True,
        timeout=60,
    )
    return SimpleNamespace(cert=cert, key=key)


@pytest.fixture
def tls_site(certificate):
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(certificate.cert, certificate.key)
    server = Site(tls=context)
    server.certificate = certificate
    yield server
    server.close()


# --------------------------------------------------------------------------
# A fake Claude
# --------------------------------------------------------------------------


def reply(
    text: str, stop: str | None = "end_turn", model: str = FIXTURE_MODEL
) -> AIMessage:
    """An AIMessage shaped like a thinking model's: a thinking block, then the text."""
    metadata = {"model_name": model}
    if stop is not None:
        metadata["stop_reason"] = stop
    return AIMessage(
        content=[
            {"type": "thinking", "thinking": "", "signature": "test"},
            {"type": "text", "text": text},
        ],
        response_metadata=metadata,
        usage_metadata={"input_tokens": 120, "output_tokens": 40, "total_tokens": 160},
    )


def fake_model(*replies: AIMessage | str) -> GenericFakeChatModel:
    """A chat model that answers with ``replies`` in order, and fails if asked again."""
    return GenericFakeChatModel(
        messages=iter(
            [item if isinstance(item, AIMessage) else reply(item) for item in replies]
        )
    )


# --------------------------------------------------------------------------
# A page, a schema, and the command line
# --------------------------------------------------------------------------

RESULTS_PAGE = """<!doctype html>
<html><head><title>Acme Corp &mdash; third-quarter results</title>
<style>.hidden { display: none }</style>
<script>var tracking = "Acme Corp revenue 999";</script></head>
<body>
<h1>Acme Corp reports third-quarter results</h1>
<p>Revenue was $1,234.5 million, up 12.50% from a year earlier.</p>
<p>Published 2026-10-01 by Investor&nbsp;Relations.</p>
<table><tr><th>Metric</th><th>Value</th></tr>
<tr><td>Net income</td><td>210.75</td></tr></table>
<p><a href="/reports/q3.pdf">Full report</a></p>
</body></html>
"""

RESULTS_SCHEMA = {
    "type": "object",
    "properties": {
        "company": {"type": "string"},
        "revenue": {"type": "string"},
        "growth_percent": {"type": "number"},
        "net_income": {"type": "number"},
        "published": {"type": "string"},
        "report_link": {"type": ["string", "null"]},
    },
    "required": ["company", "revenue", "growth_percent", "net_income", "published"],
    "additionalProperties": False,
}

#: Every value is in RESULTS_PAGE, numbers in the page's own spelling.
GOOD_ANSWER = (
    '{"company": "Acme Corp", "revenue": "$1,234.5 million", "growth_percent": 12.50, '
    '"net_income": 210.75, "published": "2026-10-01", "report_link": "/reports/q3.pdf"}'
)


@pytest.fixture
def schema_file(tmp_path):
    path = tmp_path / "results.schema.json"
    path.write_text(json.dumps(RESULTS_SCHEMA))
    return path


@pytest.fixture
def results_site(site):
    site.robots("User-agent: *\nAllow: /\n")
    site.page("/results", RESULTS_PAGE)
    return site


@pytest.fixture
def run(tmp_path, capsys):
    """Run the command line in-process; return its exit code, stderr and proposal.

    ``model_id`` is passed as ``--model`` unless the arguments already name one;
    None passes no ``--model`` at all, for the tests of where the id comes from.
    Pages are fetched under ``local_servers_only``; ``address_policy=None``
    runs the tool's own policy, exactly as the command line does.
    """

    def go(
        *arguments: str,
        model=None,
        output: Path | None = None,
        model_id: str | None = FIXTURE_MODEL,
        address_policy=local_servers_only,
    ):
        output = output or tmp_path / "proposal.json"
        argv = list(arguments)
        if "--output" not in argv:
            argv += ["--output", str(output)]
        if model_id is not None and "--model" not in argv:
            argv += ["--model", model_id]
        code = cli.main(argv, model=model, address_policy=address_policy)
        captured = capsys.readouterr()
        proposal = json.loads(output.read_text()) if output.exists() else None
        return SimpleNamespace(
            code=code,
            err=captured.err,
            out=captured.out,
            proposal=proposal,
            output=output,
        )

    return go
