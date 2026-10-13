"""Protection 2: the tool fetches, the library never does, and fetching is bounded."""

from __future__ import annotations

import dataclasses
import ipaddress
import socket
import time

import pytest
from conftest import RESULTS_PAGE, Site, local_servers_only

from qip_scraper import fetch, guard

ALLOW_ALL = "User-agent: *\nAllow: /\n"
#: A global address for a name that only a test's resolver knows. Under a
#: proxy nothing connects to it: the proxy is handed the name.
PUBLIC_ADDRESS = "93.184.215.14"
_LIMIT_FIELDS = frozenset(field.name for field in dataclasses.fields(fetch.FetchLimits))


def fetcher(**settings) -> fetch.Fetcher:
    """A Fetcher under the tests' address policy (conftest), taking any
    FetchLimits field and any Fetcher option by keyword."""
    limits = {
        name: settings.pop(name) for name in list(settings) if name in _LIMIT_FIELDS
    }
    settings.setdefault("address_policy", local_servers_only)
    return fetch.Fetcher(fetch.FetchLimits(**limits), **settings)


def resolver(table: dict[str, list[str]]):
    """A resolver that knows only ``table`` and records what it was asked."""

    def resolve(host: str, port: int, timeout: float) -> list[str]:
        resolve.asked.append(host)
        if host not in table:
            raise OSError(f"{host} is not in the test's table")
        return table[host]

    resolve.asked = []
    return resolve


# --------------------------------------------------------------------------
# What may be fetched at all
# --------------------------------------------------------------------------


@pytest.mark.parametrize(
    "url",
    [
        "file:///etc/passwd",
        "ftp://example.com/a",
        "javascript:alert(1)",
        "data:text/html,hi",
        "example.com/no-scheme",
        "http:///no-host",
        "gopher://example.com/",
    ],
)
def test_only_http_and_https_are_fetched(url):
    with pytest.raises(fetch.FetchRefused):
        fetch.check_url(url)


def test_a_url_carrying_credentials_is_refused(site):
    url = site.url("/results").replace("http://", "http://operator:hunter2@")
    with pytest.raises(fetch.FetchRefused, match="credentials"):
        fetcher().fetch(url)
    assert site.requests == []


# --------------------------------------------------------------------------
# Addresses: global unicast only, and the address checked is the one reached
# --------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("address", "refused_as"),
    [
        ("127.0.0.1", "loopback"),
        ("127.255.0.9", "loopback"),
        ("::1", "loopback"),
        ("10.1.2.3", "private"),
        ("172.16.0.1", "private"),
        ("192.168.0.1", "private"),
        ("fd00::1", "private"),
        ("169.254.169.254", "link-local"),
        ("fe80::1", "link-local"),
        ("224.0.0.1", "multicast"),
        ("ff02::1", "multicast"),
        ("0.0.0.0", "unspecified"),
        ("::", "unspecified"),
        ("240.0.0.1", "reserved"),
        ("::7f00:1", "reserved"),
        ("fec0::1", "site-local"),
        ("::ffff:127.0.0.1", "IPv4-mapped"),
        ("::ffff:93.184.215.14", "IPv4-mapped"),
        ("100.64.0.1", "not a global address"),
        (PUBLIC_ADDRESS, None),
        ("2606:4700::6810:84e5", None),
    ],
)
def test_only_a_global_unicast_address_is_admitted(address, refused_as):
    # Python 3.12 calls 224.0.0.1, ff02::1, fec0::1 and ::7f00:1 global, so
    # is_global alone would admit four of the rows above.
    reason = fetch.address_refusal(ipaddress.ip_address(address))
    if refused_as is None:
        assert reason is None
    else:
        assert reason is not None and refused_as in reason


def test_the_tools_own_policy_refuses_a_loopback_page_before_asking_it_anything(site):
    # Every other test here admits 127.0.0.1, where the local servers listen;
    # this one runs the policy the command line runs.
    site.robots(ALLOW_ALL)
    url = site.page("/results", RESULTS_PAGE)
    with pytest.raises(fetch.FetchRefused, match="127.0.0.1 is a loopback address"):
        fetcher(address_policy=fetch.address_refusal).fetch(url)
    assert site.requests == []


@pytest.mark.parametrize(
    ("location", "refusal"),
    [
        ("http://169.254.169.254/latest/meta-data/", "169.254.169.254 is a link-local"),
        ("http://internal.test/admin", "internal.test resolves to 10.0.0.7, a private"),
        ("http://[::1]:8080/", "::1 is a loopback address"),
        ("http://127.0.0.2/", "127.0.0.2 is a loopback address"),
    ],
)
def test_a_redirect_inward_is_refused_without_a_connection(site, location, refusal):
    site.robots(ALLOW_ALL)
    url = site.page("/go", "", headers={"Location": location}, status=302)
    names = resolver({"internal.test": ["10.0.0.7"]})
    # The fence records any connection, or lookup, to anything but 127.0.0.1.
    with (
        guard.network_fence({"127.0.0.1"}) as attempts,
        pytest.raises(fetch.FetchRefused, match=refusal) as refused,
    ):
        fetcher(resolve=names).fetch(url)
    assert site.paths() == ["/robots.txt", "/go"]  # the redirect was served...
    assert [verdict.allowed for verdict in refused.value.robots] == [True]
    assert attempts == []  # ...and its target was never reached


