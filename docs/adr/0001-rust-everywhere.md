# 0001 — Rust for everything, including the web interface

**Status:** accepted

## Decision

Every line of the platform is Rust, including the operator interface, which is
server-rendered HTML with no JavaScript at all.

## Why

The requirement was Rust with no exceptions. That rules out the two obvious
ways to build a browser interface: a JavaScript front end, and compiling Rust
to WebAssembly — `wasm-bindgen` generates a JavaScript glue layer, so a
WebAssembly interface is a Rust interface with JavaScript in it.

What remains is server-side rendering, which turns out to suit the problem.
An operator interface for a trading platform is tables, forms and links. The
interactions are "show me the current state" and "halt the platform", both of
which a server can answer. Nothing here needs a client-side runtime.

## What it costs

* No client-side interactivity. A filter or a sort is a round trip.
* Every state change is a form submission and a redirect.
* Charts, if they are ever wanted, will have to be server-rendered SVG.

## What we get

* The content-security policy can be `default-src 'none'` with no script source
  at all. Cross-site scripting is not something the interface tries to catch;
  it is something the policy makes impossible.
* No build step, no bundler, no dependency tree for the front end, and no
  version of the platform's state that lives in a browser and can disagree with
  the server's.
* One language means one set of types. The view model is the same struct the
  API serialises.

## What would make this wrong

An interface that genuinely needs streaming updates — a live order blotter
refreshing many times a second — would be poorly served by full-page reloads.
If that requirement arrives, server-sent events are the smallest change that
would meet it, and they need no client-side framework either.
