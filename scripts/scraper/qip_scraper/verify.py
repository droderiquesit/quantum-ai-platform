"""What the model was shown, what it answered, and whether the answer is in the page.

Nothing here imports ScrapeGraphAI, and nothing here trusts it:

* The text the model is given is derived from the fetched bytes by one
  declared, deterministic procedure (``NORMALISATION``), and the verbatim
  check runs against exactly that text.
* The model's raw reply is parsed here, strictly. The library parses it with
  LangChain's tolerant parser, which closes a JSON document cut off mid-value
  and so turns a truncated answer into a plausible, shorter one.
* Every number keeps the spelling the model wrote. ``12.50`` is looked for as
  ``12.50``, never as Python's ``12.5``: a model may transcribe a number, not
  compute, round or reformat one (ADR 0005's rule, restated for extraction).
* The answer is validated against the JSON Schema with ``jsonschema``. The
  library only asks for the schema in the prompt.
"""

from __future__ import annotations

import codecs
import hashlib
import html.parser
import importlib.util
import json
import re
import unicodedata
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import jsonschema

#: Recorded in every proposal, so a reviewer knows exactly what "found
#: verbatim" was measured against.
NORMALISATION = (
    "bytes decoded with the charset the response declared (else the page's "
    "meta charset, else UTF-8), strictly; HTML parsed with Python's "
    "html.parser, entities decoded, script/style/noscript/template/svg/math/"
    "iframe/object/embed/canvas content dropped, block elements on their own "
    "lines, table cells separated by tabs, http(s) and relative link targets "
    "appended as <target>; Unicode NFC. Values and text are compared after "
    "folding every run of whitespace to one space; nothing else is changed, "
    "so case, punctuation, digits and separators must match exactly"
)

#: The library's own prompt tells the model to answer "NA" when it finds
#: nothing (prompts/generate_answer_node_prompts.py).
NOT_FOUND_SENTINEL = "NA"
MAX_SCHEMA_BYTES = 1024 * 1024
_CONTEXT = 40


class Refused(Exception):
    """A page, an answer or a schema this tool will not accept."""


def lone_surrogate(text: str) -> int | None:
    """Where ``text`` holds a lone surrogate, or None if it holds none.

    A lone surrogate (U+D800 to U+DFFF, unpaired) is what Python makes of a
    JSON ``\\ud800`` escape, of a non-UTF-8 command-line argument, or of a
    UTF-7 page. It is not a Unicode character and no UTF-8 file can hold
    one: written into a proposal, it stopped the write halfway.
    """
    try:
        text.encode("utf-8")
    except UnicodeEncodeError as error:
        return error.start
    return None


# --------------------------------------------------------------------------
# Bytes to the text the model sees
# --------------------------------------------------------------------------

_META_CHARSET = re.compile(
    rb"""<meta[^>]+charset\s*=\s*["']?\s*([A-Za-z0-9_.:-]+)""", re.IGNORECASE
)


def decode_body(body: bytes, media_type: str, declared: str | None) -> tuple[str, str]:
    """Decode strictly, and say which charset was used and why.

    A byte that does not decode refuses the page: replacing it would put a
    character in front of the model that the page never contained.
    """
    if body.startswith(codecs.BOM_UTF8):
        charset, source = "utf-8-sig", "byte order mark"
    elif body.startswith((codecs.BOM_UTF16_LE, codecs.BOM_UTF16_BE)):
        charset, source = "utf-16", "byte order mark"
    elif declared:
        charset, source = declared, "Content-Type header"
    else:
        sniffed = (
            _META_CHARSET.search(body[:4096]) if media_type != "text/plain" else None
        )
        if sniffed:
            charset, source = sniffed.group(1).decode("ascii"), "meta charset"
        else:
            charset, source = "utf-8", "default"
    try:
        codecs.lookup(charset)
    except LookupError:
        raise Refused(
            f"declares the charset {charset!r} ({source}), which Python does not know"
        ) from None
    try:
        decoded = body.decode(charset)
    except UnicodeDecodeError as error:
        raise Refused(
            f"does not decode as {charset} ({source}) at byte {error.start}; refused rather "
            "than shown to the model with replacement characters"
        ) from None
    # codecs.lookup also finds codecs that are not text encodings (zlib,
    # base64, rot13), and decoding with one raises LookupError; some text
    # codecs raise a bare UnicodeError. Either ended the run with a traceback.
    except (LookupError, UnicodeError) as error:
        raise Refused(
            f"declares the charset {charset!r} ({source}), which does not decode text "
            f"({type(error).__name__})"
        ) from None
    offset = lone_surrogate(decoded)
    if offset is not None:
        raise Refused(
            f"decodes as {charset} ({source}) to a lone surrogate at character {offset}, "
            "which is not a Unicode character"
        )
    return decoded, f"{charset} ({source})"


