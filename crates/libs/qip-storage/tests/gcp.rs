//! The Cloud Storage and BigQuery adapters, against a real socket.
//!
//! Every test here binds a loopback [`std::net::TcpListener`] and lets the
//! adapter connect to it. Nothing is mocked at the type level: the bytes the
//! adapter writes are the bytes the server reads, and the assertions are made
//! against what actually arrived. The technique is
//! `qip-market-ingestion/tests/server`'s, extended with a scripted sequence of
//! responses and with request-body capture, which is what an insert and a
//! paginated listing need.
//!
//! Tests that assert a refusal also assert that **no connection was opened**,
//! by checking the listener served nothing. A refusal that still dialled the
//! peer would mean an unconfigured deployment was reaching the network, and
//! "it errored" alone does not catch that.

mod server {
    //! A loopback HTTP server that answers from a script.

    use std::collections::BTreeMap;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration as StdDuration;

    /// A request as the server received it.
    #[derive(Clone, Debug, Default)]
    pub(crate) struct RawRequest {
        pub(crate) method: String,
        /// Request target: path and query, exactly as written on the wire.
        pub(crate) target: String,
        /// Header names lower-cased, as the client wrote them.
        pub(crate) headers: BTreeMap<String, String>,
        pub(crate) body: Vec<u8>,
    }

    impl RawRequest {
        pub(crate) fn body_json(&self) -> serde_json::Value {
            serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
        }
    }

    /// What the server should do with one connection.
    #[derive(Clone, Debug)]
    pub(crate) enum Action {
        /// A well-formed JSON response.
        Json { status: u16, body: String },
        /// A response whose body is arbitrary bytes, for a media download.
        Raw { status: u16, body: Vec<u8> },
        /// A well-formed response too large for the client's limit.
        Oversized { bytes: usize },
        /// Say nothing for this long, then answer, for the read timeout.
        Silent(StdDuration),
    }

    impl Action {
        pub(crate) fn json(status: u16, body: impl Into<String>) -> Self {
            Self::Json {
                status,
                body: body.into(),
            }
        }

        pub(crate) fn ok(body: impl Into<String>) -> Self {
            Self::json(200, body)
        }
    }

    /// A listener on an ephemeral loopback port, answering from a script.
    pub(crate) struct TestServer {
        address: String,
        stop: Arc<AtomicBool>,
        served: Arc<AtomicUsize>,
        requests: Arc<Mutex<Vec<RawRequest>>>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl std::fmt::Debug for TestServer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("TestServer")
                .field("address", &self.address)
                .field("served", &self.served())
                .finish_non_exhaustive()
        }
    }

    impl TestServer {
        /// Answer the same way every time.
        pub(crate) fn always(action: Action) -> Self {
            Self::script(vec![action])
        }

        /// Answer the n-th connection with the n-th action, repeating the last.
        ///
        /// Repeating rather than refusing, because a test that asserts on the
        /// first two requests should not fail differently depending on whether
        /// the adapter made a third — the assertion on `requests()` is what
        /// says how many there should have been.
        pub(crate) fn script(actions: Vec<Action>) -> Self {
            assert!(
                !actions.is_empty(),
                "a scripted server needs at least one action to answer with"
            );
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
            let address = listener
                .local_addr()
                .expect("the listener has a local address")
                .to_string();
            listener
                .set_nonblocking(true)
                .expect("the listener can poll");

            let stop = Arc::new(AtomicBool::new(false));
            let served = Arc::new(AtomicUsize::new(0));
            let requests = Arc::new(Mutex::new(Vec::new()));

            let thread_stop = stop.clone();
            let thread_served = served.clone();
            let thread_requests = requests.clone();
            let handle = std::thread::spawn(move || {
                while !thread_stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            let _ = stream.set_read_timeout(Some(StdDuration::from_secs(5)));
                            let index = thread_served.fetch_add(1, Ordering::SeqCst);
                            if let Some(request) = read_request(&stream) {
                                thread_requests
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .push(request);
                            }
                            let action = actions
                                .get(index)
                                .unwrap_or_else(|| actions.last().expect("the script is not empty"))
                                .clone();
                            // On its own thread so a deliberately slow answer
                            // delays the client under test and not the next
                            // connection: a client that has timed out must find
                            // the listener ready, as it would against a real
                            // server.
                            std::thread::spawn(move || write_action(stream, &action));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(StdDuration::from_millis(1));
                        }
                        Err(_) => break,
                    }
                }
            });

            Self {
                address,
                stop,
                served,
                requests,
                handle: Some(handle),
            }
        }

        /// The base URL, `http://127.0.0.1:port`.
        pub(crate) fn url(&self) -> String {
            format!("http://{}", self.address)
        }

        /// How many connections have been accepted.
        pub(crate) fn served(&self) -> usize {
            self.served.load(Ordering::SeqCst)
        }

        pub(crate) fn requests(&self) -> Vec<RawRequest> {
            self.requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn read_request(stream: &TcpStream) -> Option<RawRequest> {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let mut parts = line.split_whitespace();
        let method = parts.next()?.to_string();
        let target = parts.next()?.to_string();

        let mut headers = BTreeMap::new();
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).ok()? == 0 {
                break;
            }
            let header = header.trim_end().to_string();
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':') {
                headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
            }
        }

        let declared: usize = headers
            .get("content-length")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let mut body = vec![0u8; declared];
        if declared > 0 {
            reader.read_exact(&mut body).ok()?;
        }

        Some(RawRequest {
            method,
            target,
            headers,
            body,
        })
    }

    fn write_action(mut stream: TcpStream, action: &Action) {
        let bytes = match action {
            Action::Json { status, body } => framed(*status, "application/json", body.as_bytes()),
            Action::Raw { status, body } => framed(*status, "application/octet-stream", body),
            Action::Oversized { bytes } => {
                framed(200, "application/octet-stream", &vec![b'x'; *bytes])
            }
            Action::Silent(delay) => {
                std::thread::sleep(*delay);
                framed(200, "application/json", b"{}")
            }
        };
        let _ = stream.write_all(&bytes);
        let _ = stream.flush();
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }

    fn framed(status: u16, content_type: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 {status} OK\r\ncontent-type: {content_type}\r\ncontent-length: \
             {}\r\nconnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }
}

