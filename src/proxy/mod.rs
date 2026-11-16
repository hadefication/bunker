//! The host-side egress proxy: the only thing a confined VM can reach.
pub mod addr;
pub mod lease;
pub mod own;
pub mod policy;
pub mod resolve;
pub mod server;