_SKIPPED = frozenset(
    {
        "script",
        "style",
        "noscript",
        "template",
        "svg",
        "math",
        "iframe",
        "object",
        "embed",
        "canvas",
    }
)
_VOID = frozenset(
    {
        "area",
        "base",
        "br",
        "col",
        "embed",
        "hr",
        "img",
        "input",
        "link",
        "meta",
        "param",
        "source",
        "track",
        "wbr",
    }
)
_BLOCKS = frozenset(
    {
        "address",
        "article",
        "aside",
        "blockquote",
        "br",
        "caption",
        "dd",
        "details",
        "dialog",
        "div",
        "dl",
        "dt",
        "fieldset",
        "figcaption",
        "figure",
        "footer",
        "form",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "header",
        "hgroup",
        "hr",
        "legend",
        "li",
        "main",
        "nav",
        "ol",
        "option",
        "p",
        "pre",
        "section",
        "summary",
        "table",
        "tbody",
        "tfoot",
        "thead",
        "title",
        "tr",
        "ul",
    }
)
_CELLS = frozenset({"td", "th"})
_NO_LINK_SCHEMES = (
    "javascript:",
    "data:",
    "mailto:",
    "tel:",
    "vbscript:",
    "about:",
    "blob:",
)
MAX_LINK_CHARS = 2048


def _link_target(href: str | None) -> str | None:
    if not href:
        return None
    href = href.strip()
    lowered = href.lower()
    if (
        not href
        or href.startswith("#")
        or len(href) > MAX_LINK_CHARS
        or lowered.startswith(_NO_LINK_SCHEMES)
    ):
        return None
    return href


