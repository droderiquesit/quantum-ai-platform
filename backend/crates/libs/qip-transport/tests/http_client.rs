//! What the HTTP client refuses, and what it survives.
//!
//! Every test here runs against a real listener on a real loopback port. The
//! interesting half is not that a well-formed response parses — it is that a
//! peer which closes mid-body, answers with more than this process will hold,
//! or says nothing at all, produces a named error rather than a truncated
//! result, an out-of-memory kill, or a wait with no end.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{Action, TestServer, address_with_no_listener};
use qip_transport::{ClientLimits, HttpClient, HttpError, HttpRequest, Method, Phase, Url};
use std::time::Duration as StdDuration;

/// Limits tight enough that a test can trip them in milliseconds and bytes.
fn tight() -> ClientLimits {
    ClientLimits {
        max_status_line: 256,
        max_header_line: 256,
        max_headers: 8,
        max_body: 512,
        max_chunk: 256,
        connect_timeout: StdDuration::from_millis(500),
        read_timeout: StdDuration::from_millis(200),
        write_timeout: StdDuration::from_millis(500),
    }
}

// --- URLs ---------------------------------------------------------------

#[test]
fn https_is_refused_by_name_rather_than_quietly_downgraded() {
    let error = Url::parse("https://central.internal/v1/mesh/publish")
        .expect_err("a scheme this build cannot speak was accepted");
    assert_eq!(error.code(), "unsupported_scheme");
    assert!(
        error.to_string().contains("no TLS stack"),
        "the refusal must say why, so nobody concludes the URL was malformed: {error}"
    );
    assert!(
        !error.is_transient(),
        "retrying an https URL will not grow a TLS stack"
    );
}

/// The refusal of a URL carrying a credential does not itself carry the
/// credential. Until 2026-09-12 `InvalidUrl` stored and printed the whole
/// URL, so the one check that existed to keep a credential out of the log
/// wrote it there, on stderr and into Cloud Logging, from every root's
/// start-up. Both the `Display` and the `Debug` of the error are checked,
/// because a root that formats `{error:?}` is as much a log line as one
/// that formats `{error}`.
///
/// Mutated by storing `raw.to_string()` in `InvalidUrl` again — confirmed
/// the refusal then prints `hunter2` and this fails; restored.
#[test]
fn a_url_that_carries_a_credential_is_refused() {
    let error = Url::parse("http://operator:hunter2@central.internal/v1/mesh/publish")
        .expect_err("userinfo was accepted");
    assert!(
        error.to_string().contains("credential"),
        "the refusal must name what is wrong with it: {error}"
    );
    for rendered in [error.to_string(), format!("{error:?}")] {
        assert!(
            !rendered.contains("hunter2") && !rendered.contains("operator"),
            "the refusal of a credential-bearing URL echoed the credential: {rendered}"
        );
        assert!(
            rendered.contains("…@central.internal/v1/mesh/publish"),
            "the refusal must keep the host so the mistake can be found: {rendered}"
        );
    }
}

