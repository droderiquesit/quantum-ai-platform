"""Protections 1 and 3, and the network fence: telemetry, tracing, imports.

Import-time behaviour cannot be observed in this process, because the
library was imported once, by conftest, through the guard. Those tests start
a fresh interpreter with a clean environment and a throwaway HOME, which is
the only honest way to watch an import happen.
"""

from __future__ import annotations

import importlib
import json
import os
import re
import socket
import subprocess
import sys
import urllib.request
from pathlib import Path
from types import SimpleNamespace

import pytest
from conftest import FIXTURE_MODEL, GOOD_ANSWER, GUARD_REPORT, SCRAPER, Site, fake_model
from langchain_core.language_models.fake_chat_models import GenericFakeChatModel

from qip_scraper import cli, guard

TELEMETRY = sys.modules["scrapegraphai.telemetry.telemetry"]


def fresh_interpreter(code: str, home: Path, **extra_environment: str):
    """Run ``code`` in a new Python with HOME=home and nothing the guard sets."""
    environment = {
        name: value
        for name, value in os.environ.items()
        if not name.startswith(
            ("SCRAPEGRAPHAI_", "LANGSMITH_", "LANGCHAIN_", "XDG_", "HF_HUB_")
        )
        and name not in ("DO_NOT_TRACK", "HOME")
    }
    environment.update(HOME=str(home), PYTHONPATH=str(SCRAPER), **extra_environment)
    return subprocess.run(
        [sys.executable, "-c", code],
        env=environment,
        cwd=SCRAPER,
        capture_output=True,
        text=True,
        timeout=180,
        check=False,
    )


# --------------------------------------------------------------------------
# Protection 1: telemetry
# --------------------------------------------------------------------------


def test_this_process_imported_the_library_with_telemetry_already_off():
    assert GUARD_REPORT.telemetry_on_at_import is False
    assert TELEMETRY.g_telemetry_enabled is False
    assert TELEMETRY.is_telemetry_enabled() is False
    assert os.environ[guard.TELEMETRY_SWITCH] == "false"


def test_a_fresh_import_reads_the_switch_off_and_writes_nothing_into_home(tmp_path):
    home = tmp_path / "operator-home"
    home.mkdir()
    probe = (
        "import json, os\n"
        "from qip_scraper import guard\n"
        "report = guard.prepare()\n"
        "import scrapegraphai.telemetry.telemetry as t\n"
        "print(json.dumps({'at_import': report.telemetry_on_at_import,\n"
        "                  'flag': t.g_telemetry_enabled,\n"
        "                  'config': t.DEFAULT_CONFIG_LOCATION,\n"
        "                  'config_existed': os.path.exists(t.DEFAULT_CONFIG_LOCATION),\n"
        "                  'private_home': report.private_home}))\n"
    )
    result = fresh_interpreter(probe, home)
    assert result.returncode == 0, result.stderr
    seen = json.loads(result.stdout.strip().splitlines()[-1])

    # The switch was in place before the import: the import itself read it off.
    assert seen["at_import"] is False
    assert seen["flag"] is False
    # The library wrote its identifier, but into the run's private home...
    assert seen["config_existed"] is True
    assert seen["config"].startswith(seen["private_home"] + os.sep)
    # ...which is gone once the process has exited, and the real home is untouched.
    assert not os.path.exists(seen["private_home"])
    assert not (home / ".scrapegraphai.conf").exists()
    assert list(home.iterdir()) == []


def test_verify_refuses_any_telemetry_state_but_off():
    home = GUARD_REPORT.private_home
    severed = guard._refuse_to_send
    good = SimpleNamespace(
        g_telemetry_enabled=False,
        is_telemetry_enabled=lambda: False,
        _send_telemetry=severed,
        _send_telemetry_threaded=severed,
        DEFAULT_CONFIG_LOCATION=os.path.join(home, ".scrapegraphai.conf"),
    )
    guard.verify_telemetry_off(good, home)  # the premise: a good state passes

    for change, expected in (
        ({"g_telemetry_enabled": True}, "still on"),
        ({"is_telemetry_enabled": lambda: True}, "reports telemetry enabled"),
        ({"_send_telemetry_threaded": lambda payload: None}, "not severed"),
        (
            {"DEFAULT_CONFIG_LOCATION": "/root/.scrapegraphai.conf"},
            "outside this run's private home",
        ),
    ):
        state = SimpleNamespace(**{**vars(good), **change})
        with pytest.raises(guard.GuardRefusal, match=expected):
            guard.verify_telemetry_off(state, home)