def test_robots_txt_redirected_inward_is_not_followed(site):
    site.page(
        "/robots.txt",
        "",
        headers={"Location": "http://10.9.8.7/robots.txt"},
        status=302,
    )
    url = site.page("/results", RESULTS_PAGE)
    with (
        guard.network_fence({"127.0.0.1"}) as attempts,
        pytest.raises(fetch.FetchRefused, match="10.9.8.7 is a private address"),
    ):
        fetcher(allow_absent_robots=True).fetch(url)
    assert site.paths() == ["/robots.txt"]
    assert attempts == []


def test_every_address_a_name_resolves_to_must_be_admitted():
    names = resolver({"mixed.test": [PUBLIC_ADDRESS, "192.168.1.20"]})
    with (
        guard.network_fence(set()) as attempts,
        pytest.raises(fetch.FetchRefused, match="192.168.1.20, a private address"),
    ):
        fetcher(resolve=names).fetch("http://mixed.test/")
    assert names.asked == ["mixed.test"]
    assert attempts == []


def test_the_connection_goes_to_the_checked_address_with_the_name_in_the_host_header(
    site,
):
    # Only the test's resolver knows pub.test. Under the fence any lookup the
    # transport made of its own would be refused and recorded: an empty record
    # is the evidence that the name was resolved once, here, and then pinned.
    site.robots(ALLOW_ALL)
    site.page("/results", RESULTS_PAGE)
    names = resolver({"pub.test": ["127.0.0.1"]})
    url = f"http://pub.test:{site.port}/results"
    with guard.network_fence({"127.0.0.1"}) as attempts:
        page = fetcher(resolve=names).fetch(url)
    assert page.http_status == 200 and page.final_url == url
    assert attempts == []
    assert "pub.test" in names.asked
    hosts = {request["headers"]["Host"] for request in site.requests}
    assert hosts == {f"pub.test:{site.port}"}


def test_over_tls_the_certificate_is_checked_against_the_name_not_the_address(
    tls_site, monkeypatch
):
    # The certificate names localhost and 127.0.0.1, not pub.test. Both names
    # are sent to 127.0.0.1; only the one the certificate names is read.
    monkeypatch.setenv("SSL_CERT_FILE", str(tls_site.certificate.cert))
    tls_site.robots(ALLOW_ALL)
    tls_site.page("/results", RESULTS_PAGE)
    names = resolver({"localhost": ["127.0.0.1"], "pub.test": ["127.0.0.1"]})
    page = fetcher(resolve=names).fetch(f"https://localhost:{tls_site.port}/results")
    assert page.http_status == 200
    with pytest.raises(fetch.FetchRefused, match="CERTIFICATE_VERIFY_FAILED"):
        fetcher(resolve=names).fetch(f"https://pub.test:{tls_site.port}/results")


def test_a_name_whose_first_address_cannot_be_reached_is_tried_at_the_next(site):
    # Nothing listens on 127.0.0.2. A policy admitting it stands in for a
    # name with two global addresses, the first of them down.
    site.robots(ALLOW_ALL)
    site.page("/results", RESULTS_PAGE)
    second = ipaddress.ip_address("127.0.0.2")
    names = resolver({"pub.test": ["127.0.0.2", "127.0.0.1"]})
    page = fetcher(
        resolve=names,
        address_policy=lambda address: (
            None if address == second else local_servers_only(address)
        ),
    ).fetch(f"http://pub.test:{site.port}/results")
    assert page.http_status == 200


def test_a_name_that_never_resolves_is_refused_at_the_connect_timeout(monkeypatch):
    def hangs(*arguments, **keywords):
        time.sleep(5)
        return []

    monkeypatch.setattr(fetch.socket, "getaddrinfo", hangs)
    started = time.monotonic()
    with pytest.raises(TimeoutError, match="no answer within 0.2s"):
        fetch.resolve_host("slow.test", 443, 0.2)
    assert time.monotonic() - started < 2.0


def test_a_name_resolves_to_each_of_its_addresses_once_in_the_resolvers_order(
    monkeypatch,
):
    # getaddrinfo can name one address more than once. Each is checked once,
    # in the order the resolver gave, which is the order they are tried in.
    v6 = "2606:4700::6810:84e5"
    answers = [
        (socket.AF_INET6, socket.SOCK_STREAM, 6, "", (v6, 443, 0, 0)),
        (socket.AF_INET, socket.SOCK_STREAM, 6, "", (PUBLIC_ADDRESS, 443)),
        (socket.AF_INET6, socket.SOCK_STREAM, 6, "", (v6, 443, 0, 0)),
    ]
    monkeypatch.setattr(fetch.socket, "getaddrinfo", lambda *a, **k: answers)
    assert fetch.resolve_host("pub.test", 443, 1.0) == [v6, PUBLIC_ADDRESS]