class _TextExtractor(html.parser.HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.parts: list[str] = []
        self.skipping = 0
        self.links: list[str | None] = []

    def handle_starttag(self, tag, attrs):
        if tag in _SKIPPED:
            if tag not in _VOID:
                self.skipping += 1
            return
        if self.skipping:
            return
        if tag in _BLOCKS:
            self.parts.append("\n")
        elif tag in _CELLS:
            self.parts.append("\t")
        if tag == "a":
            self.links.append(_link_target(dict(attrs).get("href")))

    def handle_endtag(self, tag):
        if tag in _SKIPPED:
            self.skipping = max(0, self.skipping - 1)
            return
        if self.skipping:
            return
        if tag == "a" and self.links:
            target = self.links.pop()
            if target:
                self.parts.append(f" <{target}>")
        if tag in _BLOCKS:
            self.parts.append("\n")

    def handle_data(self, data):
        if not self.skipping:
            self.parts.append(data)


def _tidy(text: str) -> str:
    text = unicodedata.normalize("NFC", text.replace("\r\n", "\n").replace("\r", "\n"))
    lines = []
    for line in text.split("\n"):
        cells = [" ".join(cell.split()) for cell in line.split("\t")]
        line = "\t".join(cell for cell in cells if cell)
        if line:
            lines.append(line)
    return "\n".join(lines)


def page_text(decoded: str, media_type: str) -> str:
    """The text the model is shown, derived from the decoded page."""
    if media_type == "text/plain":
        return _tidy(decoded)
    extractor = _TextExtractor()
    extractor.feed(decoded)
    extractor.close()
    return _tidy("".join(extractor.parts))


def fold(text: str) -> str:
    """Every run of whitespace to one space, after NFC; nothing else."""
    return " ".join(unicodedata.normalize("NFC", text).split())


def sha256_text(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


# --------------------------------------------------------------------------
# The model's answer, parsed strictly
# --------------------------------------------------------------------------


class NumberLiteral:
    """A JSON number that remembers how the model spelled it."""

    __slots__ = ("spelling", "value")

    def __init__(self, spelling: str, value: float) -> None:
        self.spelling = spelling
        self.value = value

    def __repr__(self) -> str:
        return f"NumberLiteral({self.spelling!r})"


def _refuse_constant(name: str):
    raise Refused(f"the answer contains {name}, which is not JSON")


def _no_duplicate_keys(pairs):
    seen: dict[str, Any] = {}
    for key, value in pairs:
        if key in seen:
            raise Refused(
                f"the answer repeats the key {key!r}; which value was meant is not knowable"
            )
        seen[key] = value
    return seen


def _unfenced(text: str) -> str:
    """The inside of one code fence around the whole of ``text``, else ``text``.

    The fence is ```` ``` ```` or ```` ```json ````, then a newline; the
    closing fence may follow a newline and spaces or tabs. Written out by
    hand: the regular expression this replaced was quadratic in a run of
    spaces after the opening fence, and the reply is the model's, which the
    page it reads can steer.
    """
    if not (text.startswith("```") and text.endswith("```")):
        return text
    newline = text.find("\n")
    if newline < 0 or text[3:newline].rstrip(" \t") not in ("", "json"):
        return text
    return text[newline + 1 : len(text) - 3].rstrip(" \t").removesuffix("\n")


def parse_answer(raw: str) -> Any:
    """Parse the model's reply as exactly one JSON document, or refuse.

    One code fence around the whole reply is removed, because that changes no
    value. Anything else that is not strict JSON is refused: trailing text, a
    document cut short, NaN, a repeated key, a lone surrogate.
    """
    text = _unfenced(raw.strip()).strip()
    if not text:
        raise Refused("the model's answer is empty")
    try:
        parsed = json.loads(
            text,
            parse_float=lambda spelling: NumberLiteral(spelling, float(spelling)),
            parse_int=lambda spelling: NumberLiteral(spelling, int(spelling)),
            parse_constant=_refuse_constant,
            object_pairs_hook=_no_duplicate_keys,
        )
    except json.JSONDecodeError as error:
        raise Refused(
            f"the model's answer is not one strict JSON document ({error.msg} at character "
            f"{error.pos}); the library's tolerant parser may have repaired it, this tool does not"
        ) from None
    if lone_surrogate(json.dumps(plain(parsed), ensure_ascii=False)) is not None:
        raise Refused(
            "the model's answer holds a lone surrogate (an unpaired \\ud800-style escape), "
            "which is not a Unicode character and which no proposal file can hold"
        )
    return parsed


def plain(value: Any) -> Any:
    """The parsed answer with every number as a Python number."""
    if isinstance(value, NumberLiteral):
        return value.value
    if isinstance(value, dict):
        return {key: plain(item) for key, item in value.items()}
    if isinstance(value, list):
        return [plain(item) for item in value]
    return value


# --------------------------------------------------------------------------
# Schemas
# --------------------------------------------------------------------------


def check_schema(schema: Any, origin: str) -> dict:
    if not isinstance(schema, dict):
        raise Refused(
            f"{origin} is not a JSON object; a boolean schema accepts or refuses everything, "
            "which is not a description of what to extract"
        )
    if lone_surrogate(json.dumps(schema, ensure_ascii=False)) is not None:
        raise Refused(
            f"{origin} holds a lone surrogate (an unpaired \\ud800-style escape), which is "
            "not a Unicode character and which no proposal file can hold"
        )
    validator = jsonschema.validators.validator_for(schema)
    try:
        validator.check_schema(schema)
    except jsonschema.SchemaError as error:
        raise Refused(f"{origin} is not a valid JSON Schema: {error.message}") from None
    return schema


def load_json_schema(path: str) -> dict:
    file = Path(path)
    try:
        size = file.stat().st_size
        if size > MAX_SCHEMA_BYTES:
            raise Refused(
                f"{path} is {size} bytes, more than the {MAX_SCHEMA_BYTES}-byte ceiling"
            )
        schema = json.loads(
            file.read_text(encoding="utf-8"), parse_constant=_refuse_constant
        )
    except OSError as error:
        raise Refused(f"{path} could not be read: {error.strerror}") from None
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        raise Refused(f"{path} is not JSON: {error}") from None
    return check_schema(schema, path)


def load_pydantic_schema(reference: str) -> tuple[dict, str]:
    """``path/to/module.py:ClassName`` to that model's JSON Schema.

    This executes the named file, which is the operator's own code; that is
    the price of taking a Pydantic model, and README.md says so.
    """
    path, separator, class_name = reference.rpartition(":")
    if not separator or not path or not class_name.isidentifier():
        raise Refused(f"{reference!r} is not path/to/module.py:ClassName")
    file = Path(path)
    if not file.is_file() or file.suffix != ".py":
        raise Refused(f"{path} is not a Python file")
    spec = importlib.util.spec_from_file_location(
        f"qip_scraper_user_schema_{abs(hash(path))}", file
    )
    if spec is None or spec.loader is None:
        raise Refused(f"{path} cannot be loaded as a module")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    model = getattr(module, class_name, None)
    from pydantic import BaseModel

    if not (isinstance(model, type) and issubclass(model, BaseModel)):
        raise Refused(f"{class_name} in {path} is not a Pydantic model")
    return check_schema(
        model.model_json_schema(), reference
    ), f"{file.name}:{class_name}"


def validate(instance: Any, schema: dict) -> None:
    """Refuse an answer the schema does not admit, naming where it fails."""
    validator = jsonschema.validators.validator_for(schema)(schema)
    errors = sorted(
        validator.iter_errors(instance), key=lambda error: list(error.absolute_path)
    )
    if errors:
        first = errors[0]
        where = "/" + "/".join(str(part) for part in first.absolute_path)
        raise Refused(
            f"the answer does not satisfy the schema at {where}: {first.message}"
            + (f" (and {len(errors) - 1} more)" if len(errors) > 1 else "")
        )


# --------------------------------------------------------------------------
# Verbatim
# --------------------------------------------------------------------------


def _pointer(parts: list) -> str:
    """RFC 6901: "" is the whole answer, and "/" would name a key "" instead."""
    return "".join(
        "/" + str(part).replace("~", "~0").replace("/", "~1") for part in parts
    )


def leaves(value: Any, path: list | None = None) -> Iterator[tuple[str, Any]]:
    """Every leaf of the parsed answer, with its JSON Pointer."""
    path = [] if path is None else path
    if isinstance(value, dict):
        for key, item in value.items():
            yield from leaves(item, [*path, key])
    elif isinstance(value, list):
        for index, item in enumerate(value):
            yield from leaves(item, [*path, index])
    else:
        yield _pointer(path), value


def _string_boundary(text: str, start: int, end: int, needle: str) -> bool:
    before = text[start - 1] if start > 0 else ""
    after = text[end] if end < len(text) else ""
    if needle[0].isalnum() and before.isalnum():
        return False
    return not (needle[-1].isalnum() and after.isalnum())


def _number_boundary(text: str, start: int, end: int, needle: str) -> bool:
    """A number is found only where it is the whole number the page wrote.

    ``12.5`` is not found in ``112.5``, ``12.50``, ``12.5.3``, ``12.5e3`` or
    ``12.5k``; ``3`` is not found in ``-3``, ``1,3`` or ``0.3``. Spaced units
    (``12.5 kg``) and symbols (``12.5%``, ``$12.5``) are allowed, and so is a
    letter before the number (``USD12.5``).
    """
    before = text[start - 1] if start > 0 else ""
    before2 = text[start - 2] if start > 1 else ""
    after = text[end] if end < len(text) else ""
    after2 = text[end + 1] if end + 1 < len(text) else ""
    if before.isdigit() or before == ".":
        return False
    if before in ",'" and before2.isdigit():
        return False
    if (
        not needle.startswith("-") and before in "-\u2212"
    ):  # hyphen-minus, or the minus sign U+2212
        return False
    if after.isdigit() or after.isalpha():
        return False
    return not (after in ".,'" and after2.isdigit())


def _find(text: str, needle: str, boundary) -> tuple[int, int | None]:
    count, first, start = 0, None, 0
    while True:
        index = text.find(needle, start)
        if index < 0:
            return count, first
        if boundary(text, index, index + len(needle), needle):
            count += 1
            first = index if first is None else first
        start = index + 1


def check_value(pointer: str, value: Any, haystack: str) -> dict:
    """The record of one leaf: what it is, and whether the page contains it."""
    record: dict[str, Any] = {"pointer": pointer}
    if value is None:
        return {
            **record,
            "type": "null",
            "value": None,
            "status": "absent",
            "reason": "the model reported no value",
        }
    if isinstance(value, bool):
        return {
            **record,
            "type": "boolean",
            "value": value,
            "status": "unverified",
            "reason": "a boolean is a judgement, not a transcription; it cannot be found verbatim",
        }
    if isinstance(value, NumberLiteral):
        needle, boundary = value.spelling, _number_boundary
        record.update(
            {"type": "number", "value": value.value, "spelling": value.spelling}
        )
    else:
        needle, boundary = fold(value), _string_boundary
        record.update({"type": "string", "value": value})
        if not needle:
            return {
                **record,
                "status": "absent",
                "reason": "an empty string claims no value",
            }
        if needle == NOT_FOUND_SENTINEL:
            return {
                **record,
                "status": "absent",
                "reason": 'the library\'s not-found marker ("NA"); no value is claimed',
            }
    occurrences, first = _find(haystack, needle, boundary)
    if occurrences == 0:
        return {
            **record,
            "status": "unverified",
            "occurrences": 0,
            "reason": f"{needle!r} does not occur in the page text as a whole "
            + ("number in this spelling" if boundary is _number_boundary else "token"),
        }
    context = haystack[max(0, first - _CONTEXT) : first + len(needle) + _CONTEXT]
    return {
        **record,
        "status": "verified",
        "occurrences": occurrences,
        "first_offset": first,
        "context": context,
    }


def check_values(answer: Any, haystack: str) -> list[dict]:
    return [check_value(pointer, value, haystack) for pointer, value in leaves(answer)]
