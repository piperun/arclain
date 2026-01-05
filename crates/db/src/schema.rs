//! Type-safe database schema definitions
//!
//! This module defines all database tables using the DbTable derive macro,
//! providing compile-time safety for SQL queries.
//!
//! # Usage
//! ```ignore
//! use crate::schema::AppConfig;
//! use mini_orm::Select;
//!
//! let sql = Select::from(AppConfig)
//!     .filter(AppConfig::key.equal("theme"))
//!     .build();
//! ```

use mini_orm::DbTable;

// ============================================================================
// Configuration Tables
// ============================================================================

/// Simple key-value configuration storage
#[derive(DbTable)]
#[table = "app_config"]
#[column(key: String)]
#[column(value: String)]
pub struct AppConfig;

/// Title text replacements for cleaning product names
#[derive(DbTable)]
#[table = "title_replacements"]
#[column(id: i64)]
#[column(original: String)]
#[column(replacement: String)]
#[column(is_system: bool)]
#[column(created_at: String)]
pub struct TitleReplacements;

// ============================================================================
// Organization Rules
// ============================================================================

/// Rules for organizing archive contents
#[derive(DbTable)]
#[table = "organization_rules"]
#[column(id: i64)]
#[column(name: String)]
#[column(description: Option<String>)]
#[column(category: String)]
#[column(priority: i32)]
#[column(is_enabled: bool)]
#[column(is_system: bool)]
#[column(trigger_json: String)]
#[column(actions_json: String)]
#[column(created_at: String)]
#[column(modified_at: Option<String>)]
pub struct OrganizationRules;

// ============================================================================
// Domain Whitelist
// ============================================================================

/// Plugin network domain whitelist entries
#[derive(DbTable)]
#[table = "domain_whitelist"]
#[column(id: i64)]
#[column(plugin_id: String)]
#[column(domain: String)]
#[column(approved: bool)]
#[column(approved_at: Option<String>)]
pub struct DomainWhitelist;

// ============================================================================
// UI Configuration Tables
// ============================================================================

/// UI toolbar/context menu items
#[derive(DbTable)]
#[table = "ui_items"]
#[column(id: String)]
#[column(region: String)]
#[column(group_id: Option<String>)]
#[column(label: String)]
#[column(icon: Option<String>)]
#[column(visible: bool)]
#[column(sort_order: i32)]
#[column(action_type: String)]
#[column(action_data: Option<String>)]
#[column(display_mode: String)]
pub struct UiItems;

/// UI region settings (toolbar, context menu, etc.)
#[derive(DbTable)]
#[table = "ui_regions"]
#[column(id: String)]
#[column(enabled: bool)]
#[column(global_display_mode: String)]
pub struct UiRegions;

/// UI display options key-value store
#[derive(DbTable)]
#[table = "ui_display_options"]
#[column(key: String)]
#[column(value: String)]
pub struct UiDisplayOptions;

// ============================================================================
// Product/Library Tables
// ============================================================================

/// Product metadata for library items
#[derive(DbTable)]
#[table = "product_metadata"]
#[column(id: String)]
#[column(source: String)]
#[column(title: Option<String>)]
#[column(circle: Option<String>)]
#[column(release_date: Option<String>)]
#[column(genres: Option<String>)]
#[column(description: Option<String>)]
#[column(cover_url: Option<String>)]
#[column(detail_url: Option<String>)]
#[column(json_blob: Option<String>)]
#[column(created_at: String)]
#[column(modified_at: Option<String>)]
pub struct ProductMetadata;

/// Product content blobs (covers, samples, etc.)
#[derive(DbTable)]
#[table = "product_content"]
#[column(id: i64)]
#[column(product_id: String)]
#[column(content_type: String)]
#[column(content_index: i32)]
#[column(data: Vec<u8>)]
#[column(mime_type: Option<String>)]
#[column(source_url: Option<String>)]
pub struct ProductContent;

// ============================================================================
// Checksum/Verification Tables
// ============================================================================

/// Checksum configuration settings
#[derive(DbTable)]
#[table = "checksum_settings"]
#[column(key: String)]
#[column(value: String)]
pub struct ChecksumSettings;

/// File checksum records
#[derive(DbTable)]
#[table = "file_checksums"]
#[column(id: i64)]
#[column(path: String)]
#[column(archive_id: String)]
#[column(hash: String)]
#[column(size: i64)]
#[column(mtime: i64)]
#[column(algorithm: String)]
#[column(computed_at: String)]
pub struct FileChecksums;

/// Merkle tree roots for archive verification
#[derive(DbTable)]
#[table = "merkle_roots"]
#[column(archive_id: String)]
#[column(root_hash: String)]
#[column(algorithm: String)]
#[column(computed_at: String)]
pub struct MerkleRoots;

/// Checksum operation tracking
#[derive(DbTable)]
#[table = "checksum_operations"]
#[column(id: i64)]
#[column(archive_id: String)]
#[column(op_type: String)]
#[column(state: String)]
#[column(started_at: String)]
#[column(completed_at: Option<String>)]
#[column(error_message: Option<String>)]
pub struct ChecksumOperations;

// ============================================================================
// Cache Tables
// ============================================================================

/// Cache index for tracking cached data
#[derive(DbTable)]
#[table = "cache_index"]
#[column(id: i64)]
#[column(product_id: String)]
#[column(cache_type: String)]
#[column(source: String)]
#[column(created_at: String)]
#[column(accessed_at: String)]
#[column(size_bytes: i64)]
pub struct CacheIndex;
