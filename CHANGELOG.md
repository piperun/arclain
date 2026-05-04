# Changelog
All notable changes to this project will be documented in this file. See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

- - -
## 0.2.0 - 2026-03-26
### Package updates
- dlsite-metadata bumped to dlsite-metadata-0.9.0
- arclain_data bumped to arclain_data-0.4.0
- arclain_db bumped to arclain_db-0.6.0
- arclain_ui bumped to arclain_ui-0.12.0
- arclain_core bumped to arclain_core-0.9.0
- arclain_plugins bumped to arclain_plugins-0.10.0
- arclain-network bumped to arclain-network-0.2.0
### Global changes
#### Features
- decouple metadata fetch from plugin mutex via RequestFetch action - (3c14e70) - piperun
#### Refactoring
- downgrade FPS counter and signal repaint to trace level - (637570c) - piperun

- - -

## 0.1.1 - 2026-03-26
### Package updates
- arclain_plugins bumped to arclain_plugins-0.9.7
- arclain_core bumped to arclain_core-0.8.9
- arclain_db bumped to arclain_db-0.5.5
- dlsite-metadata bumped to dlsite-metadata-0.8.7
### Global changes
#### Bug Fixes
- resolve metadata persistence and image display after gameta port - (e107c7b) - piperun

- - -