def test_a_resolver_failure_reaches_the_caller(monkeypatch):
    # The caller turns it into a refusal; swallowed here, it would read as a
    # name with no addresses, and the reason would be lost.
    def fails(*arguments, **keywords):
        raise socket.gaierror(socket.EAI_NONAME, "Name or service not known")

    monkeypatch.setattr(fetch.socket, "getaddrinfo", fails)
    with pytest.raises(socket.gaierror, match="Name or service not known"):
        fetch.resolve_host("nowhere.test", 443, 1.0)


# --------------------------------------------------------------------------
# TLS
# --------------------------------------------------------------------------


def test_tls_verification_refuses_a_certificate_nothing_trusts(tls_site):
    tls_site.robots(ALLOW_ALL)
    url = tls_site.page("/results", RESULTS_PAGE)
    with pytest.raises(
        fetch.FetchRefused, match="CERTIFICATE_VERIFY_FAILED"
    ) as refused:
        fetcher().fetch(url)
    assert refused.value.robots[0].state == "unreachable"
    # The handshake failed, so no request ever reached the server.
    assert tls_site.requests == []


def test_the_same_server_is_read_once_its_certificate_is_trusted(tls_site, monkeypatch):
    # The control for the test above: the failure was about trust, not a broken server.
    tls_site.robots(ALLOW_ALL)
    url = tls_site.page("/results", RESULTS_PAGE)
    monkeypatch.setenv("SSL_CERT_FILE", str(tls_site.certificate.cert))
    page = fetcher().fetch(url)
    assert page.http_status == 200
    assert tls_site.paths() == ["/robots.txt", "/results"]


# --------------------------------------------------------------------------
# Timeouts and size
# --------------------------------------------------------------------------


def test_a_server_that_never_answers_is_refused_at_the_read_timeout(site):
    site.robots(ALLOW_ALL)
    url = site.page("/slow", "late", delay=3.0)
    started = time.monotonic()
    with pytest.raises(fetch.FetchRefused, match="timed out"):
        fetcher(read_timeout=0.5).fetch(url)
    assert time.monotonic() - started < 2.5


def test_a_body_dripped_past_the_deadline_is_refused(site):
    site.robots(ALLOW_ALL)
    url = site.page("/drip", "x" * 200, drip=0.02)
    started = time.monotonic()
    with pytest.raises(fetch.FetchRefused, match="took longer than 0.5s"):
        fetcher(read_timeout=2.0, total_timeout=0.5).fetch(url)
    assert time.monotonic() - started < 2.5


def test_the_timeouts_are_explicit_on_every_phase():
    client = fetcher(connect_timeout=3.0, read_timeout=7.0)._client(None)
    timeout = client.timeout
    client.close()
    assert (timeout.connect, timeout.read, timeout.write, timeout.pool) == (
        3.0,
        7.0,
        7.0,
        3.0,
    )


def test_a_page_declaring_more_than_the_ceiling_is_refused_unread(site):
    site.robots(ALLOW_ALL)
    url = site.page("/big", "x" * 2_000)
    with pytest.raises(fetch.FetchRefused, match="declares 2000 bytes"):
        fetcher(max_page_bytes=1_000).fetch(url)


def test_a_page_streaming_past_the_ceiling_is_refused_not_truncated(site):
    site.robots(ALLOW_ALL)
    url = site.page("/big", "x" * 2_000, undeclared=True)
    with pytest.raises(fetch.FetchRefused, match="refused rather than truncated"):
        fetcher(max_page_bytes=1_000).fetch(url)


def test_a_page_at_the_ceiling_is_read_whole(site):
    site.robots(ALLOW_ALL)
    url = site.page("/exact", "x" * 1_000, "text/plain", undeclared=True)
    assert len(fetcher(max_page_bytes=1_000).fetch(url).body) == 1_000


@pytest.mark.parametrize(
    "coding", ["gzip", "deflate", "zstd", "br", "compress", "identity, gzip", "GZIP"]
)
def test_a_compressed_page_is_refused_unread(site, coding):
    site.robots(ALLOW_ALL)
    url = site.page(
        "/packed", b"\x1f\x8b\x08 packed", headers={"Content-Encoding": coding}
    )
    with pytest.raises(fetch.FetchRefused, match="Content-Encoding"):
        fetcher().fetch(url)


def test_a_compressed_robots_txt_refuses_the_page(site):
    site.page(
        "/robots.txt",
        b"\x1f\x8b\x08 packed",
        "text/plain",
        headers={"Content-Encoding": "gzip"},
    )
    url = site.page("/results", RESULTS_PAGE)
    with pytest.raises(fetch.FetchRefused, match="Content-Encoding gzip"):
        fetcher(allow_absent_robots=True).fetch(url)
    assert site.paths() == ["/robots.txt"]


