"""Protection 4 and the graph: the model, the key, and the one shape the library may take."""

from __future__ import annotations

import io
import json
import logging
import os
from types import SimpleNamespace

import pytest
from conftest import FIXTURE_MODEL, GOOD_ANSWER, RESULTS_SCHEMA, Site, fake_model, reply
from langchain_core.language_models.fake_chat_models import GenericFakeChatModel

from qip_scraper import cli, extract, guard

FAKE_KEY = "test-key-0000-not-a-real-credential"


def failing(message: str) -> GenericFakeChatModel:
    """A chat model whose one call fails with ``message``."""

    def answers():
        raise RuntimeError(message)
        yield  # unreachable: it makes this function a generator

    return GenericFakeChatModel(messages=answers())


# --------------------------------------------------------------------------
# The model and its sampling
# --------------------------------------------------------------------------


def test_the_flag_names_the_model_before_the_variable():
    environment = {extract.MODEL_VARIABLE: "claude-fixture-from-environment"}
    assert extract.resolve_model_id("claude-fixture-from-flag", environment) == (
        "claude-fixture-from-flag",
        "--model",
    )


def test_the_variable_names_the_model_when_there_is_no_flag():
    environment = {extract.MODEL_VARIABLE: "claude-fixture-from-environment"}
    assert extract.resolve_model_id(None, environment) == (
        "claude-fixture-from-environment",
        extract.MODEL_VARIABLE,
    )


def test_with_neither_there_is_no_model_to_fall_back_on():
    with pytest.raises(
        ValueError, match="neither --model nor QIP_SCRAPER_MODEL is set"
    ):
        extract.resolve_model_id(None, {})
    with pytest.raises(ValueError, match="set but empty"):
        extract.resolve_model_id(None, {extract.MODEL_VARIABLE: ""})


def test_no_temperature_is_set_unless_asked_for():
    settings = extract.ModelSettings(model_id=FIXTURE_MODEL)
    assert settings.temperature is None
    assert settings.record()["temperature"] is None
    assert extract.ModelSettings(model_id=FIXTURE_MODEL, temperature=0).temperature == 0


@pytest.mark.parametrize("temperature", [0.7, 1.0, -0.5, float("nan")])
def test_zero_is_the_only_temperature_that_can_be_asked_for(temperature):
    with pytest.raises(ValueError, match="accepts only 0"):
        extract.ModelSettings(model_id=FIXTURE_MODEL, temperature=temperature)


@pytest.mark.parametrize(
    "model_id",
    ["gpt-4o", "claude", "claude-", "Claude-Fixture", "claude-fixture 2", ""],
)
def test_a_model_id_that_is_not_claude_is_refused(model_id):
    with pytest.raises(ValueError, match="not a Claude model id"):
        extract.ModelSettings(model_id=model_id)


def test_a_model_timeout_past_its_ceiling_is_refused():
    with pytest.raises(ValueError, match="refused, not adjusted"):
        extract.ModelSettings(
            model_id=FIXTURE_MODEL, timeout=extract.MAX_MODEL_TIMEOUT + 1
        )


@pytest.mark.parametrize(
    ("value", "accepted"),
    [
        (None, True),
        ("https://gateway.example", True),
        ("http://127.0.0.1:8080", True),
        ("http://api.example.com", False),
        ("https://user:pw@api.example.com", False),
        ("ftp://api.example.com", False),
    ],
)
def test_the_key_is_only_ever_sent_over_tls_or_to_loopback(value, accepted):
    environment = {} if value is None else {"ANTHROPIC_BASE_URL": value}
    if accepted:
        assert extract.model_endpoint(environment) == (
            value or extract.DEFAULT_ENDPOINT
        )
    else:
        with pytest.raises(ValueError, match="refused"):
            extract.model_endpoint(environment)


def test_the_fence_admits_the_model_host_and_its_proxy_only():
    assert extract.fence_hosts("https://api.anthropic.com", {}) == {"api.anthropic.com"}
    proxied = {"HTTPS_PROXY": "http://egress.internal:3128"}
    assert extract.fence_hosts("https://api.anthropic.com", proxied) == {
        "api.anthropic.com",
        "egress.internal",
    }
    bypassed = {**proxied, "NO_PROXY": "api.anthropic.com"}
    assert extract.fence_hosts("https://api.anthropic.com", bypassed) == {
        "api.anthropic.com"
    }


# --------------------------------------------------------------------------
# The key: ANTHROPIC_API_KEY or ANTHROPIC_API_KEY_FILE, never both
# --------------------------------------------------------------------------


def test_the_key_is_read_from_the_variable():
    assert extract.read_api_key({"ANTHROPIC_API_KEY": FAKE_KEY}) == FAKE_KEY