/// The egress gate admits `http://127.0.0.1:<port>` and nothing else, and
/// what it echoes on refusal is safe to log. The gate lived in
/// `qip-market-ingestion` with a parser of its own until 2026-09-12 and
/// read `http://127.0.0.1:9105@evil.example/` as loopback; it lives beside
/// the parser now so that the host it checks is the host a request would
/// connect to, by construction. The port-less row is the one the earlier
/// gate admitted and Terraform's `startswith("http://127.0.0.1:")` never
/// did. The credential rows are refused for their userinfo and the refusal
/// is checked for the token at both the parser's seam and the gate's.
///
/// Mutated four ways. Admitting `host.eq_ignore_ascii_case("localhost")`
/// beside the literal — confirmed `http://localhost:9106` is then admitted
/// and this fails. Deleting the `port_is_explicit` check — confirmed
/// `http://127.0.0.1` is then admitted and this fails. Echoing `base_url`
/// in place of `shown` in the `https` arm — confirmed the refusal of
/// `https://svc:TOKEN@127.0.0.1:9106/` then prints `TOKEN` and this fails.
/// Echoing `{base_url:?}` in the parse-failure arm — confirmed the refusal
/// of `http://svc:TOKEN@127.0.0.1:9106/` then prints `TOKEN` and this
/// fails. Each restored. The same mutation on the host arm does **not**
/// fire, and that is structural rather than a gap in the rows: the parser
/// refuses userinfo before the host and port arms run, so neither can
/// reach an address carrying one; they echo `shown` anyway so that a
/// reordering of the arms cannot reopen the leak silently.
#[test]
fn an_egress_address_is_loopback_with_a_port_and_a_refusal_never_echoes_a_credential() {
    use qip_transport::http::{redact_for_echo, require_loopback_egress};

    for admitted in [
        "http://127.0.0.1:9105",
        "http://127.0.0.1:9105/",
        "http://127.0.0.1:9106/v1/chat",
    ] {
        require_loopback_egress(admitted)
            .unwrap_or_else(|error| panic!("premise: {admitted} was refused: {error}"));
    }
    for (refused, expected) in [
        ("https://router.huggingface.co", "never at the vendor"),
        ("https://127.0.0.1:9106", "never at the vendor"),
        ("http://10.0.0.5:9106", "loopback"),
        ("http://router.huggingface.co/", "loopback"),
        ("http://127.0.0.1.evil.example:9106", "loopback"),
        ("http://localhost:9106", "loopback"),
        ("http://LOCALHOST:9106/v1", "loopback"),
        ("http://[::1]:9106", "loopback"),
        ("http://127.0.0.1", "names no port"),
        ("http://127.0.0.1/v1", "names no port"),
        ("http://127.0.0.1:9106@evil.example/", "userinfo"),
        ("http://evil.example@127.0.0.1:9106/", "userinfo"),
        ("http://svc:TOKEN@127.0.0.1:9106/", "userinfo"),
        ("ftp://127.0.0.1:9106", "absolute http://"),
        ("127.0.0.1:9106", "absolute http://"),
        ("", "absolute http://"),
    ] {
        let error = require_loopback_egress(refused)
            .expect_err(&format!("{refused:?} was admitted as an egress address"));
        assert!(
            error.message().contains(expected),
            "the refusal of {refused:?} does not say `{expected}`: {}",
            error.message()
        );
    }

    // The token never reaches the message, whichever arm refuses the
    // address: the parser's (userinfo on a loopback host) and the gate's
    // own (userinfo the parser would have refused, but an `https` prefix or
    // an off-loopback host is decided first and echoes the address).
    for carrying in [
        "http://svc:TOKEN@127.0.0.1:9106/",
        "https://svc:TOKEN@127.0.0.1:9106/",
        "http://svc:TOKEN@10.0.0.5:9106/",
        "http://TOKEN@127.0.0.1/",
    ] {
        let error = require_loopback_egress(carrying).expect_err("premise: refused");
        assert!(
            !error.message().contains("TOKEN") && !error.message().contains("svc"),
            "the refusal of an address carrying a credential echoed it: {}",
            error.message()
        );
    }

    // The redaction itself, on text the parser refuses: everything through
    // the last `@` before the parameter region goes, the path after it
    // stays, and the query goes whole.
    assert_eq!(
        redact_for_echo("http://svc:TOK@EN@127.0.0.1:9106/v1?x=1"),
        "http://…@127.0.0.1:9106/v1?…"
    );
    // Corrected 2026-09-13 (round 5): this used to assert an `@` past a
    // real, delimiter-bounded authority was kept verbatim. Two rounds of
    // trying to tell "an `@` that ends a real authority" apart from "an `@`
    // that is the credential a scheme-typo hid past a delimiter" each left
    // a fifth bypass standing — see `redact_for_echo`'s doc comment. The
    // query now goes as a region, which is why the real host survives here
    // rather than being masked as a suspected credential: cutting the
    // parameters off first leaves no `@` in what remains.
    assert_eq!(
        redact_for_echo("http://127.0.0.1:9106/v1?to=a@b"),
        "http://127.0.0.1:9106/v1?…",
        "the query goes whole, so an `@` inside it never has to be judged"
    );
    // Corrected 2026-09-12: this used to assert the scheme-less string was
    // passed through unchanged, on the premise that every credential-bearing
    // string this process handles has a `://`. It does not — see the
    // dedicated regression below — so a scheme-less string carrying an `@`
    // is exactly the case that must redact, not the case that is exempt.
    assert_eq!(redact_for_echo("no scheme@here"), "…@here");

    // The floor under the accepted cost of over-redaction, which nothing
    // pinned until 2026-09-13: a refusal has to stay diagnosable. Redaction
    // may mask the credential, but the operator must still be able to tell
    // *which* of several configured addresses was refused. This holds
    // through a different path than `shown` — the host arm prints
    // `url.host()` from the parse, and a parsed host cannot carry userinfo
    // because `Url::parse` refuses that first — so without this assertion a
    // future round that redacts harder has nothing telling it where the
    // floor is.
    let error = require_loopback_egress("http://someservice/API_SECRET_VALUE@upstream.example")
        .expect_err("premise: an off-loopback host is refused");
    assert!(
        error.message().contains("someservice"),
        "a refusal an operator cannot trace back to the address they set is not diagnosable: {}",
        error.message()
    );
    assert!(
        !error.message().contains("API_SECRET_VALUE"),
        "the one-character `:`→`/` typo must not print the secret it hides: {}",
        error.message()
    );

    // A control character in a refused address does not get to end the log
    // line and start one the operator did not write.
    let forged = redact_for_echo("http://a@127.0.0.1:9105\r\nFATAL forged record");
    assert!(
        !forged.contains('\n') && !forged.contains('\r'),
        "a rejected address carried a line break into the message: {forged:?}"
    );
    assert!(
        forged.contains("\\u{000a}"),
        "the escape should still show what was there, rather than dropping it: {forged:?}"
    );
}

/// The scenario the prior round's fix did not cover: an operator drops the
/// `http://` and sets a base URL as `svc:TOKEN@127.0.0.1:9106`.
/// `redact_for_echo` used to require `"://"` before it would redact anything,
/// so this string — which has none — came back unchanged from both call
/// sites that are supposed to keep a credential out of an error message:
/// `require_loopback_egress`'s own `shown`, and `Url::parse`'s `invalid`
/// closure, which it calls a second time on the same raw string once the
/// parse fails for having no scheme. The result was `TOKEN` printed twice,
/// in the clear, in the fatal start-up error every one of the six egress
/// call sites wraps this in.
///
/// Mutated by reverting `redact_for_echo` to require a scheme (restoring the
/// `let Some((scheme, rest)) = raw.split_once("://") else { return
/// raw.to_string(); }` early return) — confirmed both assertions below then
/// fail, `TOKEN` appearing in the `Display` and the `Debug` of the refusal;
/// restored, confirmed both pass again.
#[test]
fn a_scheme_less_credential_bearing_egress_address_is_still_redacted() {
    use qip_transport::http::require_loopback_egress;

    let error = require_loopback_egress("svc:TOKEN@127.0.0.1:9106")
        .expect_err("a scheme-less address was admitted as an egress address");
    for rendered in [error.to_string(), format!("{error:?}")] {
        assert!(
            !rendered.contains("TOKEN") && !rendered.contains("svc"),
            "the refusal of a scheme-less credential-bearing address echoed the credential: \
             {rendered}"
        );
    }
}

