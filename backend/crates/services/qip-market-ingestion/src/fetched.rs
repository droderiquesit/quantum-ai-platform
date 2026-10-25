//! The one way a vendor adapter in this crate reads an HTTP response
//! (R12-SEC-9065, ADR 0158 decision 16).
//!
//! [`crate::rest`], [`crate::depth`], [`crate::narrative`] and
//! [`crate::alternative`] each send through `qip_transport::HttpClient`, and
//! until 2026-10-24 each handed the body straight to `serde_json` as a
//! `&str` — so a vendor's bytes were, for the length of a function, a string
//! like any other, and the next line could as easily have formatted them into
//! something that acts. Here the body leaves the socket as
//! [`Untrusted<Vec<u8>>`] and reaches the adapter's wire type only through
//! [`decode_json`], which is a `validate`.
//!
//! What the validation is, said plainly: UTF-8, then the adapter's own wire
//! schema, refusing anything that does not fit. It is a structural check, not
//! a judgement of the text — a headline that decodes is still a stranger's
//! headline inside the wire struct. What it buys is that the path from a
//! socket to a typed record is one named step, and the only other way to the
//! bytes is [`excerpt`], a bounded quote for an operator's error message.

use qip_core::error::{Error, Result};
use qip_core::untrusted::Untrusted;
use qip_transport::{HttpClient, HttpRequest};
use serde::Deserialize;

/// How many bytes of a body an error message quotes. The same bound
/// `qip_transport::HttpResponse::body_excerpt` kept before the body was
/// wrapped, so an operator's refusal text reads as it did.
const EXCERPT_BYTES: usize = 200;

/// Send `request` and hand back a successful answer's body, untrusted.
///
/// `origin` names the vendor for the wrapper's `Debug`. A non-2xx answer is
/// refused through `refuse(status, excerpt)`, the adapter's own reading of
/// what the status means for an operator.
pub(crate) fn get(
    client: &HttpClient,
    request: &HttpRequest,
    origin: &str,
    refuse: impl FnOnce(u16, &str) -> Error,
) -> Result<Untrusted<Vec<u8>>> {
    let response = client.send(request).map_err(Error::from)?;
    let status = response.status;
    let success = response.is_success();
    let body = Untrusted::new(origin, response.body);
    if !success {
        return Err(refuse(status, &excerpt(&body)));
    }
    Ok(body)
}

/// The first bytes of a body, for an error message an operator reads.
/// Bounded, with line breaks flattened so one quote cannot forge the next
/// line of a log.
pub(crate) fn excerpt(body: &Untrusted<Vec<u8>>) -> String {
    let bytes = body.inspect();
    let cut = bytes.len().min(EXCERPT_BYTES);
    String::from_utf8_lossy(&bytes[..cut]).replace(['\r', '\n'], " ")
}

/// Validate `body` into the adapter's wire type `T`, or refuse.
///
/// `sender` begins the refusal — "`vendor` sent a body", "`vendor` sent a
/// depth updates body" — and the excerpt ends it, so the message names who
/// sent what and shows the operator the first bytes.
pub(crate) fn decode_json<T: for<'de> Deserialize<'de>>(
    body: Untrusted<Vec<u8>>,
    sender: &str,
) -> Result<T> {
    let quoted = excerpt(&body);
    body.validate(|bytes| {
        let text = std::str::from_utf8(&bytes)
            .map_err(|error| Error::schema(format!("{sender} that is not valid UTF-8: {error}")))?;
        serde_json::from_str(text).map_err(|error| {
            Error::schema(format!(
                "{sender} this decoder cannot read: {error}. The first bytes of it were: {quoted}"
            ))
        })
    })
}
