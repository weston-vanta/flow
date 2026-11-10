//! flow: the shared library behind the `flow` CLI and the `flowd` daemon.
//!
//! Three layers: the generic [`store`], the domain models over its facets ([`session`], [`pr`], [`branch`]), and
//! the application services the binaries call ([`services`], [`hook`]).

pub mod branch;
pub mod hook;
pub mod pr;
pub mod services;
pub mod session;
pub mod store;