/// The third round's finding, reproduced directly against `redact_for_echo`
/// rather than only through `require_loopback_egress`: a scheme-less
/// credential whose path or query contains the ordinary substring
/// `"://"` — any `?redirect=`, `?callback=` or `?fallback=` parameter
/// naming another URL — used to make `redact_for_echo`'s own
/// `raw.split_once("://")` match on that later occurrence instead of
/// finding no scheme at all, misreading almost the whole string as a
/// "scheme" and leaving the real credential outside anything the function
/// checked for `'@'`.
///
/// The fourth round's finding is here too: a scheme-less string whose first
/// character is already one of the three delimiters (`://`, `//`, `/`, `?`,
/// `#` with the scheme dropped) made the naive authority-candidate empty,
/// or — for a bare `://` with nothing before it — the single character
/// `:`; neither is a real host, so the old code's "no `@` in the candidate,
/// therefore no credential" read as true of a string whose credential sat
/// one character later. That round's fix widened the search only when a
/// `split_authority`-based check said the candidate could not be a real
/// host, and left one shape — `svc/TOKEN@127.0.0.1:9106` — as a documented,
/// deliberately-accepted residual, on the premise that a bare single-label
/// hostname is indistinguishable from a scheme-typo'd credential.
///
/// **The fifth round found that premise didn't hold either.** A fresh
/// security review of the fourth round's fix found the same "could this be
/// a host" check trusted far more than bare single-label words: any
/// `word:validport` shape (`abc:80`), and pure digits (`123`), passed it
/// too — and, because the check ran the same way whether or not a scheme
/// was present, `http://svc/TOKEN@127.0.0.1:9106` and a one-character
/// `:`→`/` typo on an ordinary `http://someservice:TOKEN@host` address
/// both leaked through `require_loopback_egress`'s real refusal message,
/// not just a synthetic string. Four rounds, four leaks, each one the next
/// hole in a heuristic that had just been made one input narrower. See
/// [`qip_transport::http::redact_for_echo`]'s doc comment for why round
/// five drops the heuristic rather than narrowing it again: it redacts
/// through the *last* `@` unconditionally, which means the rows below that
/// used to assert an `@` past a real-looking host was kept now assert it is
/// redacted too — over-redaction, on purpose, in exchange for there being
/// no more "is this a host" judgment left to be wrong about.
///
/// **Round six** closed the half of the defect class that no `@` rule could
/// ever have reached: a credential with no `@` in it, `?api_key=…`, which
/// every arm of `require_loopback_egress` printed in full. The parameter
/// region now goes whole, cut off *before* the `@` search rather than
/// after — see the ordering row below, and the mutation report at the end
/// of this comment for what happens when that order is reversed.
///
/// Table-driven over the full matrix the security review asked for,
/// because every row is the same property — does this string get the
/// exact redaction it should — and a table keeps that property visible
/// rather than restated twenty-eight times with twenty-eight slightly
/// different names.
///
/// # Mutations, and what each one actually printed
///
/// Reported from what a run named, not from what the design implies. Round
/// five's report claimed five specific rows failed under one mutation; the
/// loop below used `assert_eq!` per row at the time, so it aborted at the
/// first mismatch and could not have observed the other four. The loop now
/// collects every mismatch and asserts once, so a mutation run enumerates
/// exactly the rows it broke and the report can quote them.
///
/// * **Deleting the parameter-region cut** (round five's shape, no query or
///   fragment masking): `9 of 28 redaction rows are wrong` — the two
///   `?redirect=` rows, `http://127.0.0.1:9105/x?y=a@b`, the `1nvalid:`
///   row, the bare `?` and `#` rows, and all three round-six rows
///   (`?api_key=SECRETVALUE`, the `?a=1@2&api_key=` ordering row, and the
///   `#token=SECRETVALUE` fragment row).
/// * **Reversing the order** — searching for `@` across the whole remainder
///   first and cutting the parameters only afterwards, which is the shape
///   this round nearly shipped: `4 of 28`, and the one that matters is
///   `http://127.0.0.1:9105/x?a=1@2&api_key=SECRETVALUE` coming back as
///   `http://…@2&api_key=SECRETVALUE`. The `@` inside the query ends the
///   search there and everything past it prints, `SECRETVALUE` included.
///   That is why the cut is first and not second.
///
/// Each mutation restored byte-for-byte afterwards; all 28 rows and all 30
/// tests in this file pass again.
#[test]
fn redact_for_echo_handles_the_full_adversarial_matrix() {
    use qip_transport::http::redact_for_echo;

    let cases: &[(&str, &str, &str)] = &[
        (
            "a later, unrelated `://` in the query must not be read as the scheme boundary — \
             this is the exact defect this round closes",
            "svc:TOKEN@127.0.0.1:9106/callback?redirect=http://evil.example/x",
            "…@127.0.0.1:9106/callback?…",
        ),
        (
            "round 2's case: no scheme, no later `://` either",
            "svc:TOKEN@127.0.0.1:9106",
            "…@127.0.0.1:9106",
        ),
        (
            "a real scheme and a real credential both redact as before",
            "http://svc:TOKEN@127.0.0.1:9105/path",
            "http://…@127.0.0.1:9105/path",
        ),
        (
            "round 5 correction: an `@` in the path used to be kept, on the premise that a \
             real host in front of it proved there was no credential. Two narrower rounds each \
             found a shape that premise didn't cover; this now redacts unconditionally, and the \
             cost is this ordinary-looking path text being masked too",
            "http://127.0.0.1:9105/path@notacredential",
            "http://…@notacredential",
        ),
        (
            "round 5 correction: same reasoning, for a query `@` instead of a path one",
            "http://127.0.0.1:9105/x?y=a@b",
            "http://127.0.0.1:9105/x?…",
        ),
        (
            "two `@` in the authority mask down to the rightmost split",
            "a@b@127.0.0.1",
            "…@127.0.0.1",
        ),
        (
            "a leading digit can never start an RFC 3986 scheme, so this stays scheme-less \
             even though a `1nvalid:` prefix looks scheme-shaped",
            "1nvalid:TOKEN@127.0.0.1:9106/x?y=http://z",
            "…@127.0.0.1:9106/x?…",
        ),
        (
            "no `@` and no `://` anywhere: nothing to redact",
            "just-a-plain-string/path",
            "just-a-plain-string/path",
        ),
        (
            "a legitimate scheme and no credential: unchanged",
            "http://127.0.0.1:9105",
            "http://127.0.0.1:9105",
        ),
        (
            "a later `://` in the query with no `@` anywhere must not be over-corrected into \
             an authority that was never there",
            "http://127.0.0.1:9105?redirect=ftp://other",
            "http://127.0.0.1:9105?…",
        ),
        (
            "the permutation the round-2 code review flagged as untested: no scheme at all, \
             and the `@` sits in the path rather than the authority. Round 5 correction: this \
             used to be the row proving a scheme-less real host protects a path `@` from \
             redaction; that protection is what let `svc/TOKEN@…` and `abc:80/TOKEN@…` hide \
             behind the same reasoning, so it no longer applies here either",
            "127.0.0.1:9105/path@notacredential",
            "…@notacredential",
        ),
        (
            "round 4's finding: a `://` typo'd down to `//`, with the scheme dropped \
             entirely, makes the authority-candidate an empty string — `\"\"` is not a real \
             host under any grammar, so finding no `@` in it must not read as `no credential`",
            "//svc:TOKEN@127.0.0.1:9106",
            "…@127.0.0.1:9106",
        ),
        (
            "round 4's finding, one slash: the same empty-authority shape",
            "/TOKEN@127.0.0.1:9106",
            "…@127.0.0.1:9106",
        ),
        (
            "round 4's finding, a bare `?`: the authority-candidate is still empty",
            "?TOKEN@127.0.0.1:9106",
            "?…",
        ),
        (
            "round 4's finding, a bare `#`: the authority-candidate is still empty",
            "#TOKEN@127.0.0.1:9106",
            "#…",
        ),
        (
            "round 4's finding: `://` with nothing before it makes the authority-candidate \
             the single character `:`, which `split_authority` refuses outright (an empty \
             host after the colon) rather than reading as an empty one — a different shape \
             of `not a real host` than the four rows above. Under the unconditional search \
             it no longer discriminates anything those rows do not; kept because it is one of \
             the six shapes the round-4 review reproduced, and a reproduction is worth keeping \
             even once the design that failed it is gone",
            "://svc:TOKEN@127.0.0.1:9106",
            "…@127.0.0.1:9106",
        ),
        (
            "round 4's documented residual, now actually closed rather than merely \
             documented: round 4 left this unredacted because `svc` parses as a legal \
             single-label hostname, indistinguishable from the row above. Round 5 does not \
             need to distinguish them — both redact now",
            "svc/TOKEN@127.0.0.1:9106",
            "…@127.0.0.1:9106",
        ),
        (
            "round 5's finding: a `word:port` authority-candidate — round 4's fix trusted \
             this unconditionally as `split_authority` parses it to a non-empty host with an \
             explicit port, exactly like a real `127.0.0.1:9105`, with nothing in the string \
             itself to say `abc` is not a real hostname",
            "abc:80/TOKEN@127.0.0.1:9106",
            "…@127.0.0.1:9106",
        ),
        (
            "round 5's finding: pure digits with no scheme, no colon, no dot — round 4's fix \
             trusted this too, since `split_authority` accepts any non-empty string with no \
             colon as a bare host",
            "123//TOKEN@127.0.0.1:9106",
            "…@127.0.0.1:9106",
        ),
        (
            "round 5's finding, and the one that matters most: this leak survives even with \
             an ordinary `http://` scheme present, because round 4's authority check ran the \
             same way whether or not a scheme was found",
            "http://svc/TOKEN@127.0.0.1:9106",
            "http://…@127.0.0.1:9106",
        ),
        (
            "round 5's finding: the scheme'd sibling of the pure-digits row above",
            "http://123//TOKEN@127.0.0.1:9106",
            "http://…@127.0.0.1:9106",
        ),
        (
            "round 5's finding, the realistic one: an operator meant \
             `http://someservice:API_SECRET_VALUE@upstream.example` and mistyped a single \
             character, `:` to `/`. `Url::parse` accepts the result as a syntactically valid \
             URL (host `someservice`, path `/API_SECRET_VALUE@upstream.example`) and only the \
             *later* loopback-host check in `require_loopback_egress` refuses it — through a \
             message built from this function",
            "http://someservice/API_SECRET_VALUE@upstream.example",
            "http://…@upstream.example",
        ),
        (
            "a pre-existing, informational observation from the round-5 security review, fixed \
             as a side effect rather than left standing: two `@` past a real authority used to \
             redact only through the first one, leaving a second credential-shaped chunk past \
             a later delimiter untouched. The unconditional last-`@` search redacts through \
             the last one regardless of how many real-looking authorities came before it",
            "http://127.0.0.1:9105@evil/TOKEN@127.0.0.1:9106",
            "http://…@127.0.0.1:9106",
        ),
        (
            "round 6's finding, and the one no `@` rule could ever have caught: a credential \
             with no `@` in it at all. This is the shape a vendor console hands an operator to \
             copy, and every arm of `require_loopback_egress` printed it in full until the \
             parameter region started going whole",
            "https://api.vendor.example/v1?api_key=SECRETVALUE",
            "https://api.vendor.example/v1?…",
        ),
        (
            "round 6: the ordering that matters. Masking the query only *after* the `@` search \
             would end that search at the `1@2` inside the query and print everything past it, \
             `SECRETVALUE` included. Cutting the parameter region off first makes the bytes \
             unreachable rather than merely unsearched",
            "http://127.0.0.1:9105/x?a=1@2&api_key=SECRETVALUE",
            "http://127.0.0.1:9105/x?…",
        ),
        (
            "round 6: a fragment is a parameter region too, and `Url::parse` refuses one \
             outright — but this function runs on addresses the parser refuses, so it cannot \
             rely on that",
            "http://127.0.0.1:9105/x#token=SECRETVALUE",
            "http://127.0.0.1:9105/x#…",
        ),
        (
            "round 6's named limit, asserted rather than left to be discovered: a credential \
             inside a *path segment* is still printed. Masking it would mean guessing which \
             segment is secret, and guessing which part of a string is sensitive is what \
             produced five consecutive leaks here. The row exists so that the limit is a \
             decision on the record and not a gap somebody finds",
            "http://127.0.0.1:9105/v1/hunter2/chat",
            "http://127.0.0.1:9105/v1/hunter2/chat",
        ),
        (
            "round 6: a control character is escaped rather than copied through, so a rejected \
             address cannot end the log line and begin one the operator did not write",
            "http://127.0.0.1:9105/x\r\nFATAL forged",
            "http://127.0.0.1:9105/x\\u{000d}\\u{000a}FATAL forged",
        ),
    ];

    // Collected rather than asserted row by row, because `assert_eq!` inside
    // the loop panics on the first mismatch and never evaluates the rest —
    // which made round 5's own mutation report claim an enumeration of five
    // failing rows that a single run could not have produced. A mutation run
    // against this loop names every row it broke, so the report can quote
    // what was read instead of what was inferred.
    let mut mismatches = Vec::new();
    for (why, input, expected) in cases {
        let actual = redact_for_echo(input);
        if actual != *expected {
            mismatches.push(format!(
                "  input    {input:?}\n  expected {expected:?}\n  actual   {actual:?}\n  because  {why}"
            ));
        }
        // The premise the whole no-under-redaction argument rests on, pinned
        // per row rather than reasoned about once: `split_scheme`'s grammar
        // excludes `@`, so it can never strip one out of `raw`, so "no `@`
        // in `rest`" means "no `@` in `raw`". If a later edit widened that
        // grammar, the early return would start concluding "no credential"
        // about strings that have one — and every expected-value row above
        // could still pass while it did.
        if input.contains('@') {
            assert_ne!(
                actual, **input,
                "an input containing `@` came back byte-identical, which is what \
                 under-redaction looks like: {input:?}"
            );
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} redaction rows are wrong:\n{}",
        mismatches.len(),
        cases.len(),
        mismatches.join("\n\n")
    );

    // Empty string and a bare `@`: no panic, and the credential-shaped
    // input still masks to something with no raw content in it.
    assert_eq!(redact_for_echo(""), "");
    assert_eq!(redact_for_echo("@"), "…@");
}

