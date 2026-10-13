"""The command line end to end: a local site, a fake Claude, a real graph.

Every run here goes through the real SmartScraperGraph inside the real
network fence with an empty allow-list, so a passing run is also evidence
that the library opened no connection of its own.
"""

from __future__ import annotations

import errno
import hashlib
import json
import re

import pytest
from conftest import (
    FIXTURE_MODEL,
    GOOD_ANSWER,
    RESULTS_PAGE,
    RESULTS_SCHEMA,
    Site,
    fake_model,
    reply,
)

from qip_scraper import cli, extract, fetch, guard, verify

KEY = "test-key-1111-not-a-real-credential"


def test_a_page_whose_every_value_is_in_it_becomes_a_complete_proposal(
    results_site, schema_file, run
):
    url = results_site.url("/results")
    result = run(
        url,
        "--prompt",
        "Extract the quarter's headline figures.",
        "--schema",
        str(schema_file),
        model=fake_model(GOOD_ANSWER),
    )

    assert result.code == cli.EXIT_OK, result.err
    proposal = result.proposal
    assert proposal["kind"] == "qip-scraper/proposal"
    assert proposal["prompt"] == "Extract the quarter's headline figures."
    assert proposal["schema"] == RESULTS_SCHEMA
    assert proposal["model"]["id"] == FIXTURE_MODEL
    assert proposal["model"]["id_source"] == "--model"
    assert proposal["guard"]["network_fence_hosts"] == []
    assert proposal["guard"]["telemetry_on"] is False

    (source,) = proposal["sources"]
    assert source["outcome"] == "extracted"
    assert source["url"] == url
    assert source["http_status"] == 200
    assert re.fullmatch(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z", source["fetched_at"])
    # The digest is of the bytes the server sent, not of anything derived.
    assert source["sha256"] == hashlib.sha256(RESULTS_PAGE.encode()).hexdigest()
    assert source["bytes"] == len(RESULTS_PAGE.encode())
    assert source["model_served"] == FIXTURE_MODEL
    assert source["stop_reason"] == "end_turn"
    assert source["extracted"] == json.loads(GOOD_ANSWER)
    assert source["counts"] == {"verified": 6, "unverified": 0, "absent": 0}
    by_pointer = {value["pointer"]: value for value in source["values"]}
    assert by_pointer["/growth_percent"]["spelling"] == "12.50"
    assert by_pointer["/growth_percent"]["status"] == "verified"
    assert proposal["verdict"]["exit_code"] == cli.EXIT_OK
    # robots.txt was read before the page, and the page only once.
    assert results_site.paths() == ["/robots.txt", "/results"]


def test_a_value_not_in_the_page_fails_the_run_and_is_marked(
    results_site, schema_file, run
):
    # 12.5 is not how the page spells it (12.50): the model reformatted a number.
    answer = GOOD_ANSWER.replace("12.50", "12.5")
    result = run(
        results_site.url("/results"),
        "--prompt",
        "Extract the figures.",
        "--schema",
        str(schema_file),
        model=fake_model(answer),
    )

    assert result.code == cli.EXIT_FINDINGS
    assert "not found verbatim" in result.err
    (source,) = result.proposal["sources"]
    marked = [value for value in source["values"] if value["status"] == "unverified"]
    assert [(value["pointer"], value["spelling"]) for value in marked] == [
        ("/growth_percent", "12.5")
    ]
    assert result.proposal["verdict"]["values"]["unverified"] == 1


def test_allow_unverified_accepts_the_run_and_still_marks_the_value(
    results_site, schema_file, run
):
    answer = GOOD_ANSWER.replace("12.50", "12.5")
    result = run(
        results_site.url("/results"),
        "--prompt",
        "Extract the figures.",
        "--schema",
        str(schema_file),
        "--allow-unverified",
        model=fake_model(answer),
    )

    assert result.code == cli.EXIT_OK
    assert result.proposal["run"]["allow_unverified"] is True
    statuses = {
        value["pointer"]: value["status"]
        for value in result.proposal["sources"][0]["values"]
    }
    assert statuses["/growth_percent"] == "unverified"


def test_an_answer_the_schema_refuses_fails_even_with_allow_unverified(
    results_site, schema_file, run
):
    answer = GOOD_ANSWER.replace('"net_income": 210.75', '"net_income": "210.75"')
    result = run(
        results_site.url("/results"),
        "--prompt",
        "Extract the figures.",
        "--schema",
        str(schema_file),
        "--allow-unverified",
        model=fake_model(answer),
    )

    assert result.code == cli.EXIT_FINDINGS
    (source,) = result.proposal["sources"]
    assert source["outcome"] == "refused"
    assert "does not satisfy the schema at /net_income" in source["reason"]
    assert "extracted" not in source


def test_more_urls_than_the_ceiling_are_refused_before_anything_is_fetched(
    site, schema_file, run
):
    urls = [site.url(f"/page{index}") for index in range(cli.MAX_URLS + 1)]
    result = run(
        *urls, "--prompt", "x", "--schema", str(schema_file), model=fake_model()
    )

    assert result.code == cli.EXIT_USAGE
    assert f"at most {cli.MAX_URLS}" in result.err
    assert site.requests == []
    assert result.proposal is None


def test_a_url_given_twice_is_refused(site, schema_file, run):
    url = site.url("/a")
    result = run(
        url, url, "--prompt", "x", "--schema", str(schema_file), model=fake_model()
    )
    assert result.code == cli.EXIT_USAGE
    assert "given twice" in result.err


def test_a_page_longer_than_the_text_ceiling_is_refused_not_chunked(
    site, schema_file, run
):
    site.robots("User-agent: *\nAllow: /\n")
    long_page = "<p>" + "word " * 2_000 + "</p>"
    url = site.page("/long", long_page)
    # The fake model has no answers: if the page reached it, the run would fail differently.
    result = run(
        url,
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        "--max-text-chars",
        "1000",
        model=fake_model(),
    )

    assert result.code == cli.EXIT_FINDINGS
    (source,) = result.proposal["sources"]
    assert source["outcome"] == "refused"
    assert "refused rather than chunked" in source["reason"]


def test_a_limit_past_its_ceiling_is_refused_not_lowered(site, schema_file, run):
    result = run(
        site.url("/a"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        "--max-page-bytes",
        str(fetch.CEILINGS["max_page_bytes"] + 1),
        model=fake_model(),
    )
    assert result.code == cli.EXIT_USAGE
    assert "refused, not adjusted" in result.err


def test_an_existing_output_file_is_never_overwritten(
    results_site, schema_file, run, tmp_path
):
    output = tmp_path / "reviewed.json"
    output.write_text("a reviewed proposal")
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        "--output",
        str(output),
        model=fake_model(GOOD_ANSWER),
    )
    assert result.code == cli.EXIT_USAGE
    assert output.read_text() == "a reviewed proposal"
    assert results_site.requests == []


def test_no_api_key_is_refused_before_anything_is_fetched(
    results_site, schema_file, run
):
    result = run(
        results_site.url("/results"), "--prompt", "x", "--schema", str(schema_file)
    )
    assert result.code == cli.EXIT_USAGE
    assert "no API key" in result.err
    assert results_site.requests == []


def test_a_non_claude_model_id_is_refused(results_site, schema_file, run):
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        "--model",
        "gpt-4o",
        model=fake_model(GOOD_ANSWER),
    )
    assert result.code == cli.EXIT_USAGE
    assert "not a Claude model id" in result.err


