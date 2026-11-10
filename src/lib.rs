//! flow: the shared library behind the `flow` CLI and the `flowd` daemon.
//!
//! Three layers: the generic [`store`], the domain models over its facets ([`session`]), and
//! the application services the binaries call ([`services`], [`hook`]).

pub mod hook;
pub mod services;
pub mod session;
pub mod store;
