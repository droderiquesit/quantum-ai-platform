//! The OpenAPI description, generated from the route table.
//!
//! Served at [`crate::routes::OPENAPI_PATH`] rather than checked in, and built
//! from [`crate::routes::ROUTES`] every time it is asked for. That is the whole
//! point of the module: a description of an API is only useful if it is true,
//! and a file committed beside the code is true on the day it is written and
//! slowly stops being true afterwards. There is nothing here to update when a
//! route is added, because there is no second list of routes.
//!
//! What the document claims is bounded by what the table knows, deliberately:
//!
//! * every path, its method, its summary, and — the field a security review
//!   is here for — the least authority that may call it, on each operation as
//!   `x-required-role` and again in the `403` description;
//! * the statuses the shared request pipeline produces for every route, which
//!   are properties of [`crate::routes::Api::handle`] rather than of any one
//!   handler;
//! * each route's success status, which the table carries for this reason.
//!
//! It describes no response *bodies*. The table does not carry their shapes,
//! so anything written here would be hand-maintained — exactly the drift this
//! module exists to prevent — and a schema that quietly stops matching what a
//! handler returns is worse for a generated client than no schema at all.
//!
//! The two unauthenticated endpoints are the exception to being generated from
//! the table: they are served ahead of it, so they cannot be in it. They are
//! described from the same constants the router matches on, so their paths
//! cannot drift either.
//!
//! Nothing here contains a credential. The security scheme declares that a
//! bearer token is required; tokens come from the environment and never from
//! source.

use crate::http::Method;
use crate::json;
use crate::routes::{DISCOVERY_PATH, OPENAPI_PATH, ROUTES, Route};

/// The OpenAPI version this document conforms to.
const OPENAPI_VERSION: &str = "3.1.1";

/// The whole document, as JSON.
pub fn document() -> String {
    format!(
        r#"{{"openapi":{},"info":{},"servers":[{}],"paths":{{{}}},"components":{}}}"#,
        json::string(OPENAPI_VERSION),
        info(),
        server(),
        paths(),
        components()
    )
}

fn info() -> String {
    format!(
        r#"{{"title":"Quantum AI Investment Platform API","version":"v1","description":{}}}"#,
        json::string(
            "Generated from the server's own route table at the moment it was requested, so it \
             describes the surface this process is actually serving. Every path states the least \
             authority that may call it in `x-required-role`. Response bodies are deliberately \
             not described: the route table does not carry their shapes, and a hand-written \
             schema here would be free to disagree with the handler."
        )
    )
}

fn server() -> String {
    format!(
        r#"{{"url":"/","description":{}}}"#,
        json::string(
            "Paths are absolute and include the version prefix. The address this process is \
             reachable at is deployment configuration and is not known to the process."
        )
    )
}