def test_the_command_refuses_to_run_while_telemetry_is_on(
    monkeypatch, results_site, schema_file, run
):
    # As if disable_telemetry() had silently stopped working in an upgrade.
    monkeypatch.setattr(TELEMETRY, "g_telemetry_enabled", True)
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=fake_model(GOOD_ANSWER),
    )

    assert result.code == cli.EXIT_GUARD
    assert "telemetry is still on" in result.err
    assert result.proposal is None
    assert results_site.requests == []


def test_the_telemetry_sender_is_severed_even_if_the_flag_came_back_on(monkeypatch):
    monkeypatch.setattr(TELEMETRY, "g_telemetry_enabled", True)
    # A destination that cannot resolve, so that even a broken guard sends nothing anywhere.
    monkeypatch.setattr(
        TELEMETRY, "TRACK_URL", "https://telemetry.invalid/v1/telemetry"
    )
    with (
        guard.network_fence(set()) as attempts,
        pytest.raises(guard.GuardRefusal, match="tried to send"),
    ):
        TELEMETRY.log_graph_execution(
            graph_name="SmartScraperGraph",
            source=["https://example.com/"],
            prompt="p",
            schema={"type": "object"},
            llm_model=FIXTURE_MODEL,
            embedder_model=None,
            source_type="url",
            execution_time=0.1,
            content="page text",
            response={"a": 1},
        )
    assert attempts == []


# --------------------------------------------------------------------------
# LangChain tracing, the second way out
# --------------------------------------------------------------------------


@pytest.mark.parametrize("switch", guard.TRACING_SWITCHES)
def test_any_tracing_switch_turned_on_is_refused(switch):
    assert guard.tracing_refusal({switch: "false"}) is None
    assert guard.tracing_refusal({switch: ""}) is None
    refusal = guard.tracing_refusal({switch: "true"})
    assert refusal is not None and switch in refusal and "LangSmith" in refusal


def test_the_command_exits_before_importing_anything_when_tracing_is_on(
    tmp_path, schema_file
):
    home = tmp_path / "home"
    home.mkdir()
    output = tmp_path / "proposal.json"
    code = (
        "import sys\n"
        "from qip_scraper import cli\n"
        f"sys.exit(cli.main(['https://example.invalid/', '--prompt', 'x', '--schema', "
        f"{str(schema_file)!r}, '--output', {str(output)!r}, '--model', "
        f"{FIXTURE_MODEL!r}]))\n"
    )
    result = fresh_interpreter(code, home, LANGSMITH_TRACING="true")
    assert result.returncode == cli.EXIT_GUARD, result.stderr
    assert "LANGSMITH_TRACING" in result.stderr
    assert not output.exists()


def test_tracing_is_off_in_this_process():
    import langsmith.utils

    assert langsmith.utils.tracing_is_enabled() is False
    for name in guard.TRACING_CREDENTIALS:
        assert name not in os.environ