def test_an_identity_body_is_read_whatever_the_header_says(site):
    # The control for the two tests above: the refusal is of compression.
    site.robots(ALLOW_ALL)
    url = site.page("/plain", RESULTS_PAGE, headers={"Content-Encoding": "identity"})
    assert fetcher().fetch(url).body == RESULTS_PAGE.encode()


def test_a_compression_bomb_is_refused_before_it_is_inflated(site):
    # httpx inflates zstd whatever Accept-Encoding said, and before a ceiling
    # on the inflated bytes saw anything: 8 KiB on the wire took over 500 MiB.
    import tracemalloc

    import zstandard

    packer = zstandard.ZstdCompressor().compressobj()
    megabyte = bytes(1024 * 1024)
    bomb = b"".join(packer.compress(megabyte) for _ in range(256)) + packer.flush()
    assert len(bomb) < 64 * 1024  # tens of KiB that inflate to 256 MiB
    site.robots(ALLOW_ALL)
    url = site.page("/bomb", bomb, headers={"Content-Encoding": "zstd"})
    tracemalloc.start()
    try:
        with pytest.raises(fetch.FetchRefused, match="Content-Encoding zstd"):
            fetcher().fetch(url)
        peak = tracemalloc.get_traced_memory()[1]
    finally:
        tracemalloc.stop()
    assert peak < 16 * 1024 * 1024


@pytest.mark.parametrize(
    "limits",
    [
        {"read_timeout": 0},
        {"total_timeout": -1},
        {"max_page_bytes": fetch.CEILINGS["max_page_bytes"] + 1},
        {"connect_timeout": float("nan")},
        {"max_redirects": -1},
    ],
)
def test_a_limit_outside_its_range_is_refused_not_adjusted(limits):
    with pytest.raises(ValueError, match="refused, not adjusted"):
        fetch.FetchLimits(**limits)


# --------------------------------------------------------------------------
# Who we say we are
# --------------------------------------------------------------------------


def test_every_request_names_the_tool_honestly(results_site):
    fetcher().fetch(results_site.url("/results"))
    agents = [request["headers"]["User-Agent"] for request in results_site.requests]
    assert len(agents) == 2 and all(agent == fetch.USER_AGENT for agent in agents)
    assert fetch.USER_AGENT.startswith("qip-scraper/")
    assert "Mozilla" not in fetch.USER_AGENT and "robots.txt" in fetch.USER_AGENT


# --------------------------------------------------------------------------
# robots.txt
# --------------------------------------------------------------------------


def test_a_disallowed_page_is_refused_and_never_requested(site):
    site.robots("User-agent: *\nDisallow: /private/\n")
    url = site.page("/private/report", RESULTS_PAGE)
    with pytest.raises(fetch.FetchRefused, match="disallowed by robots.txt") as refused:
        fetcher().fetch(url)
    assert site.paths() == ["/robots.txt"]
    assert refused.value.robots[0].deciding_rule == "Disallow: /private/"


def test_the_longest_rule_wins_even_where_the_stdlib_parser_reads_file_order(site):
    # urllib.robotparser takes the first matching line, so it allows this page;
    # RFC 9309 takes the longest, which disallows it. Either refusing refuses.
    site.robots("User-agent: *\nAllow: /\nDisallow: /private/\n")
    url = site.page("/private/report", RESULTS_PAGE)
    with pytest.raises(fetch.FetchRefused, match="RFC 9309") as refused:
        fetcher().fetch(url)
    verdict = refused.value.robots[0]
    assert verdict.robotparser_allows is True
    assert verdict.rfc9309_allows is False
    assert "/private/report" not in site.paths()


def test_a_group_naming_the_tool_overrides_the_wildcard_group(site):
    site.robots("User-agent: *\nAllow: /\n\nUser-agent: qip-scraper\nDisallow: /\n")
    url = site.page("/results", RESULTS_PAGE)
    with pytest.raises(fetch.FetchRefused, match="disallowed"):
        fetcher().fetch(url)
    assert site.paths() == ["/robots.txt"]


def test_an_allowed_page_is_fetched_with_its_verdict_recorded(results_site):
    page = fetcher().fetch(results_site.url("/results"))
    (verdict,) = page.robots
    assert verdict.allowed and verdict.state == "served"
    assert verdict.robotparser_allows and verdict.rfc9309_allows


def test_an_absent_robots_txt_is_not_permission(site):
    url = site.page("/results", RESULTS_PAGE)  # /robots.txt answers 404
    with pytest.raises(fetch.FetchRefused, match="not permission"):
        fetcher().fetch(url)
    assert site.paths() == ["/robots.txt"]


