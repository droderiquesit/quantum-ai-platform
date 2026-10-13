"""Fetching, done by this tool and never by the library.

ScrapeGraphAI's default fetch path drives Chromium with certificate errors
ignored and a bot-detection evasion layer on, reads no robots.txt, can route
through free public proxies it harvests, and extracts an HTTP error page as
if it were content (2.3.1: ``nodes/fetch_node.py``, ``docloaders/chromium.py``,
``utils/proxy_rotation.py``). None of that runs here. This module fetches
each page itself, under these rules:

* http and https only, and no credentials in a URL.
* Global unicast addresses only. Before every request (the page, its
  robots.txt, and every redirect hop of either) the host is resolved here,
  and every address it resolves to must be global: loopback, link-local,
  private, multicast, reserved, unspecified, site-local and IPv4-mapped
  addresses are refused, in both families. The connection then goes to the
  address that was checked: the URL handed to the transport names the
  address, while the Host header, the TLS server name and the certificate
  check keep the name, so a second DNS answer cannot swap the address after
  the check. Nothing relaxes this; there is no option for it.
  Through a proxy the proxy resolves the name and connects, so there the
  check is made on this machine's resolution of the name before the proxy is
  asked, and a name this machine cannot resolve is refused. A name that
  resolves differently for the proxy (DNS rebinding between the two
  lookups) is not covered: the proxy's own egress policy stands there.
* robots.txt is read before the page, for every origin a redirect reaches.
  Two readings must both allow: ``urllib.robotparser``, and an RFC 9309
  longest-match evaluation. The stdlib parser applies rules in file order,
  so ``Allow: /`` followed by ``Disallow: /private/`` lets it fetch
  ``/private/x``; RFC 9309 says the longer rule wins. Either one refusing
  refuses. An unreachable robots.txt refuses (RFC 9309 2.3.1.4); an absent
  one refuses unless the operator says otherwise, because the platform's own
  rule is that an absent robots.txt is not permission (qip-data-finder,
  ``legal.rs``).
* robots.txt judges the path that is sent. Each URL is put in the form httpx
  will request before anything judges it, which removes dot segments: judged
  as written, ``/open/../no-crawl/x`` passed ``Disallow: /no-crawl/`` and
  httpx then asked for ``/no-crawl/x``. A dot segment that survives because
  it was percent-encoded (or carries a path parameter, ``..;``) is refused,
  because the server, not this tool, decides what it names. robots.txt is
  parsed and matched in linear time: it is the server's to write.
* A server's malformed input costs that server's page and nothing else: a
  robots.txt line the stdlib parser cannot read, a Crawl-delay too large to
  be a number, or a Location that is not a URL refuses the page, and the run
  goes on. Each is caught at the one call that parses the server's value, so
  a fault in this module's own code still ends the run as a fault.
* TLS verification on, through the system trust store (``SSL_CERT_FILE`` and
  ``SSL_CERT_DIR`` honoured, as OpenSSL does).
* Explicit connect, read, write and pool timeouts, a timeout on name
  resolution, and a deadline for the whole body, because a read timeout
  alone lets a server drip one byte per interval for ever.
* A byte ceiling on the bytes read from the socket, refused rather than
  truncated: a truncated page is not a smaller page. Only an identity body is
  read. httpx inflates gzip, deflate and zstd bodies whatever was asked for,
  and 8 KiB of zstd inflates to 256 MiB before a ceiling on the inflated
  bytes sees the first chunk, so a body with any other Content-Encoding is
  refused unread, robots.txt included.
* An honest User-Agent that names this tool, so a publisher can tell it no.
* The proxy the environment names (``HTTPS_PROXY`` / ``HTTP_PROXY``, with
  ``NO_PROXY``), which is the platform's egress design. Never a proxy list,
  never a broker; nothing here can choose a proxy the operator did not set.
"""

from __future__ import annotations

import dataclasses
import hashlib
import ipaddress
import os
import re
import socket
import ssl
import threading
import time
import urllib.parse
import urllib.request
import urllib.robotparser
from collections.abc import Callable, Mapping, Sequence
from datetime import UTC, datetime
from typing import Any

import httpx

from . import TOOL_NAME, __version__

#: The token a robots.txt group names to address this tool (RFC 9309 2.2.1).
ROBOTS_AGENT = TOOL_NAME
USER_AGENT = (
    f"{TOOL_NAME}/{__version__} (operator-run extraction proposals for a "
    "research desk; honours robots.txt)"
)
ACCEPT = "text/html, application/xhtml+xml;q=0.9, text/plain;q=0.8"
ACCEPTED_MEDIA_TYPES = frozenset({"text/html", "application/xhtml+xml", "text/plain"})
REDIRECT_STATUSES = frozenset({301, 302, 303, 307, 308})
#: RFC 9309 2.3.1.2: follow at least five consecutive redirects for robots.txt.
ROBOTS_REDIRECTS = 5
#: Response headers kept in the proposal. Last-Modified and Date matter most:
#: they are the closest thing a page offers to when its content became known.
KEPT_HEADERS = (
    "content-type",
    "content-length",
    "content-encoding",
    "last-modified",
    "etag",
    "date",
)


