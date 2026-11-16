//! The host-side egress proxy. Task 2 brings `addr` forward because config validation refuses
//! internal IP literals with the same rule the proxy enforces.
pub mod addr;
