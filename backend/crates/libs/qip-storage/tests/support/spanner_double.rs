//! An in-process Cloud Spanner REST double on a loopback port: sessions,
//! strong and transactional reads by key or range, read-write transactions,
//! commits and rollbacks over one key-value table. Adapted from the double
//! `qip-ledgerd/tests/spanner.rs` drives the ledger against, without the
//! write-conflict abort that suite's fence depends on.
//!
//! What it does not prove is that Google answers the way it does; the
//! request shapes follow the published REST reference, and the first run
//! against a real instance is the evidence for that, not this file.

use qip_storage::gcp::spanner::{SpannerConfig, SpannerDatabase, SpannerKv};
use qip_storage::{GcpAccess, StaticToken};
use qip_transport::server::{Handler, Method, Request, Response, Server, ServerLimits};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const DATABASE: &str = "projects/qip-test/instances/registry/databases/manifests";

#[derive(Default)]
struct State {
    rows: BTreeMap<String, String>,
    sessions: u64,
    next_transaction: u64,
    open: Vec<String>,
}

#[derive(Default)]
struct FakeSpanner {
    state: Mutex<State>,
}

fn error(status: u16, message: &str) -> Response {
    Response::json(
        status,
        json!({ "error": { "message": message } }).to_string(),
    )
}

fn key_of(value: &Value) -> Option<String> {
    value.as_array()?.first()?.as_str().map(str::to_string)
}

impl FakeSpanner {
    fn read(state: &State, body: &Value) -> Response {
        let limit: usize = body["limit"]
            .as_str()
            .and_then(|l| l.parse().ok())
            .unwrap_or(usize::MAX);
        let mut keys: Vec<String> = Vec::new();
        if let Some(wanted) = body["keySet"]["keys"].as_array() {
            keys.extend(
                wanted
                    .iter()
                    .filter_map(key_of)
                    .filter(|key| state.rows.contains_key(key)),
            );
        } else if let Some(ranges) = body["keySet"]["ranges"].as_array() {
            for range in ranges {
                let end = key_of(&range["endOpen"]).unwrap_or_default();
                let (start, open) = match key_of(&range["startOpen"]) {
                    Some(start) => (start, true),
                    None => (key_of(&range["startClosed"]).unwrap_or_default(), false),
                };
                keys.extend(
                    state
                        .rows
                        .keys()
                        .filter(|key| if open { **key > start } else { **key >= start })
                        .filter(|key| **key < end)
                        .cloned(),
                );
            }
        } else {
            return error(400, "keySet has neither keys nor ranges");
        }
        keys.truncate(limit);
        let rows: Vec<Value> = keys
            .iter()
            .map(|key| json!([key, state.rows[key]]))
            .collect();
        Response::json(200, json!({ "rows": rows }).to_string())
    }

    fn commit(state: &mut State, body: &Value) -> Response {
        if let Some(id) = body["transactionId"].as_str() {
            if !state.open.iter().any(|open| open == id) {
                return error(404, "no such transaction");
            }
            state.open.retain(|open| open != id);
        }
        for mutation in body["mutations"].as_array().cloned().unwrap_or_default() {
            if let Some(values) = mutation["insertOrUpdate"]["values"].as_array() {
                for value in values {
                    let pair = value.as_array().cloned().unwrap_or_default();
                    let (Some(key), Some(text)) = (
                        pair.first().and_then(Value::as_str),
                        pair.get(1).and_then(Value::as_str),
                    ) else {
                        return error(400, "a value is not [k, v]");
                    };
                    state.rows.insert(key.to_string(), text.to_string());
                }
            } else if let Some(keys) = mutation["delete"]["keySet"]["keys"].as_array() {
                for key in keys.iter().filter_map(key_of) {
                    state.rows.remove(&key);
                }
            } else {
                return error(400, "an unknown mutation");
            }
        }
        Response::json(
            200,
            json!({ "commitTimestamp": "2026-10-31T00:00:00Z" }).to_string(),
        )
    }
}

impl Handler for FakeSpanner {
    fn handle(&self, request: &Request) -> Response {
        if !matches!(request.method, Method::Post) {
            return error(405, "POST only");
        }
        if request.header("authorization").is_none() {
            return error(401, "no credential");
        }
        let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        let Ok(mut state) = self.state.lock() else {
            return error(500, "the double's state is poisoned");
        };
        let path = request.path.strip_prefix("/v1/").unwrap_or_default();
        if path == format!("{DATABASE}/sessions") {
            state.sessions += 1;
            let name = format!("{DATABASE}/sessions/s{}", state.sessions);
            return Response::json(200, json!({ "name": name }).to_string());
        }
        let Some((session, method)) = path.rsplit_once(':') else {
            return error(404, "no route");
        };
        if !session.starts_with(&format!("{DATABASE}/sessions/")) {
            return error(404, "no such session");
        }
        match method {
            "beginTransaction" => {
                state.next_transaction += 1;
                let id = format!("t{}", state.next_transaction);
                state.open.push(id.clone());
                Response::json(200, json!({ "id": id }).to_string())
            }
            "read" => Self::read(&state, &body),
            "commit" => Self::commit(&mut state, &body),
            "rollback" => {
                if let Some(id) = body["transactionId"].as_str() {
                    state.open.retain(|open| open != id);
                }
                Response::json(200, "{}")
            }
            _ => error(404, "no such method"),
        }
    }
}

/// The double, serving until dropped.
pub(crate) struct Double {
    fake: Arc<FakeSpanner>,
    address: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Double {
    pub(crate) fn start() -> Self {
        let fake = Arc::new(FakeSpanner::default());
        let server = Server::bind("127.0.0.1:0", fake.clone(), ServerLimits::default())
            .expect("the double binds");
        let address = server.local_address().expect("address");
        let stop = server.shutdown_handle();
        let thread = std::thread::spawn(move || {
            let _ = server.serve();
        });
        Self {
            fake,
            address,
            stop,
            thread: Some(thread),
        }
    }

    /// A client for the double's one database.
    pub(crate) fn kv(&self) -> SpannerKv {
        let access = GcpAccess::unconfigured()
            .with_endpoint(&format!("http://{}", self.address))
            .expect("loopback endpoint")
            .with_tokens(Arc::new(
                StaticToken::new("fixture-spanner-access").expect("a token"),
            ));
        SpannerKv::new(SpannerConfig::new(
            SpannerDatabase::new("qip-test", "registry", "manifests").expect("names"),
            access,
        ))
        .expect("a configured client")
    }

    /// Every key the table holds, in order.
    pub(crate) fn keys(&self) -> Vec<String> {
        self.fake
            .state
            .lock()
            .expect("the double's state")
            .rows
            .keys()
            .cloned()
            .collect()
    }
}

impl Drop for Double {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(&self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
