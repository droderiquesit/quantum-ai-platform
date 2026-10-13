"""Protection 5: the answer is a proposal, checked value by value against the page."""

from __future__ import annotations

import json
import time

import pytest
from conftest import RESULTS_PAGE, RESULTS_SCHEMA

from qip_scraper import verify

PAGE_TEXT = verify.page_text(RESULTS_PAGE, "text/html")
HAYSTACK = verify.fold(PAGE_TEXT)


def status_of(answer: str, haystack: str = HAYSTACK) -> dict:
    return {
        check["pointer"]: check["status"]
        for check in verify.check_values(verify.parse_answer(answer), haystack)
    }


# --------------------------------------------------------------------------
# Bytes to the text the model sees
# --------------------------------------------------------------------------


def test_the_model_sees_the_page_text_and_not_its_scripts_or_styles():
    assert "Acme Corp reports third-quarter results" in PAGE_TEXT
    assert "Acme Corp — third-quarter results" in PAGE_TEXT  # &mdash; decoded
    assert "Investor Relations" in HAYSTACK  # &nbsp; folded to a space for matching
    assert "tracking" not in PAGE_TEXT and "display: none" not in PAGE_TEXT
    assert "Net income\t210.75" in PAGE_TEXT  # cells keep their row
    assert "Full report </reports/q3.pdf>" in PAGE_TEXT  # link targets are shown


def test_unsafe_link_targets_are_not_shown():
    text = verify.page_text(
        '<a href="javascript:steal()">x</a><a href="mailto:a@b.c">y</a>', "text/html"
    )
    assert text == "xy"


def test_plain_text_is_read_as_written():
    assert verify.page_text("a\r\n\tb  c\n\n\nd", "text/plain") == "a\nb c\nd"


@pytest.mark.parametrize(
    ("body", "declared", "expected"),
    [
        ("café".encode(), "utf-8", "café"),
        ("café".encode("latin-1"), "iso-8859-1", "café"),
        (
            b'<meta charset="windows-1252"><p>caf\xe9</p>',
            None,
            '<meta charset="windows-1252"><p>café</p>',
        ),
        (b"\xef\xbb\xbfplain", None, "plain"),
    ],
)
def test_bodies_decode_with_the_charset_they_declare(body, declared, expected):
    assert verify.decode_body(body, "text/html", declared)[0] == expected


def test_bytes_that_do_not_decode_refuse_the_page():
    with pytest.raises(verify.Refused, match="does not decode as utf-8"):
        verify.decode_body(b"caf\xe9", "text/html", "utf-8")
    with pytest.raises(verify.Refused, match="does not know"):
        verify.decode_body(b"x", "text/html", "no-such-charset")


@pytest.mark.parametrize("charset", ["zlib", "base64", "rot13", "hex"])
def test_a_charset_that_is_a_codec_but_not_a_text_encoding_refuses_the_page(charset):
    # codecs.lookup finds these, and decoding with one raised LookupError,
    # which ended the whole run with a traceback and exit 1.
    with pytest.raises(verify.Refused, match="which does not decode text"):
        verify.decode_body(b"<p>Acme Corp</p>", "text/html", charset)


def test_a_text_codec_that_raises_a_bare_unicode_error_refuses_the_page():
    # idna and punycode raise UnicodeError itself, which a handler for
    # UnicodeDecodeError alone lets through.
    with pytest.raises(verify.Refused, match=r"does not decode text \(UnicodeError\)"):
        verify.decode_body(b"xn--zz", "text/plain", "idna")


def test_a_page_that_decodes_to_a_lone_surrogate_is_refused():
    # UTF-7 can spell an unpaired surrogate; no proposal file could hold it.
    assert b"+2AA-".decode("utf-7") == "\ud800"
    with pytest.raises(verify.Refused, match="lone surrogate at character 3"):
        verify.decode_body(b"<p>+2AA-</p>", "text/html", "utf-7")


def test_an_answer_holding_a_lone_surrogate_is_refused_and_a_pair_is_read():
    with pytest.raises(verify.Refused, match="lone surrogate"):
        verify.parse_answer('{"company": "\\ud800"}')
    with pytest.raises(verify.Refused, match="lone surrogate"):
        verify.parse_answer('{"\\udce9": "Acme Corp"}')
    assert verify.parse_answer('{"face": "\\ud83d\\ude00"}') == {"face": "\U0001f600"}