def test_a_pydantic_model_module_supplies_the_schema(results_site, run, tmp_path):
    module = tmp_path / "models.py"
    module.write_text(
        "from pydantic import BaseModel\n\n"
        "class Results(BaseModel):\n"
        "    company: str\n"
        "    net_income: float\n"
    )
    answer = '{"company": "Acme Corp", "net_income": 210.75}'
    result = run(
        results_site.url("/results"),
        "--prompt",
        "Extract the company and net income.",
        "--pydantic-model",
        f"{module}:Results",
        model=fake_model(answer),
    )

    assert result.code == cli.EXIT_OK, result.err
    assert result.proposal["schema_source"] == "models.py:Results"
    assert result.proposal["schema"]["required"] == ["company", "net_income"]
    assert result.proposal["sources"][0]["counts"] == {
        "verified": 2,
        "unverified": 0,
        "absent": 0,
    }


def test_absent_values_are_recorded_and_do_not_fail_the_run(
    results_site, schema_file, run
):
    answer = GOOD_ANSWER.replace('"/reports/q3.pdf"', "null")
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=fake_model(answer),
    )
    assert result.code == cli.EXIT_OK, result.err
    statuses = {
        value["pointer"]: value["status"]
        for value in result.proposal["sources"][0]["values"]
    }
    assert statuses["/report_link"] == "absent"


