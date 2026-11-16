//! In-app secrets: the Keychain is the master copy; values reach the guest's `shared/.env` on stdin.
pub mod dotenv;
pub mod generate;
pub mod keychain;
pub mod meta;
pub mod store;
