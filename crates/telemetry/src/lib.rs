//! Traces, metrics and logs over OTLP, and the subscriber they pass through.
//!
//! stdout keeps its one JSON object per line; the OTLP copy is additive.
//! [`Settings`] arrives parsed — nothing here reads the environment.

mod context;
mod provider;
mod settings;
mod subscriber;

pub use context::{continue_from, trace_id};
pub use settings::{Sampling, Settings};
pub use subscriber::{Error, SCOPE, Telemetry, init};