def test_a_library_imported_before_the_guard_is_refused(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    code = (
        "import scrapegraphai\n"
        "from qip_scraper import guard\n"
        "try:\n"
        "    guard.prepare()\n"
        "except guard.GuardRefusal as refusal:\n"
        "    print('REFUSED', refusal)\n"
    )
    result = fresh_interpreter(code, home)
    assert result.returncode == 0, result.stderr
    assert "REFUSED scrapegraphai was imported before the guard ran" in result.stdout


# --------------------------------------------------------------------------
# Protection 3: only SmartScraperGraph; nothing that reaches exec()
# --------------------------------------------------------------------------

_EXECUTES = re.compile(
    r"\bexec\s*\(|\beval\s*\(|\bexec_module\b|\bsubprocess\b|\bsimple_eval\b|\bos\.system\b|__import__\s*\("
)
LIBRARY_ROOT = Path(
    importlib.util.find_spec("scrapegraphai").submodule_search_locations[0]
)


def library_modules() -> list[str]:
    names = []
    for path in sorted(LIBRARY_ROOT.rglob("*.py")):
        parts = ["scrapegraphai", *path.relative_to(LIBRARY_ROOT).with_suffix("").parts]
        if parts[-1] == "__init__":
            parts.pop()
        names.append(".".join(parts))
    return names


def test_the_code_generation_node_is_a_stub_and_its_file_never_runs():
    module = importlib.import_module("scrapegraphai.nodes.generate_code_node")
    assert "CVE-2026-5532" in module.__refused__
    assert module.__spec__.origin is None  # built from nothing, never read from disk
    with pytest.raises(guard.ForbiddenComponent, match="exec"):
        module.GenerateCodeNode(input="user_prompt", output=["generated_code"])

    from scrapegraphai import graphs, nodes

    with pytest.raises(guard.ForbiddenComponent, match="refused by qip-scraper"):
        graphs.CodeGeneratorGraph("prompt", "<html></html>", {"llm": {}})
    with pytest.raises(guard.ForbiddenComponent):
        nodes.GenerateCodeNode(input="x", output=["y"])
    with pytest.raises(guard.ForbiddenComponent):
        graphs.ScriptCreatorGraph("prompt", "<html></html>", {"llm": {}})


def test_every_library_module_outside_the_allow_list_is_a_stub():
    names = library_modules()
    # The premise: the walk saw the library, including the module this is about.
    assert len(names) > 100 and "scrapegraphai.nodes.generate_code_node" in names
    assert guard.ALLOWED_LIBRARY_MODULES <= set(names)

    real, refused = [], []
    for name in names:
        module = importlib.import_module(name)
        (refused if hasattr(module, "__refused__") else real).append(name)
    assert sorted(real) == sorted(guard.ALLOWED_LIBRARY_MODULES)
    assert len(refused) == len(names) - len(guard.ALLOWED_LIBRARY_MODULES)


def test_no_library_module_the_tool_runs_can_execute_code():
    # The premise: the scanner finds what it is meant to find.
    assert _EXECUTES.search(
        (LIBRARY_ROOT / "nodes" / "generate_code_node.py").read_text()
    )
    assert _EXECUTES.search(
        (LIBRARY_ROOT / "nodes" / "conditional_node.py").read_text()
    )
    assert _EXECUTES.search((LIBRARY_ROOT / "docloaders" / "plasmate.py").read_text())

    for name in sorted(guard.ALLOWED_LIBRARY_MODULES):
        origin = importlib.util.find_spec(name).origin
        found = _EXECUTES.search(Path(origin).read_text())
        assert found is None, f"{name} contains {found.group(0)!r}"


def test_the_browser_proxy_broker_search_and_vendor_clients_are_stubs():
    from fp.fp import FreeProxy
    from playwright.sync_api import sync_playwright

    with pytest.raises(guard.ForbiddenComponent):
        FreeProxy()
    with pytest.raises(guard.ForbiddenComponent):
        sync_playwright()
    for name in guard.REFUSED_THIRD_PARTY:
        module = sys.modules.get(name)
        assert module is None or hasattr(module, "__refused__"), name
    from scrapegraphai.utils.proxy_rotation import search_proxy_servers

    with pytest.raises(guard.ForbiddenComponent, match="free-proxy broker"):
        search_proxy_servers()


# --------------------------------------------------------------------------
# The network fence
# --------------------------------------------------------------------------


def test_the_fence_refuses_and_records_every_host_it_was_not_given(site):
    url = site.page("/", "hello", "text/plain")
    with guard.network_fence(set()) as attempts:
        with pytest.raises(OSError):
            socket.create_connection(("127.0.0.1", site.port), timeout=2)
        with pytest.raises(OSError):
            urllib.request.urlopen(url, timeout=2)
    assert len(attempts) == 2 and all("127.0.0.1" in attempt for attempt in attempts)
    assert site.requests == []
    # Closed again: the network is back.
    assert urllib.request.urlopen(url, timeout=5).read() == b"hello"


def test_the_fence_admits_the_hosts_it_was_given(site):
    url = site.page("/", "hello", "text/plain")
    with guard.network_fence({"127.0.0.1"}) as attempts:
        assert urllib.request.urlopen(url, timeout=5).read() == b"hello"
    assert attempts == []


def test_the_fence_refuses_a_tunnel_through_an_admitted_proxy(site):
    opener = urllib.request.build_opener(
        urllib.request.ProxyHandler({"https": f"http://127.0.0.1:{site.port}"})
    )
    with guard.network_fence({"127.0.0.1"}) as attempts, pytest.raises(OSError):
        opener.open("https://sgai-oss-tracing.onrender.com/v1/telemetry", timeout=2)
    assert attempts == ["tunnelling to sgai-oss-tracing.onrender.com"]


def test_a_connection_attempted_while_the_library_runs_refuses_the_whole_run(
    results_site, schema_file, run
):
    leak = Site()
    leak_url = leak.page("/collect", "ok", "text/plain")

    class LeakyModel(GenericFakeChatModel):
        def _generate(self, *args, **kwargs):
            try:
                urllib.request.urlopen(leak_url, timeout=2)
            except OSError:
                pass  # swallowed, as the library's own telemetry sender does
            return super()._generate(*args, **kwargs)

    try:
        model = LeakyModel(messages=iter([fake_model(GOOD_ANSWER).messages.__next__()]))
        result = run(
            results_site.url("/results"),
            "--prompt",
            "x",
            "--schema",
            str(schema_file),
            model=model,
        )
    finally:
        leak.close()

    assert result.code == cli.EXIT_GUARD
    assert "network fence stopped 1 connection attempt" in result.err
    assert result.proposal is None
    assert leak.requests == []
