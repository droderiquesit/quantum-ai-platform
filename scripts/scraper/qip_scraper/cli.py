"""The command line: arguments, two phases, and the proposal.

Phase one fetches every page, with the library imported but idle. Phase two
runs the library over the fetched text inside the network fence, which
admits the model's host and nothing else, and stays closed until the last
page is done, so that no thread the library starts can reach the network
after its graph returns.

Exit codes: 0, every page was read and every value found verbatim (or
``--allow-unverified`` was given); 1, a proposal was written but a page was
refused, an answer failed its schema, or a value was not found verbatim; 2,
the command line, the schema, the key or the output directory was refused and
nothing was fetched; 3, a safety guard refused (telemetry not off, tracing on,
a refused library component used, or a connection attempted past the fence)
and nothing was written; 4, the proposal could not be written whole, and
nothing was left at the output path; 5, the run failed unexpectedly, and
nothing was written.

Codes 4 and 5 exist so that a failure is never read as a finding. An
uncaught exception exits 1, which is the code for "a proposal was written",
and a crash once left a 0-byte proposal behind with exactly that code.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import importlib.metadata
import json
import logging
import os
import platform
import shutil
import sys
import tempfile
import traceback
from pathlib import Path
from typing import Any

from . import TOOL_NAME, __version__, extract, fetch, guard, verify

EXIT_OK = 0
EXIT_FINDINGS = 1
EXIT_USAGE = 2
EXIT_GUARD = 3
EXIT_OUTPUT = 4
EXIT_INTERNAL = 5

MAX_URLS = 10
MAX_PROMPT_CHARS = 8_000
DEFAULT_MAX_TEXT_CHARS = 150_000
MAX_TEXT_CHARS_CEILING = 600_000
#: A refusal reason is cut to this many characters, after redaction.
MAX_REASON_CHARS = 1_000

PROPOSAL_KIND = "qip-scraper/proposal"
NOTICE = (
    "A proposal for a person to review, not data. Nothing in the platform reads this file. "
    "A value marked verified was found verbatim in the text derived from the fetched bytes: "
    "that proves it occurs in the page, not that it is the right value for its field."
)


class UsageRefused(Exception):
    pass


def _say(message: str) -> None:
    print(f"{TOOL_NAME}: {message}", file=sys.stderr)


def redact(text: str, secrets) -> str:
    for secret in secrets:
        if secret and len(secret) >= 8:
            text = text.replace(secret, "[redacted]")
    return text


def _bounded(text: str) -> str:
    """``text`` cut to ``MAX_REASON_CHARS``. Only ever applied to redacted
    text: redaction looks for the whole key, and a key cut in two by an
    earlier shortening left its first half in the proposal."""
    if len(text) <= MAX_REASON_CHARS:
        return text
    return text[:MAX_REASON_CHARS] + "..."


class _RedactingFilter(logging.Filter):
    """Takes the key out of a log record before any handler writes it.

    The library logs a failed model call's exception text at ERROR, through
    a handler it bound to stderr when it was imported, out of reach of
    ``redact``. An API error that echoed the key would print it there.
    """

    def __init__(self, secrets) -> None:
        super().__init__()
        self.secrets = list(secrets)

    def filter(self, record: logging.LogRecord) -> bool:
        message = record.getMessage()
        cleaned = redact(message, self.secrets)
        if record.exc_info:
            trace = "".join(traceback.format_exception(*record.exc_info))
            if redact(trace, self.secrets) != trace:
                cleaned += " | " + redact(repr(record.exc_info[1]), self.secrets)
                record.exc_info, record.exc_text = None, None
        if cleaned != message:
            record.msg, record.args = cleaned, ()
        return True


def _redact_logs(secrets) -> None:
    """Put the filter on every handler that exists: the root's, every
    logger's (the library's among them), and logging's last resort."""
    redactor = _RedactingFilter(secrets)
    handlers = set(logging.getLogger().handlers)
    for candidate in logging.Logger.manager.loggerDict.values():
        if isinstance(candidate, logging.Logger):
            handlers.update(candidate.handlers)
    if logging.lastResort is not None:
        handlers.add(logging.lastResort)
    for handler in handlers:
        handler.addFilter(redactor)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog=TOOL_NAME,
        description=(
            "Fetch each URL (robots.txt honoured, TLS verified, bounded), ask Claude through "
            "ScrapeGraphAI's SmartScraperGraph to extract what the prompt asks for in the "
            "shape the schema gives, and write a proposal marking every value found, or not "
            "found, verbatim in the page."
        ),
    )
    parser.add_argument(
        "urls", nargs="+", metavar="URL", help=f"http(s) pages, at most {MAX_URLS}"
    )
    parser.add_argument(
        "--prompt", required=True, help="what to extract, in plain language"
    )
    schema = parser.add_mutually_exclusive_group(required=True)
    schema.add_argument(
        "--schema", metavar="FILE.json", help="a JSON Schema for the answer"
    )
    schema.add_argument(
        "--pydantic-model",
        metavar="FILE.py:CLASS",
        help="a Pydantic model whose JSON Schema is used (the file is executed)",
    )
    parser.add_argument(
        "--output",
        required=True,
        metavar="FILE.json",
        help="where to write the proposal; must not exist",
    )
    parser.add_argument(
        "--model",
        default=None,
        help=f"Claude model id; if absent, {extract.MODEL_VARIABLE} names it. There is no "
        "default, and a run with neither is refused",
    )
    parser.add_argument(
        "--temperature",
        type=float,
        default=None,
        help="send temperature 0 (the only value accepted). Without it no temperature is "
        "sent. Give it only for a model whose API accepts a temperature: some reject any "
        "temperature with HTTP 400",
    )
    parser.add_argument(
        "--allow-unverified",
        action="store_true",
        help="exit 0 even if a value was not found verbatim (it stays marked)",
    )
    parser.add_argument(
        "--allow-absent-robots",
        action="store_true",
        help="fetch from a host that has no robots.txt (recorded per page)",
    )
    parser.add_argument(
        "--max-page-bytes",
        type=int,
        default=fetch.FetchLimits.max_page_bytes,
        help=f"refuse a page larger than this (ceiling {fetch.CEILINGS['max_page_bytes']})",
    )
    parser.add_argument(
        "--max-text-chars",
        type=int,
        default=DEFAULT_MAX_TEXT_CHARS,
        help=f"refuse a page whose text is longer (ceiling {MAX_TEXT_CHARS_CEILING})",
    )
    parser.add_argument(
        "--model-timeout",
        type=float,
        default=extract.DEFAULT_MODEL_TIMEOUT,
        help=f"seconds per model request (ceiling {extract.MAX_MODEL_TIMEOUT})",
    )
    parser.add_argument(
        "--version", action="version", version=f"{TOOL_NAME} {__version__}"
    )
    return parser


