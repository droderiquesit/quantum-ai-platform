"""The model, the key, and the one graph this tool runs.

The model is Claude through ``langchain-anthropic``, and the operator names
it: ``--model``, or else the ``QIP_SCRAPER_MODEL`` environment variable.
There is no default, so the repository names no model and the choice is the
operator's, recorded in every proposal with where it came from.

No temperature is sent unless the operator asks for ``--temperature 0``.
Some Claude models reject any temperature with HTTP 400, so sending one by
default would break them, and a list of which ones do would be a list of
model ids to keep current. The proposal records what was sent. A temperature
of 0 never made a hosted model's answer reproducible anyway; the verbatim
check is the control that does not depend on the model behaving.

A reply may hold thinking blocks before its text; only the text is read.

Nothing here imports LangChain or ScrapeGraphAI at module level, and the
functions that do refuse to run until ``guard.prepare()`` has completed.
"""

from __future__ import annotations

import dataclasses
import json
import re
import urllib.parse
from collections.abc import Mapping
from typing import Any

from . import fetch, guard

#: Names the model when ``--model`` is not given. Read by ``resolve_model_id``
#: only, from the mapping the command line passes in.
MODEL_VARIABLE = "QIP_SCRAPER_MODEL"
DEFAULT_ENDPOINT = "https://api.anthropic.com"
#: Room for adaptive thinking plus a JSON answer, and still short enough for a
#: non-streaming request to finish inside the client's timeout.
MAX_TOKENS = 16_000
DEFAULT_MODEL_TIMEOUT = 300.0
MAX_MODEL_TIMEOUT = 900.0
#: The SDK's own default: connection errors, 408, 409, 429 and 5xx are
#: retried, so the wall-clock bound is timeout x (retries + 1).
MAX_RETRIES = 2
#: The library refuses a model instance without this. Its only reader is
#: ParseNode's chunker, which html_mode keeps off the graph, so the number is
#: stated for the record and decides nothing.
LIBRARY_MODEL_TOKENS = 200_000

API_KEY_VARIABLE = "ANTHROPIC_API_KEY"
API_KEY_FILE_VARIABLE = API_KEY_VARIABLE + "_FILE"
MAX_KEY_FILE_BYTES = 4096
_MODEL_ID = re.compile(r"claude-[a-z0-9]+(?:-[a-z0-9]+)*")
_LOOPBACK = frozenset({"127.0.0.1", "::1", "localhost"})

TEMPERATURE_SENT = "sent, because --temperature 0 was given"
TEMPERATURE_NOT_SENT = (
    "not sent, because no --temperature was given: the model's default sampling "
    "applies and the answer is not reproducible; the verbatim check, not the "
    "sampling, is what this proposal relies on"
)

#: Prepended by the library to its own prompt template, so it must contain
#: no braces: the template is formatted with str.format.
EXTRACTION_INSTRUCTION = (
    "Copy every value exactly as the website content writes it: the same "
    "characters, digits, signs, separators, units and capitalisation. Never "
    "calculate, convert, round, total, translate or reformat a value.\n"
)


class KeyRefused(Exception):
    """The API key could not be read under the platform's _FILE rule."""


class ExtractionFailed(Exception):
    """The graph did not produce one complete answer this tool will read."""


def resolve_model_id(flag: str | None, environ: Mapping[str, str]) -> tuple[str, str]:
    """The model id and where it came from: ``--model`` first, then the variable.

    Neither set is refused, because there is no default to fall back on. A
    variable that is set but empty is refused too, rather than read as unset:
    an empty value is never what was meant.
    """
    if flag is not None:
        return flag, "--model"
    value = environ.get(MODEL_VARIABLE)
    if value is None:
        raise ValueError(
            f"no model: neither --model nor {MODEL_VARIABLE} is set, and there is "
            "no default"
        )
    if not value.strip():
        raise ValueError(
            f"{MODEL_VARIABLE} is set but empty; set it to a Claude model id or "
            "pass --model"
        )
    return value, MODEL_VARIABLE