/// The security review's own reproduction, against `Url::parse` rather
/// than `redact_for_echo`: the same scheme-less, later-`"://"` string must
/// fall through to the "no scheme" refusal — not be misparsed into
/// [`HttpError::UnsupportedScheme`] carrying the credential as its
/// "scheme" — and that refusal's message must not contain the token.
///
/// Mutated by reverting `Url::parse`'s scheme detection to
/// `raw.split_once("://")` and removing the `debug_assert` beside it (the
/// assert alone would catch a plain `split_scheme` regression first and
/// panic before this test's own assertions ran, which proves the guard
/// works but not what the pre-fix code actually returned) — confirmed this
/// then returns
/// `UnsupportedScheme { scheme: "svc:token@127.0.0.1:9106/callback?redirect=http" }`
/// instead of `InvalidUrl`, and the token survives in that field in the
/// clear; restored byte-for-byte, confirmed `InvalidUrl` with no token is
/// produced again.
#[test]
fn a_scheme_less_credential_with_a_later_marker_falls_through_to_invalid_url_not_unsupported_scheme()
 {
    let raw = "svc:TOKEN@127.0.0.1:9106/callback?redirect=http://evil.example/x";
    let error = Url::parse(raw).expect_err("a scheme-less credential-bearing string parsed");

    assert!(
        matches!(error, HttpError::InvalidUrl { .. }),
        "expected the no-scheme refusal, got {error:?}, which means the later `://` in the \
         query was read as a scheme boundary"
    );
    assert_eq!(
        error.code(),
        "invalid_url",
        "a scheme-less string must never be classified as unsupported_scheme, because that \
         variant's Display prints its field with no redaction"
    );
    for rendered in [error.to_string(), format!("{error:?}")] {
        assert!(
            !rendered.contains("TOKEN") && !rendered.contains("svc"),
            "the no-scheme refusal echoed the credential: {rendered}"
        );
    }
}

