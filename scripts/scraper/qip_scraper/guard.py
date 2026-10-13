"""Make the process safe for ScrapeGraphAI before it is imported, and keep it so.

Three things in scrapegraphai 2.3.1 decide this module's shape, each read
from the wheel (sha256 c03605ce...62f9) rather than from its README:

* Telemetry is on by default. After a run it POSTs the prompt, the schema,
  the page text, the model's answer, the model name and the URL to
  ``https://sgai-oss-tracing.onrender.com/v1/telemetry`` from a daemon thread.
  ``SCRAPEGRAPHAI_TELEMETRY_ENABLED`` is read once, at import, and an
  unrecognised value leaves telemetry on with a warning. The same import
  writes an install identifier to ``~/.scrapegraphai.conf``.
* ``nodes/generate_code_node.py`` runs model-written Python with ``exec()``
  and the full builtins (the CVE-2026-5532 class). Other modules evaluate
  expressions (``simpleeval``), import arbitrary files (``exec_module``) or
  start subprocesses. Importing ``scrapegraphai.graphs`` imports all of them.
* Its default fetch path drives Chromium with certificate errors ignored and
  an evasion layer on, can harvest free public proxies, and can search.

So, in order: the home directory is replaced by a private temporary one, the
telemetry switch is set, LangChain tracing (a second way out for the same
data) is refused or switched off, an import guard is installed that replaces
every library module SmartScraperGraph does not need with an inert stub, and
only then is the library imported. Its telemetry flag is read back and the
run is refused unless it is off. ``network_fence`` is the last line: while
the library runs, no connection may be opened except to the model.
"""

from __future__ import annotations

import atexit
import contextlib
import dataclasses
import http.client
import importlib
import importlib.abc
import importlib.machinery
import os
import shutil
import socket
import sys
import tempfile
from collections.abc import Iterator, Mapping

from . import TOOL_NAME


class GuardRefusal(Exception):
    """The process is not in a state in which the library may run."""


class ForbiddenComponent(Exception):
    """Something tried to use a part of the library this tool refuses."""


class NetworkRefused(ConnectionRefusedError):
    """A connection the network fence did not admit.

    A ``ConnectionRefusedError`` so that HTTP clients treat it as the
    connection failure it is. The fence also records every refusal, because a
    library that swallows the error (the telemetry sender does) would
    otherwise leave no trace of the attempt.
    """


# --------------------------------------------------------------------------
# The import guard
# --------------------------------------------------------------------------

LIBRARY = "scrapegraphai"

#: The library modules SmartScraperGraph needs to read supplied content with a
#: caller-built model. Every other ``scrapegraphai.*`` module is replaced by a
#: stub whose every attribute is a class that raises when called.
#:
#: ``parse_node`` is here only because SmartScraperGraph constructs a
#: ParseNode unconditionally; ``html_mode`` keeps it off the graph, and the
#: chunker and tokenizer it would call are stubs. ``extract.py`` checks the
#: graph's shape before every run.
ALLOWED_LIBRARY_MODULES = frozenset(
    {
        "scrapegraphai",
        "scrapegraphai.graphs",
        "scrapegraphai.graphs.abstract_graph",
        "scrapegraphai.graphs.base_graph",
        "scrapegraphai.graphs.smart_scraper_graph",
        "scrapegraphai.nodes",
        "scrapegraphai.nodes.base_node",
        "scrapegraphai.nodes.fetch_node",
        "scrapegraphai.nodes.parse_node",
        "scrapegraphai.nodes.generate_answer_node",
        "scrapegraphai.prompts",
        "scrapegraphai.prompts.generate_answer_node_prompts",
        "scrapegraphai.telemetry",
        "scrapegraphai.telemetry.telemetry",
        "scrapegraphai.utils",
        "scrapegraphai.utils.logging",
        "scrapegraphai.utils.llm_callback_manager",
        "scrapegraphai.utils.custom_callback",
        "scrapegraphai.utils.model_costs",
        "scrapegraphai.utils.output_parser",
    }
)

_RUNS_CODE = (
    "it executes or evaluates code (model-written Python through exec(), "
    "expressions through simpleeval, files through exec_module, or "
    "subprocesses); only SmartScraperGraph reading supplied content is permitted"
)
_FETCHES = (
    "it opens a network path of its own (a browser with certificate errors "
    "ignored, a vendor fetch service, a search engine or a free-proxy broker); "
    "this tool fetches every page itself"
)

