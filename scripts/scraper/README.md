# qip-scraper

An operator-run tool that reads web pages and writes **proposals**: JSON files
for a person to review. It fetches each page itself and hands the text to
[ScrapeGraphAI](https://pypi.org/project/scrapegraphai/)'s `SmartScraperGraph`,
with Claude as the model. Every extracted value is marked as found, or not
found, verbatim in the page.

It is not part of the platform. No Rust crate, no service, no cycle stage, no
event-log topic and no catalogue (`qip-data-finder`) reads anything it
writes. A value reaches the platform only if a person copies it, by hand, into
a file the platform already validates. It is not a crawler either: it reads
the URLs it is given and follows no links.

## Status: the ADR was waived, the protections were not

On 2026-10-13 the owner directed that this tool be built without an ADR first.
That waiver covers **process only**. Every protection below is enforced in
code, held by a test, and was mutation-checked when it landed. A change that
weakens one of them needs the review the waiver skipped.

## The protections, and why each exists

ScrapeGraphAI 2.3.1 was read from its wheel (sha256 `c03605ce…62f9`) before
any of this was written. Each line names the behaviour it closes.

| # | Protection | Why |
|---|---|---|
| 1 | `SCRAPEGRAPHAI_TELEMETRY_ENABLED=false` is set before the library is imported, `disable_telemetry()` is called, and the run is refused unless the flag reads off (exit 3). | Telemetry is on by default and posts the prompt, the schema, the page text, the answer, the model and the URL to the vendor. A mistyped switch only produces a warning upstream. |
| 1 | The library's send functions are replaced with ones that refuse. | If the flag is ever turned back on, nothing is sent; the run fails instead. |
| 1 | `HOME` and the XDG directories point at a private temporary directory, which is deleted at exit. | The first import writes `~/.scrapegraphai.conf` into the operator's home. |
| 1 | LangChain tracing is refused if it is switched on, and switched off otherwise. | A LangSmith trace carries the same prompt, page and answer by another route. |
| 2 | The tool fetches every page itself. The library is given text, never a URL. | The library's fetch path ignores certificate errors, applies bot-detection evasion, reads no robots.txt and extracts error pages as content. |
| 2 | Only global unicast addresses are connected to, in IPv4 and IPv6. Before every request (the page, its robots.txt, and every redirect hop of either) the host is resolved by the tool, and every address it resolves to must be global: loopback, link-local, private, multicast, reserved, unspecified, site-local and IPv4-mapped addresses are refused. The connection goes to the address that was checked, while the Host header, the TLS server name and the certificate check keep the name. No option relaxes this. | A redirect, or a DNS answer, is the server's to choose, and would otherwise point the tool at the operator's own machine, the local network or a cloud metadata address. Connecting to the checked address means a second DNS answer (rebinding) cannot swap it. Through a proxy see "What is not verified". |
| 2 | TLS verification is on, through the system trust store. | A page read over unverified TLS can have been rewritten by anyone on the path. |
| 2 | Name resolution, connect, read, write and pool timeouts are explicit, and the whole body has a deadline. | A read timeout alone lets a server drip one byte at a time for ever. |
| 2 | There is a byte ceiling per page on the bytes read from the socket, and a larger page is refused, not truncated. Only an identity body is read: a page or robots.txt sent with any other `Content-Encoding` is refused unread. | A truncated page is not a smaller page. httpx inflates gzip, deflate and zstd whatever was asked for, and before a ceiling on the inflated bytes sees them: 8 KiB of zstd took over 500 MiB of memory against a 5 MiB ceiling. |
| 2 | The User-Agent names the tool: `qip-scraper/0.1.0 (…)`. | A publisher must be able to tell it no. |
| 2 | Only http and https URLs are read, and a URL carrying credentials is refused. | A credential in a URL ends up in the proposal and in access logs. |
| 2 | robots.txt is read before every page and for every redirect hop. Both `urllib.robotparser` and an RFC 9309 longest-match reading must allow the page. | The stdlib parser applies rules in file order, so `Allow: /` before `Disallow: /private/` lets it fetch `/private/x`. |
| 2 | robots.txt judges the path that is sent: each URL, and each redirect target, is put in the form httpx requests (dot segments removed) before anything judges it, and that form is the one recorded. A dot segment httpx leaves in place (`%2e%2e`, behind a backslash, or with a path parameter, `..;`) is refused. | Judged as written, `/open/../no-crawl/x` passed `Disallow: /no-crawl/`, and httpx then asked for `/no-crawl/x`. A server that decodes `%2e%2e` before resolving it would do the same, and servlet containers read `..;` as `..`. |
| 2 | robots.txt is parsed and its patterns matched in linear time, and so is the code fence around the model's answer. | They are the server's and the model's to write. Built as regular expressions, seven `*` against an 80-character path took about 8 s, and a robots.txt line of 20,000 spaces took 1.8 s, growing with the square of its length. |
| 2 | An unreachable robots.txt refuses the page. An absent one (HTTP 404) refuses it unless `--allow-absent-robots` is given, and that choice is recorded. | RFC 9309 2.3.1.4 covers the unreachable case. For the absent case, the platform's own rule is that an absent robots.txt is not permission (`qip-data-finder`, `legal.rs`). |
| 2 | A server's malformed input refuses that server's page and nothing else: a robots.txt line `urllib.robotparser` cannot read (`Crawl-delay: ²`, a `Crawl-delay` of thousands of digits, `Disallow: http://[`), a `Crawl-delay` too large to be a number of seconds, or a `Location`, on a page or on its robots.txt, that is not a URL (`http://[`). The run goes on with the other URLs, and the reason is in the proposal. | Each of these raised out of the standard library and ended the whole run with exit 5, losing every other URL. Each is caught at the one call that parses the server's value, not around the page, so a fault in this tool's own code still ends the run with exit 5 rather than being filed as a refused page. |
| 2 | `HTTPS_PROXY`/`HTTP_PROXY`/`NO_PROXY` from the environment are honoured, and nothing else can choose a proxy. | Honouring them is the platform's egress design. The library can harvest free public proxies, and that code is refused at import. |
| 2 | Each page's text begins with a newline before it is handed over, and the graph is checked to be in content mode. | The library fetches any source that starts with `http`. |
| 3 | An import guard replaces every `scrapegraphai` module except the few SmartScraperGraph needs (`guard.ALLOWED_LIBRARY_MODULES`) with an inert stub, whose classes refuse to be called. | `GenerateCodeNode` runs model-written Python with `exec()` (the CVE-2026-5532 class). Other modules evaluate expressions, import files or start subprocesses. The real code never loads. |
| 3 | The browser driver, the free-proxy harvester, the search client and the vendor SDK are refused too. | Nothing can reach them by another route. |
| 3 | A network fence admits only the model's host (and its proxy, if one is configured) while the library runs. Any other attempt refuses the whole run (exit 3). | It is the last line: telemetry, a tokenizer download or a library fetch all fail closed. |
| 4 | Claude through `langchain-anthropic`. The operator names the model with `--model`, or else with `QIP_SCRAPER_MODEL`, and the flag wins. With neither, the run is refused before any connection (exit 2). The proposal records the id and where it came from. | There is no default: the repository names no model, so the choice is the operator's and is on the record. The library does not declare `langchain-anthropic`, so this tool pins it. |
| 4 | The key comes from `ANTHROPIC_API_KEY` or from the file `ANTHROPIC_API_KEY_FILE` names. Setting both is refused, and so is an empty value. The key is never logged, and error text is redacted whole before it is shortened. | This is the platform's `_FILE` convention (`qip_core::secret`): a key in a file is not in `/proc/<pid>/environ`. Redaction looks for the whole key: when error text was cut at 500 characters first, a key across the cut left its first characters in the proposal. |
| 4 | No temperature is sent unless `--temperature 0` is given, and 0 is the only value accepted. The proposal records what was sent: 0, or none. | Some Claude models reject any temperature with HTTP 400, so sending one by default would break them. Pass `--temperature 0` only for a model that accepts it. See "What is not verified". |
| 4 | Only an `end_turn` answer is read. `max_tokens`, `refusal` or an unreported stop reason refuses the page. | A cut-off answer must not be repaired into a plausible shorter one. |
| 5 | The model's raw reply is parsed strictly, with each number keeping the spelling the model wrote. | The library's LangChain parser closes a cut-off JSON document. A float re-rendering would turn `12.50` into `12.5`. |
| 5 | Every leaf value is checked against the page text, and numbers are matched as whole numbers in their own spelling. | A model may transcribe a value, but it may not compute, round or reformat one (ADR 0005's rule). |
| 5 | The answer is validated against the JSON Schema with `jsonschema`, and refused on failure. `--allow-unverified` does not change that. | The library only asks for the schema in the prompt and never validates it. |
| 5 | Exit 1 if any value is not found verbatim, unless `--allow-unverified` is given. Either way each such value is marked `unverified`. | This keeps the reviewer's attention on the values that need it. |
| 5 | The proposal is written whole or not at all: encoded first, written to a temporary file beside the output, synced, and linked into place, which fails rather than replace a file. Text that is not Unicode (a lone surrogate) is refused where it enters: a command-line argument, the schema, a page's text, the model's answer. A page whose declared charset is not a text encoding (`zlib`, `base64`) is refused like any page that does not decode. A run that cannot write its proposal exits 4, and one that fails unexpectedly exits 5. | A lone surrogate in an answer stopped the write halfway and left a 0-byte proposal, and a page declaring `charset=zlib` ended the run with a traceback. Both exited 1, the code that means a proposal was written. |
| 5 | A page whose text exceeds `--max-text-chars` is refused, not chunked. The graph runs as Fetch → GenerateAnswer with no ParseNode. | The library's per-chunk merge can return a value that appears in no chunk. |
| 6 | At most 10 URLs per run, a byte ceiling per page, a text ceiling per page, explicit timeouts everywhere, and limits past their ceilings refused rather than lowered. | Bounded retention: nothing here grows without a limit. |

Only these pieces of the library run: its prompt template, the schema pasted into
that prompt, and LangChain plumbing. Everything else is the tool's own code.

## Install

Python 3.12 is required: the library needs it, and the lock is resolved for it.

```sh
cd scripts/scraper
python3.12 -m venv .venv
.venv/bin/python -m pip install --require-hashes --only-binary :all: -r requirements.lock
```

`requirements.lock` pins every package of the resolved tree, for every
platform, with hashes. `--only-binary :all:` means no source distribution's
build script runs. Playwright's Python package is installed because the
library depends on it, but **do not run `playwright install`**: the tool needs
no browser, and the import guard refuses the code that would drive one.

Regenerate the lock only as part of an upgrade (see "Upgrading"):

```sh
uv pip compile requirements.in --python-version 3.12 --universal \
  --only-binary :all: --generate-hashes -o requirements.lock
```

## Use

```sh
export ANTHROPIC_API_KEY_FILE=/path/to/anthropic.key   # or ANTHROPIC_API_KEY, never both
export QIP_SCRAPER_MODEL='<a Claude model id>'          # or pass --model; one is required

cat > /tmp/results.schema.json <<'EOF'
{
  "type": "object",
  "properties": {
    "company":  {"type": "string"},
    "revenue":  {"type": "string"},
    "published": {"type": "string"}
  },
  "required": ["company", "revenue", "published"],
  "additionalProperties": false
}
EOF

.venv/bin/python -m qip_scraper https://investors.example.com/news/q3-results \
  --prompt "The company's name, the headline revenue figure exactly as written, and the publication date." \
  --schema /tmp/results.schema.json \
  --output /tmp/q3-results.proposal.json
```

* `--pydantic-model path/models.py:ClassName` replaces `--schema`. **The file
  is executed** to load the model, and its JSON Schema is what gets validated.
* `--model <id>` names the model and wins over `QIP_SCRAPER_MODEL`. The id
  must look like `claude-…`. There is no default: a run with neither is
  refused before anything is fetched.
* `--temperature 0` sends a temperature of 0, the only value accepted. Without
  it no temperature is sent. Pass it only for a model whose API accepts a
  temperature: some reject any temperature with HTTP 400.
* `--allow-unverified` accepts values not found verbatim, and they stay marked.
* `--allow-absent-robots` fetches from hosts with no robots.txt, and that is
  recorded per page.
* `--max-page-bytes`, `--max-text-chars` and `--model-timeout` lower the
  limits. A value past a ceiling is refused.

Ask for values the way the page writes them. A string field verifies
`"$1,234.5 million"` against a page that says `$1,234.5 million`. A number
field can only verify `1234.5` if the page writes `1234.5`. Booleans are
judgements, so they never verify.

### The proposal

Each run writes one file, whole or not at all, readable by its owner only
(mode 0600), and never over an existing one. The directory it goes in must
allow hard links, which is how it is put in place without replacing anything,
and that is checked before anything is fetched or the model is asked: a
directory is made there, a file and a hard link to it are made inside it, and
all three are removed. A directory that refuses (FAT, some network mounts)
refuses the run with exit 2, rather than at the end with the pages fetched
and the model paid for.
It holds:

* The tool and library versions.
* The guard's findings: the telemetry switch and its state, the fence's hosts
  and refusals.
* The model record: id and where it came from (`--model` or
  `QIP_SCRAPER_MODEL`), endpoint, the temperature sent (0, or none),
  `max_tokens`, timeout and retries.
* The prompt, the extra instruction added to it, the schema and its sha256.
* The text normalisation used.
* For each URL: the final URL and redirects, `fetched_at` (UTC), HTTP status,
  kept headers, byte count, **the sha256 of the fetched bytes**, the proxy,
  each robots.txt verdict, the model that served the answer, its stop reason
  and token usage, the extracted object, and one record per leaf. Each leaf
  record has a JSON Pointer, the value, the number's spelling, a status
  (`verified`, `unverified` or `absent`), the occurrence count and a short
  context.

Page text is not kept: only its length and sha256.

`absent` covers `null`, `""` and the library's own `"NA"` marker, which means
no value is claimed. Absent values do not fail a run.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Every page was read, and every value was found verbatim (or `--allow-unverified` was given). |
| 1 | A proposal was written, but a page was refused, an answer failed its schema, or a value was not found verbatim. |
| 2 | The command line, the schema, the key or the output directory was refused. Nothing was fetched. |
| 3 | A safety guard refused: telemetry not off, tracing on, a refused component used, or a connection past the fence. Nothing was written. |
| 4 | The proposal could not be written whole: it held text no UTF-8 file can hold, the disk refused it, or a file appeared at the output path during the run. Nothing was left at the output path. |
| 5 | The run failed unexpectedly. The redacted traceback is on stderr, and nothing was written. |

## Tests

```sh
cd scripts/scraper
.venv/bin/python -m pytest
```

The tests are fully offline. They use local HTTP(S) servers, a throwaway
certificate minted by `openssl` at test time (which must be on `PATH`), a fake
chat model, and a local stand-in for the Messages API that the real
`ChatAnthropic` talks to. No key and no network are needed.

The local servers listen on 127.0.0.1, which the tool refuses to connect to.
The tests fetch through an address policy that admits that one address and
refuses everything else as the tool does. It is passed as a parameter of
`cli.main`, which the command line has no way to set, and one test runs the
command line's own policy against a local server to show it refused.

## Upgrading

Upgrading the library is a review, not a version bump. Re-read
`telemetry/telemetry.py` (the destination has moved once already), the fetch
path, `graphs/__init__.py` and `nodes/__init__.py` (any new module is refused
until it is on the allow-list), and every allow-listed module. Then pin the
new version in `requirements.in`, regenerate the lock, and run the tests.
`test_guard.py` fails if:

* any allow-listed module gains `exec`, `eval`, `exec_module`, `subprocess` or
  `simple_eval`;
* the set of real modules changes;
* the telemetry switch stops working at import.

## What is not verified

* **No live call to the Anthropic API was made.** The request shape (model,
  `max_tokens`, temperature present or absent, key header) was checked against
  a local stand-in, not against `api.anthropic.com`.
* **Temperature 0 is in force only when asked for.** By default no
  temperature is sent and the model samples as it does by default. Whether a
  given model accepts `--temperature 0` is not checked here; one that rejects
  it fails the request with HTTP 400, and the page is refused. Either way, a
  hosted model's answer is not reproducible, and the verbatim check is the
  control.
* **No live page was fetched.** TLS, timeouts, size limits, encodings,
  robots.txt, proxies and the address policy were exercised against local
  servers, and names were resolved by a stand-in resolver, never by DNS.
* **"Verified" means the value occurs in the page**, as a whole token, in the
  same spelling. It does not mean the value belongs to the field, that its unit
  or scale was read correctly, or that it is the only such value. A person
  still reviews every proposal.
* **Licensing and personal data are not checked.** `qip-data-finder`'s
  licensing gate and personal-data screen do not run here. The operator
  decides whether a page may be read and sent to a hosted model. The tool
  extracts whatever the prompt asks for.
* **The fence has one blind spot.** When the model is reached through a proxy,
  the proxy host is admitted, and a client that does its own `CONNECT` (httpx)
  could tunnel elsewhere through it. `http.client` tunnels (urllib, requests,
  and the library's telemetry sender) are still checked.
* **Through a proxy, the address check is on this machine's answer, not the
  proxy's.** Without a proxy, the tool connects to the address it checked.
  Through `HTTPS_PROXY` or `HTTP_PROXY` the proxy resolves the name and
  connects, so the tool checks its own resolution of the name before the proxy
  is asked. A name that resolves to a global address here and to an inward one
  for the proxy (DNS rebinding between the two lookups, or a split-horizon
  resolver) is not caught here; the proxy's own egress policy is what stands
  there. A name this machine cannot resolve is refused, so behind a proxy that
  is the only route to DNS, every page is refused.
* **robots.txt is judged on the path this tool sends.** A server that maps
  other spellings to the same resource can serve a disallowed page under an
  allowed one: merged slashes (`//no-crawl/x`), case-insensitive paths, or
  `%2F` decoded as a separator. Dot segments are the exception, because httpx
  resolves them before sending: literal ones are judged as resolved, and ones
  httpx leaves in place (`%2e%2e`, backslashes, `..;`) are refused.
* **Text a browser would hide counts as being in the page.** The model's text
  and the verbatim check both come from the HTML as served, including text no
  browser shows: CSS `display: none`, the `hidden` attribute, off-screen or
  zero-size text. A value can be verified against text no reader of the page
  sees.
* **Keys the schema does not name pass validation** unless the schema says
  `"additionalProperties": false`, as the example above does. Their values
  are still checked verbatim and marked like any other, so an invented value
  still fails the run unless `--allow-unverified` is given. A Pydantic model
  allows extra keys unless it is configured with `extra="forbid"`. The tool
  does not add `additionalProperties: false` itself: doing so would change
  the operator's schema, at every level and through every `$ref`.
* **Pages rendered by JavaScript are out of reach.** There is no browser, so
  the text is what the server sent.
* The model sees the library's rendering of the text: the Python repr of a
  LangChain `Document`, so quotes and newlines arrive escaped.
* JSON Schema `format` keywords are annotations only, as `jsonschema` treats
  them by default. A Pydantic model's custom validators do not run, because
  only its JSON Schema is used.
* Tested on Linux with Python 3.12 only. The private-home redirection assumes
  a POSIX `HOME`.
* **CI does not run these tests.** `ci.yml`'s `scripts` job runs `node --test`
  only, and adding a Python job is a workflow change this tool did not make.
* **The lock has not been audited.** No vulnerability scan of it (pip-audit
  or OSV) was run when it was generated, or since. Run one before relying on
  it. The floors in `requirements.in` raise `langchain-core` and `simpleeval`
  above the advisories the library's own floors admit; nothing else was
  checked.
* The library's PyPI metadata declares no licence. The wheel carries an MIT
  licence file, and that file is what was read.
