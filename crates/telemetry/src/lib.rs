//! OpenTelemetry over OTLP, and the subscriber every signal passes through.
//!
//! **Three signals, one destination.** Traces, metrics and logs all leave over
//! OTLP/HTTP to the endpoint a deployment names — which is what makes one
//! request findable three ways: the span tree, the log lines carrying its
//! `trace_id`, and its effect on the rate and latency graphs.
//!
//! **stdout is untouched.** The formatter still writes one JSON object per line
//! and that is still what a deployment reads; the OTLP copy is additive rather
//! than a replacement.
//!
//! **Nothing here reads the environment.** [`Settings`] arrives parsed, from the
//! one config a binary builds in `main` — the same boundary [`env`] draws, and
//! the reason the `OTEL_*` group needs no exception to it.
//!
//! [`env`]: https://docs.rs/env

mod context;
mod provider;
mod settings;
mod subscriber;

pub use context::{continue_from, trace_id};
pub use settings::{Sampling, Settings};
pub use subscriber::{Error, SCOPE, Telemetry, init};