/// The legitimate case `split_scheme`'s anchoring must not break: a
/// well-formed but genuinely unsupported scheme still produces
/// `UnsupportedScheme` with exactly that scheme as a clean token.
#[test]
fn a_genuinely_unsupported_scheme_is_still_reported_with_a_clean_token() {
    let error = Url::parse("ftp://127.0.0.1:9105").expect_err("ftp was accepted");
    assert_eq!(
        error,
        HttpError::UnsupportedScheme {
            scheme: "ftp".to_string()
        }
    );
}

/// `HttpError::UnsupportedScheme.scheme` can never carry `@`, `:` or `/`,
/// across a set of inputs chosen to try to produce one: a credential
/// before a real scheme, a credential with no scheme at all but a later
/// `"://"`, and a path-shaped string with no scheme. Each is checked
/// structurally — by reading the field when the variant is reached at all
/// — rather than trusted to be clean because the code review said so.
#[test]
fn unsupported_scheme_never_carries_unbounded_content() {
    let adversarial = [
        "svc:TOKEN@127.0.0.1:9106/callback?redirect=http://evil.example/x",
        "1nvalid:TOKEN@127.0.0.1:9106/x?y=http://z",
        "no-scheme-at-all/path?x=http://y",
        "http://svc:TOKEN@127.0.0.1:9106/path",
        "",
        "@",
        "://leading-marker-with-nothing-before-it",
    ];
    for raw in adversarial {
        if let Err(HttpError::UnsupportedScheme { scheme }) = Url::parse(raw) {
            assert!(
                scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')),
                "input {raw:?} reached UnsupportedScheme with a field outside the RFC 3986 \
                 scheme grammar: {scheme:?}"
            );
            assert!(
                !scheme.contains('@') && !scheme.contains(':') && !scheme.contains('/'),
                "input {raw:?} produced an UnsupportedScheme field that could carry a \
                 credential: {scheme:?}"
            );
        }
    }
}