class FetchRefused(Exception):
    """A URL this tool will not read, and why."""

    def __init__(self, url: str, reason: str, *, robots=(), status: int | None = None):
        super().__init__(f"{url}: {reason}")
        self.url = url
        self.reason = reason
        self.robots = tuple(robots)
        self.status = status


class _NotConnected(FetchRefused):
    """No connection to one address; the next of the name's addresses may be tried."""


#: Upper bounds a caller may not exceed. A value past one is refused, not
#: lowered: a limit silently corrected is a caller bug that survives.
CEILINGS = {
    "connect_timeout": 60.0,
    "read_timeout": 120.0,
    "total_timeout": 300.0,
    "max_page_bytes": 20 * 1024 * 1024,
    "max_robots_bytes": 1024 * 1024,
    "max_redirects": 10,
    "max_crawl_delay": 120.0,
}


@dataclasses.dataclass(frozen=True)
class FetchLimits:
    connect_timeout: float = 10.0
    read_timeout: float = 20.0
    total_timeout: float = 60.0
    max_page_bytes: int = 5 * 1024 * 1024
    #: RFC 9309 2.5 asks parsers to read at least 500 KiB.
    max_robots_bytes: int = 512 * 1024
    max_redirects: int = 5
    #: Longest Crawl-delay this tool will wait out between two pages of one
    #: origin; a longer one refuses the page rather than stalling the run.
    max_crawl_delay: float = 30.0

    def __post_init__(self) -> None:
        for name, ceiling in CEILINGS.items():
            value = getattr(self, name)
            floor_ok = value >= 0 if name == "max_redirects" else value > 0
            if not floor_ok or not value <= ceiling:
                raise ValueError(
                    f"{name} must be {'at least 0' if name == 'max_redirects' else 'above 0'} "
                    f"and at most {ceiling}; {value!r} is refused, not adjusted"
                )


def utc_now() -> str:
    return datetime.now(UTC).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def check_url(url: str) -> urllib.parse.SplitResult:
    """Refuse anything but an http(s) URL with a host and no credentials."""
    try:
        parts = urllib.parse.urlsplit(url)
        parts.port  # noqa: B018 - raises ValueError for a malformed port
    except ValueError as error:
        raise FetchRefused(url, f"not a valid URL ({error})") from None
    if parts.scheme not in ("http", "https"):
        raise FetchRefused(
            url,
            f"only http and https are fetched; {parts.scheme or 'a URL with no scheme'} is refused",
        )
    if not parts.hostname:
        raise FetchRefused(url, "has no host")
    if parts.username is not None or parts.password is not None:
        raise FetchRefused(
            url,
            "carries credentials in its userinfo; a credential in a URL lands in the "
            "proposal, in the logs and in the server's access log",
        )
    return parts


def canonical_url(url: str) -> str:
    """``url`` in the form httpx will request it, which is the form judged and recorded.

    httpx removes dot segments before it sends a path, so a URL judged as
    written is not the URL fetched: ``/open/../no-crawl/x`` passed
    ``Disallow: /no-crawl/`` and httpx then asked for ``/no-crawl/x``. A dot
    segment still present in the decoded path got past httpx as a
    percent-escape (``%2e%2e``), behind a backslash, or with a path parameter
    (``..;``, which servlet containers read as ``..``), and is refused: what
    it names is decided by the server, and a server that resolves it serves a
    path robots.txt was never asked about.
    """
    check_url(url)
    try:
        target = httpx.URL(url)
    except httpx.InvalidURL as error:
        raise FetchRefused(url, f"not a valid URL ({error})") from None
    segments = target.path.replace("\\", "/").split("/")
    if any(segment.partition(";")[0] in (".", "..") for segment in segments):
        raise FetchRefused(
            url,
            "its path holds a dot segment written as a percent-escape, behind a "
            "backslash or with a path parameter; the server would decide which path "
            "that names, so the URL is refused rather than judged against robots.txt "
            "by a path it may not be",
        )
    return str(target)


def origin_of(parts: urllib.parse.SplitResult) -> str:
    default = {"http": 80, "https": 443}[parts.scheme]
    port = parts.port or default
    host = parts.hostname or ""
    if ":" in host:
        host = f"[{host}]"
    return f"{parts.scheme}://{host}" + ("" if port == default else f":{port}")