#: Modules refused for a named reason. Everything else outside the allow-list
#: is refused with ``_NOT_ALLOWED``; these are spelled out so that the refusal
#: a person reads names what the module would have done.
REFUSAL_REASONS = {
    "scrapegraphai.nodes.generate_code_node": _RUNS_CODE + " (CVE-2026-5532 class)",
    "scrapegraphai.graphs.code_generator_graph": _RUNS_CODE + " (CVE-2026-5532 class)",
    "scrapegraphai.graphs.script_creator_graph": _RUNS_CODE,
    "scrapegraphai.graphs.script_creator_multi_graph": _RUNS_CODE,
    "scrapegraphai.nodes.generate_scraper_node": _RUNS_CODE,
    "scrapegraphai.nodes.merge_generated_scripts_node": _RUNS_CODE,
    "scrapegraphai.nodes.conditional_node": _RUNS_CODE,
    "scrapegraphai.utils.code_error_analysis": _RUNS_CODE,
    "scrapegraphai.utils.code_error_correction": _RUNS_CODE,
    "scrapegraphai.utils.sys_dynamic_import": _RUNS_CODE,
    "scrapegraphai.docloaders": _FETCHES,
    "scrapegraphai.docloaders.chromium": _FETCHES,
    "scrapegraphai.docloaders.browser_base": _FETCHES,
    "scrapegraphai.docloaders.scrape_do": _FETCHES,
    "scrapegraphai.docloaders.plasmate": _FETCHES,
    "scrapegraphai.utils.proxy_rotation": _FETCHES,
    "scrapegraphai.utils.research_web": _FETCHES,
    "scrapegraphai.integrations.scrapegraph_py_compat": _FETCHES,
}
_NOT_ALLOWED = (
    "it is not on qip-scraper's allow-list; only SmartScraperGraph reading "
    "supplied content is permitted"
)

#: Third-party top-level modules that only the refused parts of the library
#: import: the browser driver and its evasion fork, the free-proxy harvester,
#: the search client, the browser-impersonating HTTP client it pulls in, and
#: the vendor's own cloud SDK. Refused so that nothing reaches them by another
#: route either.
REFUSED_THIRD_PARTY = {
    "playwright": _FETCHES,
    "undetected_playwright": _FETCHES,
    "fp": _FETCHES,
    "ddgs": _FETCHES,
    "primp": _FETCHES,
    "scrapegraph_py": _FETCHES,
    "browserbase": _FETCHES,
    "burr": _NOT_ALLOWED,
}


def refusal_reason(fullname: str) -> str | None:
    """Why ``fullname`` is refused, or None if it may be imported for real."""
    top = fullname.partition(".")[0]
    if top in REFUSED_THIRD_PARTY:
        return REFUSED_THIRD_PARTY[top]
    if top != LIBRARY or fullname in ALLOWED_LIBRARY_MODULES:
        return None
    return REFUSAL_REASONS.get(fullname, _NOT_ALLOWED)


def _refused_component(module_name: str, attribute: str, reason: str) -> type:
    """A class standing in for a refused function or class: calling it raises.

    A class rather than a function so that ``isinstance`` checks against it
    (the library's callback manager makes several) still work and answer
    False, and so that subclassing it at import time does not fail.
    """
    qualified = f"{module_name}.{attribute}"
    message = f"{qualified} is refused by {TOOL_NAME}: {reason}"

    def __new__(cls, *args, **kwargs):
        raise ForbiddenComponent(message)

    return type(
        attribute,
        (),
        {
            "__new__": __new__,
            "__module__": module_name,
            "__qualname__": attribute,
            "__doc__": message,
            "__refused__": reason,
        },
    )


class _RefusingLoader(importlib.abc.Loader):
    """Builds the stub that stands in for a refused module.

    The real module's code is never executed: the stub is created from
    nothing, so an ``exec()`` written in the refused file cannot run however
    the stub is used.
    """

    def __init__(self, reason: str) -> None:
        self.reason = reason

    def create_module(self, spec):
        return None

    def exec_module(self, module) -> None:
        reason = self.reason
        name = module.__name__

        def __getattr__(attribute: str):
            if attribute.startswith("__") and attribute.endswith("__"):
                raise AttributeError(attribute)
            refused = _refused_component(name, attribute, reason)
            setattr(module, attribute, refused)
            return refused

        module.__dict__.update(
            {"__refused__": reason, "__all__": [], "__getattr__": __getattr__}
        )