def test_the_key_is_read_from_the_file_with_its_trailing_newline_stripped(tmp_path):
    path = tmp_path / "key"
    path.write_text(FAKE_KEY + "\n")
    assert extract.read_api_key({"ANTHROPIC_API_KEY_FILE": str(path)}) == FAKE_KEY


def test_a_key_file_under_the_operators_home_is_found_after_home_moves(tmp_path):
    (tmp_path / "key").write_text(FAKE_KEY)
    assert (
        extract.read_api_key({"ANTHROPIC_API_KEY_FILE": "~/key"}, home=str(tmp_path))
        == FAKE_KEY
    )


def test_both_sources_set_is_refused(tmp_path):
    path = tmp_path / "key"
    path.write_text(FAKE_KEY)
    with pytest.raises(extract.KeyRefused, match="both set") as refused:
        extract.read_api_key(
            {"ANTHROPIC_API_KEY": FAKE_KEY, "ANTHROPIC_API_KEY_FILE": str(path)}
        )
    assert FAKE_KEY not in str(refused.value)


@pytest.mark.parametrize(
    ("environment", "match"),
    [
        ({}, "no API key"),
        ({"ANTHROPIC_API_KEY": "  "}, "set but empty"),
        ({"ANTHROPIC_API_KEY_FILE": "/nonexistent/key"}, "could not be read"),
    ],
)
def test_a_missing_or_empty_key_is_refused(environment, match):
    with pytest.raises(extract.KeyRefused, match=match):
        extract.read_api_key(environment)


def test_an_empty_or_oversized_key_file_is_refused(tmp_path):
    empty, large = tmp_path / "empty", tmp_path / "large"
    empty.write_text("\n")
    large.write_bytes(b"k" * (extract.MAX_KEY_FILE_BYTES + 1))
    with pytest.raises(extract.KeyRefused, match="empty"):
        extract.read_api_key({"ANTHROPIC_API_KEY_FILE": str(empty)})
    with pytest.raises(extract.KeyRefused, match="larger than"):
        extract.read_api_key({"ANTHROPIC_API_KEY_FILE": str(large)})


# --------------------------------------------------------------------------
# The graph: one shape, reading supplied text
# --------------------------------------------------------------------------


def build_graph(source: str, **config_changes):
    from scrapegraphai.graphs import SmartScraperGraph

    model = fake_model(GOOD_ANSWER)
    config = {**extract.graph_config(model, 600), **config_changes}
    graph = SmartScraperGraph(
        prompt="p",
        source=source,
        config=config,
        schema=extract.schema_model(RESULTS_SCHEMA),
    )
    return graph, model


def test_the_graph_the_tool_builds_has_the_one_permitted_shape():
    graph, model = build_graph("\nsome text")
    extract.check_graph(graph, model)  # the premise: the tool's own graph passes
    assert [type(node).__name__ for node in graph.graph.nodes] == [
        "FetchNode",
        "GenerateAnswerNode",
    ]


def test_a_graph_that_would_chunk_the_page_is_refused():
    graph, model = build_graph("\nsome text", html_mode=False)
    with pytest.raises(extract.ExtractionFailed, match="ParseNode would chunk"):
        extract.check_graph(graph, model)


def test_a_graph_whose_source_starts_with_http_is_refused_because_the_library_would_fetch_it():
    graph, model = build_graph("https://example.com/ is where the figures are")
    with pytest.raises(extract.ExtractionFailed, match="treat the page as a URL"):
        extract.check_graph(graph, model)


def test_page_text_that_begins_with_a_url_is_still_only_read(
    results_site, run, tmp_path
):
    results_site.page("/plain", "https://example.com/ lists Acme Corp", "text/plain")
    schema = tmp_path / "s.json"
    schema.write_text(
        json.dumps({"type": "object", "properties": {"company": {"type": "string"}}})
    )
    result = run(
        results_site.url("/plain"),
        "--prompt",
        "Which company?",
        "--schema",
        str(schema),
        model=fake_model('{"company": "Acme Corp"}'),
    )
    assert result.code == cli.EXIT_OK, result.err
    assert result.proposal["sources"][0]["counts"]["verified"] == 1


def test_the_configuration_holds_nothing_that_opens_a_path():
    config = extract.graph_config(fake_model(), 600)
    assert set(config) == extract.ALLOWED_CONFIG_KEYS
    assert config["html_mode"] is True
    assert (
        "{" not in extract.EXTRACTION_INSTRUCTION
        and "}" not in extract.EXTRACTION_INSTRUCTION
    )


def test_the_library_is_refused_to_anything_that_skips_the_guard(monkeypatch):
    monkeypatch.setattr(guard, "_REPORT", None)
    with pytest.raises(guard.GuardRefusal, match="only after"):
        extract.ask(
            fake_model(GOOD_ANSWER),
            extract.new_recorder(),
            prompt="p",
            text="t",
            schema=RESULTS_SCHEMA,
            library_timeout=600,
        )