def test_an_absent_robots_txt_is_fetched_past_only_when_the_operator_says_so(site):
    url = site.page("/results", RESULTS_PAGE)
    page = fetcher(allow_absent_robots=True).fetch(url)
    (verdict,) = page.robots
    assert verdict.state == "absent" and verdict.http_status == 404
    assert "--allow-absent-robots" in verdict.reason


@pytest.mark.parametrize("status", [401, 403])
def test_a_forbidden_robots_txt_disallows_everything_whatever_the_operator_says(
    site, status
):
    site.robots("nope", status=status)
    url = site.page("/results", RESULTS_PAGE)
    with pytest.raises(fetch.FetchRefused, match="disallowing everything"):
        fetcher(allow_absent_robots=True).fetch(url)


def test_an_unreachable_robots_txt_disallows_everything(site):
    site.robots("broken", status=503)
    url = site.page("/results", RESULTS_PAGE)
    with pytest.raises(fetch.FetchRefused, match="unreachable robots.txt"):
        fetcher(allow_absent_robots=True).fetch(url)


def test_a_robots_txt_that_never_answers_disallows_everything(site):
    site.robots(ALLOW_ALL, delay=3.0)
    url = site.page("/results", RESULTS_PAGE)
    with pytest.raises(fetch.FetchRefused, match="timed out"):
        fetcher(read_timeout=0.5).fetch(url)
    assert "/results" not in site.paths()


def test_a_robots_txt_that_is_not_utf8_is_unreadable_not_absent(site):
    site.page("/robots.txt", b"User-agent: *\nDisallow: /\xff\xfe\n", "text/plain")
    url = site.page("/results", RESULTS_PAGE)
    with pytest.raises(fetch.FetchRefused, match="not UTF-8"):
        fetcher(allow_absent_robots=True).fetch(url)


@pytest.mark.parametrize(
    ("robots", "refusal"),
    [
        ("Crawl-delay: ²\nAllow: /", "urllib.robotparser cannot read"),
        ("Request-rate: ²/²\nAllow: /", "urllib.robotparser cannot read"),
        (
            "Crawl-delay: " + "9" * 5_000 + "\nAllow: /",
            "urllib.robotparser cannot read",
        ),
        ("Disallow: http://[", "urllib.robotparser cannot read"),
        ("Crawl-delay: 1" + "0" * 400 + "\nAllow: /", "Crawl-delay too large"),
    ],
    ids=[
        "superscript Crawl-delay",
        "superscript Request-rate",
        "Crawl-delay of 5,000 digits",
        "rule path urlparse refuses",
        "Crawl-delay past a float",
    ],
)
def test_a_robots_txt_the_stdlib_parser_cannot_read_refuses_only_its_page(
    site, robots, refusal
):
    # Each raised out of urllib.robotparser (ValueError) or out of float()
    # (OverflowError), which ended the whole run with exit 5 and lost every
    # other URL in it.
    site.robots(f"User-agent: *\n{robots}\n")
    url = site.page("/results", RESULTS_PAGE)
    with pytest.raises(fetch.FetchRefused, match=refusal) as refused:
        fetcher().fetch(url)
    assert site.paths() == ["/robots.txt"]
    assert [verdict.allowed for verdict in refused.value.robots] == [False]


@pytest.mark.parametrize(
    "location",
    ["http://[", "//[", "http://example.com\xef\xbc\x83@evil.test/"],
    ids=["unclosed IPv6", "scheme-relative unclosed IPv6", "host NFKC folds to #"],
)
@pytest.mark.parametrize("redirected", ["the page", "its robots.txt"])
def test_a_location_that_is_not_a_url_refuses_only_its_page(site, location, redirected):
    # urljoin raised ValueError on each, which ended the whole run with exit 5.
    # The last is UTF-8 on the wire: httpx reads a fullwidth "#" in the host,
    # and urllib's NFKC check turns it into a separator.
    if redirected == "the page":
        site.robots(ALLOW_ALL)
        url = site.page("/go", "", headers={"Location": location}, status=302)
    else:
        site.page("/robots.txt", "", headers={"Location": location}, status=302)
        url = site.page("/go", RESULTS_PAGE)
    with pytest.raises(fetch.FetchRefused, match="which is not a URL"):
        fetcher().fetch(url)
    assert site.paths() == ["/robots.txt"] + (
        ["/go"] if redirected == "the page" else []
    )