def _plan(args) -> tuple[fetch.FetchLimits, extract.ModelSettings, str, Path]:
    # Python turns an argument that is not UTF-8 into lone surrogates, which
    # no proposal can hold: refused here, before anything is fetched or paid
    # for. The output path is the one argument the proposal does not record.
    for name, value in sorted(vars(args).items()):
        if name == "output":
            continue
        for item in value if isinstance(value, list) else [value]:
            if isinstance(item, str) and verify.lone_surrogate(item) is not None:
                raise UsageRefused(
                    f"the {name} argument is not valid UTF-8 ({item!r}); it would reach "
                    "the proposal as something it does not say"
                )
    if len(args.urls) > MAX_URLS:
        raise UsageRefused(
            f"{len(args.urls)} URLs given; at most {MAX_URLS} are read per run"
        )
    if len(set(args.urls)) != len(args.urls):
        raise UsageRefused("a URL is given twice; name each page once")
    for url in args.urls:
        try:
            fetch.check_url(url)
        except fetch.FetchRefused as refusal:
            raise UsageRefused(str(refusal)) from None
    if not args.prompt.strip():
        raise UsageRefused("the prompt is empty")
    if len(args.prompt) > MAX_PROMPT_CHARS:
        raise UsageRefused(f"the prompt is longer than {MAX_PROMPT_CHARS} characters")
    if not 0 < args.max_text_chars <= MAX_TEXT_CHARS_CEILING:
        raise UsageRefused(
            f"--max-text-chars must be above 0 and at most {MAX_TEXT_CHARS_CEILING}; "
            f"{args.max_text_chars} is refused, not adjusted"
        )
    output = Path(args.output)
    if output.exists():
        raise UsageRefused(
            f"{output} exists; a proposal is never written over another file"
        )
    if not output.parent.is_dir():
        raise UsageRefused(f"{output.parent} is not a directory")
    try:
        model_id, model_source = extract.resolve_model_id(args.model, os.environ)
        limits = fetch.FetchLimits(max_page_bytes=args.max_page_bytes)
        settings = extract.ModelSettings(
            model_id=model_id,
            endpoint=extract.model_endpoint(os.environ),
            timeout=args.model_timeout,
            temperature=args.temperature,
        )
    except ValueError as error:
        raise UsageRefused(str(error)) from None
    # Last, once every check that touches nothing has passed.
    refusal = _hard_link_refusal(output)
    if refusal is not None:
        raise UsageRefused(
            f"{refusal}; refused before anything is fetched or the model is asked"
        )
    return limits, settings, model_source, output