#[test]
fn a_url_parses_into_the_three_parts_a_request_needs() {
    let url = Url::parse("http://cell-us-east:8080/v1/mesh/publish?since=7")
        .expect("a well-formed URL was refused");
    assert_eq!(url.host(), "cell-us-east");
    assert_eq!(url.port(), 8080);
    assert_eq!(url.target(), "/v1/mesh/publish?since=7");
    assert_eq!(url.authority(), "cell-us-east:8080");

    let default_port = Url::parse("http://central.internal").expect("a bare host was refused");
    assert_eq!(default_port.port(), 80);
    assert_eq!(
        default_port.target(),
        "/",
        "a URL with no path must still put a target on the request line"
    );

    let ipv6 = Url::parse("http://[::1]:9000/health").expect("an IPv6 literal was refused");
    assert_eq!(ipv6.host(), "::1");
    assert_eq!(ipv6.port(), 9000);
    assert_eq!(ipv6.authority(), "[::1]:9000");
}

#[test]
fn a_path_carrying_a_control_character_cannot_split_the_request_line() {
    for hostile in [
        "http://peer/v1/mesh\r\nx-injected: yes",
        "http://peer/v1 /mesh",
    ] {
        assert!(
            Url::parse(hostile).is_err(),
            "{hostile} would have been written into the request line as-is"
        );
    }
}

// --- the happy path, and what it proves about the request ---------------