def model_endpoint(environ: Mapping[str, str]) -> str:
    """The Messages API base URL: ANTHROPIC_BASE_URL if set, else Anthropic's.

    Recorded in the proposal. Clear-text http is refused except to a loopback
    address, because the API key travels in a header.
    """
    value = environ.get("ANTHROPIC_BASE_URL") or DEFAULT_ENDPOINT
    parts = urllib.parse.urlsplit(value)
    if parts.username is not None or parts.password is not None:
        raise ValueError(
            "ANTHROPIC_BASE_URL carries credentials in its userinfo; refused"
        )
    if parts.scheme == "https" and parts.hostname:
        return value.rstrip("/")
    if parts.scheme == "http" and parts.hostname in _LOOPBACK:
        return value.rstrip("/")
    raise ValueError(
        f"ANTHROPIC_BASE_URL={value!r} would send the API key somewhere other than an "
        "https endpoint or a loopback address; refused"
    )


def fence_hosts(endpoint: str, environ: Mapping[str, str]) -> frozenset[str]:
    """The hosts the network fence admits while the library runs.

    The model's host, and the proxy that reaches it when the environment
    names one for it (``HTTPS_PROXY`` unless ``NO_PROXY`` bypasses the host,
    or ``ANTHROPIC_PROXY``, which langchain-anthropic reads).
    """
    hosts = {urllib.parse.urlsplit(endpoint).hostname or ""}
    for proxy in (
        fetch.select_proxy(endpoint, environ),
        environ.get("ANTHROPIC_PROXY"),
    ):
        if proxy:
            hosts.add(urllib.parse.urlsplit(proxy).hostname or "")
    return frozenset(host for host in hosts if host)


def read_api_key(environ: Mapping[str, str], home: str | None = None) -> str:
    """The API key, from ANTHROPIC_API_KEY or the file ANTHROPIC_API_KEY_FILE names.

    The contract is ``qip_core::secret``'s: one source or the other, never
    both; a file's trailing whitespace is stripped; an empty value is
    refused, not treated as absent. No message here contains the key.
    ``home`` expands a leading ``~`` in the file's path, and is the home the
    process had before the guard replaced it.
    """
    direct = environ.get(API_KEY_VARIABLE)
    path = environ.get(API_KEY_FILE_VARIABLE)
    if direct is not None and path is not None:
        raise KeyRefused(
            f"{API_KEY_VARIABLE} and {API_KEY_FILE_VARIABLE} are both set. They can "
            "disagree, and which one wins would be an accident of this code rather than "
            "a decision anybody made; set exactly one"
        )
    if path is not None:
        if home and (path == "~" or path.startswith("~/")):
            path = home + path[1:]
        try:
            with open(path, "rb") as handle:
                raw = handle.read(MAX_KEY_FILE_BYTES + 1)
        except OSError as error:
            raise KeyRefused(
                f"{API_KEY_FILE_VARIABLE} names {path}, which could not be read: {error.strerror}"
            ) from None
        if len(raw) > MAX_KEY_FILE_BYTES:
            raise KeyRefused(
                f"{API_KEY_FILE_VARIABLE} names {path}, which is larger than "
                f"{MAX_KEY_FILE_BYTES} bytes; that is not an API key"
            )
        try:
            value = raw.decode("utf-8").rstrip()
        except UnicodeDecodeError:
            raise KeyRefused(
                f"{API_KEY_FILE_VARIABLE} names {path}, which is not UTF-8"
            ) from None
        if not value:
            raise KeyRefused(
                f"{API_KEY_FILE_VARIABLE} names {path}, which is empty. An empty credential "
                "is not a credential"
            )
        return value
    if direct is not None:
        if not direct.strip():
            raise KeyRefused(f"{API_KEY_VARIABLE} is set but empty")
        return direct
    raise KeyRefused(
        f"no API key: set {API_KEY_VARIABLE}, or {API_KEY_FILE_VARIABLE} to a file holding it"
    )


