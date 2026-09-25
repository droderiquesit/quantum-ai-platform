//! Broker core: partitions, producer table, groups, QoS admission — ADR 0100 §1.
//!
//! Scaffolded by SLICE-50.

pub mod acl;
pub mod admission;
pub mod broker;
pub mod group;
pub mod partition;
pub mod producer;
pub mod service;