#[test]
fn a_response_is_read_back_whole() {
    let server = TestServer::always(Action::json(200, r#"{"accepted":1}"#));
    let client = HttpClient::new(tight());

    let response = client
        .get(&server.url_for("/v1/mesh/health"))
        .expect("a well-formed response failed to read");

    assert_eq!(response.status, 200);
    assert_eq!(response.body_as_str().expect("utf-8"), r#"{"accepted":1}"#);
    assert_eq!(
        response.header("content-type"),
        Some("application/json"),
        "header names must be matched without regard to what the peer capitalised"
    );
}

#[test]
fn the_client_writes_each_framing_header_exactly_once_even_when_a_caller_supplies_one() {
    let server = TestServer::always(Action::json(200, "{}"));
    let client = HttpClient::new(tight());

    // A caller trying to set the framing headers by hand. Two content-length
    // headers, or one that disagrees with the body, is the original
    // request-smuggling bug, so these are dropped rather than merged.
    let request = HttpRequest::json(
        Method::Post,
        &server.url_for("/v1/mesh/publish"),
        b"{\"sender\":\"x\"}".to_vec(),
    )
    .expect("a well-formed request was refused")
    .with_header("content-length", "999999")
    .with_header("host", "somewhere-else")
    .with_header("x-region", "us-east");

    client.send(&request).expect("the request failed");

    let seen = server.requests();
    let received = seen.first().expect("the server saw no request");
    assert_eq!(received.header_counts.get("content-length"), Some(&1));
    assert_eq!(received.header_counts.get("host"), Some(&1));
    assert_eq!(
        received.headers.get("content-length").map(String::as_str),
        Some("14"),
        "the declared length must be the body actually written, not the one a caller asked for"
    );
    assert_eq!(
        received.headers.get("host").map(String::as_str),
        Some(&server.url()[7..]),
        "the host header must name the peer actually connected to"
    );
    assert_eq!(
        received.headers.get("x-region").map(String::as_str),
        Some("us-east"),
        "a header that is not reserved must still be written"
    );
    assert_eq!(received.body_as_str(), r#"{"sender":"x"}"#);
}

#[test]
fn a_head_response_is_read_without_waiting_for_a_body_that_is_not_coming() {
    // A HEAD answer carries the content-length of the body it would have sent
    // and none of the bytes. A client that read the declared length would hang
    // until its own timeout.
    let server = TestServer::always(Action::Raw(
        b"HTTP/1.1 200 OK\r\ncontent-length: 4096\r\nconnection: close\r\n\r\n".to_vec(),
    ));
    let client = HttpClient::new(tight());
    let request = HttpRequest::new(Method::Head, &server.url_for("/v1/mesh/health"))
        .expect("a well-formed request was refused");

    let response = client.send(&request).expect("the HEAD request failed");
    assert_eq!(response.status, 200);
    assert!(response.body.is_empty());
}

#[test]
fn a_status_with_no_body_is_not_waited_on() {
    let server = TestServer::always(Action::Raw(
        b"HTTP/1.1 204 No Content\r\nconnection: close\r\n\r\n".to_vec(),
    ));
    let client = HttpClient::new(tight());
    let response = client
        .get(&server.url_for("/v1/mesh/health"))
        .expect("a 204 failed to read");
    assert_eq!(response.status, 204);
    assert!(response.body.is_empty());
}

// --- chunked ------------------------------------------------------------

#[test]
fn a_chunked_response_is_reassembled_in_order_including_its_trailers() {
    let server = TestServer::always(Action::Chunked {
        status: 200,
        chunks: vec![
            r#"{"frames":["#.to_string(),
            r#"{"position":1},"#.to_string(),
            r#"{"position":2}]}"#.to_string(),
        ],
    });
    let client = HttpClient::new(tight());

    let response = client
        .get(&server.url_for("/v1/mesh/poll"))
        .expect("a chunked response failed to read");

    assert_eq!(response.status, 200);
    assert_eq!(
        response.body_as_str().expect("utf-8"),
        r#"{"frames":[{"position":1},{"position":2}]}"#,
        "the chunks must be concatenated in order with no framing bytes left in"
    );
}

#[test]
fn a_chunked_body_that_would_exceed_the_limit_is_refused_at_the_chunk_that_crosses_it() {
    // Three chunks of 200 bytes against a 512-byte limit: the first two fit
    // and the third is refused, so the refusal happens on the chunk header
    // rather than after 600 bytes have been buffered.
    let server = TestServer::always(Action::Chunked {
        status: 200,
        chunks: vec!["x".repeat(200), "x".repeat(200), "x".repeat(200)],
    });
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/v1/mesh/poll"))
        .expect_err("a chunked body over the limit was accepted");

    assert_eq!(error.code(), "body_too_large");
    assert!(
        !error.is_transient(),
        "a peer that sends too much will send too much again"
    );
}

#[test]
fn declaring_both_a_length_and_chunked_encoding_is_refused() {
    let server = TestServer::always(Action::Raw(
        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\ntransfer-encoding: chunked\r\nconnection: \
          close\r\n\r\n2\r\nhi\r\n0\r\n\r\n"
            .to_vec(),
    ));
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/v1/mesh/poll"))
        .expect_err("a response with two framings was accepted");

    assert_eq!(error.code(), "malformed");
    assert!(
        error.to_string().contains("where the body ends"),
        "the refusal must say what the ambiguity is: {error}"
    );
}

// --- the failure modes that matter --------------------------------------

#[test]
fn a_peer_that_closes_mid_body_is_a_close_and_not_a_short_read() {
    let server = TestServer::always(Action::Truncated {
        declared: 400,
        written: 10,
    });
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/v1/mesh/poll"))
        .expect_err("a truncated body was accepted as complete");

    assert_eq!(
        error.code(),
        "closed_early",
        "a body that stopped short must never be returned as if it were whole: {error}"
    );
    assert!(
        matches!(error, HttpError::ClosedEarly { phase: Phase::Body }),
        "the error must say where it stopped: {error:?}"
    );
    assert!(
        error.is_transient(),
        "a peer that died mid-response may be alive on the next attempt"
    );
}

#[test]
fn a_declared_body_over_the_limit_is_refused_before_it_is_read() {
    let server = TestServer::always(Action::Oversized { bytes: 4096 });
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/v1/mesh/poll"))
        .expect_err("an oversized body was accepted");

    match error {
        HttpError::BodyTooLarge { limit, at_least } => {
            assert_eq!(limit, 512);
            assert_eq!(
                at_least, 4096,
                "the declared length is what was refused, which is the evidence that nothing was \
                 allocated for it"
            );
        }
        other => panic!("expected a body-too-large refusal, got {other:?}"),
    }
}

#[test]
fn a_body_with_no_framing_at_all_is_still_bounded() {
    // No content-length and no chunking: the body ends when the connection
    // does, which is a peer's licence to send forever.
    let server = TestServer::always(Action::Unframed { bytes: 4096 });
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/v1/mesh/poll"))
        .expect_err("an unframed body over the limit was accepted");

    assert_eq!(error.code(), "body_too_large");
}

#[test]
fn a_body_exactly_at_the_limit_is_accepted() {
    // The boundary, in both directions: 512 is accepted, 513 is not. An
    // off-by-one here would either refuse legitimate traffic or admit one byte
    // more than the limit claims.
    let client = HttpClient::new(tight());

    let at_limit = TestServer::always(Action::Oversized { bytes: 512 });
    let response = client
        .get(&at_limit.url_for("/x"))
        .expect("a body exactly at the limit was refused");
    assert_eq!(response.body.len(), 512);

    let over = TestServer::always(Action::Oversized { bytes: 513 });
    assert!(
        client.get(&over.url_for("/x")).is_err(),
        "one byte over the limit was accepted"
    );
}

#[test]
fn a_peer_that_says_nothing_trips_the_read_timeout_rather_than_waiting_forever() {
    // The server holds the connection open for well past the client's read
    // timeout. Without the timeout this test would never return, which is
    // exactly the production failure.
    let server = TestServer::always(Action::Silent(StdDuration::from_millis(1_200)));
    let client = HttpClient::new(tight());

    let started = std::time::Instant::now();
    let error = client
        .get(&server.url_for("/v1/mesh/poll"))
        .expect_err("a silent peer was waited on indefinitely");
    let elapsed = started.elapsed();

    assert!(
        matches!(
            error,
            HttpError::ReadTimeout {
                phase: Phase::StatusLine,
                ..
            }
        ),
        "the timeout must name where it gave up: {error:?}"
    );
    assert!(
        elapsed < StdDuration::from_millis(1_000),
        "the client waited {elapsed:?}, which is past its own 200ms read timeout"
    );
    assert!(
        error.is_transient(),
        "a peer that was slow once may not be slow next time"
    );
}

#[test]
fn a_peer_that_accepts_and_closes_without_answering_is_reported_as_a_close() {
    let server = TestServer::always(Action::Hangup);
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/v1/mesh/poll"))
        .expect_err("a connection that produced no response was accepted");

    assert!(
        matches!(
            error,
            HttpError::ClosedEarly {
                phase: Phase::StatusLine
            }
        ),
        "expected a close during the status line, got {error:?}"
    );
}

#[test]
fn a_refused_connection_is_typed_and_transient() {
    let address = address_with_no_listener();
    let client = HttpClient::new(tight());

    let error = client
        .get(&format!("{address}/v1/mesh/publish"))
        .expect_err("connecting to a closed port succeeded");

    assert_eq!(
        error.code(),
        "connect_failed",
        "a refused connection must be distinguishable from a peer that answered badly: {error}"
    );
    assert!(
        error.is_transient(),
        "a peer that is restarting refuses connections, and that is the case retries exist for"
    );
}

#[test]
fn a_response_that_is_not_http_is_malformed_and_is_not_retried() {
    let server = TestServer::always(Action::Raw(b"GARBAGE\r\n\r\n".to_vec()));
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/v1/mesh/poll"))
        .expect_err("a non-HTTP response was accepted");

    assert!(
        matches!(
            error,
            HttpError::Malformed {
                phase: Phase::StatusLine,
                ..
            }
        ),
        "expected a malformed status line, got {error:?}"
    );
    assert!(
        !error.is_transient(),
        "whatever is on that port will answer the same way next time, and spending a retry ladder \
         on it delays every message behind it"
    );
}

#[test]
fn a_header_list_that_never_ends_is_refused() {
    let mut response = b"HTTP/1.1 200 OK\r\n".to_vec();
    for index in 0..40 {
        response.extend_from_slice(format!("x-filler-{index}: value\r\n").as_bytes());
    }
    response.extend_from_slice(b"content-length: 0\r\nconnection: close\r\n\r\n");

    let server = TestServer::always(Action::Raw(response));
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/x"))
        .expect_err("an unbounded header list was accepted");
    assert_eq!(error.code(), "too_many_headers");
}

#[test]
fn a_single_header_longer_than_the_limit_is_refused() {
    let mut response = b"HTTP/1.1 200 OK\r\nx-huge: ".to_vec();
    response.extend(std::iter::repeat_n(b'v', 4096));
    response.extend_from_slice(b"\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");

    let server = TestServer::always(Action::Raw(response));
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/x"))
        .expect_err("an oversized header line was accepted");
    assert_eq!(error.code(), "line_too_long");
}

#[test]
fn two_content_length_headers_that_disagree_are_refused() {
    let server = TestServer::always(Action::Raw(
        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\ncontent-length: 40\r\nconnection: \
          close\r\n\r\nhi"
            .to_vec(),
    ));
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/x"))
        .expect_err("two disagreeing content-lengths were accepted");
    assert_eq!(error.code(), "malformed");
}

#[test]
fn a_status_that_is_not_a_success_is_a_response_and_not_an_error() {
    // The client's job is to get the answer back. Whether a 503 should be
    // retried and a 400 dead-lettered is the transport's decision, and it
    // cannot make it if the client has collapsed both into `Err`.
    let server = TestServer::always(Action::json(503, r#"{"error":"the inbox is full"}"#));
    let client = HttpClient::new(tight());

    let response = client
        .get(&server.url_for("/v1/mesh/publish"))
        .expect("a 503 was reported as a transport failure");
    assert_eq!(response.status, 503);
    assert!(!response.is_success());
    assert!(response.body_excerpt().contains("inbox is full"));
}

#[test]
fn every_http_error_is_classified_and_no_variant_is_left_unnamed() {
    // A property over the whole error surface: each one has a stable code, and
    // no two share one. A new variant added without a code would be reported
    // as whichever existing one it was copied from.
    let errors = [
        HttpError::InvalidUrl {
            url: "x".into(),
            detail: "y".into(),
        },
        HttpError::UnsupportedScheme {
            scheme: "https".into(),
        },
        HttpError::Resolve {
            authority: "x".into(),
            detail: "y".into(),
        },
        HttpError::NoAddress {
            authority: "x".into(),
        },
        HttpError::ConnectFailed {
            address: "x".into(),
            detail: "y".into(),
        },
        HttpError::ConnectTimeout {
            authority: "x".into(),
            after: StdDuration::from_secs(1),
        },
        HttpError::WriteFailed { detail: "y".into() },
        HttpError::ReadTimeout {
            phase: Phase::Body,
            after: StdDuration::from_secs(1),
        },
        HttpError::ClosedEarly { phase: Phase::Body },
        HttpError::ReadFailed {
            phase: Phase::Body,
            detail: "y".into(),
        },
        HttpError::Malformed {
            phase: Phase::Body,
            detail: "y".into(),
        },
        HttpError::BodyTooLarge {
            limit: 1,
            at_least: 2,
        },
        HttpError::LineTooLong {
            phase: Phase::Headers,
            limit: 1,
        },
        HttpError::TooManyHeaders { limit: 1 },
    ];

    let mut codes = std::collections::BTreeSet::new();
    for error in &errors {
        assert!(
            codes.insert(error.code()),
            "{} is used by two variants, so metrics cannot tell them apart",
            error.code()
        );
        assert!(
            !error.to_string().is_empty(),
            "{} renders as nothing",
            error.code()
        );
        // Every one has to become a platform error without losing its message.
        let platform: qip_core::Error = error.clone().into();
        assert!(
            platform.message().contains(
                error
                    .to_string()
                    .split(':')
                    .next()
                    .unwrap_or_default()
                    .trim()
            ) || !platform.message().is_empty(),
            "{} loses its detail when it crosses into qip_core::Error",
            error.code()
        );
    }
    assert_eq!(codes.len(), errors.len());
}