## 0.1.0 - 2026-03-26
### Package updates
- arclain_core bumped to arclain_core-0.8.8
- dlsite-metadata bumped to dlsite-metadata-0.8.6
### Global changes
#### Features
- (**data**) introduce resource management feature with unified data access - (8c9f1ee) - piperun
- (**dialog**) add CloseDialog action to plugin system and UI handling - (142fb66) - piperun
- (**dialogs**) implement password and preferences dialogs with helpers - (a20923e) - piperun
- (**dlsite-fetch**) add a new crate for fetching DLSite product pages - (d505949) - piperun
- (**logging**) Enhance logging in sync_rules and OrganizePanel for better traceability - (29c4119) - piperun
- (**network**) introduce arclain-network crate with async HTTP client and security features - (4f6c6f2) - piperun
- (**plugin_dialog**) implement signal-based state management for plugin dialogs and enhance performance - (9c1ea5e) - piperun
- (**proxy**) implement proxy configuration handling and integrate into services - (e97f645) - piperun
- (**settings**) implement security settings page with encryption and vault management - (453e58b) - piperun
- (**ui**) integrate diesel for SQLite support and refactor database interactions - (81726e0) - piperun
- (**ui**) add plugin list view with filtering and selection - (20fd461) - piperun
- (**ui**) rewrite it to be feature first. - (8629365) - piperun
- update package versions across multiple crates to latest stable releases - (04c6ffd) - piperun
- add cover image URL conversion to thumbnail and fix DLSite CDN folder padding - (1ec3a19) - piperun
- enhance caching and image handling in plugins, add CDN thumbnail URL construction - (74202a0) - piperun
- add plugin page display name and metadata rendering - (ae27fdf) - piperun
- add get_product_metadata function with fallback caching mechanism - (933e321) - piperun
- add initial implementation of gameta_server with HTTP API and metadata fetching - (b878fa9) - piperun
- add 'nul' entry to .gitignore - (7e56583) - piperun
- deduplicate screenshot URLs and filter out cover image in HTML parsing - (fe29adc) - piperun
- implement cache management features including garbage collection, old search cache cleanup, and cache entry migration - (594bd30) - piperun
- add carousel gallery widget with lightbox support - (0ba38d4) - piperun
- add merge dialog for multi-part archives - (fe49aea) - piperun
- implement drag-and-drop functionality for file extraction - (3ace121) - piperun
- Introduce gameta_lib for metadata parsing and URL building - (7a0fe26) - piperun
- update package versions for arclain_core, arclain_data, arclain_db, arclain_plugins, and arclain_ui - (a693ab3) - piperun
- add file creation functionality and enhance cache management features - (a8a5b0e) - piperun
- add DbTable derive macro for type-safe table and column references - (49c13d5) - piperun
- Implement type-safe SQL query builders for SELECT and UPDATE operations - (7583eb0) - piperun
- Add release build scripts for Windows and Linux - (b6aa50a) - piperun
- Update dependencies and add integration tests for archive browser functionality - (6b279e8) - piperun
- Update arclain_core version to 0.6.5 and refactor AppState for improved plugin management - (6be6017) - piperun
- Update package versions for arclain_core, arclain_db, arclain_plugins, arclain_ui, dlsite-metadata, and gstreamer-preview - (4c30eb3) - piperun
- Add navigation support for plugin pages and enhance UI interaction - (630b86f) - piperun
- Add Rayon support for parallel extraction in ZIP backend and enhance progress reporting - (047161a) - piperun
- Implement ZIP backend using the `zip` crate and update backend selection logic - (95faabf) - piperun
- Enhance DLSite metadata plugin with image navigation and improved description formatting - (6701faa) - piperun
- Enhance plugin settings and network configuration - (4e135a4) - piperun
- add network settings page and update related tests - (fb77162) - piperun
- add SOCKS5 proxy support with configuration options - (105ac2a) - piperun
- Implement geo-blocking detection and metadata summaries for DLSite products - (ed6e284) - piperun
- Update dependencies and enhance DLsite metadata handling with geo-blocking support - (820ad63) - piperun
- Add configuration for plugin aliases and update dependencies in Cargo.toml - (78347e5) - piperun
- Introduce reactive signals for UI updates and event handling - (caf3c8b) - piperun
- Implement unified product metadata storage and caching - (c1f3d8d) - piperun
- Introduce unified plugin layout structure and update UI rendering logic - (6ab86ff) - piperun
- Implement top-level tab functionality for plugins - (6539562) - piperun
- Implement core application structure and UI pages for settings and password rule management. - (9b1bac1) - piperun
- Introduce WASM plugin system with host functions, UI extensibility, and organization rules. - (32c2f7a) - piperun
- Establish `ui` and `core` crates, implementing the main UI application structure and initial feature integration. - (2e10abb) - piperun
- Implement initial content organization and caching features with dedicated UI and database structures. - (5f7193f) - piperun
- WIP Add metadata caching, plugin host functions for archive, HTTP, logging, metadata, and settings, a core archive organizer, and real data integration tests. - (df347a2) - piperun
- Introduce database layer, plugin system, and organization features with a new DLSite metadata plugin. - (4919888) - piperun
- Introduce archive organization, metadata handling, and a Wasm-based plugin system. - (dd1b36d) - piperun
- Implement initial DLsite API client, data models, and core application structure with plugin system. - (9df067d) - piperun
- Introduce plugin system with SDK, runtime, and WIT definitions, and integrate DB-backed configuration and secrets into application state. - (c7051f4) - piperun
- Implement WASI Component Model based plugin system with a new SDK, host runtime, and example plugins. - (933f53b) - piperun
- add plugin management UI and initial DLSite metadata and GStreamer preview plugins with host function support. - (c0d8393) - piperun
- Implement automatic password matching from configurable regex rules and establish a core configuration module with encrypted secrets storage. - (2675619) - piperun
- Add plugin manager, password rule UI components, and a plugin build script. - (c6f0fe2) - piperun
#### Bug Fixes
- (**dependencies**) remove diesel and reqwest from UI crate dependencies - (5181736) - piperun
- (**dependencies**) downgrade windows-sys and unicode-width versions for compatibility - (98d2be7) - piperun
- align dlsite plugin and organization engine with gameta API - (397a529) - piperun
- update dlsite-metadata plugin for gameta API changes - (00e71cb) - piperun
- update project names in launch configuration for consistency - (d83ef67) - piperun
#### Tests
- improve test infrastructure and parallel safety - (b141785) - piperun
#### Refactoring
- (**dependencies**) rename arclain-http to arclain-network and update dependencies - (29e5bfd) - piperun
- (**ui**) remove drag result enum and start_drag function - (db8c1a7) - piperun
- replace release scripts with cross-platform release.py - (d13034b) - piperun
- replace metadata layer with gameta - (2badfef) - piperun
- extract gameta crates to separate repository - (829aa0f) - piperun
#### Miscellaneous Chores
- clean up stale artifacts and modernize cog.toml - (c150815) - piperun
- add cocogitto for conventional commit versioning - (d4c99bb) - piperun
- use generic test data and add JSON-based metadata loading - (1fff1ac) - piperun
- Update package versions for arclain_ui and arclain_widgets - (14f4699) - piperun
- add gitattributes for line ending normalization and UI guidelines - (89057e9) - piperun
- bump version to 0.11.15 in Cargo.toml - (dbcc65e) - piperun
- update package versions and enhance settings page tests - (2e4d893) - piperun
- update crate versions across the monorepo - (184b989) - piperun

- - -

Changelog generated by [cocogitto](https://github.com/cocogitto/cocogitto).