@pytest.mark.parametrize(
    ("rules", "path", "allowed"),
    [
        ("Disallow: /a\nAllow: /a/b", "/a/b/c", True),
        ("Allow: /a\nDisallow: /a/b", "/a/b/c", False),
        ("Disallow: /*.pdf$", "/x/report.pdf", False),
        ("Disallow: /*.pdf$", "/x/report.pdf?v=1", True),
        ("Disallow: /search?", "/search?q=1", False),
        ("Disallow: /%7Euser", "/~user/page", False),
        ("Allow: /p\nDisallow: /p", "/p", True),
        ("Disallow:", "/anything", True),
        # * matches any run, including none; without $ a match is a prefix.
        ("Disallow: /*/private/", "/a/private/x", False),
        ("Disallow: /a*b*c", "/a-b-c-d", False),
        ("Disallow: /a*b*c", "/a-c-b", True),
        ("Disallow: /**", "/anything", False),
        # A final $ anchors the end, and the anchored piece may not overlap
        # what the pieces before it matched.
        ("Disallow: /a*b*c$", "/a-b-c-d", True),
        ("Disallow: /a*b*c$", "/a-b-c", False),
        ("Disallow: /*a$", "/a", False),
        ("Disallow: /a*a$", "/a", True),
        ("Disallow: /x$", "/x/", True),
        ("Disallow: /x$", "/x", False),
        # A $ anywhere but the end is the character itself.
        ("Disallow: /a$b", "/a$b/c", False),
    ],
)
def test_rfc9309_precedence(rules, path, allowed):
    assert (
        fetch.rfc9309_decision(f"User-agent: *\n{rules}\n", "qip-scraper", path)[0]
        is allowed
    )


def test_a_pattern_full_of_wildcards_is_matched_in_linear_time():
    # The reviewer's case: as a regular expression, seven wildcards against an
    # 80-character path took about eight seconds, and robots.txt is the server's.
    pattern, path = "/*a*a*a*a*a*a*b", "/" + "a" * 79
    assert pattern.count("*") == 7 and len(path) == 80
    started = time.monotonic()
    decision = fetch.rfc9309_decision(
        f"User-agent: *\nDisallow: {pattern}\n", "qip-scraper", path
    )
    assert time.monotonic() - started < 1.0
    assert decision == (True, None)


def test_a_robots_line_full_of_spaces_is_parsed_in_linear_time():
    # The line regex this replaced was quadratic in a run of inner whitespace:
    # 20,000 spaces took 1.8s, and robots.txt may hold 512 KiB.
    body = "User-agent: *\nDisallow: /x" + " " * 50_000 + "y\nDisallow: /private\n"
    started = time.monotonic()
    decision = fetch.rfc9309_decision(body, "qip-scraper", "/private/a")
    assert time.monotonic() - started < 1.0
    assert decision == (False, "Disallow: /private")


def test_robots_txt_judges_the_path_httpx_will_send(site):
    # httpx sends /open/../no-crawl/x as /no-crawl/x, so that is the path
    # both readings judge.
    site.robots("User-agent: *\nDisallow: /no-crawl/\n")
    verdict = fetcher().robots_verdict(site.url("/open/../no-crawl/x"))
    assert not verdict.allowed
    assert verdict.deciding_rule == "Disallow: /no-crawl/"
    assert verdict.robotparser_allows is False


@pytest.mark.parametrize("redirected", [False, True], ids=["given", "redirected to"])
def test_a_dot_segment_cannot_walk_a_url_past_a_disallow(site, redirected):
    # Judged as written, /open/../no-crawl/x passed "Disallow: /no-crawl/" and
    # httpx then asked for /no-crawl/x. urljoin keeps an absolute Location's
    # dot segments, so a redirect carried them in the same way.
    site.robots("User-agent: *\nDisallow: /no-crawl/\n")
    site.page("/no-crawl/x", RESULTS_PAGE)
    target = site.url("/open/../no-crawl/x")
    url = (
        site.page("/start", "", headers={"Location": target}, status=302)
        if redirected
        else target
    )
    with pytest.raises(fetch.FetchRefused, match="Disallow: /no-crawl/") as refused:
        fetcher().fetch(url)
    assert "/no-crawl/x" not in site.paths()
    assert site.paths() == (
        ["/robots.txt", "/start"] if redirected else ["/robots.txt"]
    )
    assert refused.value.robots[-1].url == site.url("/no-crawl/x")


@pytest.mark.parametrize("redirected", [False, True], ids=["given", "redirected to"])
def test_a_url_is_judged_and_recorded_as_it_is_requested(site, redirected):
    # The control for the test above: dot segments that resolve to an allowed
    # path are fetched, under the path that was actually asked for.
    site.robots("User-agent: *\nDisallow: /no-crawl/\n")
    site.page("/open/y", RESULTS_PAGE)
    target = site.url("/no-crawl/../open/y")
    url = (
        site.page("/start", "", headers={"Location": target}, status=302)
        if redirected
        else target
    )
    page = fetcher().fetch(url)
    assert page.final_url == site.url("/open/y")
    assert page.redirects == ((site.url("/open/y"),) if redirected else ())
    assert site.paths()[-1] == "/open/y"


