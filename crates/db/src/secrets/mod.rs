//! Encrypted secrets storage feature module

mod secrets_db;

#[cfg(test)]
mod tests;

pub use secrets_db::{PassRule, SecretMutation, SecretsDb};