class ImportGuard(importlib.abc.MetaPathFinder):
    """A meta-path finder that answers for every refused module first."""

    def find_spec(self, fullname, path=None, target=None):
        reason = refusal_reason(fullname)
        if reason is None:
            return None
        parent_name = fullname.rpartition(".")[0]
        parent = sys.modules.get(parent_name) if parent_name else None
        if parent is not None and not hasattr(parent, "__refused__"):
            # The parent is real, so the real module can be looked up without
            # being executed. A name that does not exist is left to fail the
            # ordinary way rather than being invented here.
            real = importlib.machinery.PathFinder.find_spec(fullname, path)
            if real is None:
                return None
            is_package = real.submodule_search_locations is not None
        elif parent is None and not parent_name:
            real = importlib.machinery.PathFinder.find_spec(fullname)
            if real is None:
                return None
            is_package = real.submodule_search_locations is not None
        else:
            # Below a refused package: refused too, and a package so that its
            # own children resolve to stubs instead of failing the import.
            is_package = True
        return importlib.machinery.ModuleSpec(
            fullname, _RefusingLoader(reason), is_package=is_package
        )


_IMPORT_GUARD = ImportGuard()


def install_import_guard() -> ImportGuard:
    """Put the guard first on ``sys.meta_path``; idempotent."""
    if _IMPORT_GUARD not in sys.meta_path:
        sys.meta_path.insert(0, _IMPORT_GUARD)
    return _IMPORT_GUARD


def import_guard_installed() -> bool:
    return bool(sys.meta_path) and sys.meta_path[0] is _IMPORT_GUARD


# --------------------------------------------------------------------------
# Environment: private home, telemetry switch, tracing
# --------------------------------------------------------------------------

TELEMETRY_SWITCH = "SCRAPEGRAPHAI_TELEMETRY_ENABLED"

#: The variables LangSmith reads to decide whether to trace (langsmith 0.14,
#: ``utils.tracing_is_enabled``). A trace carries the prompt, the page text
#: and the answer to LangSmith's servers, which is the telemetry problem by
#: another route.
TRACING_SWITCHES = (
    "LANGSMITH_TRACING_V2",
    "LANGCHAIN_TRACING_V2",
    "LANGSMITH_TRACING",
    "LANGCHAIN_TRACING",
)
#: Removed from the process so that no LangChain component can authenticate
#: to a tracing endpoint even if something switched tracing back on.
TRACING_CREDENTIALS = (
    "LANGSMITH_API_KEY",
    "LANGCHAIN_API_KEY",
    "LANGSMITH_ENDPOINT",
    "LANGCHAIN_ENDPOINT",
)
#: Opt-outs other libraries in the tree honour. Belt and braces: the network
#: fence refuses their hosts anyway.
QUIET_ENVIRONMENT = {
    "HF_HUB_OFFLINE": "1",
    "HF_HUB_DISABLE_TELEMETRY": "1",
    "DO_NOT_TRACK": "1",
}
_FALSE_SPELLINGS = frozenset({"", "false", "0", "no", "off"})


def tracing_refusal(environ: Mapping[str, str]) -> str | None:
    """Why the environment must not run, if it asks for LangChain tracing.

    A refusal rather than a quiet override: the operator asked for tracing,
    and the right answer to a configuration that would relax a protection is
    to stop and say so, not to lower it and carry on.
    """
    enabled = [
        name
        for name in TRACING_SWITCHES
        if environ.get(name, "").strip().lower() not in _FALSE_SPELLINGS
    ]
    if not enabled:
        return None
    return (
        f"{', '.join(enabled)} turn on LangChain tracing, which would send the "
        "prompt, the page text and the model's answer to LangSmith; unset "
        f"{'it' if len(enabled) == 1 else 'them'} to run {TOOL_NAME}"
    )


def _private_home() -> str:
    """Point HOME and the XDG directories at a temporary directory.

    The library writes ``~/.scrapegraphai.conf`` at import, and other
    libraries in the tree write caches under the home or XDG directories. A
    private directory, removed when the process exits, keeps all of it out
    of the operator's home. It stays in place for the whole process, because
    a library that reads HOME later would otherwise write there after all.
    """
    home = tempfile.mkdtemp(prefix=f"{TOOL_NAME}-home-")
    atexit.register(shutil.rmtree, home, True)
    os.environ["HOME"] = home
    for variable, relative in (
        ("XDG_CONFIG_HOME", ".config"),
        ("XDG_CACHE_HOME", ".cache"),
        ("XDG_DATA_HOME", os.path.join(".local", "share")),
        ("XDG_STATE_HOME", os.path.join(".local", "state")),
    ):
        path = os.path.join(home, relative)
        os.makedirs(path, exist_ok=True)
        os.environ[variable] = path
    return home