@pytest.mark.parametrize(
    "path",
    [
        "/open/%2e%2e/no-crawl/x",
        "/open/.%2E/no-crawl/x",
        "/open\\..\\no-crawl/x",
        "/open/..;/no-crawl/x",
        "/open/..;jsessionid=1/no-crawl/x",
    ],
)
@pytest.mark.parametrize("redirected", [False, True], ids=["given", "redirected to"])
def test_a_dot_segment_httpx_does_not_remove_is_refused_unsent(site, path, redirected):
    # httpx leaves %2e%2e, backslashes and "..;" alone; a server that decodes
    # them first (servlet containers read "..;" as "..") would serve
    # /no-crawl/x for a path robots.txt was never asked about.
    site.robots("User-agent: *\nDisallow: /no-crawl/\n")
    target = site.url(path)
    url = (
        site.page("/start", "", headers={"Location": target}, status=302)
        if redirected
        else target
    )
    with pytest.raises(fetch.FetchRefused, match="dot segment"):
        fetcher().fetch(url)
    assert site.paths() == (["/robots.txt", "/start"] if redirected else [])


def test_a_path_parameter_on_an_ordinary_segment_is_not_refused():
    # The control for the ";" cases above: only a dot segment is refused.
    url = "http://example.test/catalog;v=2/item;id=7"
    assert fetch.canonical_url(url) == url


# --------------------------------------------------------------------------
# Redirects, status and type
# --------------------------------------------------------------------------


def test_a_redirect_is_followed_and_judged_by_its_target_robots_txt(site):
    other = Site()
    try:
        other.robots("User-agent: *\nDisallow: /\n")
        other.page("/moved", RESULTS_PAGE)
        site.robots(ALLOW_ALL)
        url = site.page(
            "/old", "", headers={"Location": other.url("/moved")}, status=302
        )
        with pytest.raises(fetch.FetchRefused, match="disallowed") as refused:
            fetcher().fetch(url)
        assert [verdict.allowed for verdict in refused.value.robots] == [True, False]
        assert other.paths() == ["/robots.txt"]
    finally:
        other.close()


def test_an_allowed_redirect_is_recorded(site):
    site.robots(ALLOW_ALL)
    site.page("/new", RESULTS_PAGE)
    url = site.page("/old", "", headers={"Location": "/new"}, status=301)
    page = fetcher().fetch(url)
    assert page.final_url == site.url("/new") and page.redirects == (site.url("/new"),)


def test_a_redirect_out_of_http_is_refused(site):
    site.robots(ALLOW_ALL)
    url = site.page("/old", "", headers={"Location": "file:///etc/passwd"}, status=302)
    with pytest.raises(fetch.FetchRefused, match="only http and https"):
        fetcher().fetch(url)


def test_a_redirect_from_https_to_http_is_refused(tls_site, site, monkeypatch):
    monkeypatch.setenv("SSL_CERT_FILE", str(tls_site.certificate.cert))
    tls_site.robots(ALLOW_ALL)
    site.robots(ALLOW_ALL)
    site.page("/plain", RESULTS_PAGE)
    url = tls_site.page(
        "/secure", "", headers={"Location": site.url("/plain")}, status=302
    )
    with pytest.raises(fetch.FetchRefused, match="would drop TLS"):
        fetcher().fetch(url)
    assert site.requests == []


def test_a_redirect_loop_is_refused(site):
    site.robots(ALLOW_ALL)
    url = site.page("/loop", "", headers={"Location": "/loop"}, status=302)
    with pytest.raises(fetch.FetchRefused, match="more than 2 redirects"):
        fetcher(max_redirects=2).fetch(url)


def test_an_error_page_is_not_content(site):
    site.robots(ALLOW_ALL)
    url = site.page("/gone", "<p>Revenue was 1</p>", status=404)
    with pytest.raises(
        fetch.FetchRefused, match="error page is not content"
    ) as refused:
        fetcher().fetch(url)
    assert refused.value.status == 404


@pytest.mark.parametrize(
    ("content_type", "match"), [("application/pdf", "only"), ("", "no Content-Type")]
)
def test_only_html_and_text_are_read(site, content_type, match):
    site.robots(ALLOW_ALL)
    url = site.page("/file", b"%PDF-1.7", content_type)
    with pytest.raises(fetch.FetchRefused, match=match):
        fetcher().fetch(url)


# --------------------------------------------------------------------------
# Proxies: the environment's, and nothing else
# --------------------------------------------------------------------------


def test_http_proxy_from_the_environment_carries_the_fetch(monkeypatch):
    proxy = Site()
    names = resolver({"example.test": [PUBLIC_ADDRESS]})
    try:
        # A forward proxy sees the absolute URL as the request target.
        proxy.page("http://example.test/robots.txt", ALLOW_ALL, "text/plain")
        proxy.page("http://example.test/results", RESULTS_PAGE)
        monkeypatch.setenv("HTTP_PROXY", f"http://127.0.0.1:{proxy.port}")
        page = fetcher(resolve=names).fetch("http://example.test/results")
    finally:
        proxy.close()
    assert page.proxy == f"http://127.0.0.1:{proxy.port}"
    assert proxy.paths() == [
        "http://example.test/robots.txt",
        "http://example.test/results",
    ]