def select_proxy(url: str, environ: Mapping[str, str]) -> str | None:
    """The proxy the environment names for ``url``, or None.

    The standard variables, lower case first as curl and urllib read them:
    ``https_proxy``/``HTTPS_PROXY`` for https, ``http_proxy``/``HTTP_PROXY``
    for http, and ``no_proxy``/``NO_PROXY`` to bypass. Nothing else can put a
    proxy on the path.
    """
    parts = urllib.parse.urlsplit(url)
    proxy = environ.get(f"{parts.scheme}_proxy") or environ.get(
        f"{parts.scheme.upper()}_PROXY"
    )
    if not proxy:
        return None
    no_proxy = environ.get("no_proxy") or environ.get("NO_PROXY") or ""
    if no_proxy and urllib.request.proxy_bypass_environment(
        parts.hostname or "", {"no": no_proxy}
    ):
        return None
    return proxy


def describe_proxy(proxy: str | None) -> str | None:
    """A proxy URL fit for a proposal: scheme, host and port, no credentials."""
    if proxy is None:
        return None
    parts = urllib.parse.urlsplit(proxy)
    host = parts.hostname or "?"
    return f"{parts.scheme}://{host}" + (f":{parts.port}" if parts.port else "")


# --------------------------------------------------------------------------
# Addresses
# --------------------------------------------------------------------------

IPAddress = ipaddress.IPv4Address | ipaddress.IPv6Address

#: Checked in this order, so that a refusal names the most specific class
#: (Python counts 240.0.0.0/4 as private as well as reserved). ``is_global``
#: alone is not enough: Python 3.12 calls multicast addresses (224.0.0.1,
#: ff02::1), IPv6 site-local ones (fec0::1) and IPv4-compatible ones
#: (::7f00:1, which embeds 127.0.0.1) global.
_REFUSED_CLASSES = (
    ("an unspecified address", "is_unspecified"),
    ("a loopback address", "is_loopback"),
    ("a link-local address", "is_link_local"),
    ("a multicast address", "is_multicast"),
    ("a site-local address", "is_site_local"),
    ("a reserved address", "is_reserved"),
    ("a private address", "is_private"),
)
#: How many of a name's addresses are tried, in the resolver's order, when
#: one cannot be connected to. Every address is checked, tried or not.
ADDRESSES_TRIED = 3


def address_refusal(address: IPAddress) -> str | None:
    """Why this tool will not connect to ``address``, or None if it will.

    Only a global unicast address is admitted. A redirect, or a name in DNS,
    is the server's to choose, and either would otherwise point this tool at
    the operator's own machine, its network, or a cloud metadata address.
    """
    if isinstance(address, ipaddress.IPv6Address) and address.ipv4_mapped is not None:
        return f"an IPv4-mapped address (of {address.ipv4_mapped})"
    for description, attribute in _REFUSED_CLASSES:
        if getattr(address, attribute, False):
            return description
    if not address.is_global:
        return "not a global address"
    return None


def resolve_host(host: str, port: int, timeout: float) -> list[str]:
    """The addresses ``host`` resolves to, in the resolver's order.

    ``getaddrinfo`` takes no timeout, so it runs on a daemon thread that is
    abandoned, not waited for, once ``timeout`` has passed.
    """
    outcome: dict[str, Any] = {}

    def lookup() -> None:
        try:
            outcome["found"] = socket.getaddrinfo(host, port, type=socket.SOCK_STREAM)
        except (OSError, UnicodeError) as error:
            outcome["error"] = error

    worker = threading.Thread(target=lookup, name="qip-scraper resolve", daemon=True)
    worker.start()
    worker.join(timeout)
    if worker.is_alive():
        raise TimeoutError(f"no answer within {timeout}s")
    if "error" in outcome:
        raise outcome["error"]
    addresses: list[str] = []
    for *_rest, sockaddr in outcome["found"]:
        if sockaddr[0] not in addresses:
            addresses.append(sockaddr[0])
    return addresses


def _pinned(target: httpx.URL, address: str) -> tuple[httpx.URL, dict, dict]:
    """The request for ``target``, sent to ``address`` and nowhere else.

    The URL the transport gets names the address, so nothing resolves the
    name a second time; the Host header keeps the name, and for https so do
    the TLS server name and the certificate check (httpcore's
    ``sni_hostname``, which it passes to the handshake as ``server_hostname``).
    """
    headers = {"Host": target.netloc.decode("ascii")}
    extensions = (
        {"sni_hostname": target.raw_host.decode("ascii")}
        if target.scheme == "https"
        else {}
    )
    return target.copy_with(host=address), headers, extensions