def test_one_refused_page_does_not_stop_the_others(site, schema_file, run):
    site.robots("User-agent: *\nDisallow: /private\n")
    site.page("/results", RESULTS_PAGE)
    site.page("/private", RESULTS_PAGE)
    result = run(
        site.url("/private"),
        site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=fake_model(GOOD_ANSWER),
    )

    assert result.code == cli.EXIT_FINDINGS
    outcomes = [source["outcome"] for source in result.proposal["sources"]]
    assert outcomes == ["refused", "extracted"]
    assert "/private" not in site.paths()


def test_the_proposal_keeps_no_page_text(results_site, schema_file, run):
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=fake_model(GOOD_ANSWER),
    )
    text = result.output.read_text()
    # A sentence of the page that no value or context window carries.
    assert "third-quarter results" in RESULTS_PAGE
    assert "reports third-quarter results" not in text
    assert result.proposal["sources"][0]["text_sha256"] == verify.sha256_text(
        verify.page_text(RESULTS_PAGE, "text/html")
    )


def test_a_reply_cut_off_at_max_tokens_is_refused(results_site, schema_file, run):
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=fake_model(reply(GOOD_ANSWER[:40], stop="max_tokens")),
    )
    assert result.code == cli.EXIT_FINDINGS
    assert "cut off" in result.proposal["sources"][0]["reason"]


# --------------------------------------------------------------------------
# Which model: --model, else QIP_SCRAPER_MODEL, else refused
# --------------------------------------------------------------------------


def test_the_model_flag_wins_over_the_environment(
    results_site, schema_file, run, monkeypatch
):
    monkeypatch.setenv(extract.MODEL_VARIABLE, "claude-fixture-from-environment")
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=fake_model(GOOD_ANSWER),
        model_id="claude-fixture-from-flag",
    )
    assert result.code == cli.EXIT_OK, result.err
    model = result.proposal["model"]
    assert (model["id"], model["id_source"]) == ("claude-fixture-from-flag", "--model")


def test_the_environment_names_the_model_when_no_flag_does(
    results_site, schema_file, run, monkeypatch
):
    monkeypatch.setenv(extract.MODEL_VARIABLE, "claude-fixture-from-environment")
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=fake_model(GOOD_ANSWER),
        model_id=None,
    )
    assert result.code == cli.EXIT_OK, result.err
    model = result.proposal["model"]
    assert (model["id"], model["id_source"]) == (
        "claude-fixture-from-environment",
        extract.MODEL_VARIABLE,
    )


def test_no_model_anywhere_is_refused_before_any_connection(
    results_site, schema_file, run
):
    # The fence records every connection attempt to any host; an empty record
    # is the evidence that the refusal came before the network was touched.
    with guard.network_fence(set()) as attempts:
        result = run(
            results_site.url("/results"),
            "--prompt",
            "x",
            "--schema",
            str(schema_file),
            model=fake_model(GOOD_ANSWER),
            model_id=None,
        )
    assert result.code == cli.EXIT_USAGE
    assert "no model: neither --model nor QIP_SCRAPER_MODEL is set" in result.err
    assert attempts == []
    assert results_site.requests == []
    assert result.proposal is None