def test_a_schema_holding_a_lone_surrogate_is_refused(tmp_path):
    path = tmp_path / "schema.json"
    path.write_text('{"type": "object", "title": "caf\\udce9"}')
    with pytest.raises(verify.Refused, match="lone surrogate"):
        verify.load_json_schema(str(path))


# --------------------------------------------------------------------------
# The model's answer, parsed strictly
# --------------------------------------------------------------------------


def test_numbers_keep_the_spelling_the_model_wrote():
    parsed = verify.parse_answer('{"a": 12.50, "b": 1e3, "c": -0.10, "d": 7}')
    assert [parsed[key].spelling for key in "abcd"] == ["12.50", "1e3", "-0.10", "7"]
    assert verify.plain(parsed) == {"a": 12.5, "b": 1000.0, "c": -0.1, "d": 7}


@pytest.mark.parametrize(
    ("raw", "match"),
    [
        ('{"company": "Acme Co', "not one strict JSON document"),  # cut off mid-value
        ('{"a": 1} and some commentary', "not one strict JSON document"),
        ('{"a": NaN}', "NaN"),
        ('{"a": 1, "a": 2}', "repeats the key"),
        ("   ", "empty"),
    ],
)
def test_an_answer_that_is_not_one_strict_json_document_is_refused(raw, match):
    with pytest.raises(verify.Refused, match=match):
        verify.parse_answer(raw)


def test_the_tolerant_parser_the_library_uses_would_have_repaired_a_cut_off_answer():
    # The reason this module parses for itself: LangChain closes the string
    # and the object and hands back a shorter value that was never written.
    from langchain_core.output_parsers import JsonOutputParser

    assert JsonOutputParser().parse('{"company": "Acme Co') == {"company": "Acme Co"}


@pytest.mark.parametrize(
    "fenced",
    [
        '```json\n{"a": "b"}\n```',
        '```\n{"a": "b"}\n```',
        '```json \t\n{"a": "b"}```',
        '```\n{"a": "b"}\n  \t```',
        '  ```json\n{"a": "b"}\n```  ',
    ],
)
def test_one_code_fence_around_the_whole_answer_is_removed(fenced):
    assert verify.plain(verify.parse_answer(fenced)) == {"a": "b"}


@pytest.mark.parametrize(
    "not_one_fence",
    ['```JSON\n{"a": "b"}\n```', '``` json\n{"a": "b"}\n```', '```json {"a": "b"}```'],
)
def test_anything_else_around_the_answer_is_refused(not_one_fence):
    with pytest.raises(verify.Refused, match="not one strict JSON document"):
        verify.parse_answer(not_one_fence)


def test_an_answer_full_of_spaces_after_a_fence_is_refused_in_linear_time():
    # The fence pattern this replaced was quadratic in the run of spaces:
    # 20,000 took 0.4s, and the reply is the model's, which a page can steer.
    started = time.monotonic()
    with pytest.raises(verify.Refused, match="not one strict JSON document"):
        verify.parse_answer("```\n" + " " * 100_000 + "x")
    assert time.monotonic() - started < 1.0


# --------------------------------------------------------------------------
# Verbatim
# --------------------------------------------------------------------------


def test_values_in_the_page_are_verified():
    assert set(
        status_of(
            '{"company": "Acme Corp", "revenue": "$1,234.5 million", "growth": 12.50,'
            ' "net": 210.75, "published": "2026-10-01", "contact": "Investor Relations"}'
        ).values()
    ) == {"verified"}


@pytest.mark.parametrize(
    "number",
    [
        "12.5",  # the page writes 12.50
        "1234.5",  # the page writes 1,234.5
        "234.5",  # inside 1,234.5
        "10.75",  # inside 210.75
        "21",  # a prefix of 210.75
        "-12.50",  # a sign the page does not have
    ],
)
def test_a_number_is_found_only_in_the_spelling_and_extent_the_page_wrote(number):
    assert status_of(f'{{"n": {number}}}') == {"/n": "unverified"}


def test_a_sign_flip_is_not_a_match():
    assert status_of('{"n": 3}', "change: -3% on the year") == {"/n": "unverified"}
    assert status_of('{"n": -3}', "change: -3% on the year") == {"/n": "verified"}


def test_a_hyphen_ends_a_number_but_does_not_start_one():
    # The year of 2026-10-01 is a whole number the page wrote; the month after
    # the hyphen could as well be a negative, so it is not taken.
    assert status_of('{"n": 2026}') == {"/n": "verified"}
    assert status_of('{"n": 10}') == {"/n": "unverified"}


