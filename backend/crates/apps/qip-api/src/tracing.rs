//! Request tracing handler wrapper.
//!
//! Wraps an inner handler to create a request-scoped span for each incoming
//! request, capturing method, path, and response status.

use crate::http::{Handler, Request, Response, StreamDecision};
use qip_observability::trace::{SpanKind, Tracer};
use std::sync::Arc;

/// A handler that wraps another handler and creates a span per request.
pub struct TracingHandler {
    inner: Arc<dyn Handler>,
    tracer: Arc<Tracer>,
}

impl std::fmt::Debug for TracingHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TracingHandler").finish()
    }
}

impl TracingHandler {
    pub fn new(inner: Arc<dyn Handler>, tracer: Arc<Tracer>) -> Self {
        Self { inner, tracer }
    }
}

impl Handler for TracingHandler {
    fn handle(&self, request: &Request) -> Response {
        let mut span = self.tracer.start(
            format!("{} {}", request.method.as_str(), request.path),
            SpanKind::Server,
        );

        span.set_attribute("http.method", request.method.as_str());
        span.set_attribute("http.url", &request.path);

        let response = self.inner.handle(request);

        span.set_attribute("http.status_code", format!("{}", response.status));

        if response.status >= 400 {
            span.finish_with_error(format!("HTTP {}", response.status));
        } else {
            span.finish();
        }

        response
    }

    fn stream(&self, request: &Request) -> StreamDecision {
        let mut span = self.tracer.start(
            format!("{} {} (stream)", request.method.as_str(), request.path),
            SpanKind::Server,
        );

        span.set_attribute("http.method", request.method.as_str());
        span.set_attribute("http.url", &request.path);

        let decision = self.inner.stream(request);

        match &decision {
            StreamDecision::NotAStream => {
                span.finish();
            }
            StreamDecision::Accepted(_) => {
                span.set_attribute("stream.decision", "accepted");
                span.finish();
            }
            StreamDecision::Refused(response) => {
                span.set_attribute("http.status_code", format!("{}", response.status));
                span.finish_with_error(format!("HTTP {}", response.status));
            }
        }

        decision
    }
}