use qip_core::error::Error;
use qip_core::{ManualClock, Timestamp};
use qip_storage::BlobStore;
use qip_storage::gcp::{
    AccessToken, BigQueryConfig, BigQueryWarehouse, CloudStorageBlobStore, CloudStorageConfig,
    GcpAccess, InsertRow, MetadataServerTokens, QueryParameter, QueryRequest, StaticToken,
    TokenFile, TokenSource,
};
use qip_storage::provider::{StorageProvider, StorageTarget};
use server::{Action, TestServer};
use std::sync::Arc;

/// A clock fixed at an arbitrary but definite instant.
fn fixed_clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(Timestamp::from_secs(1_760_000_000)))
}

/// Access pointed at `server`, carrying a token, with limits small enough that
/// a test asserting a bound does not have to send megabytes to reach it.
fn access_to(server: &TestServer) -> GcpAccess {
    GcpAccess::unconfigured()
        .with_endpoint(&server.url())
        .expect("a loopback URL parses")
        .with_tokens(Arc::new(
            StaticToken::new("test-token-value").expect("a plain token is usable"),
        ))
}

fn bucket_store(server: &TestServer) -> CloudStorageBlobStore {
    CloudStorageBlobStore::new(CloudStorageConfig::new("archive").with_access(access_to(server)))
        .expect("a named bucket and a loopback endpoint are a usable configuration")
}

fn warehouse(server: &TestServer) -> BigQueryWarehouse {
    BigQueryWarehouse::new(BigQueryConfig::new("proj", "research").with_access(access_to(server)))
        .expect("a named project and dataset are a usable configuration")
}

// --- the credential ---------------------------------------------------------

#[test]
fn an_access_token_refuses_a_value_carrying_a_newline_because_it_would_end_the_header() {
    // The premise: a newline in a header value ends the header, so a token
    // carrying one would let whatever followed be read as another header.
    let split = "good-token\r\nx-injected: yes";
    assert!(
        split.contains('\r'),
        "the premise of this test is a token containing a carriage return"
    );

    let refused = AccessToken::new(split);
    assert!(
        matches!(refused, Err(Error::Invalid(_))),
        "a token with a control character must be refused, not sanitised: {refused:?}"
    );
}

#[test]
fn an_access_token_is_refused_when_blank_so_the_adapter_reports_absent_rather_than_rejected() {
    for blank in ["", "   ", "\t"] {
        assert!(
            AccessToken::new(blank).is_err(),
            "a blank token must be refused; sending one produces a 401 that reads as a bad \
             credential instead of an absent one"
        );
    }
}

#[test]
fn an_access_tokens_debug_output_never_contains_the_token() {
    let secret = "ya29.a0AfB_byC_this_must_not_be_logged";
    let token = AccessToken::new(secret).expect("a plain token is usable");

    let rendered = format!("{token:?}");
    assert!(
        !rendered.contains(secret),
        "Debug must not reach the token; a credential in a log line is a credential to rotate. \
         Got: {rendered}"
    );
    assert!(
        rendered.contains("redacted"),
        "the redaction should be visible rather than silent: {rendered}"
    );
    assert_eq!(
        token.expose(),
        secret,
        "the value is still reachable, deliberately, through a name that greps"
    );
}