@dataclasses.dataclass(frozen=True)
class ModelSettings:
    #: No default: the operator names the model (``resolve_model_id``).
    model_id: str
    endpoint: str = DEFAULT_ENDPOINT
    timeout: float = DEFAULT_MODEL_TIMEOUT
    max_retries: int = MAX_RETRIES
    max_tokens: int = MAX_TOKENS
    #: None sends no temperature at all. 0 is the only value accepted: the
    #: flag exists to pin sampling, not to tune it.
    temperature: float | None = None

    def __post_init__(self) -> None:
        if not _MODEL_ID.fullmatch(self.model_id):
            raise ValueError(f"{self.model_id!r} is not a Claude model id (claude-...)")
        if not 0 < self.timeout <= MAX_MODEL_TIMEOUT:
            raise ValueError(
                f"the model timeout must be above 0 and at most {MAX_MODEL_TIMEOUT}s; "
                f"{self.timeout!r} is refused, not adjusted"
            )
        if self.temperature is not None and self.temperature != 0:
            raise ValueError(
                f"--temperature accepts only 0, which pins sampling; {self.temperature!r} "
                "is refused, not adjusted"
            )

    @property
    def library_timeout(self) -> int:
        """The library's own timeout, longer than the client's worst case.

        GenerateAnswerNode reads the clock only after a call returns and then
        discards a slow answer as a timeout, so it must never be the shorter
        of the two.
        """
        return int(self.timeout * (self.max_retries + 1)) + 60

    def record(self) -> dict:
        """The model as the proposal records it: what was sent, not what might be."""
        return {
            "provider": "anthropic",
            "id": self.model_id,
            "endpoint": self.endpoint,
            "temperature": self.temperature,
            "temperature_note": TEMPERATURE_SENT
            if self.temperature is not None
            else TEMPERATURE_NOT_SENT,
            "max_tokens": self.max_tokens,
            "timeout_seconds": self.timeout,
            "max_retries": self.max_retries,
        }


def _require_guard() -> None:
    if not guard.prepared():
        raise guard.GuardRefusal(
            "the library may be used only after qip_scraper.guard.prepare() has completed"
        )


def new_recorder():
    """A LangChain callback that keeps the raw reply of every model call."""
    from langchain_core.callbacks import BaseCallbackHandler

    class AnswerRecorder(BaseCallbackHandler):
        def __init__(self) -> None:
            super().__init__()
            self.generations: list = []
            self.errors: list = []

        def reset(self) -> None:
            self.generations.clear()
            self.errors.clear()

        def on_llm_end(self, response, **kwargs) -> None:
            for batch in response.generations:
                self.generations.extend(batch)

        def on_llm_error(self, error, **kwargs) -> None:
            self.errors.append(error)

    return AnswerRecorder()


def build_model(settings: ModelSettings, api_key: str, recorder):
    """ChatAnthropic, configured as the proposal records it."""
    _require_guard()
    from langchain_anthropic import ChatAnthropic
    from pydantic import SecretStr

    return ChatAnthropic(
        model=settings.model_id,
        api_key=SecretStr(api_key),
        base_url=settings.endpoint,
        timeout=settings.timeout,
        max_retries=settings.max_retries,
        max_tokens=settings.max_tokens,
        temperature=settings.temperature,
        streaming=False,
        callbacks=[recorder],
    )


def attach_recorder(model, recorder) -> None:
    """Add the recorder to a model built elsewhere (the tests' fakes)."""
    model.callbacks = [*(model.callbacks or []), recorder]


def schema_model(schema: dict):
    """A Pydantic class that carries ``schema`` into the library unchanged.

    The library accepts only a Pydantic class and uses it for one thing: the
    JSON Schema it pastes into the prompt. This class answers with the
    operator's schema verbatim, so the schema the model is shown is the
    schema the answer is validated against.
    """
    _require_guard()
    from pydantic import BaseModel

    frozen = json.dumps(schema)

    class ProposalSchema(BaseModel):
        @classmethod
        def model_json_schema(cls, *args, **kwargs):
            return json.loads(frozen)

    return ProposalSchema


ALLOWED_CONFIG_KEYS = frozenset(
    {"llm", "html_mode", "verbose", "headless", "additional_info", "timeout"}
)


def graph_config(model, library_timeout: int) -> dict:
    """The only configuration the graph is given.

    Nothing that opens a path (``browser_base``, ``scrape_do``, ``plasmate``,
    ``loader_kwargs``, ``storage_state``, ``cache_path``, ``burr_kwargs``) and
    nothing that adds a model call (``reattempt``, ``reasoning``) can be in
    it: the key set is checked, not assumed.
    """
    config = {
        "llm": {"model_instance": model, "model_tokens": LIBRARY_MODEL_TOKENS},
        # Fetch -> GenerateAnswer: no ParseNode, so no chunking, no merge of
        # per-chunk answers, and no tokenizer download.
        "html_mode": True,
        "verbose": False,
        "headless": True,
        "additional_info": EXTRACTION_INSTRUCTION,
        "timeout": library_timeout,
    }
    if set(config) - ALLOWED_CONFIG_KEYS or set(config["llm"]) != {
        "model_instance",
        "model_tokens",
    }:
        raise ExtractionFailed(
            "the graph configuration holds a key this tool does not allow"
        )
    return config