def test_an_attached_magnitude_or_unit_is_not_a_match():
    assert status_of('{"n": 12.5}', "assets of 12.5bn") == {"/n": "unverified"}
    assert status_of('{"n": 12.5}', "assets of 12.5 bn") == {"/n": "verified"}
    assert status_of('{"n": 12.5}', "a margin of 12.5%.") == {"/n": "verified"}


def test_a_string_must_be_a_whole_token():
    assert status_of('{"s": "Apple"}', "Pineapple Inc") == {"/s": "unverified"}
    assert status_of('{"s": "Acme"}', "Acme's results") == {"/s": "verified"}
    assert status_of('{"s": "acme corp"}') == {
        "/s": "unverified"
    }  # case is part of the spelling


def test_whitespace_is_the_only_thing_folded():
    assert status_of('{"s": "Investor  Relations"}') == {"/s": "verified"}
    assert status_of('{"s": "Investor-Relations"}') == {"/s": "unverified"}


def test_booleans_are_judgements_and_never_verify():
    (check,) = verify.check_values(
        verify.parse_answer('{"listed": true}'), "listed: true"
    )
    assert check["status"] == "unverified" and "judgement" in check["reason"]


@pytest.mark.parametrize("value", ["null", '"NA"', '""', '"  "'])
def test_no_value_is_recorded_as_absent(value):
    assert status_of(f'{{"x": {value}}}') == {"/x": "absent"}


def test_a_scalar_answer_is_the_whole_document():
    assert status_of('"Acme Corp"') == {"": "verified"}


def test_nested_values_carry_json_pointers():
    statuses = status_of('{"rows": [{"a/b": "Acme Corp"}, {"c~d": 999}]}')
    assert statuses == {"/rows/0/a~1b": "verified", "/rows/1/c~0d": "unverified"}


def test_a_verified_value_carries_where_it_was_found():
    (check,) = verify.check_values(verify.parse_answer('{"net": 210.75}'), HAYSTACK)
    assert check["occurrences"] == 1
    assert HAYSTACK[check["first_offset"] :].startswith("210.75")
    assert "Net income 210.75" in check["context"]


# --------------------------------------------------------------------------
# Schemas
# --------------------------------------------------------------------------


def test_an_answer_the_schema_admits_passes():
    verify.validate(
        {
            "company": "Acme Corp",
            "revenue": "x",
            "growth_percent": 1.5,
            "net_income": 2,
            "published": "2026-10-01",
        },
        RESULTS_SCHEMA,
    )


@pytest.mark.parametrize(
    ("answer", "where"),
    [
        ({"company": "Acme Corp"}, "/"),
        (
            {
                "company": 1,
                "revenue": "x",
                "growth_percent": 1,
                "net_income": 2,
                "published": "d",
            },
            "/company",
        ),
        (
            {
                "company": "a",
                "revenue": "x",
                "growth_percent": 1,
                "net_income": 2,
                "published": "d",
                "extra": True,
            },
            "/",
        ),
    ],
)
def test_an_answer_the_schema_refuses_is_refused_with_the_place(answer, where):
    with pytest.raises(verify.Refused, match=f"at {where}:"):
        verify.validate(answer, RESULTS_SCHEMA)


def test_a_schema_file_must_be_a_valid_json_schema_object(tmp_path):
    for content, match in (
        ("not json", "is not JSON"),
        ("true", "boolean schema"),
        ('{"type": "no-such-type"}', "not a valid JSON Schema"),
    ):
        path = tmp_path / "schema.json"
        path.write_text(content)
        with pytest.raises(verify.Refused, match=match):
            verify.load_json_schema(str(path))
    path.write_text(json.dumps(RESULTS_SCHEMA))
    assert verify.load_json_schema(str(path)) == RESULTS_SCHEMA


def test_a_pydantic_reference_must_name_a_model_in_a_python_file(tmp_path):
    module = tmp_path / "models.py"
    module.write_text("class NotAModel:\n    pass\n")
    for reference, match in (
        (f"{module}", "path/to/module.py:ClassName"),
        (f"{tmp_path / 'missing.py'}:X", "not a Python file"),
        (f"{module}:NotAModel", "not a Pydantic model"),
    ):
        with pytest.raises(verify.Refused, match=match):
            verify.load_pydantic_schema(reference)