@pytest.mark.parametrize(
    ("text", "stop", "match"),
    [
        (GOOD_ANSWER, "refusal", "declined"),
        # A refusal usually comes with no text, which the library's parser
        # fails on first; the cause reported is still the refusal.
        ("", "refusal", "declined"),
        (GOOD_ANSWER[:30], "max_tokens", "cut off"),
        (GOOD_ANSWER, None, "unreported stop reason"),
        (GOOD_ANSWER, "pause_turn", "only a complete answer"),
        ("I could not find the figures.", "end_turn", "extraction failed"),
    ],
)
def test_only_a_complete_answer_is_read(text, stop, match):
    model = fake_model(reply(text, stop=stop))
    recorder = extract.new_recorder()
    extract.attach_recorder(model, recorder)
    with (
        guard.network_fence(set()),
        pytest.raises(extract.ExtractionFailed, match=match),
    ):
        extract.ask(
            model,
            recorder,
            prompt="p",
            text="Acme Corp",
            schema=RESULTS_SCHEMA,
            library_timeout=600,
        )


def test_an_answer_cut_off_mid_value_is_refused_although_the_library_repaired_it(
    results_site, schema_file, run
):
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=fake_model('{"company": "Acme Co'),
    )
    (source,) = result.proposal["sources"]
    assert source["outcome"] == "refused"
    assert "not one strict JSON document" in source["reason"]
    assert result.code == cli.EXIT_FINDINGS


# --------------------------------------------------------------------------
# The real ChatAnthropic, against a local stand-in for the Messages API
# --------------------------------------------------------------------------


def messages_api(answer: str, model: str) -> Site:
    api = Site()
    message = {
        "id": "msg_test",
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": [{"type": "text", "text": answer}],
        "stop_reason": "end_turn",
        "stop_sequence": None,
        "usage": {"input_tokens": 321, "output_tokens": 54},
    }
    api.page("/v1/messages", json.dumps(message), "application/json")
    return api


@pytest.mark.parametrize(
    ("flags", "sent"),
    [((), None), (("--temperature", "0"), 0)],
    ids=["no temperature unless asked", "temperature 0 when asked"],
)
def test_the_request_carries_the_model_the_key_and_the_temperature_the_proposal_records(
    flags,
    sent,
    results_site,
    schema_file,
    run,
    monkeypatch,
):
    api = messages_api(GOOD_ANSWER, FIXTURE_MODEL)
    monkeypatch.setenv("ANTHROPIC_BASE_URL", api.url(""))
    monkeypatch.setenv("ANTHROPIC_API_KEY", FAKE_KEY)
    try:
        result = run(
            results_site.url("/results"),
            "--prompt",
            "Extract the figures.",
            "--schema",
            str(schema_file),
            *flags,
        )
    finally:
        api.close()

    assert result.code == cli.EXIT_OK, result.err
    (request,) = api.requests
    body = json.loads(request["body"])
    assert request["path"] == "/v1/messages"
    assert body["model"] == FIXTURE_MODEL
    assert body["max_tokens"] == extract.MAX_TOKENS
    # The body sent, not the settings intended: absent unless asked for, 0 when asked.
    assert ("temperature" in body) is (sent is not None)
    assert body.get("temperature") == sent
    headers = {name.lower(): value for name, value in request["headers"].items()}
    assert headers["x-api-key"] == FAKE_KEY  # the key went to the model, and...
    # ...nowhere else: not to the site, not to the log, not into the proposal.
    assert all(
        FAKE_KEY not in json.dumps(other["headers"]) for other in results_site.requests
    )
    assert FAKE_KEY not in result.err and FAKE_KEY not in result.output.read_text()

    model = result.proposal["model"]
    assert (model["id"], model["endpoint"], model["temperature"]) == (
        FIXTURE_MODEL,
        api.url(""),
        sent,
    )
    assert result.proposal["guard"]["network_fence_hosts"] == ["127.0.0.1"]
    assert result.proposal["sources"][0]["model_served"] == FIXTURE_MODEL
    assert result.proposal["sources"][0]["usage"]["input_tokens"] == 321