def _hard_link_refusal(output: Path) -> str | None:
    """Why the proposal could not be put in place beside ``output``, or None.

    The proposal is linked into place (``_write_whole``), which is what keeps
    it from ever replacing a file, and FAT and some network mounts have no
    hard links. Found out at the end of a run, that was found out after every
    page had been fetched and the model paid for, and the run exited 4. So a
    file is made beside ``output``, linked, and both are removed, first: in a
    directory of their own, made for the check, so that no name the check
    uses can be a file somebody else already has.
    """
    directory = output.parent
    try:
        workspace = tempfile.mkdtemp(
            prefix=f".{output.name}.", suffix=".link-check", dir=directory
        )
    except OSError as error:
        return f"{directory} would not take a file ({error.strerror})"
    try:
        probe = os.path.join(workspace, "probe")
        with open(probe, "xb"):
            pass
        try:
            os.link(probe, os.path.join(workspace, "link"))
        except OSError as error:
            return (
                f"{directory} does not allow a hard link ({error.strerror}), and the "
                "proposal is put in place with one so that it can never replace a file"
            )
    except OSError as error:
        return f"{directory} would not take a file ({error.strerror})"
    finally:
        shutil.rmtree(workspace, ignore_errors=True)
    return None


def _version(distribution: str) -> str | None:
    try:
        return importlib.metadata.version(distribution)
    except importlib.metadata.PackageNotFoundError:
        return None


def _counts(values: list[dict]) -> dict:
    counts = {"verified": 0, "unverified": 0, "absent": 0}
    for value in values:
        counts[value["status"]] += 1
    return counts