def test_an_empty_model_variable_is_refused_not_read_as_unset(
    results_site, schema_file, run, monkeypatch
):
    monkeypatch.setenv(extract.MODEL_VARIABLE, "  ")
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=fake_model(GOOD_ANSWER),
        model_id=None,
    )
    assert result.code == cli.EXIT_USAGE
    assert "QIP_SCRAPER_MODEL is set but empty" in result.err
    assert results_site.requests == []


def test_a_model_from_the_environment_is_held_to_the_same_id_shape(
    results_site, schema_file, run, monkeypatch
):
    monkeypatch.setenv(extract.MODEL_VARIABLE, "gpt-4o")
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=fake_model(GOOD_ANSWER),
        model_id=None,
    )
    assert result.code == cli.EXIT_USAGE
    assert "'gpt-4o' is not a Claude model id" in result.err
    assert results_site.requests == []


# --------------------------------------------------------------------------
# Temperature: none unless asked for, and then only 0
# --------------------------------------------------------------------------


def test_no_temperature_is_recorded_unless_asked_for(results_site, schema_file, run):
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=fake_model(GOOD_ANSWER),
    )
    assert result.code == cli.EXIT_OK, result.err
    model = result.proposal["model"]
    assert model["temperature"] is None
    assert model["temperature_note"] == extract.TEMPERATURE_NOT_SENT


def test_temperature_zero_is_recorded_when_asked_for(results_site, schema_file, run):
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        "--temperature",
        "0",
        model=fake_model(GOOD_ANSWER),
    )
    assert result.code == cli.EXIT_OK, result.err
    model = result.proposal["model"]
    assert model["temperature"] == 0
    assert model["temperature_note"] == extract.TEMPERATURE_SENT


def test_a_temperature_other_than_zero_is_refused_not_adjusted(
    results_site, schema_file, run
):
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        "--temperature",
        "0.7",
        model=fake_model(GOOD_ANSWER),
    )
    assert result.code == cli.EXIT_USAGE
    assert "--temperature accepts only 0" in result.err
    assert results_site.requests == []


# --------------------------------------------------------------------------
# Addresses: the command line runs the tool's own policy
# --------------------------------------------------------------------------


def test_the_command_line_fetches_under_the_tools_own_address_policy(
    results_site, schema_file, run
):
    # Every other run here admits 127.0.0.1 through a parameter the command
    # line cannot set; without it, the local server is what it is: loopback.
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=fake_model(GOOD_ANSWER),
        address_policy=None,
    )
    assert result.code == cli.EXIT_FINDINGS
    (source,) = result.proposal["sources"]
    assert source["outcome"] == "refused"
    assert "127.0.0.1 is a loopback address" in source["reason"]
    assert results_site.requests == []


# --------------------------------------------------------------------------
# A hostile server costs its own page, not the run
# --------------------------------------------------------------------------


def _robots_crawl_delay_unreadable(server: Site) -> None:
    server.robots("User-agent: *\nCrawl-delay: ²\nAllow: /\n")
    server.page("/p", RESULTS_PAGE)


def _page_redirect_not_a_url(server: Site) -> None:
    server.robots("User-agent: *\nAllow: /\n")
    server.page("/p", "", headers={"Location": "http://["}, status=302)


def _robots_redirect_not_a_url(server: Site) -> None:
    server.page("/robots.txt", "", headers={"Location": "http://["}, status=302)
    server.page("/p", RESULTS_PAGE)


