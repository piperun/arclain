//! Configuration management
//! 
//! This module provides configuration and password management functionality:
//! - Application settings
//! - Password rules for automatic detection
//! - Database configuration

pub mod database;
pub mod settings;

pub use database::{open_databases, ConfigDb, ConfigDbs, DbPaths, SecretsDb, SecretsKey};
pub use settings::{Config, ConfigStore, PassRule};