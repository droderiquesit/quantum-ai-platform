//! The BigQuery warehouse client, against a real socket.
//!
//! Moved here with the client (DATA-058) from `qip-storage/tests/gcp.rs`,
//! which keeps the Cloud Storage half and the credential tests. The loopback
//! server below is that file's, copied rather than shared: a test scaffold
//! reached across crates would be an edge between two crates' tests that
//! nothing else in the workspace has.
//!
//! Every test binds a loopback [`std::net::TcpListener`] and lets the client
//! connect to it. Tests that assert a refusal also assert that **no
//! connection was opened**, by checking the listener served nothing.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

// The scaffold is the storage suite's, copied whole so the two stay
// recognisably the same server; this suite scripts only part of it.
#[allow(dead_code)]
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
use qip_mesh::bigquery::{
    BigQueryConfig, BigQueryWarehouse, InsertRow, QueryParameter, QueryRequest,
};
use qip_storage::gcp::{GcpAccess, StaticToken};
use server::{Action, TestServer};
use std::sync::Arc;

/// Access pointed at `server`, carrying a token.
fn access_to(server: &TestServer) -> GcpAccess {
    GcpAccess::unconfigured()
        .with_endpoint(&server.url())
        .expect("a loopback URL parses")
        .with_tokens(Arc::new(
            StaticToken::new("test-token-value").expect("a plain token is usable"),
        ))
}

fn warehouse(server: &TestServer) -> BigQueryWarehouse {
    BigQueryWarehouse::new(BigQueryConfig::new("proj", "research").with_access(access_to(server)))
        .expect("a named project and dataset are a usable configuration")
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