/// Every path, each with the operations declared for it.
///
/// Grouped by path in the order the table declares them, so two methods on the
/// same path — the kill switch, which can be tripped and cleared — arrive as
/// one path object with two operations rather than as a duplicate key.
fn paths() -> String {
    let mut grouped: Vec<(String, Vec<&'static Route>)> = Vec::new();
    for route in ROUTES {
        let path = absolute_path(route.pattern);
        match grouped.iter_mut().find(|(existing, _)| *existing == path) {
            Some((_, routes)) => routes.push(route),
            None => grouped.push((path, vec![route])),
        }
    }

    let mut rendered: Vec<String> = grouped
        .iter()
        .map(|(path, routes)| {
            let operations: Vec<String> = routes
                .iter()
                .map(|route| {
                    format!(
                        r#"{}:{}"#,
                        json::string(method_key(route.method)),
                        operation(route)
                    )
                })
                .collect();
            format!(
                "{}:{{{}{}}}",
                json::string(path),
                parameters(routes[0].pattern),
                operations.join(",")
            )
        })
        .collect();

    rendered.push(unauthenticated(
        DISCOVERY_PATH,
        "discovery",
        "the route table, with the authority each route requires",
        "Unauthenticated on purpose: a client has to know which version it is talking to \
         before it can present a credential correctly, and the route table is not a secret.",
    ));
    rendered.push(unauthenticated(
        OPENAPI_PATH,
        "openapi",
        "this document",
        "Unauthenticated for the same reason discovery is, and generated from the route table \
         at the moment it is requested.",
    ));
    rendered.join(",")
}

/// One operation, from one row of the table.
fn operation(route: &Route) -> String {
    format!(
        r#"{{"operationId":{},"summary":{},"description":{},"tags":[{}],"x-required-role":{},"security":[{{"bearerToken":[]}}],"responses":{{{}}}}}"#,
        json::string(&operation_id(route)),
        json::string(route.summary),
        json::string(&format!(
            "Requires at least the {} role. Roles accumulate: a role above this one may call \
             it too.",
            route.required_role.as_str()
        )),
        json::string(tag_of(route.pattern)),
        json::string(route.required_role.as_str()),
        responses(route)
    )
}

/// The statuses a caller can actually receive.
///
/// The success status comes from the table. The rest are produced by the
/// shared pipeline in front of every handler — authentication, the rate
/// limiter, the role check, and the refusal to serve from a poisoned lock — so
/// they are the same for every route by construction rather than by being
/// copied onto each one.
fn responses(route: &Route) -> String {
    let success = format!(
        r#""{}":{{"description":{},"content":{{"application/json":{{"schema":{{"type":"object"}}}}}}}}"#,
        route.success,
        json::string(route.summary)
    );
    let refusals = [
        (
            401,
            "No credential was presented, or the one presented was not recognised or has \
             expired. The response does not say which.",
        ),
        (
            403,
            "The credential is recognised but holds less than the role this route requires.",
        ),
        (
            429,
            "The caller's request allowance for the current window is exhausted.",
        ),
        (
            503,
            "The platform is in an inconsistent state after an internal failure and is not \
             serving.",
        ),
    ];
    let mut rendered = vec![success];
    for (status, description) in refusals {
        rendered.push(format!(
            r#""{}":{{"description":{}}}"#,
            status,
            json::string(description)
        ));
    }
    rendered.join(",")
}

/// Path parameters, taken from the pattern.
///
/// `:name` in the table becomes `{name}` in the document, and each one is
/// declared required — a path parameter that is optional is a different path.
/// Declared on the path item rather than on each operation, because every
/// method on a path shares its parameters.
fn parameters(pattern: &str) -> String {
    let declared: Vec<String> = pattern
        .split('/')
        .filter_map(|segment| segment.strip_prefix(':'))
        .map(|name| {
            format!(
                r#"{{"name":{},"in":"path","required":true,"schema":{{"type":"string"}}}}"#,
                json::string(name)
            )
        })
        .collect();
    if declared.is_empty() {
        return String::new();
    }
    format!(r#""parameters":[{}],"#, declared.join(","))
}

/// One of the two endpoints served ahead of the route table.
fn unauthenticated(path: &str, id: &str, summary: &str, description: &str) -> String {
    format!(
        r#"{}:{{"get":{{"operationId":{},"summary":{},"description":{},"tags":["meta"],"security":[],"responses":{{"200":{{"description":{},"content":{{"application/json":{{"schema":{{"type":"object"}}}}}}}}}}}}}}"#,
        json::string(path),
        json::string(id),
        json::string(summary),
        json::string(description),
        json::string(summary)
    )
}

/// The one way in.
///
/// A declaration of the scheme, not a credential: tokens are read from the
/// environment at start-up and no token appears anywhere in this crate.
fn components() -> String {
    format!(
        r#"{{"securitySchemes":{{"bearerToken":{{"type":"http","scheme":"bearer","description":{}}}}}}}"#,
        json::string(
            "A bearer token issued out of band. The server stores only the SHA-256 of each \
             token and compares in constant time."
        )
    )
}

/// The full path a client calls, with pattern parameters in OpenAPI's syntax.
fn absolute_path(pattern: &str) -> String {
    let converted: Vec<String> = pattern
        .split('/')
        .map(|segment| match segment.strip_prefix(':') {
            Some(name) => format!("{{{name}}}"),
            None => segment.to_string(),
        })
        .collect();
    format!("{}{}", crate::routes::VERSION_PREFIX, converted.join("/"))
}

/// A stable identifier for one operation, derived from its method and path.
///
/// Unique because the table cannot hold the same method on the same pattern
/// twice without one of them being unreachable.
fn operation_id(route: &Route) -> String {
    let mut id = route.method.as_str().to_ascii_lowercase();
    for segment in route.pattern.split('/').filter(|s| !s.is_empty()) {
        id.push('_');
        id.push_str(&segment.trim_start_matches(':').replace('-', "_"));
    }
    id
}

/// The tag an operation is filed under: the first segment of its path.
fn tag_of(pattern: &str) -> &str {
    pattern
        .split('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or("api")
        .trim_start_matches(':')
}

/// OpenAPI spells its methods in lower case.
const fn method_key(method: Method) -> &'static str {
    match method {
        Method::Get => "get",
        Method::Post => "post",
        Method::Put => "put",
        Method::Delete => "delete",
        Method::Head => "head",
        Method::Options => "options",
    }
}