def _fetch_all(
    urls, fetcher: fetch.Fetcher, max_text_chars: int
) -> list[tuple[dict, str | None]]:
    sources: list[tuple[dict, str | None]] = []
    for url in urls:
        entry: dict[str, Any] = {"url": url}
        try:
            page = fetcher.fetch(url)
        except fetch.FetchRefused as refusal:
            entry.update(
                outcome="refused",
                stage="fetch",
                reason=refusal.reason,
                http_status=refusal.status,
                robots=[verdict.as_record() for verdict in refusal.robots],
            )
            _say(f"refused {url}: {refusal.reason}")
            sources.append((entry, None))
            continue
        entry.update(
            final_url=page.final_url,
            redirects=list(page.redirects),
            fetched_at=page.fetched_at,
            http_status=page.http_status,
            media_type=page.media_type,
            headers=page.headers,
            bytes=len(page.body),
            sha256=page.sha256,
            proxy=page.proxy,
            robots=[verdict.as_record() for verdict in page.robots],
        )
        _say(
            f"fetched {url}: HTTP {page.http_status}, {len(page.body)} bytes, sha256 {page.sha256}"
        )
        try:
            decoded, charset = verify.decode_body(
                page.body, page.media_type, page.charset
            )
            text = verify.page_text(decoded, page.media_type)
        except verify.Refused as refusal:
            entry.update(
                outcome="refused", stage="decode", reason=f"the page {refusal}"
            )
            _say(f"refused {url}: the page {refusal}")
            sources.append((entry, None))
            continue
        entry.update(
            charset=charset, text_chars=len(text), text_sha256=verify.sha256_text(text)
        )
        if not text.strip():
            reason = "the page has no text to extract from"
        elif len(text) > max_text_chars:
            reason = (
                f"the page's text is {len(text)} characters, more than the {max_text_chars} "
                "allowed; refused rather than chunked, because the library's merge of "
                "per-chunk answers can return a value that is in no chunk"
            )
        else:
            reason = None
        if reason:
            entry.update(outcome="refused", stage="decode", reason=reason)
            _say(f"refused {url}: {reason}")
            sources.append((entry, None))
            continue
        sources.append((entry, text))
    return sources


def _extract_one(
    entry: dict,
    text: str,
    *,
    model,
    recorder,
    args,
    schema: dict,
    settings: extract.ModelSettings,
    secrets,
) -> None:
    try:
        answer = extract.ask(
            model,
            recorder,
            prompt=args.prompt,
            text=text,
            schema=schema,
            library_timeout=settings.library_timeout,
        )
        entry.update(
            model_served=answer.served_model,
            stop_reason=answer.stop_reason,
            usage=answer.usage,
        )
        parsed = verify.parse_answer(answer.raw)
        as_plain = verify.plain(parsed)
        if as_plain != answer.library_answer:
            raise verify.Refused(
                "the library's reading of the answer differs from the model's raw reply; "
                "the raw reply is what is checked, and two readings that disagree are "
                f"refused. The library read: {answer.library_answer!r}"
            )
        verify.validate(as_plain, schema)
        values = verify.check_values(parsed, verify.fold(text))
    except (extract.ExtractionFailed, verify.Refused) as refusal:
        # Redacted whole, then shortened, never the other way round.
        reason = _bounded(redact(str(refusal), secrets))
        entry.update(outcome="refused", stage="extract", reason=reason)
        _say(f"refused {entry['url']}: {reason}")
        return
    counts = _counts(values)
    entry.update(outcome="extracted", extracted=as_plain, values=values, counts=counts)
    _say(
        f"extracted {entry['url']}: {counts['verified']} verified, {counts['unverified']} "
        f"not found verbatim, {counts['absent']} absent"
    )


def _write_whole(output: Path, text: str) -> str | None:
    """Write ``text`` to ``output`` whole or not at all, and never over a file.

    The text is encoded before anything is created, so text no UTF-8 file can
    hold refuses the write instead of stopping it halfway: a lone surrogate
    once left a 0-byte proposal behind. The bytes go to a temporary file
    beside ``output`` and are synced, and only then linked into place;
    ``os.link`` fails if ``output`` exists, where a rename would replace it.
    Returns why nothing was written, or None.
    """
    try:
        data = text.encode("utf-8")
    except UnicodeEncodeError as error:
        return (
            f"the proposal holds {error.object[error.start : error.end]!r} at character "
            f"{error.start}, a lone surrogate, which is not Unicode and which no UTF-8 "
            "file can hold"
        )
    try:
        descriptor, temporary = tempfile.mkstemp(
            prefix=f".{output.name}.", suffix=".partial", dir=output.parent
        )
    except OSError as error:
        return f"{output.parent} would not take a temporary file: {error.strerror}"
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.link(temporary, output)
    except FileExistsError:
        return (
            f"{output} appeared while the run was in progress, and a proposal is never "
            "written over another file"
        )
    except OSError as error:
        return f"{output} could not be written: {error.strerror}"
    finally:
        with contextlib.suppress(OSError):
            os.unlink(temporary)
    return None