@pytest.mark.parametrize(
    ("serve", "reason"),
    [
        (_robots_crawl_delay_unreadable, "urllib.robotparser cannot read"),
        (_page_redirect_not_a_url, "which is not a URL"),
        (_robots_redirect_not_a_url, "which is not a URL"),
    ],
    ids=["Crawl-delay: ²", "page Location: http://[", "robots.txt Location: http://["],
)
def test_a_hostile_server_costs_its_own_page_and_not_the_run(
    results_site, schema_file, run, serve, reason
):
    # The review's three cases. Each used to end the whole run with exit 5
    # and no proposal, losing the ordinary page given after it.
    server = Site()
    try:
        serve(server)
        result = run(
            server.url("/p"),
            results_site.url("/results"),
            "--prompt",
            "x",
            "--schema",
            str(schema_file),
            model=fake_model(GOOD_ANSWER),
        )
    finally:
        server.close()
    assert result.code == cli.EXIT_FINDINGS, result.err
    hostile, ordinary = result.proposal["sources"]
    assert (hostile["outcome"], hostile["stage"]) == ("refused", "fetch")
    assert reason in hostile["reason"]
    assert ordinary["outcome"] == "extracted"


# --------------------------------------------------------------------------
# The output directory is checked before anything is fetched
# --------------------------------------------------------------------------


def test_an_output_directory_without_hard_links_is_refused_before_any_fetch(
    results_site, schema_file, run, tmp_path, monkeypatch
):
    # FAT and some network mounts have no hard links, and the proposal is
    # linked into place. Found out at the end, the pages had been fetched and
    # the model paid for, and the run exited 4.
    def no_hard_links(source, destination):
        raise OSError(errno.EPERM, "Operation not permitted")

    monkeypatch.setattr(cli.os, "link", no_hard_links)
    model = fake_model(GOOD_ANSWER)
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=model,
    )
    assert result.code == cli.EXIT_USAGE
    assert "does not allow a hard link (Operation not permitted)" in result.err
    assert results_site.requests == []
    assert len(list(model.messages)) == 1  # the model was never asked
    assert _only_the_schema_is_left(tmp_path)


def test_the_hard_link_check_leaves_nothing_behind(
    results_site, schema_file, run, tmp_path
):
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=fake_model(GOOD_ANSWER),
    )
    assert result.code == cli.EXIT_OK, result.err
    assert sorted(path.name for path in tmp_path.iterdir()) == [
        "proposal.json",
        "results.schema.json",
    ]


# --------------------------------------------------------------------------
# Text that is not Unicode: refused where it enters
# --------------------------------------------------------------------------


def test_a_page_whose_charset_decodes_no_text_is_refused_and_the_run_goes_on(
    site, schema_file, run
):
    # charset=zlib used to end the run with a LookupError traceback and exit 1,
    # the code for "a proposal was written", with no proposal.
    site.robots("User-agent: *\nAllow: /\n")
    site.page("/packed", "<p>Acme Corp</p>", "text/html; charset=zlib")
    site.page("/results", RESULTS_PAGE)
    result = run(
        site.url("/packed"),
        site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=fake_model(GOOD_ANSWER),
    )
    assert result.code == cli.EXIT_FINDINGS
    packed, results = result.proposal["sources"]
    assert (packed["outcome"], packed["stage"]) == ("refused", "decode")
    assert "'zlib'" in packed["reason"] and "does not decode text" in packed["reason"]
    assert results["outcome"] == "extracted"


def test_an_answer_holding_a_lone_surrogate_refuses_its_page_and_not_the_run(
    results_site, schema_file, run
):
    # This answer used to stop the write halfway and leave a 0-byte proposal.
    answer = GOOD_ANSWER.replace('"Acme Corp"', '"\\ud800"')
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=fake_model(answer),
    )
    assert result.code == cli.EXIT_FINDINGS
    (source,) = result.proposal["sources"]
    assert (source["outcome"], source["stage"]) == ("refused", "extract")
    assert "lone surrogate" in source["reason"]


def test_an_argument_that_is_not_utf8_is_refused_before_anything_is_fetched(
    results_site, schema_file, run
):
    # How Python hands over a prompt typed in Latin-1: a lone surrogate.
    result = run(
        results_site.url("/results"),
        "--prompt",
        "caf\udce9 figures",
        "--schema",
        str(schema_file),
        model=fake_model(GOOD_ANSWER),
    )
    assert result.code == cli.EXIT_USAGE
    assert "the prompt argument is not valid UTF-8" in result.err
    assert results_site.requests == []
    assert result.proposal is None


