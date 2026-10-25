//! Host-owned manual conversion boundary. No COM, network implementation,
//! persistent settings or credential-store access is contained here.
#![forbid(unsafe_code)]

pub mod diagnostic_http;
pub mod manual;
pub mod policy;