def main(argv: list[str] | None = None, *, model=None, address_policy=None) -> int:
    """Run the tool, and give every way a run can end its own exit code.

    ``model`` and ``address_policy`` are for tests, and the command line can
    set neither. ``model`` is a LangChain chat model to use instead of
    ChatAnthropic, in which case no key is read and the fence admits no host
    at all. ``address_policy`` stands in for ``fetch.address_refusal``,
    because the tests' local servers listen on 127.0.0.1.
    """
    secrets: list[str] = []
    try:
        return _run(argv, model=model, address_policy=address_policy, secrets=secrets)
    except (guard.GuardRefusal, guard.ForbiddenComponent) as refusal:
        _say(f"refused: {redact(str(refusal), secrets)}; no proposal is written")
        return EXIT_GUARD
    # Blind on purpose: whatever failed, the code says the run failed. An
    # uncaught exception exits 1, the code for "a proposal was written".
    except Exception:  # noqa: BLE001
        _say(
            redact(traceback.format_exc(), secrets).rstrip()
            + "\nfailed: an unexpected error ended the run, and no proposal was "
            f"written (exit {EXIT_INTERNAL})"
        )
        return EXIT_INTERNAL


def _run(argv, *, model, address_policy, secrets: list[str]) -> int:
    parser = build_parser()
    try:
        args = parser.parse_args(argv)
    except SystemExit as exit_:
        return int(exit_.code or 0)
    try:
        limits, settings, model_source, output = _plan(args)
    except UsageRefused as refusal:
        _say(f"refused: {refusal}")
        return EXIT_USAGE

    original_home = os.environ.get("HOME")
    try:
        report = guard.prepare()
    except guard.GuardRefusal as refusal:
        _say(f"refused: {refusal}")
        return EXIT_GUARD
    started_at = fetch.utc_now()

    try:
        if args.schema is not None:
            schema, schema_source = verify.load_json_schema(args.schema), args.schema
        else:
            schema, schema_source = verify.load_pydantic_schema(args.pydantic_model)
    except verify.Refused as refusal:
        _say(f"refused: {refusal}")
        return EXIT_USAGE

    recorder = extract.new_recorder()
    if model is None:
        try:
            api_key = extract.read_api_key(os.environ, original_home)
        except extract.KeyRefused as refusal:
            _say(f"refused: {refusal}")
            return EXIT_USAGE
        secrets.append(api_key)
        chat = extract.build_model(settings, api_key, recorder)
        # After the model client is built, so that every library's handlers exist.
        _redact_logs(secrets)
        hosts = extract.fence_hosts(settings.endpoint, os.environ)
        model_record = {
            **settings.record(),
            "id_source": model_source,
            "client": f"langchain-anthropic {_version('langchain-anthropic')}",
        }
    else:
        chat = model
        extract.attach_recorder(chat, recorder)
        hosts = frozenset()
        model_record = {
            **settings.record(),
            "id_source": model_source,
            "provider": "injected for a test",
            "client": type(model).__name__,
            "endpoint": None,
        }

    fetcher = fetch.Fetcher(
        limits,
        allow_absent_robots=args.allow_absent_robots,
        **({} if address_policy is None else {"address_policy": address_policy}),
    )
    sources = _fetch_all(args.urls, fetcher, args.max_text_chars)

    try:
        with guard.network_fence(hosts) as attempts:
            for entry, text in sources:
                if text is not None:
                    _extract_one(
                        entry,
                        text,
                        model=chat,
                        recorder=recorder,
                        args=args,
                        schema=schema,
                        settings=settings,
                        secrets=secrets,
                    )
    except (guard.GuardRefusal, guard.ForbiddenComponent) as refusal:
        _say(f"refused: {redact(str(refusal), secrets)}; no proposal is written")
        return EXIT_GUARD
    if attempts:
        _say(
            f"refused: the network fence stopped {len(attempts)} connection attempt(s) while "
            f"the library ran ({'; '.join(attempts[:5])}); no proposal is written"
        )
        return EXIT_GUARD

    entries = [entry for entry, _text in sources]
    refused = [entry for entry in entries if entry["outcome"] == "refused"]
    totals = {"verified": 0, "unverified": 0, "absent": 0}
    for entry in entries:
        for status, count in entry.get("counts", {}).items():
            totals[status] += count
    if refused:
        code = EXIT_FINDINGS
        reason = f"{len(refused)} of {len(entries)} page(s) refused; see each source's reason"
    elif totals["unverified"] and not args.allow_unverified:
        code = EXIT_FINDINGS
        reason = (
            f"{totals['unverified']} value(s) not found verbatim; each is marked unverified. "
            "Rerun with --allow-unverified to accept the proposal with them marked"
        )
    else:
        code = EXIT_OK
        reason = (
            "every page read; every value found verbatim"
            if not totals["unverified"]
            else f"{totals['unverified']} value(s) not found verbatim, accepted by "
            "--allow-unverified and marked unverified"
        )

    proposal = {
        "kind": PROPOSAL_KIND,
        "format": 1,
        "notice": NOTICE,
        "tool": {
            "name": TOOL_NAME,
            "version": __version__,
            "python": platform.python_version(),
            "scrapegraphai": report.library_version,
            "langchain_anthropic": _version("langchain-anthropic"),
            "langchain_core": _version("langchain-core"),
            "httpx": _version("httpx"),
            "jsonschema": _version("jsonschema"),
        },
        "run": {
            "started_at": started_at,
            "finished_at": fetch.utc_now(),
            "allow_unverified": args.allow_unverified,
            "allow_absent_robots": args.allow_absent_robots,
            "user_agent": fetch.USER_AGENT,
            "limits": {
                "max_urls": MAX_URLS,
                "max_page_bytes": limits.max_page_bytes,
                "max_text_chars": args.max_text_chars,
                "connect_timeout_seconds": limits.connect_timeout,
                "read_timeout_seconds": limits.read_timeout,
                "total_fetch_seconds": limits.total_timeout,
                "max_redirects": limits.max_redirects,
            },
        },
        "guard": {
            "telemetry_switch": f"{guard.TELEMETRY_SWITCH}={report.telemetry_switch_value}",
            "telemetry_on_at_import": report.telemetry_on_at_import,
            "telemetry_on": report.telemetry_on_after_disable,
            "library_config_file_in_private_home": True,
            "langsmith_tracing": report.langsmith_tracing,
            "graph": "SmartScraperGraph, html_mode: FetchNode -> GenerateAnswerNode, on supplied text",
            "network_fence_hosts": sorted(hosts),
            "network_fence_refusals": 0,
        },
        "model": model_record,
        "prompt": args.prompt,
        "extraction_instruction": extract.EXTRACTION_INSTRUCTION,
        "schema_source": schema_source,
        "schema_sha256": hashlib.sha256(
            json.dumps(schema, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest(),
        "schema": schema,
        "normalisation": verify.NORMALISATION,
        "sources": entries,
        "verdict": {
            "exit_code": code,
            "reason": reason,
            "pages": len(entries),
            "pages_extracted": len(entries) - len(refused),
            "pages_refused": len(refused),
            "values": totals,
        },
    }
    text = redact(json.dumps(proposal, indent=2, ensure_ascii=False) + "\n", secrets)
    unwritten = _write_whole(output, text)
    if unwritten is not None:
        _say(f"refused: {unwritten}; nothing was written to {output}")
        return EXIT_OUTPUT
    _say(f"wrote {output}: {reason}")
    return code