# --------------------------------------------------------------------------
# robots.txt
# --------------------------------------------------------------------------

_ROBOTS_KEY = frozenset("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz-")
_UNRESERVED = frozenset(
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~"
)
_PERCENT = re.compile(r"%([0-9A-Fa-f]{2})")


def _normalise_path(path: str) -> str:
    """RFC 9309 2.2.2: encode what must be encoded, decode what may not be."""
    encoded = urllib.parse.quote(path, safe="/?#[]@!$&'()*+,;=:%-._~")

    def fix(match: re.Match) -> str:
        character = chr(int(match.group(1), 16))
        return character if character in _UNRESERVED else "%" + match.group(1).upper()

    return _PERCENT.sub(fix, encoded)


def _robots_line(raw: str) -> tuple[str, str] | None:
    """One robots.txt line as (key in lower case, value), or None.

    Split by hand. The regular expression this replaced backtracked over a
    run of whitespace inside a value, quadratically: 20,000 spaces took 1.8s,
    and the 512 KiB a robots.txt may hold would have taken minutes.
    """
    key, colon, value = raw.split("#", 1)[0].partition(":")
    key = key.strip()
    if not colon or not key or not set(key) <= _ROBOTS_KEY:
        return None
    return key.lower(), value.strip()


def _robots_groups(body: str) -> list[tuple[list[str], list[tuple[bool, str]]]]:
    groups: list[tuple[list[str], list[tuple[bool, str]]]] = []
    agents: list[str] = []
    rules: list[tuple[bool, str]] = []
    in_rules = False
    for raw in body.splitlines():
        line = _robots_line(raw)
        if line is None:
            continue
        key, value = line
        if key == "user-agent":
            if in_rules:
                groups.append((agents, rules))
                agents, rules, in_rules = [], [], False
            agents.append(value.split("/", 1)[0].strip().lower())
        elif key in ("allow", "disallow") and agents:
            in_rules = True
            rules.append((key == "allow", value))
    if agents:
        groups.append((agents, rules))
    return groups


def _pattern_matches(pattern: str, path: str) -> bool:
    """RFC 9309 2.2.3: ``*`` matches any run of characters, a final ``$``
    anchors the end, and otherwise the pattern need match only a prefix.

    Matched piece by piece with ``str.find``, leftmost first, which is exact
    when ``*`` is the only wildcard, and takes time linear in the path for
    each piece. The regular expression this replaced turned each ``*`` into
    ``.*`` and backtracked: seven wildcards against an 80-character path
    took about eight seconds, and robots.txt is the server's to write.
    """
    anchored = pattern.endswith("$")
    pieces = (pattern[:-1] if anchored else pattern).split("*")
    if not path.startswith(pieces[0]):
        return False
    if len(pieces) == 1:
        return path == pieces[0] if anchored else True
    start, end = len(pieces[0]), len(path)
    middle = pieces[1:]
    if anchored:
        last = middle.pop()
        if not path.endswith(last) or end - len(last) < start:
            return False
        end -= len(last)
    for piece in middle:
        found = path.find(piece, start, end)
        if found < 0:
            return False
        start = found + len(piece)
    return True


def rfc9309_decision(body: str, agent: str, path: str) -> tuple[bool, str | None]:
    """Whether RFC 9309 lets ``agent`` fetch ``path``, and the deciding rule.

    Groups naming the agent are combined; only if none does are the ``*``
    groups used. The longest matching rule wins, and at equal length Allow
    wins. ``path`` is the URL's path plus its query.
    """
    if path == "/robots.txt":
        return True, None
    groups = _robots_groups(body)
    agent = agent.lower()
    chosen = [group for group in groups if agent in group[0]] or [
        group for group in groups if "*" in group[0]
    ]
    target = _normalise_path(path or "/")
    best: tuple[int, bool, str] | None = None
    for _agents, rules in chosen:
        for allow, pattern in rules:
            if not pattern:
                continue  # an empty Disallow forbids nothing; an empty Allow says nothing
            normalised = _normalise_path(pattern)
            if _pattern_matches(normalised, target):
                candidate = (len(normalised), allow, pattern)
                if best is None or candidate[:2] > best[:2]:
                    best = candidate
    if best is None:
        return True, None
    return best[1], f"{'Allow' if best[1] else 'Disallow'}: {best[2]}"


@dataclasses.dataclass(frozen=True)
class RobotsVerdict:
    """robots.txt's answer for one URL, as recorded in the proposal."""

    url: str
    robots_url: str
    state: str  # served | absent | forbidden | unreachable
    http_status: int | None
    robotparser_allows: bool | None
    rfc9309_allows: bool | None
    deciding_rule: str | None
    crawl_delay: float | None
    allowed: bool
    reason: str

    def as_record(self) -> dict:
        return dataclasses.asdict(self)


