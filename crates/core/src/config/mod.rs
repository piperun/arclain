//! Configuration management
//!
//! Configuration management
//!
//! This module provides configuration and password management functionality:
//! - Application settings
//! - Password rules for automatic detection
//! - Database configuration

pub mod database;
pub mod defaults;
pub mod settings;
pub mod sync;

pub use database::{
    open_databases, ConfigDb, ConfigDbs, DbPaths, DbTitleReplacement, SecretsDb, SecretsKey,
};
pub use settings::{Config, ConfigStore, PassRule};