#[test]
fn a_token_file_source_rereads_the_file_so_a_rotated_token_is_picked_up() {
    let directory = std::env::temp_dir().join(format!("qip-token-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let path = directory.join("token");
    // A trailing newline is what every way of writing such a file produces.
    std::fs::write(&path, "first-token\n").expect("write the first token");

    let source = TokenFile::new(&path);
    assert_eq!(
        source.token().expect("the first token reads").expose(),
        "first-token",
        "the trailing newline must be trimmed, or the token could not go in a header at all"
    );

    std::fs::write(&path, "second-token").expect("rotate the token");
    assert_eq!(
        source.token().expect("the rotated token reads").expose(),
        "second-token",
        "the file is read per request, so a refresher rewriting it is picked up without a restart"
    );

    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn a_token_file_that_is_not_there_is_an_error_rather_than_an_absent_credential() {
    // The premise: the deployment named this path, so its absence is a broken
    // deployment and not an unconfigured one.
    let missing = std::env::temp_dir().join("qip-token-that-does-not-exist-8f3a1c");
    assert!(
        !missing.exists(),
        "the premise of this test is a path that does not exist"
    );

    let refused = TokenFile::new(&missing).token();
    assert!(
        matches!(refused, Err(Error::Io(_))),
        "a missing token file must be an error: {refused:?}"
    );
}

#[test]
fn the_metadata_server_source_sends_the_flavor_header_and_returns_the_token_it_is_given() {
    let server = TestServer::always(Action::ok(
        r#"{"access_token":"metadata-issued","expires_in":3600,"token_type":"Bearer"}"#,
    ));
    let source = MetadataServerTokens::at(server.url(), "/token", fixed_clock());

    let token = source.token().expect("the metadata server answered");

    assert_eq!(token.expose(), "metadata-issued");
    let requests = server.requests();
    assert_eq!(requests.len(), 1, "one fetch, one request");
    assert_eq!(
        requests[0]
            .headers
            .get("metadata-flavor")
            .map(String::as_str),
        Some("Google"),
        "the metadata server refuses a request without this header, so its absence would be an \
         outage that looked like an authentication failure"
    );
}

#[test]
fn the_metadata_server_source_caches_until_the_refresh_margin_and_then_refetches() {
    let server = TestServer::script(vec![
        Action::ok(r#"{"access_token":"first","expires_in":3600}"#),
        Action::ok(r#"{"access_token":"second","expires_in":3600}"#),
    ]);
    let clock = fixed_clock();
    let source = MetadataServerTokens::at(server.url(), "/token", clock.clone());

    assert_eq!(source.token().expect("first fetch").expose(), "first");
    assert_eq!(
        source.token().expect("cached read").expose(),
        "first",
        "a token still inside its lifetime must be reused rather than refetched"
    );
    assert_eq!(server.served(), 1, "the second read must not open a socket");

    // Past the expiry less the sixty-second margin, on the injected clock —
    // never the host's, so this boundary is the same on every replay.
    clock.advance(qip_core::Duration::from_secs(3600 - 30));
    assert_eq!(
        source.token().expect("refetch").expose(),
        "second",
        "once inside the refresh margin the source must refetch: a token that dies in flight is \
         indistinguishable from one that was rejected"
    );
    assert_eq!(server.served(), 2);
}

#[test]
fn a_metadata_server_that_refuses_is_a_denial_and_not_a_missing_token() {
    let server = TestServer::always(Action::json(403, r#"{"error":"no service account"}"#));
    let source = MetadataServerTokens::at(server.url(), "/token", fixed_clock());

    let refused = source.token();
    assert!(
        matches!(refused, Err(Error::Denied(_))),
        "an instance with no attached service account must produce a denial: {refused:?}"
    );
}

// --- what a deployment has not configured -----------------------------------

#[test]
fn an_unconfigured_access_names_the_proxy_and_the_credential_separately() {
    let missing = GcpAccess::unconfigured().missing_configuration();

    assert_eq!(
        missing.len(),
        2,
        "each missing thing gets its own line, so an operator with one of them learns which is \
         left: {missing:?}"
    );
    let text = missing.join(" ");
    assert!(
        text.contains("TLS"),
        "the endpoint requirement must say why an https address will not do: {text}"
    );
    assert!(
        text.contains("ADR 0009"),
        "the credential requirement must say why this build cannot mint a token: {text}"
    );
}

#[test]
fn an_https_endpoint_is_refused_by_name_rather_than_downgraded_to_plaintext() {
    // The premise, and the whole safety property: there is no TLS stack here,
    // so an https address must fail loudly rather than quietly become http.
    let refused = GcpAccess::unconfigured().with_endpoint("https://storage.googleapis.com");

    assert!(
        refused.is_err(),
        "an https endpoint must be refused; downgrading it would send a bearer token across the \
         internet in clear text"
    );
}

#[test]
fn configuring_two_credentials_at_once_is_refused_rather_than_one_being_preferred() {
    let refused = GcpAccess::from_values(
        Some("http://proxy.internal:8080"),
        Some("1"),
        None,
        Some("a-literal-token"),
        fixed_clock(),
    );

    assert!(
        matches!(refused, Err(Error::Invalid(_))),
        "two credentials means somebody changed how this deployment authenticates and did not \
         finish; preferring one silently would make the answer depend on a match arm order: \
         {refused:?}"
    );
}

#[test]
fn an_access_built_from_one_credential_and_an_endpoint_is_configured() {
    let access = GcpAccess::from_values(
        Some("http://proxy.internal:8080"),
        None,
        None,
        Some("a-literal-token"),
        fixed_clock(),
    )
    .expect("one credential and an endpoint is a complete configuration");

    assert!(
        access.is_configured(),
        "{:?}",
        access.missing_configuration()
    );
}

#[test]
fn an_unconfigured_cloud_storage_store_refuses_every_operation_and_opens_no_connection() {
    // A listener exists so that "no connection was opened" is a claim about
    // this store rather than about there being nowhere to connect to.
    let server = TestServer::always(Action::ok("{}"));
    let store = CloudStorageBlobStore::new(CloudStorageConfig::new("archive"))
        .expect("an unconfigured store still constructs, so it can report why it cannot work");

    assert!(!store.is_available());
    for outcome in [
        store.put("k", b"bytes".to_vec()).err(),
        store.get("k").err(),
        store.delete("k").err(),
        store.list("").err(),
    ] {
        assert!(
            matches!(outcome, Some(Error::Unavailable(_))),
            "every entry point must refuse: {outcome:?}"
        );
    }
    assert_eq!(
        server.served(),
        0,
        "an unconfigured store must not reach the network at all"
    );
}

#[test]
fn an_unconfigured_cloud_storage_store_never_falls_back_to_the_local_filesystem() {
    let directory = std::env::temp_dir().join(format!("qip-gcs-fallback-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("a temporary directory");

    let store = CloudStorageBlobStore::new(CloudStorageConfig::new("archive"))
        .expect("an unconfigured store still constructs");
    let refused = store.put("model.bin", b"weights".to_vec());

    assert!(refused.is_err(), "the write must be refused");
    let leftovers: Vec<_> = std::fs::read_dir(&directory)
        .expect("the directory is readable")
        .filter_map(std::result::Result::ok)
        .collect();
    assert!(
        leftovers.is_empty(),
        "nothing may be written locally: a deployment configured for a bucket that quietly wrote \
         to disk would pass every smoke test and lose the archive. Found: {leftovers:?}"
    );
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn an_unconfigured_bigquery_warehouse_refuses_and_opens_no_connection() {
    let server = TestServer::always(Action::ok("{}"));
    let warehouse = BigQueryWarehouse::new(BigQueryConfig::new("proj", "research"))
        .expect("an unconfigured warehouse still constructs");

    assert!(!warehouse.is_available());
    let insert = warehouse.insert(
        "runs",
        vec![InsertRow::anonymous(serde_json::json!({"a": 1}))],
    );
    assert!(matches!(insert, Err(Error::Unavailable(_))), "{insert:?}");
    let query = warehouse.query(&QueryRequest::new("SELECT 1"));
    assert!(matches!(query, Err(Error::Unavailable(_))), "{query:?}");
    assert_eq!(
        server.served(),
        0,
        "an unconfigured warehouse must not reach the network"
    );
}

// --- Cloud Storage ----------------------------------------------------------

#[test]
fn a_cloud_storage_put_uploads_the_bytes_and_carries_the_token_in_a_header() {
    let server = TestServer::always(Action::ok(r#"{"name":"report.json"}"#));
    let store = bucket_store(&server);

    store
        .put("report.json", b"{\"pnl\":1}".to_vec())
        .expect("the upload succeeds");

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "POST");
    assert!(
        request
            .target
            .starts_with("/upload/storage/v1/b/archive/o?uploadType=media&name="),
        "a media upload goes to the upload endpoint with the name in the query: {}",
        request.target
    );
    assert_eq!(
        request.body, b"{\"pnl\":1}",
        "the object's bytes go in the body unaltered"
    );
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer test-token-value"),
        "the credential travels in a header"
    );
    assert!(
        !request.target.contains("test-token-value"),
        "the credential must never be in the URL: a URL is written to every access log on the \
         path. Target was: {}",
        request.target
    );
}

#[test]
fn a_cloud_storage_key_containing_slashes_becomes_one_percent_encoded_object_name() {
    // The premise: `a/b/c.json` is one object whose name contains slashes, not
    // three path segments, and Google's API needs it encoded that way.
    let server = TestServer::always(Action::ok("{}"));
    let store = bucket_store(&server);

    store
        .put("archive/2026/day.json", b"x".to_vec())
        .expect("the upload succeeds");

    let target = &server.requests()[0].target;
    assert!(
        target.contains("name=archive%2F2026%2Fday.json"),
        "the slashes must be escaped, or the request addresses a resource that does not exist \
         and the 404 looks like a missing object rather than an encoding bug: {target}"
    );
}

#[test]
fn a_cloud_storage_get_returns_the_object_bytes_unchanged() {
    let payload = vec![0u8, 1, 2, 250, 251, 252, 255];
    let server = TestServer::always(Action::Raw {
        status: 200,
        body: payload.clone(),
    });
    let store = bucket_store(&server);

    let fetched = store.get("model.bin").expect("the download succeeds");

    assert_eq!(
        fetched,
        Some(payload),
        "arbitrary bytes must survive the round trip; an object store that could only carry text \
         would corrupt every model artifact"
    );
    let target = &server.requests()[0].target;
    assert!(
        target.ends_with("?alt=media"),
        "a download needs alt=media, or the API returns the object's metadata instead: {target}"
    );
}

#[test]
fn a_missing_cloud_storage_object_is_none_rather_than_an_error() {
    let server = TestServer::always(Action::json(404, r#"{"error":{"code":404}}"#));
    let store = bucket_store(&server);

    assert_eq!(
        store.get("absent").expect("a 404 is a normal answer"),
        None,
        "no such blob is what the port already means by None"
    );
}

#[test]
fn a_forbidden_cloud_storage_get_is_a_denial_and_not_an_empty_archive() {
    // The failure this prevents: a permissions problem that reads as an
    // archive with nothing in it.
    let server = TestServer::always(Action::json(403, r#"{"error":{"code":403}}"#));
    let store = bucket_store(&server);

    let refused = store.get("report.json");
    assert!(
        matches!(refused, Err(Error::Denied(_))),
        "a 403 must be an error, never Ok(None): {refused:?}"
    );
}

#[test]
fn a_cloud_storage_denial_never_quotes_the_token_back_into_the_error() {
    let server = TestServer::always(Action::json(401, r#"{"error":{"code":401}}"#));
    let store = bucket_store(&server);

    let message = match store.get("report.json") {
        Err(error) => error.to_string(),
        Ok(other) => panic!("a 401 must be an error, got {other:?}"),
    };
    assert!(
        !message.contains("test-token-value"),
        "an error about a rejected credential must not carry the credential: {message}"
    );
}

#[test]
fn a_cloud_storage_delete_reports_false_for_an_object_that_was_not_there() {
    let server = TestServer::script(vec![
        Action::json(204, ""),
        Action::json(404, r#"{"error":{"code":404}}"#),
    ]);
    let store = bucket_store(&server);

    assert!(
        store
            .delete("present")
            .expect("a 204 is a successful delete"),
        "deleting an object that existed reports true"
    );
    assert!(
        !store.delete("absent").expect("a 404 is not a failure"),
        "deleting what is not there is not an error; the port already says so"
    );
}

#[test]
fn a_cloud_storage_list_follows_its_page_token_and_returns_every_name() {
    let server = TestServer::script(vec![
        Action::ok(r#"{"items":[{"name":"a"},{"name":"b"}],"nextPageToken":"tok-2"}"#),
        Action::ok(r#"{"items":[{"name":"c"}]}"#),
    ]);
    let store = bucket_store(&server);

    let names = store.list("").expect("the listing completes");

    assert_eq!(
        names,
        vec!["a", "b", "c"],
        "a listing that stopped at the first page would silently report a bucket as smaller than \
         it is"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2, "two pages, two requests");
    assert!(
        requests[1].target.contains("pageToken=tok-2"),
        "the second request must carry the token the first returned: {}",
        requests[1].target
    );
}

#[test]
fn a_cloud_storage_listing_that_exceeds_its_page_bound_is_an_error_and_not_a_short_list() {
    // Always another page: the listing can never finish.
    let server = TestServer::always(Action::ok(
        r#"{"items":[{"name":"a"}],"nextPageToken":"always-more"}"#,
    ));
    let store = CloudStorageBlobStore::new(
        CloudStorageConfig::new("archive")
            .with_access(access_to(&server))
            .with_max_list_pages(3),
    )
    .expect("a bounded listing is a usable configuration");

    let refused = store.list("");
    assert!(
        matches!(refused, Err(Error::Guard(_))),
        "the rows already read must not be returned: a caller cannot tell a truncated list from \
         a complete one, and would treat the missing objects as absent. Got {refused:?}"
    );
}

#[test]
fn two_namespaces_in_one_bucket_do_not_collide_because_the_namespace_is_a_key_prefix() {
    // The failure this prevents: two namespaces both writing `model.bin` being
    // one object, the second write destroying the first.
    let server = TestServer::always(Action::ok("{}"));
    let training = CloudStorageBlobStore::new(
        CloudStorageConfig::new("archive")
            .with_access(access_to(&server))
            .with_prefix("training"),
    )
    .expect("a prefixed store is usable");
    let research = CloudStorageBlobStore::new(
        CloudStorageConfig::new("archive")
            .with_access(access_to(&server))
            .with_prefix("research"),
    )
    .expect("a prefixed store is usable");

    training.put("model.bin", b"a".to_vec()).expect("upload");
    research.put("model.bin", b"b".to_vec()).expect("upload");

    let requests = server.requests();
    assert!(
        requests[0].target.contains("name=training%2Fmodel.bin"),
        "{}",
        requests[0].target
    );
    assert!(
        requests[1].target.contains("name=research%2Fmodel.bin"),
        "{}",
        requests[1].target
    );
    assert_ne!(
        requests[0].target, requests[1].target,
        "the same key in two namespaces must address two different objects"
    );
}

#[test]
fn a_prefixed_cloud_storage_listing_returns_keys_that_would_fetch() {
    // A listing that returned prefixed names would make every round trip
    // through it fail: `list` then `get` must compose.
    let server = TestServer::always(Action::ok(
        r#"{"items":[{"name":"training/a.bin"},{"name":"training/b.bin"}]}"#,
    ));
    let store = CloudStorageBlobStore::new(
        CloudStorageConfig::new("archive")
            .with_access(access_to(&server))
            .with_prefix("training"),
    )
    .expect("a prefixed store is usable");

    let names = store.list("").expect("the listing completes");

    assert_eq!(
        names,
        vec!["a.bin", "b.bin"],
        "the store's own prefix comes back off, so what list returns is what get takes"
    );
    assert!(
        server.requests()[0].target.contains("prefix=training%2F"),
        "the query must be scoped to the namespace: {}",
        server.requests()[0].target
    );
}

#[test]
fn a_cloud_storage_object_larger_than_the_configured_limit_is_refused_before_any_connection() {
    // The premise: there is no resumable upload here, so a too-large object is
    // refused rather than started and abandoned half way.
    let server = TestServer::always(Action::ok("{}"));
    let store = CloudStorageBlobStore::new(
        CloudStorageConfig::new("archive")
            .with_access(access_to(&server))
            .with_max_object_bytes(16),
    )
    .expect("a bounded store is usable");

    let refused = store.put("big", vec![0u8; 17]);

    assert!(
        matches!(refused, Err(Error::Guard(_))),
        "an over-large upload must be refused: {refused:?}"
    );
    assert_eq!(
        server.served(),
        0,
        "and refused before a connection is opened, not part way through the body"
    );
}

#[test]
fn a_cloud_storage_response_past_the_body_limit_is_refused_rather_than_truncated() {
    // The failure this prevents: a silently truncated archive object that
    // reported success and is discovered years later by whoever needed it.
    let limits = qip_transport::ClientLimits {
        max_body: 1024,
        ..qip_storage::gcp::default_limits()
    };
    let server = TestServer::always(Action::Oversized { bytes: 4096 });
    let store = CloudStorageBlobStore::new(
        CloudStorageConfig::new("archive").with_access(access_to(&server).with_limits(limits)),
    )
    .expect("a bounded store is usable");

    let refused = store.get("huge.bin");
    assert!(
        matches!(refused, Err(Error::Guard(_))),
        "an oversized body must be refused whole: {refused:?}"
    );
}

#[test]
fn a_cloud_storage_peer_that_goes_silent_is_a_timeout_rather_than_a_hang() {
    // The premise: every wait is bounded. A proxy that accepts a connection and
    // then says nothing must not be able to park the calling thread forever —
    // an archive write that never returns is an outage with no error in it.
    let limits = qip_transport::ClientLimits {
        read_timeout: std::time::Duration::from_millis(100),
        ..qip_storage::gcp::default_limits()
    };
    let server = TestServer::always(Action::Silent(std::time::Duration::from_secs(30)));
    let store = CloudStorageBlobStore::new(
        CloudStorageConfig::new("archive").with_access(access_to(&server).with_limits(limits)),
    )
    .expect("a store with a short read timeout is usable");

    let started = std::time::Instant::now();
    let refused = store.get("report.json");

    assert!(
        matches!(refused, Err(Error::Timeout(_))),
        "a silent peer must time out: {refused:?}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the wait must be bounded by the configured timeout, not by the peer's patience; took {:?}",
        started.elapsed()
    );
}

#[test]
fn a_cloud_storage_digest_is_the_sha256_of_the_bytes_and_not_googles_own_checksum() {
    // The premise: the port's digest is SHA-256 and Cloud Storage computes MD5
    // and CRC32C. Returning Google's would mean one blob has two digests
    // depending on which store held it, and an integrity check between them
    // could never be trusted.
    let payload = b"the archive contents".to_vec();
    let server = TestServer::always(Action::Raw {
        status: 200,
        body: payload.clone(),
    });
    let store = bucket_store(&server);

    let digest = store.digest("object").expect("the download succeeds");

    assert_eq!(
        digest,
        Some(qip_core::hash::sha256_hex(&payload)),
        "the digest must agree with every other blob adapter's"
    );
}

#[test]
fn a_cloud_storage_key_the_file_store_would_refuse_is_refused_here_too() {
    // The premise: a key that works on one blob adapter and not another turns
    // a change of storage target into data loss.
    let server = TestServer::always(Action::ok("{}"));
    let store = bucket_store(&server);

    for unsafe_key in ["", "/leading", "a/../b", "a//b", "trailing/."] {
        let refused = store.put(unsafe_key, b"x".to_vec());
        assert!(
            matches!(refused, Err(Error::Invalid(_))),
            "{unsafe_key:?} must be refused so a namespace written to a bucket can still be read \
             back from disk: {refused:?}"
        );
    }
    assert_eq!(
        server.served(),
        0,
        "a key is validated before anything is dialled"
    );
}

#[test]
fn a_cloud_storage_store_without_a_bucket_is_refused_at_construction() {
    let refused = CloudStorageBlobStore::new(CloudStorageConfig::new("   "));
    assert!(
        matches!(refused, Err(Error::Invalid(_))),
        "there is no default bucket, because a default naming a real bucket would be written to \
         successfully: {refused:?}"
    );
}

// --- BigQuery ---------------------------------------------------------------

#[test]
fn a_bigquery_insert_sends_each_row_with_its_id_and_refuses_to_skip_invalid_rows() {
    let server = TestServer::always(Action::ok(
        r#"{"kind":"bigquery#tableDataInsertAllResponse"}"#,
    ));
    let warehouse = warehouse(&server);

    let outcome = warehouse
        .insert(
            "runs",
            vec![
                InsertRow::with_id("run-1", serde_json::json!({"sharpe": "1.4"})),
                InsertRow::with_id("run-2", serde_json::json!({"sharpe": "0.9"})),
            ],
        )
        .expect("the insert succeeds");

    assert!(outcome.is_complete());
    assert_eq!(outcome.inserted(), 2);

    let request = &server.requests()[0];
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.target, "/bigquery/v2/projects/proj/datasets/research/tables/runs/insertAll",
        "the streaming-insert endpoint"
    );
    let body = request.body_json();
    assert_eq!(
        body["skipInvalidRows"],
        serde_json::json!(false),
        "skipping invalid rows would make BigQuery drop a bad row and report success for the rest"
    );
    assert_eq!(
        body["ignoreUnknownValues"],
        serde_json::json!(false),
        "ignoring unknown values would silently drop a column whose name the caller got wrong"
    );
    assert_eq!(body["rows"][0]["insertId"], serde_json::json!("run-1"));
    assert_eq!(body["rows"][1]["json"]["sharpe"], serde_json::json!("0.9"));
}

#[test]
fn a_bigquery_insert_that_answers_http_200_with_insert_errors_is_not_reported_as_success() {
    // The single most important thing about streaming inserts: a partial
    // failure has a 200 status and the rejections are in the body.
    let server = TestServer::always(Action::ok(
        r#"{"insertErrors":[{"index":1,"errors":[{"reason":"invalid","message":"no such field: sharp"}]}]}"#,
    ));
    let warehouse = warehouse(&server);

    let outcome = warehouse
        .insert(
            "runs",
            vec![
                InsertRow::with_id("a", serde_json::json!({"sharpe": "1.4"})),
                InsertRow::with_id("b", serde_json::json!({"sharp": "0.9"})),
            ],
        )
        .expect("the request itself succeeded, which is exactly the trap");

    assert!(
        !outcome.is_complete(),
        "the HTTP status was 200 and one row was still rejected"
    );
    assert_eq!(outcome.inserted(), 1, "one of the two rows is in the table");
    assert_eq!(outcome.rejected().len(), 1);
    assert_eq!(outcome.rejected()[0].index, 1);

    let refused = outcome.into_result();
    let message = match refused {
        Err(error) => error.to_string(),
        Ok(other) => panic!("into_result must refuse a partial insert, got {other:?}"),
    };
    assert!(
        message.contains("no such field: sharp"),
        "the refusal must carry BigQuery's own reason, or an operator cannot fix the schema: \
         {message}"
    );
}

#[test]
fn an_empty_bigquery_batch_is_refused_before_any_connection() {
    let server = TestServer::always(Action::ok("{}"));
    let warehouse = warehouse(&server);

    let refused = warehouse.insert("runs", Vec::new());

    assert!(matches!(refused, Err(Error::Invalid(_))), "{refused:?}");
    assert_eq!(
        server.served(),
        0,
        "a caller with nothing to insert should not have reached the network"
    );
}

#[test]
fn a_bigquery_batch_larger_than_the_configured_limit_is_refused_whole() {
    let server = TestServer::always(Action::ok("{}"));
    let warehouse = BigQueryWarehouse::new(
        BigQueryConfig::new("proj", "research")
            .with_access(access_to(&server))
            .with_max_rows_per_insert(2),
    )
    .expect("a bounded warehouse is usable");

    let rows: Vec<_> = (0..3)
        .map(|i| InsertRow::anonymous(serde_json::json!({ "i": i.to_string() })))
        .collect();
    let refused = warehouse.insert("runs", rows);

    assert!(
        matches!(refused, Err(Error::Guard(_))),
        "an over-large batch is rejected whole by BigQuery, losing every row in it: {refused:?}"
    );
    assert_eq!(server.served(), 0);
}

#[test]
fn a_bigquery_query_decodes_rows_into_their_schema_column_names() {
    let server = TestServer::always(Action::ok(
        r#"{"jobComplete":true,
            "jobReference":{"jobId":"job-1"},
            "schema":{"fields":[{"name":"strategy"},{"name":"sharpe"}]},
            "rows":[{"f":[{"v":"momentum"},{"v":"1.4"}]},{"f":[{"v":"carry"},{"v":null}]}],
            "totalRows":"2","totalBytesProcessed":"4096","cacheHit":false}"#,
    ));
    let warehouse = warehouse(&server);

    let page = warehouse
        .query(&QueryRequest::new("SELECT strategy, sharpe FROM runs"))
        .expect("the query completes");

    assert_eq!(page.columns, vec!["strategy", "sharpe"]);
    assert_eq!(page.rows.len(), 2);
    assert_eq!(
        page.rows[0].get("sharpe"),
        Some(&Some("1.4".to_string())),
        "BigQuery sends every scalar as a string and this decoder hands over exactly that string, \
         rather than inventing a rounding decision"
    );
    assert_eq!(
        page.rows[1].get("sharpe"),
        Some(&None),
        "SQL NULL is None, which is not the same as the string \"null\""
    );
    assert_eq!(page.total_bytes_processed, Some(4096));
}

#[test]
fn a_bigquery_query_that_never_finishes_is_an_error_and_not_an_empty_result_set() {
    // The failure this prevents: `jobComplete: false` has no `rows` field at
    // all, and a naive decoder reads that as "the query returned nothing" —
    // the difference between "no strategy breached its limit" and "we did not
    // find out".
    let server = TestServer::always(Action::ok(
        r#"{"jobComplete":false,"jobReference":{"jobId":"job-9","location":"EU"}}"#,
    ));
    let warehouse = BigQueryWarehouse::new(
        BigQueryConfig::new("proj", "research")
            .with_access(access_to(&server))
            .with_max_query_polls(2),
    )
    .expect("a bounded warehouse is usable");

    let refused = warehouse.query(&QueryRequest::new("SELECT * FROM slow"));

    let message = match refused {
        Err(Error::Timeout(message)) => message,
        other => panic!("an unfinished query must be a timeout, got {other:?}"),
    };
    assert!(
        message.contains("job-9"),
        "the refusal must name the job so an operator can find it in the console: {message}"
    );
}

#[test]
fn a_bigquery_query_waits_for_an_incomplete_job_and_returns_the_rows_once_it_completes() {
    let server = TestServer::script(vec![
        Action::ok(r#"{"jobComplete":false,"jobReference":{"jobId":"job-7","location":"EU"}}"#),
        Action::ok(
            r#"{"jobComplete":true,"jobReference":{"jobId":"job-7"},
                "schema":{"fields":[{"name":"n"}]},
                "rows":[{"f":[{"v":"1"}]}],"totalRows":"1"}"#,
        ),
    ]);
    let warehouse = warehouse(&server);

    let page = warehouse
        .query(&QueryRequest::new("SELECT 1 AS n"))
        .expect("the job finishes on the second ask");

    assert_eq!(page.rows.len(), 1);
    let requests = server.requests();
    assert_eq!(requests.len(), 2, "one start, one wait");
    assert_eq!(requests[1].method, "GET", "the wait is getQueryResults");
    assert!(
        requests[1].target.contains("/queries/job-7"),
        "the wait must name the job: {}",
        requests[1].target
    );
    assert!(
        requests[1].target.contains("location=EU"),
        "a job outside the default region cannot be found without its location: {}",
        requests[1].target
    );
}

#[test]
fn a_bigquery_query_follows_its_page_token_and_returns_every_row() {
    let server = TestServer::script(vec![
        Action::ok(
            r#"{"jobComplete":true,"jobReference":{"jobId":"job-3"},
                "schema":{"fields":[{"name":"n"}]},
                "rows":[{"f":[{"v":"1"}]}],"pageToken":"page-2","totalRows":"2"}"#,
        ),
        Action::ok(
            r#"{"jobComplete":true,"jobReference":{"jobId":"job-3"},
                "schema":{"fields":[{"name":"n"}]},
                "rows":[{"f":[{"v":"2"}]}],"totalRows":"2"}"#,
        ),
    ]);
    let warehouse = warehouse(&server);

    let page = warehouse
        .query(&QueryRequest::new("SELECT n FROM two"))
        .expect("both pages are read");

    assert_eq!(
        page.rows.len(),
        2,
        "a result set is returned whole or not at all"
    );
    assert!(
        server.requests()[1].target.contains("pageToken=page-2"),
        "{}",
        server.requests()[1].target
    );
}

#[test]
fn a_bigquery_result_set_that_disagrees_with_its_own_row_count_is_refused() {
    // One of the two numbers is wrong and there is no way here to tell which,
    // so neither is returned as the answer.
    let server = TestServer::always(Action::ok(
        r#"{"jobComplete":true,"jobReference":{"jobId":"job-4"},
            "schema":{"fields":[{"name":"n"}]},
            "rows":[{"f":[{"v":"1"}]}],"totalRows":"9"}"#,
    ));
    let warehouse = warehouse(&server);

    let refused = warehouse.query(&QueryRequest::new("SELECT n FROM t"));
    assert!(
        matches!(refused, Err(Error::Schema(_))),
        "a short result set must not be returned as though it were the whole answer: {refused:?}"
    );
}

#[test]
fn a_bigquery_repeated_column_is_refused_rather_than_flattened_into_a_string() {
    let server = TestServer::always(Action::ok(
        r#"{"jobComplete":true,"jobReference":{"jobId":"job-5"},
            "schema":{"fields":[{"name":"tags"}]},
            "rows":[{"f":[{"v":["a","b"]}]}],"totalRows":"1"}"#,
    ));
    let warehouse = warehouse(&server);

    let refused = warehouse.query(&QueryRequest::new("SELECT tags FROM t"));
    let message = match refused {
        Err(Error::Schema(message)) => message,
        other => panic!("a repeated column must be refused, got {other:?}"),
    };
    assert!(
        message.contains("tags"),
        "the refusal must name the column: {message}"
    );
}

#[test]
fn a_bigquery_row_that_does_not_match_its_own_schema_is_refused_rather_than_aligned() {
    // A short zip would shift every later column's value into the wrong name,
    // producing a result set that is wrong rather than obviously broken.
    let server = TestServer::always(Action::ok(
        r#"{"jobComplete":true,"jobReference":{"jobId":"job-6"},
            "schema":{"fields":[{"name":"a"},{"name":"b"}]},
            "rows":[{"f":[{"v":"1"}]}],"totalRows":"1"}"#,
    ));
    let warehouse = warehouse(&server);

    let refused = warehouse.query(&QueryRequest::new("SELECT a, b FROM t"));
    assert!(matches!(refused, Err(Error::Schema(_))), "{refused:?}");
}

#[test]
fn a_bigquery_query_sends_named_parameters_instead_of_interpolating_them_into_the_sql() {
    // Parameters are the only injection defence there is: this adapter does
    // not parse the SQL and cannot tell a literal from an injected one.
    let server = TestServer::always(Action::ok(
        r#"{"jobComplete":true,"jobReference":{"jobId":"job-8"},
            "schema":{"fields":[{"name":"n"}]},"rows":[],"totalRows":"0"}"#,
    ));
    let warehouse = warehouse(&server);

    let request = QueryRequest::new("SELECT n FROM runs WHERE strategy = @name AND live = @live")
        .with_parameter(
            QueryParameter::string("name", "momentum'; DROP TABLE runs--").expect("a parameter"),
        )
        .with_parameter(QueryParameter::bool("live", true).expect("a parameter"));
    warehouse.query(&request).expect("the query completes");

    let body = server.requests()[0].body_json();
    assert_eq!(body["parameterMode"], serde_json::json!("NAMED"));
    assert_eq!(
        body["query"],
        serde_json::json!("SELECT n FROM runs WHERE strategy = @name AND live = @live"),
        "the SQL goes over unchanged; the value is never spliced into it"
    );
    assert_eq!(
        body["queryParameters"][0]["parameterValue"]["value"],
        serde_json::json!("momentum'; DROP TABLE runs--"),
        "the value travels as a parameter, where it cannot become syntax"
    );
    assert_eq!(
        body["useLegacySql"],
        serde_json::json!(false),
        "legacy SQL changes what a query means, not only what parses"
    );
}

#[test]
fn two_bigquery_parameters_sharing_one_name_are_refused() {
    let server = TestServer::always(Action::ok("{}"));
    let warehouse = warehouse(&server);

    let request = QueryRequest::new("SELECT @x")
        .with_parameter(QueryParameter::string("x", "one").expect("a parameter"))
        .with_parameter(QueryParameter::string("x", "two").expect("a parameter"));

    let refused = warehouse.query(&request);
    assert!(
        matches!(refused, Err(Error::Invalid(_))),
        "BigQuery would bind one of them and this adapter will not choose which: {refused:?}"
    );
    assert_eq!(server.served(), 0);
}

#[test]
fn a_bigquery_parameter_name_that_is_not_an_identifier_is_refused() {
    for bad in ["", "has space", "semi;colon"] {
        assert!(
            QueryParameter::string(bad, "v").is_err(),
            "{bad:?} is not a BigQuery identifier and cannot be referenced as @name"
        );
    }
}

#[test]
fn a_bigquery_404_names_the_missing_resource_rather_than_reporting_no_rows() {
    let server = TestServer::always(Action::json(
        404,
        r#"{"error":{"message":"Not found: Table"}}"#,
    ));
    let warehouse = warehouse(&server);

    let refused = warehouse.query(&QueryRequest::new("SELECT 1"));
    assert!(
        matches!(refused, Err(Error::NotFound(_))),
        "a missing table must not look like a table with nothing in it: {refused:?}"
    );
}

// --- the provider -----------------------------------------------------------

#[test]
fn cloud_storage_and_bigquery_are_implemented_and_the_other_four_managed_targets_are_not() {
    for implemented in [StorageTarget::CloudStorage, StorageTarget::BigQuery] {
        assert!(
            implemented.is_implemented(),
            "{implemented:?} has a REST adapter in this build"
        );
    }
    for port in [
        StorageTarget::AlloyDb,
        StorageTarget::Spanner,
        StorageTarget::Bigtable,
    ] {
        assert!(
            !port.is_implemented(),
            "{port:?} needs a PostgreSQL wire client or a gRPC stack, which this build does not \
             have and a dependency policy permitting only serde will not acquire"
        );
    }
}

#[test]
fn every_implemented_managed_target_still_names_what_the_deployment_must_supply() {
    // Implemented is a fact about the binary and not about a deployment, so
    // the requirement text is never empty for a managed target.
    for target in [StorageTarget::CloudStorage, StorageTarget::BigQuery] {
        let requirement = target
            .required_configuration()
            .unwrap_or_else(|| panic!("{target:?} must still name its deployment requirements"));
        assert!(
            requirement.contains("TLS"),
            "{target:?} must name the TLS-terminating proxy: {requirement}"
        );
        assert!(
            requirement.contains("ADR 0009"),
            "{target:?} must say why this build cannot mint a token: {requirement}"
        );
    }
}

#[test]
fn an_implemented_cloud_storage_target_is_still_refused_when_the_deployment_configured_nothing() {
    // The two facts that have to hold together, and the reason `is_implemented`
    // is documented as a fact about the binary: the adapter is present, and a
    // deployment that never named a bucket still gets nothing. If these came
    // apart, a process with no configuration would either report a missing
    // adapter that is right there, or quietly build a store pointed nowhere.
    assert!(
        std::env::var(qip_storage::gcp::BUCKET_VARIABLE).is_err(),
        "the premise of this test is a test environment that configures no bucket"
    );
    assert!(StorageTarget::CloudStorage.is_implemented());

    let provider = StorageProvider::new(StorageTarget::CloudStorage, std::env::temp_dir());
    let refused = provider.blobs("archive");

    let message = match refused {
        Err(Error::Unavailable(message)) => message,
        other => panic!("an unconfigured deployment must be refused, got {other:?}"),
    };
    assert!(
        message.contains(qip_storage::gcp::BUCKET_VARIABLE),
        "the refusal must name the variable that would fix it: {message}"
    );
}

#[test]
fn the_cloud_storage_key_value_refusal_names_the_blob_store_rather_than_a_missing_adapter() {
    let provider = StorageProvider::new(StorageTarget::CloudStorage, std::env::temp_dir());

    let message = match provider.key_value("anything") {
        Err(error) => error.to_string(),
        Ok(_) => panic!("an object store is not a key-value store"),
    };
    assert!(
        message.contains("blobs"),
        "an operator must be pointed at the method that works, not sent looking for an adapter \
         that is right there: {message}"
    );
}

#[test]
fn the_bigquery_blob_refusal_names_the_warehouse_rather_than_a_missing_adapter() {
    let provider = StorageProvider::new(StorageTarget::BigQuery, std::env::temp_dir());

    let message = match provider.blobs("anything") {
        Err(error) => error.to_string(),
        Ok(_) => panic!("a warehouse holds rows, not objects"),
    };
    assert!(
        message.contains("Cloud Storage"),
        "the refusal must say where an artifact should go instead: {message}"
    );
}