def check_graph(graph, model) -> None:
    """Refuse a graph that is not the one shape this tool reasons about."""
    if graph.input_key != "local_dir":
        raise ExtractionFailed(
            f"the library would treat the page as a URL to fetch (input {graph.input_key!r}); "
            "it may only read the text it is given"
        )
    names = [type(node).__name__ for node in graph.graph.nodes]
    if names != ["FetchNode", "GenerateAnswerNode"]:
        raise ExtractionFailed(
            f"the graph is {names}, not FetchNode -> GenerateAnswerNode; a ParseNode would "
            "chunk the page and merge per-chunk answers into values present in no chunk"
        )
    if graph.graph.edges != {"Fetch": "GenerateAnswer"}:
        raise ExtractionFailed(f"the graph's edges are {graph.graph.edges}")
    if graph.llm_model is not model:
        raise ExtractionFailed("the graph is not using the model this tool built")
    if getattr(graph, "model_tokens_defaulted", True) is not False:
        raise ExtractionFailed("the library fell back to a default token window")
    if getattr(graph.graph, "use_burr", False):
        raise ExtractionFailed("the graph would run through Burr")


@dataclasses.dataclass(frozen=True)
class ModelAnswer:
    raw: str
    stop_reason: str | None
    served_model: str | None
    usage: dict | None
    library_answer: Any


def ask(
    model, recorder, *, prompt: str, text: str, schema: dict, library_timeout: int
) -> ModelAnswer:
    """Run SmartScraperGraph once over ``text`` and return the model's raw reply."""
    _require_guard()
    from scrapegraphai.graphs import SmartScraperGraph

    config = graph_config(model, library_timeout)
    # The library fetches whenever its source starts with "http"; a page whose
    # text happens to begin with a URL must not flip that switch.
    source = "\n" + text
    graph = SmartScraperGraph(
        prompt=prompt, source=source, config=config, schema=schema_model(schema)
    )
    check_graph(graph, model)
    recorder.reset()
    failure: Exception | None = None
    library_answer = None
    try:
        library_answer = graph.run()
    except (guard.GuardRefusal, guard.ForbiddenComponent):
        raise
    # Blind on purpose: an API error, a parse failure or anything else the
    # library raises refuses this page, and the run goes on to the next.
    except Exception as error:  # noqa: BLE001
        failure = error
    if len(recorder.generations) > 1:
        raise ExtractionFailed(
            f"the model was called {len(recorder.generations)} times; this tool reads "
            "exactly one answer per page"
        )
    # A failure's text is passed on whole. The caller redacts the key and only
    # then shortens it: shortened first, a key across the cut left a prefix
    # that redaction, which looks for the whole key, could not find.
    if not recorder.generations:
        detail = f"{type(failure).__name__}: {failure}" if failure else "no model call"
        raise ExtractionFailed(
            f"the extraction failed before the model answered: {detail}"
        )
    # The reply the model sent is judged before what the library made of it:
    # a refusal or a cut-off answer is the cause, whatever the parser then did.
    generation = recorder.generations[0]
    message = getattr(generation, "message", None)
    raw = message.text if message is not None else generation.text
    metadata = dict(getattr(message, "response_metadata", None) or {})
    usage = getattr(message, "usage_metadata", None)
    stop = metadata.get("stop_reason")
    if stop == "max_tokens":
        raise ExtractionFailed(
            f"the answer was cut off at the {MAX_TOKENS}-token ceiling; a truncated answer "
            "is refused, not repaired"
        )
    if stop == "refusal":
        raise ExtractionFailed("the model declined to answer (stop_reason refusal)")
    if stop not in ("end_turn", "stop_sequence"):
        raise ExtractionFailed(
            f"the model stopped for {stop!r}; only a complete answer (end_turn) is read, and "
            "an unreported stop reason cannot be told from a truncated one"
        )
    if failure is not None:
        raise ExtractionFailed(
            f"the extraction failed: {type(failure).__name__}: {failure}"
        )
    return ModelAnswer(
        raw=raw,
        stop_reason=stop,
        served_model=metadata.get("model_name") or metadata.get("model"),
        usage=dict(usage) if usage else None,
        library_answer=library_answer,
    )