def test_https_proxy_from_the_environment_is_asked_for_the_tunnel(monkeypatch):
    proxy = Site()
    names = resolver({"example.test": [PUBLIC_ADDRESS]})
    try:
        monkeypatch.setenv("HTTPS_PROXY", f"http://127.0.0.1:{proxy.port}")
        with pytest.raises(fetch.FetchRefused):
            fetcher(resolve=names).fetch("https://example.test/results")
    finally:
        proxy.close()
    # The proxy refused the tunnel (403), but it was asked: HTTPS_PROXY was honoured.
    assert [(request["method"], request["path"]) for request in proxy.requests] == [
        ("CONNECT", "example.test:443")
    ]


@pytest.mark.parametrize(
    ("table", "refusal"),
    [
        ({"intranet.test": ["10.20.30.40"]}, "10.20.30.40, a private address"),
        ({}, "could not be resolved"),
        ({"intranet.test": []}, "resolved to no address"),
    ],
    ids=["resolves inward", "does not resolve here", "resolves to nothing"],
)
def test_through_a_proxy_the_name_is_checked_before_the_proxy_is_asked(
    monkeypatch, table, refusal
):
    # The proxy connects by name, so the check runs on this machine's answer
    # for the name, and no answer at all is a refusal, not a pass.
    proxy = Site()
    names = resolver(table)
    try:
        proxy.page("http://intranet.test/robots.txt", ALLOW_ALL, "text/plain")
        proxy.page("http://intranet.test/payroll", RESULTS_PAGE)
        monkeypatch.setenv("HTTP_PROXY", f"http://127.0.0.1:{proxy.port}")
        with pytest.raises(fetch.FetchRefused, match=refusal):
            fetcher(resolve=names).fetch("http://intranet.test/payroll")
    finally:
        proxy.close()
    assert names.asked == ["intranet.test"]
    assert proxy.requests == []


def test_through_a_proxy_robots_txt_redirected_inward_is_not_followed(monkeypatch):
    # Pages are checked hop by hop before anything is asked for them; a
    # robots.txt redirect is followed inside the robots.txt read, so the
    # check on every request is what stops this one.
    proxy = Site()
    names = resolver(
        {"example.test": [PUBLIC_ADDRESS], "intranet.test": ["10.20.30.40"]}
    )
    try:
        proxy.page(
            "http://example.test/robots.txt",
            "",
            headers={"Location": "http://intranet.test/robots.txt"},
            status=302,
        )
        proxy.page("http://intranet.test/robots.txt", ALLOW_ALL, "text/plain")
        proxy.page("http://example.test/results", RESULTS_PAGE)
        monkeypatch.setenv("HTTP_PROXY", f"http://127.0.0.1:{proxy.port}")
        with pytest.raises(fetch.FetchRefused, match="10.20.30.40, a private address"):
            fetcher(resolve=names).fetch("http://example.test/results")
    finally:
        proxy.close()
    assert proxy.paths() == ["http://example.test/robots.txt"]


def test_no_proxy_bypasses_the_environment_proxy():
    environment = {
        "HTTPS_PROXY": "http://proxy.internal:3128",
        "NO_PROXY": "example.com",
    }
    assert fetch.select_proxy("https://example.com/a", environment) is None
    assert (
        fetch.select_proxy("https://example.org/a", environment)
        == "http://proxy.internal:3128"
    )
    assert fetch.select_proxy("https://example.org/a", {}) is None


def test_a_recorded_proxy_never_carries_its_credentials():
    assert (
        fetch.describe_proxy("http://user:secret@proxy.internal:3128")
        == "http://proxy.internal:3128"
    )


def test_the_free_proxy_broker_cannot_be_reached():
    from scrapegraphai.utils.proxy_rotation import parse_or_search_proxy

    with pytest.raises(guard.ForbiddenComponent):
        parse_or_search_proxy({"server": "broker"})


# --------------------------------------------------------------------------
# Crawl-delay
# --------------------------------------------------------------------------


def test_crawl_delay_is_waited_out_between_pages_of_one_origin(site):
    site.robots("User-agent: *\nAllow: /\nCrawl-delay: 2\n")
    first, second = site.page("/a", RESULTS_PAGE), site.page("/b", RESULTS_PAGE)
    slept, now = [], [100.0]
    paced = fetcher(sleep=slept.append, monotonic=lambda: now[0])
    paced.fetch(first)
    now[0] += 0.5
    paced.fetch(second)
    assert slept == [pytest.approx(1.5)]


def test_a_crawl_delay_longer_than_the_tool_will_wait_refuses_the_page(site):
    site.robots("User-agent: *\nAllow: /\nCrawl-delay: 3600\n")
    url = site.page("/a", RESULTS_PAGE)
    with pytest.raises(fetch.FetchRefused, match="Crawl-delay of 3600"):
        fetcher().fetch(url)
    assert site.paths() == ["/robots.txt"]