# --------------------------------------------------------------------------
# A run that cannot finish says so, and leaves nothing behind
# --------------------------------------------------------------------------


def _only_the_schema_is_left(tmp_path) -> bool:
    return sorted(path.name for path in tmp_path.iterdir()) == ["results.schema.json"]


def test_text_no_file_can_hold_stops_the_write_with_its_own_code(
    results_site, schema_file, run, tmp_path
):
    # The model's own name comes back from the API and is checked by nothing
    # before the write, so it stands for whatever else might carry such text.
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=fake_model(reply(GOOD_ANSWER, model="claude-\ud800")),
    )
    assert result.code == cli.EXIT_OUTPUT
    assert "a lone surrogate" in result.err
    assert result.proposal is None
    assert _only_the_schema_is_left(tmp_path)


def test_a_write_that_fails_partway_leaves_no_file_at_all(
    results_site, schema_file, run, tmp_path, monkeypatch
):
    def disk_full(descriptor):
        raise OSError(errno.ENOSPC, "No space left on device")

    monkeypatch.setattr(cli.os, "fsync", disk_full)
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=fake_model(GOOD_ANSWER),
    )
    assert result.code == cli.EXIT_OUTPUT
    assert "No space left on device" in result.err
    # Neither the proposal nor the temporary file it was being written to.
    assert _only_the_schema_is_left(tmp_path)


def test_a_file_that_appears_while_the_run_is_going_is_not_written_over(
    results_site, schema_file, run, tmp_path, monkeypatch
):
    output = tmp_path / "proposal.json"
    fetch_all = cli._fetch_all

    def and_meanwhile(*arguments, **keywords):
        sources = fetch_all(*arguments, **keywords)
        output.write_text('{"written by": "someone else"}')
        return sources

    monkeypatch.setattr(cli, "_fetch_all", and_meanwhile)
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        output=output,
        model=fake_model(GOOD_ANSWER),
    )
    assert result.code == cli.EXIT_OUTPUT
    assert "never written over another file" in result.err
    assert result.proposal == {"written by": "someone else"}
    assert sorted(path.name for path in tmp_path.iterdir()) == [
        "proposal.json",
        "results.schema.json",
    ]


def test_an_unexpected_failure_has_its_own_code_and_the_key_is_redacted(
    results_site, schema_file, run, tmp_path, monkeypatch
):
    # An uncaught exception exits 1, which reads as "a proposal was written".
    def breaks(decoded, media_type):
        raise RuntimeError(f"the text extractor broke near {KEY}")

    monkeypatch.setenv("ANTHROPIC_API_KEY", KEY)
    monkeypatch.setattr(verify, "page_text", breaks)
    result = run(
        results_site.url("/results"), "--prompt", "x", "--schema", str(schema_file)
    )
    assert result.code == cli.EXIT_INTERNAL
    assert "RuntimeError: the text extractor broke near [redacted]" in result.err
    assert KEY not in result.err
    assert result.proposal is None
    assert _only_the_schema_is_left(tmp_path)


def test_a_guard_refusal_anywhere_in_the_run_exits_3(
    results_site, schema_file, run, monkeypatch
):
    # Not only inside the fence: a guard refusal is a guard refusal wherever
    # it is raised, and is never filed as an unexpected failure.
    def refuses():
        raise guard.GuardRefusal("the guard says no")

    monkeypatch.setattr(extract, "new_recorder", refuses)
    result = run(
        results_site.url("/results"),
        "--prompt",
        "x",
        "--schema",
        str(schema_file),
        model=fake_model(GOOD_ANSWER),
    )
    assert result.code == cli.EXIT_GUARD
    assert "refused: the guard says no; no proposal is written" in result.err
    assert result.proposal is None