def _refuse_to_send(*args, **kwargs):
    raise GuardRefusal(
        "ScrapeGraphAI's telemetry tried to send a payload after it was "
        "switched off; the run is refused"
    )


def verify_telemetry_off(telemetry, private_home: str) -> None:
    """Refuse unless the library's telemetry is off and its file is private.

    ``telemetry`` is the ``scrapegraphai.telemetry.telemetry`` module.
    Upstream's own answer to a mistyped switch is a warning; this one is a
    refusal.
    """
    if getattr(telemetry, "g_telemetry_enabled", True) is not False:
        raise GuardRefusal(
            "ScrapeGraphAI's telemetry is still on after disable_telemetry() "
            "(scrapegraphai.telemetry.telemetry.g_telemetry_enabled is not "
            "False); refusing to run, because a run would post the page text "
            "and the answer to the vendor"
        )
    if telemetry.is_telemetry_enabled() is not False:
        raise GuardRefusal(
            "ScrapeGraphAI reports telemetry enabled (is_telemetry_enabled() "
            "is not False); refusing to run"
        )
    if (
        getattr(telemetry, "_send_telemetry", None) is not _refuse_to_send
        or getattr(telemetry, "_send_telemetry_threaded", None) is not _refuse_to_send
    ):
        raise GuardRefusal(
            "ScrapeGraphAI's telemetry sender is not severed; refusing to run"
        )
    location = os.path.realpath(getattr(telemetry, "DEFAULT_CONFIG_LOCATION", ""))
    if os.path.commonpath(
        [location, os.path.realpath(private_home)]
    ) != os.path.realpath(private_home):
        raise GuardRefusal(
            f"ScrapeGraphAI's config file is {location}, outside this run's "
            "private home; refusing to run"
        )


@dataclasses.dataclass(frozen=True)
class GuardReport:
    """What ``prepare`` found and did, recorded in every proposal."""

    private_home: str
    library_version: str
    telemetry_switch_value: str
    telemetry_on_at_import: bool
    telemetry_on_after_disable: bool
    library_config_file: str
    langsmith_tracing: bool


_REPORT: GuardReport | None = None


def prepared() -> bool:
    """Whether ``prepare`` has completed in this process."""
    return _REPORT is not None and import_guard_installed()


def prepare() -> GuardReport:
    """Make the process safe for the library, import it, and prove it.

    Idempotent: a second call re-verifies the telemetry state and returns the
    first report. Raises ``GuardRefusal`` for anything that is not as it must
    be; the caller turns that into a non-zero exit.
    """
    global _REPORT
    if _REPORT is not None:
        verify_telemetry_off(
            sys.modules["scrapegraphai.telemetry.telemetry"], _REPORT.private_home
        )
        if not import_guard_installed():
            raise GuardRefusal("the import guard is no longer first on sys.meta_path")
        return _REPORT

    imported = sorted(
        name
        for name in sys.modules
        if name == LIBRARY or name.startswith(LIBRARY + ".")
    )
    if imported:
        raise GuardRefusal(
            f"{LIBRARY} was imported before the guard ran ({', '.join(imported[:3])}"
            f"{', ...' if len(imported) > 3 else ''}); its telemetry switch was read "
            "and its config file written at that import, so this process cannot "
            "vouch for either"
        )

    refusal = tracing_refusal(os.environ)
    if refusal is not None:
        raise GuardRefusal(refusal)
    for name in TRACING_SWITCHES:
        os.environ[name] = "false"
    for name in TRACING_CREDENTIALS:
        os.environ.pop(name, None)
    os.environ.update(QUIET_ENVIRONMENT)

    home = _private_home()
    os.environ[TELEMETRY_SWITCH] = "false"
    install_import_guard()

    telemetry = importlib.import_module("scrapegraphai.telemetry.telemetry")
    on_at_import = telemetry.g_telemetry_enabled is not False
    if on_at_import:
        raise GuardRefusal(
            f"ScrapeGraphAI ignored {TELEMETRY_SWITCH}=false at import; its "
            "telemetry switch has changed, and this tool will not run a version "
            "whose switch it does not understand"
        )
    telemetry.disable_telemetry()
    telemetry._send_telemetry = _refuse_to_send
    telemetry._send_telemetry_threaded = _refuse_to_send
    verify_telemetry_off(telemetry, home)

    import langsmith.utils

    # get_env_var is lru_cached, so a value read before the switches were set
    # would otherwise outlive them.
    langsmith.utils.get_env_var.cache_clear()
    tracing = bool(langsmith.utils.tracing_is_enabled())
    if tracing:
        raise GuardRefusal(
            "LangSmith reports tracing enabled after every switch was set to "
            "false; refusing to run"
        )

    # The library sets its own logger to INFO at import, and at INFO it logs
    # the whole page it is given. Import the one graph now, through the guard,
    # so that a library that cannot be imported safely fails here, before
    # anything has been fetched.
    import logging

    logging.getLogger(LIBRARY).setLevel(logging.WARNING)
    importlib.import_module("scrapegraphai.graphs.smart_scraper_graph")

    _REPORT = GuardReport(
        private_home=home,
        library_version=getattr(telemetry, "VERSION", "unknown"),
        telemetry_switch_value=os.environ[TELEMETRY_SWITCH],
        telemetry_on_at_import=on_at_import,
        telemetry_on_after_disable=telemetry.g_telemetry_enabled is not False,
        library_config_file=telemetry.DEFAULT_CONFIG_LOCATION,
        langsmith_tracing=tracing,
    )
    return _REPORT