def test_a_key_in_an_error_message_is_redacted_wherever_it_could_be_written(
    results_site, schema_file, run, monkeypatch, caplog
):
    # The library logs a failed call's exception text at ERROR through a
    # handler it bound to stderr at import. A handler attached here before the
    # run stands in for it: it is the channel this tool's own messages do not
    # pass through, and the one that printed the key before the log filter.
    stream = io.StringIO()
    library_handler = logging.StreamHandler(stream)
    logging.getLogger("scrapegraphai").addHandler(library_handler)
    api = Site()
    api.page(
        "/v1/messages",
        json.dumps(
            {
                "type": "error",
                "error": {
                    "type": "authentication_error",
                    "message": f"invalid x-api-key {FAKE_KEY}",
                },
            }
        ),
        "application/json",
        status=401,
    )
    monkeypatch.setenv("ANTHROPIC_BASE_URL", api.url(""))
    monkeypatch.setenv("ANTHROPIC_API_KEY", FAKE_KEY)
    try:
        result = run(
            results_site.url("/results"), "--prompt", "x", "--schema", str(schema_file)
        )
    finally:
        api.close()
        logging.getLogger("scrapegraphai").removeHandler(library_handler)
    assert result.code == cli.EXIT_FINDINGS
    assert "AuthenticationError" in result.proposal["sources"][0]["reason"]
    assert "[redacted]" in result.proposal["sources"][0]["reason"]
    # The premise: the library did log the failure, through both channels.
    assert "Error during chain execution" in stream.getvalue()
    assert "Error during chain execution" in caplog.text
    for channel in (
        result.err,
        result.output.read_text(),
        stream.getvalue(),
        caplog.text,
    ):
        assert FAKE_KEY not in channel


def test_a_key_across_the_old_cut_at_500_characters_is_redacted_whole(
    results_site, schema_file, run, monkeypatch
):
    # The review's case. The extraction step cut a failure's text at 500
    # characters before the command line redacted it, and redaction looks for
    # the whole key: with the key across the cut, its first ten characters
    # reached the proposal and stderr.
    before = (
        "AnthropicAuthenticationError: Error code: 401 - {'type': 'error', 'error': "
        "{'type': 'authentication_error', 'message': '"
    )
    message = "p" * (490 - len(before)) + FAKE_KEY
    api = Site()
    api.page(
        "/v1/messages",
        json.dumps(
            {
                "type": "error",
                "error": {"type": "authentication_error", "message": message},
            }
        ),
        "application/json",
        status=401,
    )
    monkeypatch.setenv("ANTHROPIC_BASE_URL", api.url(""))
    monkeypatch.setenv("ANTHROPIC_API_KEY", FAKE_KEY)
    try:
        result = run(
            results_site.url("/results"), "--prompt", "x", "--schema", str(schema_file)
        )
    finally:
        api.close()
    reason = result.proposal["sources"][0]["reason"]
    # The premise: the client's text is the one the offset was counted for.
    assert before in reason
    assert "[redacted]" in reason
    for channel in (result.err, result.output.read_text()):
        assert FAKE_KEY[:10] not in channel


def test_a_failure_reaches_the_caller_whole_so_the_key_can_be_found_in_it():
    model = failing("p" * 2_000 + FAKE_KEY)
    recorder = extract.new_recorder()
    extract.attach_recorder(model, recorder)
    with (
        guard.network_fence(set()),
        pytest.raises(extract.ExtractionFailed, match="RuntimeError") as failed,
    ):
        extract.ask(
            model,
            recorder,
            prompt="p",
            text="Acme Corp",
            schema=RESULTS_SCHEMA,
            library_timeout=600,
        )
    assert str(failed.value).endswith(FAKE_KEY)


def test_a_reason_is_redacted_before_it_is_shortened():
    # The key sits across the place the reason is cut: shortened first, the
    # key's first ten characters would survive a redaction that found nothing.
    prefix = "the extraction failed before the model answered: RuntimeError: "
    padding = cli.MAX_REASON_CHARS - len(prefix) - 10
    model = failing("p" * padding + FAKE_KEY + "q" * 500)
    recorder = extract.new_recorder()
    extract.attach_recorder(model, recorder)
    entry = {"url": "https://example.test/"}
    with guard.network_fence(set()):
        cli._extract_one(
            entry,
            "Acme Corp",
            model=model,
            recorder=recorder,
            args=SimpleNamespace(prompt="p"),
            schema=RESULTS_SCHEMA,
            settings=extract.ModelSettings(model_id=FIXTURE_MODEL),
            secrets=[FAKE_KEY],
        )
    reason = entry["reason"]
    assert reason.startswith(prefix)
    assert "[redacted]" in reason
    assert FAKE_KEY[:10] not in reason
    assert len(reason) == cli.MAX_REASON_CHARS + len("...")


def test_no_key_variable_survives_in_a_proposal_or_the_environment_it_names(
    results_site, schema_file, run, monkeypatch
):
    monkeypatch.setenv("ANTHROPIC_API_KEY", FAKE_KEY)
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=fake_model(GOOD_ANSWER),
    )
    assert result.code == cli.EXIT_OK
    assert FAKE_KEY not in result.output.read_text()
    assert os.environ["ANTHROPIC_API_KEY"] == FAKE_KEY  # read, never rewritten