@dataclasses.dataclass(frozen=True)
class _RobotsFile:
    robots_url: str
    state: str
    http_status: int | None
    body: str | None
    detail: str


@dataclasses.dataclass(frozen=True)
class FetchedPage:
    requested_url: str
    final_url: str
    redirects: tuple[str, ...]
    fetched_at: str
    http_status: int
    media_type: str
    charset: str | None
    headers: dict
    body: bytes
    sha256: str
    proxy: str | None
    robots: tuple[RobotsVerdict, ...]


@dataclasses.dataclass(frozen=True)
class _Response:
    url: str
    status: int
    headers: httpx.Headers
    body: bytes
    proxy: str | None


class Fetcher:
    """Fetches pages one at a time, keeping robots.txt and pacing per origin."""

    def __init__(
        self,
        limits: FetchLimits | None = None,
        *,
        allow_absent_robots: bool = False,
        environ: Mapping[str, str] | None = None,
        sleep: Callable[[float], None] = time.sleep,
        monotonic: Callable[[], float] = time.monotonic,
        resolve: Callable[[str, int, float], Sequence[str]] = resolve_host,
        address_policy: Callable[[IPAddress], str | None] = address_refusal,
    ) -> None:
        """``resolve`` and ``address_policy`` exist for the tests, whose local
        servers listen on 127.0.0.1. The command line cannot set either."""
        self.limits = limits or FetchLimits()
        self.allow_absent_robots = allow_absent_robots
        self.environ = os.environ if environ is None else environ
        self._sleep = sleep
        self._monotonic = monotonic
        self._resolve = resolve
        self._address_policy = address_policy
        self._robots: dict[str, _RobotsFile] = {}
        self._last_page_at: dict[str, float] = {}

    # -- transport ---------------------------------------------------------

    def _client(self, proxy: str | None) -> httpx.Client:
        limits = self.limits
        return httpx.Client(
            # The system trust store, with SSL_CERT_FILE/SSL_CERT_DIR honoured
            # by OpenSSL itself: verification on, hostname checked, TLS 1.2+.
            verify=ssl.create_default_context(),
            # The environment is read by select_proxy and nowhere else, so
            # the proxy recorded in a proposal is the proxy that was used.
            trust_env=False,
            proxy=proxy,
            follow_redirects=False,
            timeout=httpx.Timeout(
                connect=limits.connect_timeout,
                read=limits.read_timeout,
                write=limits.read_timeout,
                pool=limits.connect_timeout,
            ),
            headers={
                "User-Agent": USER_AGENT,
                "Accept": ACCEPT,
                # The hashed bytes are then the bytes on the wire, and a
                # second fetch can be compared with the first.
                "Accept-Encoding": "identity",
            },
        )

    def _admitted(self, url: str) -> list[str]:
        """Every address ``url``'s host resolves to, each one admitted by the
        address policy, or a refusal naming the first that is not."""
        try:
            target = httpx.URL(url)
        except httpx.InvalidURL as error:
            raise FetchRefused(url, f"not a valid URL ({error})") from None
        host = target.raw_host.decode("ascii")
        try:
            names = [str(ipaddress.ip_address(host))]
            resolved = False
        except ValueError:
            port = target.port or (443 if target.scheme == "https" else 80)
            try:
                names = list(self._resolve(host, port, self.limits.connect_timeout))
            except (OSError, UnicodeError) as error:
                raise FetchRefused(
                    url,
                    f"{host} could not be resolved ({type(error).__name__}: {error}); an "
                    "address that cannot be checked is not connected to",
                ) from None
            resolved = True
        if not names:
            raise FetchRefused(url, f"{host} resolved to no address")
        admitted: list[str] = []
        for name in names:
            try:
                address = ipaddress.ip_address(name.partition("%")[0])
            except ValueError:
                raise FetchRefused(
                    url, f"{host} resolved to {name!r}, which is not an IP address"
                ) from None
            reason = self._address_policy(address)
            if reason is not None:
                raise FetchRefused(
                    url,
                    (f"{host} resolves to {address}, " if resolved else f"{host} is ")
                    + f"{reason}. This tool connects only to global unicast addresses, "
                    "and no option changes that",
                )
            admitted.append(str(address))
        return admitted

    def _get(self, url: str, max_bytes: int) -> _Response:
        """One GET of ``url``, to an address checked for this very request."""
        proxy = select_proxy(url, self.environ)
        addresses = self._admitted(url)
        deadline = self._monotonic() + self.limits.total_timeout
        target = httpx.URL(url)
        if proxy is not None:
            # The proxy resolves the name and connects. The check above ran on
            # this machine's own answer for the name; see the module docstring
            # for what that leaves to the proxy.
            return self._exchange(url, target, {}, {}, proxy, max_bytes, deadline)
        failures = []
        for address in addresses[:ADDRESSES_TRIED]:
            request_url, headers, extensions = _pinned(target, address)
            try:
                return self._exchange(
                    url, request_url, headers, extensions, None, max_bytes, deadline
                )
            except _NotConnected as failure:
                failures.append(f"{address}: {failure.reason}")
        raise FetchRefused(url, "; ".join(failures))

    def _exchange(
        self,
        url: str,
        request_url: httpx.URL,
        headers: dict,
        extensions: dict,
        proxy: str | None,
        max_bytes: int,
        deadline: float,
    ) -> _Response:
        try:
            with (
                self._client(proxy) as client,
                client.stream(
                    "GET", request_url, headers=headers, extensions=extensions
                ) as response,
            ):
                status = response.status_code
                if status in REDIRECT_STATUSES:
                    return _Response(
                        url, status, response.headers, b"", describe_proxy(proxy)
                    )
                codings = [
                    coding.strip().lower()
                    for coding in response.headers.get_list(
                        "content-encoding", split_commas=True
                    )
                    if coding.strip()
                ]
                if any(coding != "identity" for coding in codings):
                    raise FetchRefused(
                        url,
                        f"sent Content-Encoding {', '.join(codings)}, though only identity "
                        "was asked for; refused unread, because the size of a compressed "
                        "body on the wire says nothing about its size once inflated",
                        status=status,
                    )
                declared = response.headers.get("content-length", "").strip()
                if declared.isdigit() and int(declared) > max_bytes:
                    raise FetchRefused(
                        url,
                        f"declares {declared} bytes, more than the {max_bytes}-byte ceiling; "
                        "refused rather than truncated",
                        status=status,
                    )
                body = bytearray()
                # The bytes off the socket, never decoded by httpx, so the
                # ceiling counts exactly what is held in memory.
                for chunk in response.iter_raw():
                    body += chunk
                    if len(body) > max_bytes:
                        raise FetchRefused(
                            url,
                            f"sent more than the {max_bytes}-byte ceiling; refused rather "
                            "than truncated, because a truncated page is not a smaller page",
                            status=status,
                        )
                    if self._monotonic() > deadline:
                        raise FetchRefused(
                            url,
                            f"took longer than {self.limits.total_timeout}s to send its body",
                            status=status,
                        )
                return _Response(
                    url, status, response.headers, bytes(body), describe_proxy(proxy)
                )
        except (httpx.ConnectError, httpx.ConnectTimeout) as error:
            raise _NotConnected(
                url, f"could not connect ({type(error).__name__}: {error})"
            ) from None
        except httpx.TimeoutException as error:
            raise FetchRefused(url, f"timed out ({type(error).__name__})") from None
        except httpx.HTTPError as error:
            raise FetchRefused(url, f"{type(error).__name__}: {error}") from None

    # -- robots.txt ---------------------------------------------------------

    def _robots_file(self, origin: str) -> _RobotsFile:
        cached = self._robots.get(origin)
        if cached is not None:
            return cached
        robots_url = origin + "/robots.txt"
        current = robots_url
        for _hop in range(ROBOTS_REDIRECTS + 1):
            try:
                response = self._get(current, self.limits.max_robots_bytes)
            except FetchRefused as refusal:
                found = _RobotsFile(
                    robots_url, "unreachable", refusal.status, None, refusal.reason
                )
                break
            if response.status in REDIRECT_STATUSES:
                location = response.headers.get("location")
                # The Location is the server's to write, and urllib raises
                # ValueError on one it cannot parse ("http://[", or a host
                # that NFKC turns into a separator): that ended the whole run.
                try:
                    following = (
                        urllib.parse.urljoin(current, location) if location else ""
                    )
                    scheme = urllib.parse.urlsplit(following).scheme
                except ValueError as error:
                    found = _RobotsFile(
                        robots_url,
                        "unreachable",
                        response.status,
                        None,
                        f"redirected to {location!r}, which is not a URL ({error})",
                    )
                    break
                if scheme not in ("http", "https"):
                    found = _RobotsFile(
                        robots_url,
                        "unreachable",
                        response.status,
                        None,
                        f"redirected to {following or 'nowhere'}, which is not http(s)",
                    )
                    break
                current = following
                continue
            status = response.status
            if 200 <= status < 300:
                try:
                    text = response.body.decode("utf-8-sig")
                except UnicodeDecodeError:
                    found = _RobotsFile(
                        robots_url,
                        "unreachable",
                        status,
                        None,
                        "robots.txt is not UTF-8; a policy nobody can read is not an absent one",
                    )
                else:
                    found = _RobotsFile(robots_url, "served", status, text, "served")
            elif status in (401, 403):
                found = _RobotsFile(
                    robots_url, "forbidden", status, None, f"HTTP {status}"
                )
            elif 400 <= status < 500:
                found = _RobotsFile(
                    robots_url, "absent", status, None, f"HTTP {status}"
                )
            else:
                found = _RobotsFile(
                    robots_url, "unreachable", status, None, f"HTTP {status}"
                )
            break
        else:
            found = _RobotsFile(
                robots_url,
                "unreachable",
                None,
                None,
                f"more than {ROBOTS_REDIRECTS} redirects",
            )
        self._robots[origin] = found
        return found

    def robots_verdict(self, url: str) -> RobotsVerdict:
        parts = check_url(url)
        robots = self._robots_file(origin_of(parts))
        # The path and query httpx will send, dot segments removed: judged as
        # written, /open/../no-crawl/x passed "Disallow: /no-crawl/".
        requested = httpx.URL(url)
        path = requested.raw_path.decode("ascii")
        common = {
            "url": url,
            "robots_url": robots.robots_url,
            "state": robots.state,
            "http_status": robots.http_status,
        }
        if robots.state == "served":
            rfc_allows, rule = rfc9309_decision(robots.body or "", ROBOTS_AGENT, path)
            parser = urllib.robotparser.RobotFileParser()
            try:
                # The stdlib parser reads Crawl-delay and Request-rate with
                # int(), which raises on a digit it cannot convert ("²") or on
                # more than 4,300 digits, and each rule's path with urlparse,
                # which raises on "http://[". Any of them ended the whole run.
                parser.parse((robots.body or "").splitlines())
            except ValueError as error:
                return RobotsVerdict(
                    **common,
                    robotparser_allows=None,
                    rfc9309_allows=rfc_allows,
                    deciding_rule=rule,
                    crawl_delay=None,
                    allowed=False,
                    reason=(
                        f"robots.txt holds a line urllib.robotparser cannot read ({error}); "
                        "a policy only one of the two readings can read is not permission, "
                        "so the page is refused"
                    ),
                )
            robotparser_allows = parser.can_fetch(ROBOTS_AGENT, str(requested))
            delay = parser.crawl_delay(ROBOTS_AGENT)
            try:
                crawl_delay = None if delay is None else float(delay)
            except OverflowError:  # an integer of hundreds of digits
                return RobotsVerdict(
                    **common,
                    robotparser_allows=robotparser_allows,
                    rfc9309_allows=rfc_allows,
                    deciding_rule=rule,
                    crawl_delay=None,
                    allowed=False,
                    reason=(
                        "robots.txt asks for a Crawl-delay too large to be a number of "
                        f"seconds, longer than the {self.limits.max_crawl_delay}s this tool "
                        "will wait"
                    ),
                )
            allowed = robotparser_allows and rfc_allows
            if allowed:
                reason = "allowed by robots.txt (urllib.robotparser and RFC 9309 agree)"
            else:
                refusers = [
                    name
                    for name, verdict in (
                        ("urllib.robotparser", robotparser_allows),
                        ("RFC 9309", rfc_allows),
                    )
                    if not verdict
                ]
                reason = (
                    f"disallowed by robots.txt according to {' and '.join(refusers)}"
                    + (f" ({rule})" if rule and not rfc_allows else "")
                )
            return RobotsVerdict(
                **common,
                robotparser_allows=robotparser_allows,
                rfc9309_allows=rfc_allows,
                deciding_rule=rule,
                crawl_delay=crawl_delay,
                allowed=allowed,
                reason=reason,
            )
        if robots.state == "absent":
            allowed = self.allow_absent_robots
            reason = (
                f"no robots.txt ({robots.detail}); fetched because --allow-absent-robots was given"
                if allowed
                else f"no robots.txt ({robots.detail}); an absent robots.txt is not permission "
                "(the platform's own rule), so the page is refused. Rerun with "
                "--allow-absent-robots to fetch it anyway and have that recorded"
            )
        elif robots.state == "forbidden":
            allowed = False
            reason = (
                f"robots.txt answered {robots.detail}, which urllib.robotparser and this "
                "tool read as disallowing everything"
            )
        else:
            allowed = False
            reason = (
                f"robots.txt could not be read ({robots.detail}); RFC 9309 2.3.1.4 "
                "treats an unreachable robots.txt as disallowing everything"
            )
        return RobotsVerdict(
            **common,
            robotparser_allows=None,
            rfc9309_allows=None,
            deciding_rule=None,
            crawl_delay=None,
            allowed=allowed,
            reason=reason,
        )

    def _pace(self, origin: str, crawl_delay: float | None, url: str, verdicts) -> None:
        if crawl_delay is None:
            return
        if crawl_delay > self.limits.max_crawl_delay:
            raise FetchRefused(
                url,
                f"robots.txt asks for a Crawl-delay of {crawl_delay}s, longer than the "
                f"{self.limits.max_crawl_delay}s this tool will wait",
                robots=verdicts,
            )
        last = self._last_page_at.get(origin)
        if last is not None:
            remaining = last + crawl_delay - self._monotonic()
            if remaining > 0:
                self._sleep(remaining)

    # -- pages ----------------------------------------------------------------

    def fetch(self, url: str) -> FetchedPage:
        """Fetch one page, or raise ``FetchRefused`` saying why not."""
        current = canonical_url(url)
        redirects: list[str] = []
        verdicts: list[RobotsVerdict] = []
        previous_scheme: str | None = None
        for _hop in range(self.limits.max_redirects + 1):
            parts = check_url(current)
            if previous_scheme == "https" and parts.scheme == "http":
                raise FetchRefused(
                    url,
                    f"redirected from https to {current}; following it would drop TLS",
                    robots=verdicts,
                )
            # Before robots.txt or the page is asked for: a refusal here names
            # the page's own host rather than its robots.txt.
            try:
                self._admitted(current)
            except FetchRefused as refusal:
                raise FetchRefused(url, refusal.reason, robots=verdicts) from None
            verdict = self.robots_verdict(current)
            verdicts.append(verdict)
            if not verdict.allowed:
                raise FetchRefused(url, verdict.reason, robots=verdicts)
            origin = origin_of(parts)
            self._pace(origin, verdict.crawl_delay, url, verdicts)
            fetched_at = utc_now()
            try:
                response = self._get(current, self.limits.max_page_bytes)
            except FetchRefused as refusal:
                raise FetchRefused(
                    url, refusal.reason, robots=verdicts, status=refusal.status
                ) from None
            finally:
                self._last_page_at[origin] = self._monotonic()
            if response.status not in REDIRECT_STATUSES:
                break
            location = response.headers.get("location")
            if not location:
                raise FetchRefused(
                    url,
                    f"HTTP {response.status} with no Location",
                    robots=verdicts,
                    status=response.status,
                )
            previous_scheme = parts.scheme
            try:
                # The server's Location, which urllib may not parse at all; the
                # ValueError it raises ended the whole run.
                joined = urllib.parse.urljoin(current, location)
            except ValueError as error:
                raise FetchRefused(
                    url,
                    f"redirected to {location!r}, which is not a URL ({error})",
                    robots=verdicts,
                    status=response.status,
                ) from None
            try:
                # An absolute Location keeps its dot segments through urljoin.
                current = canonical_url(joined)
            except FetchRefused as refusal:
                raise FetchRefused(
                    url,
                    f"redirected to {location!r}: {refusal.reason}",
                    robots=verdicts,
                    status=response.status,
                ) from None
            redirects.append(current)
        else:
            raise FetchRefused(
                url, f"more than {self.limits.max_redirects} redirects", robots=verdicts
            )

        if not 200 <= response.status < 300:
            raise FetchRefused(
                url,
                f"answered HTTP {response.status}; an error page is not content",
                robots=verdicts,
                status=response.status,
            )
        content_type = response.headers.get("content-type")
        if not content_type:
            raise FetchRefused(
                url,
                "sent no Content-Type; refusing to guess what the bytes are",
                robots=verdicts,
                status=response.status,
            )
        media_type, _, parameters = content_type.partition(";")
        media_type = media_type.strip().lower()
        if media_type not in ACCEPTED_MEDIA_TYPES:
            raise FetchRefused(
                url,
                f"is {media_type}; only {', '.join(sorted(ACCEPTED_MEDIA_TYPES))} are read",
                robots=verdicts,
                status=response.status,
            )
        charset = None
        for parameter in parameters.split(";"):
            name, _, value = parameter.partition("=")
            if name.strip().lower() == "charset" and value.strip():
                charset = value.strip().strip('"').strip("'")
        return FetchedPage(
            requested_url=url,
            final_url=current,
            redirects=tuple(redirects),
            fetched_at=fetched_at,
            http_status=response.status,
            media_type=media_type,
            charset=charset,
            headers={
                name: response.headers[name]
                for name in KEPT_HEADERS
                if name in response.headers
            },
            body=response.body,
            sha256=hashlib.sha256(response.body).hexdigest(),
            proxy=response.proxy,
            robots=tuple(verdicts),
        )