# --------------------------------------------------------------------------
# The network fence
# --------------------------------------------------------------------------


def _host_of(value) -> str:
    if isinstance(value, bytes):
        value = value.decode("idna", "replace")
    return str(value).strip("[]").lower().rstrip(".")


@contextlib.contextmanager
def network_fence(allowed_hosts) -> Iterator[list[str]]:
    """While open, refuse every connection except to ``allowed_hosts``.

    Yields the list of refused attempts, which the caller must check: an
    empty list is the evidence that nothing but the model was reached. Name
    resolution is refused for any other host, a socket connect to an address
    that did not come from resolving an allowed host is refused, and so is an
    ``http.client`` tunnel (urllib's and requests' CONNECT through a proxy)
    to any other host.

    What it does not see: a client that tunnels through an allowed proxy
    without ``http.client`` (httpx does its own CONNECT). The model client is
    such a client, which is why the proxy host is admitted only when the
    model is reached through one; README.md says so.
    """
    allowed = frozenset(_host_of(host) for host in allowed_hosts)
    resolved: set[str] = set()
    attempts: list[str] = []

    real_getaddrinfo = socket.getaddrinfo
    real_connect = socket.socket.connect
    real_connect_ex = socket.socket.connect_ex
    real_set_tunnel = http.client.HTTPConnection.set_tunnel

    def refuse(what: str) -> NetworkRefused:
        attempts.append(what)
        return NetworkRefused(f"{TOOL_NAME} network fence refused {what}")

    def getaddrinfo(host, port, *args, **kwargs):
        name = _host_of(host) if host is not None else ""
        if name not in allowed:
            raise refuse(f"resolving {name or '(none)'}:{port}")
        results = real_getaddrinfo(host, port, *args, **kwargs)
        for result in results:
            resolved.add(_host_of(result[4][0]))
        return results

    def admitted(sock, address) -> bool:
        if sock.family not in (socket.AF_INET, socket.AF_INET6):
            return True
        host = _host_of(address[0])
        return host in resolved or host in allowed

    def connect(self, address):
        if not admitted(self, address):
            raise refuse(f"connecting to {address[0]}:{address[1]}")
        return real_connect(self, address)

    def connect_ex(self, address):
        if not admitted(self, address):
            raise refuse(f"connecting to {address[0]}:{address[1]}")
        return real_connect_ex(self, address)

    def set_tunnel(self, host, *args, **kwargs):
        if _host_of(host) not in allowed:
            raise refuse(f"tunnelling to {host}")
        return real_set_tunnel(self, host, *args, **kwargs)

    socket.getaddrinfo = getaddrinfo
    socket.socket.connect = connect
    socket.socket.connect_ex = connect_ex
    http.client.HTTPConnection.set_tunnel = set_tunnel
    try:
        yield attempts
    finally:
        socket.getaddrinfo = real_getaddrinfo
        socket.socket.connect = real_connect
        socket.socket.connect_ex = real_connect_ex
        http.client.HTTPConnection.set_tunnel = real_set_tunnel
