# Changelog
All notable changes to this project will be documented in this file. See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

- - -
## 2.3.1 - 2026-05-28
### Packages
- dlsite-metadata locked to dlsite-metadata-0.11.0
### Global changes
#### Bug Fixes
- (**ui**) keep search palette selection in view while arrowing - (0d8aa15) - 0xdev
- (**ui**) let focused text inputs swallow editing chords - (6e4140b) - 0xdev
- (**ui**) polish unified search palette - (de79c87) - 0xdev

- - -

## 2.3.0 - 2026-05-28
### Packages
- dlsite-metadata locked to dlsite-metadata-0.11.0
### Global changes
#### Features
- (**ui**) unified search palette over tabs and active archive - (890804b) - 0xdev
#### Continuous Integration
- (**release**) push branch and tag separately to fire codeberg pipeline - (5bfdf46) - 0xdev
#### Miscellaneous Chores
- (**just**) add typecheck recipe for the python helpers - (07487a2) - 0xdev
- (**scripts**) track build-helper tests, ignore python cache - (c81c60e) - 0xdev

- - -

## 2.2.6 - 2026-05-28
### Packages
- dlsite-metadata locked to dlsite-metadata-0.11.0
### Global changes
#### Features
- (**password**) upgrade legacy auto-saved rules to broad patterns at startup - (7f12df9) - 0xdev
- (**ui**) move tab scrollbar to its own strip below the tabs - (bc952d0) - 0xdev
- (**ui**) make the tab-strip position pill draggable - (ff6cc66) - 0xdev
#### Bug Fixes
- (**ui**) scrollbar pointer cursor + phosphor icon on Test Regex button - (d9132cc) - 0xdev
- (**ui**) dedupe dropped paths so one drop never double-opens an archive - (d2687b2) - 0xdev
#### Build system
- (**scripts**) add push-release recipe (GitHub first to fire Actions) - (5ab725f) - 0xdev
#### Continuous Integration
- (**github**) let workflow_dispatch on a tag upload to the release - (1461207) - 0xdev
#### Refactoring
- (**plugins**) remove dead archive_backend plumbing - (a4aa67c) - 0xdev
#### Miscellaneous Chores
- (**ui**) gate format_mode to unix to clear Windows dead-code warning - (d264fbf) - 0xdev

- - -

## 2.2.5 - 2026-05-27
### Packages
- dlsite-metadata locked to dlsite-metadata-0.11.0
### Global changes
#### Features
- (**ui**) backend-error dialog + worker repaint backstop - (e3463ee) - 0xdev
- (**ui**) force XWayland on Linux to enable drag-and-drop - (eb5807e) - 0xdev
- (**ui**) round black icon background centered on design - (f7941ab) - 0xdev
- (**widgets**) debug overlays on the remaining 8 widgets - (325aaa8) - 0xdev
#### Bug Fixes
- (**password**) broaden auto-saved rule patterns to actually match siblings - (cdbd6d3) - 0xdev
- (**plugins**) queue archive-open events + pin each to its originating tab - (06fa6d7) - 0xdev
- (**scripts**) drop -A flag from just bump recipes (cog 7 convention) - (04e8f1c) - 0xdev
- (**ui**) list_archive writes to the target tab, not the active one - (251f56e) - 0xdev
- (**ui**) pump metadata signal for every tab, not just the active one - (5ea8461) - 0xdev
- (**ui**) sync plugin archive context to active tab every frame - (f31054a) - 0xdev
#### Performance Improvements
- (**plugins**) list_archive_files reads bridge cache instead of re-listing - (01a8bb5) - 0xdev
#### Documentation
- (**mvu**) document the lock-free plugin-runtime poll exception - (3bb7887) - 0xdev
#### Tests
- (**ui**) render-emit tests for the MVU-converted features - (ab69e32) - 0xdev
- (**ui**) ToggleItemVisibility symmetry coverage for Toolbar + ContextMenu - (d8f0c9d) - 0xdev
- (**ui**) happy-path dispatcher tests against real temp-file DBs - (45d89ba) - 0xdev
- (**ui**) dispatcher tests for the MVU-converted features - (928fb1b) - 0xdev
- (**ui**) drop stale FileEntry.selected init from browser test - (000a36c) - 0xdev
#### Build system
- (**scripts**) split release.py into focused modules + justfile entry - (2875625) - 0xdev
#### Continuous Integration
- (**github**) bump codeberg-release poll budget from 10 to 30 min - (bdd4417) - 0xdev
- (**github**) opt all workflows into Node.js 24 for JS actions - (45249b2) - 0xdev
#### Refactoring
- (**archive_browser**) route post-render plugin dispatch via Action - (afc32cc) - 0xdev
- (**organization**) convert RulesPage to action-emitting views - (f3ff0f3) - 0xdev
- (**organization**) convert ProfilesPage to action-emitting view - (9e5f9bb) - 0xdev
- (**plugins**) ActiveTabBridge replaces held per-tab signal handles - (f6f4e45) - 0xdev
- (**plugins**) drop dead PluginAction/UiElement variants + scaffolds - (4bae967) - 0xdev
- (**process**) convert Process page to action-emitting views - (0b91d22) - 0xdev
- (**settings**) LayoutEditor consumes from canonical signals - (b7593bc) - 0xdev
- (**settings**) dedup Interface page item cache via signals - (2faf532) - 0xdev
- (**settings**) cache security-page default paths at state construction - (cceece8) - 0xdev
- (**settings**) convert Interface page to action-emitting view - (9d1b0c0) - 0xdev
- (**settings**) extract LayoutEditor<R> abstraction + MVU convert - (4cf6e6e) - 0xdev
- (**ui**) split selection into HashSet on BrowserViewState - (95a2eec) - 0xdev
#### Miscellaneous Chores
- (**ci**) dev Containerfile + compose with CI-step mirrors - (d8361f8) - 0xdev
- (**deps**) refresh Cargo.lock — registry version bumps from CI test runs - (72a6a1c) - 0xdev

- - -

## 2.2.4 - 2026-05-22
### Packages
- dlsite-metadata locked to dlsite-metadata-0.11.0
### Global changes
#### Features
- (**widgets**) debug overlays for TextButton + ToggleSwitch - (5631dad) - 0xdev
#### Bug Fixes
- (**ci**) use branch name for publish-release target_commitish - (fd9c10f) - 0xdev
- (**ci**) serialize wasm-plugins after cargo-check/cargo-test - (6a9aba6) - 0xdev

- - -

## 2.2.3 - 2026-05-22
### Packages
- dlsite-metadata locked to dlsite-metadata-0.11.0
### Global changes
#### Bug Fixes
- (**ci**) collapse build-linux package+checksum steps into one shell - (523f062) - 0xdev
#### Continuous Integration
- nextest on woodpecker, narrow github tests workflow to PRs - (7e6bf88) - 0xdev

- - -

## 2.2.2 - 2026-05-22
### Packages
- dlsite-metadata locked to dlsite-metadata-0.11.0
### Global changes
#### Bug Fixes
- (**scripts**) read version from [workspace.package] not crate Cargo.toml - (e313399) - 0xdev
#### Continuous Integration
- (**github**) rust-cache on windows-build, new tests workflow with nextest - (f05f6a0) - 0xdev

- - -

## 2.2.1 - 2026-05-22
### Packages
- dlsite-metadata locked to dlsite-metadata-0.11.0
### Global changes
#### Bug Fixes
- (**ci**) pass --skip-tests to release.py to avoid duplicate test pass as root - (273dea8) - 0xdev

- - -

## 2.2.0 - 2026-05-22
### Packages
- dlsite-metadata locked to dlsite-metadata-0.11.0
### Global changes
#### Features
- (**ui**) wired-eye app icon, rasterized from bundled SVG - (e6fce32) - 0xdev
- (**ui**) back<-home button order + global FPS/frame-time debug HUD - (2aca69d) - 0xdev
- (**widgets**) debug-rect overlay for IconButton - (af91cfe) - 0xdev
#### Bug Fixes
- (**ci**) chown CARGO_HOME/RUSTUP_HOME to runner in windows step - (b7dfa0c) - 0xdev
- (**ci**) per-platform .sha256 sidecars, drop cross-step depends_on - (c78853e) - 0xdev
- (**db**) chmod 0700 on secrets dir parent (Unix) - (cc034a9) - 0xdev
- (**db**) chmod 0600 on ConfigDb + SecretsDb files (Unix) - (2e8ef28) - 0xdev
- (**plugins**) deflake token_bucket_allows_burst_up_to_capacity - (d7f699e) - 0xdev
#### Documentation
- (**app_fs**) tighten the Windows caveat — inheritance is not a DACL - (f9f3469) - 0xdev
- add GPL-3.0 LICENSE (pulled from codeberg auto-init) - (7f697e8) - 0xdev
- rewrite README with banner, Fluffy target, GPL-3.0 license - (1f1dbda) - 0xdev
#### Tests
- (**core**) add end-to-end mock test for dlsite sync pipeline - (4ba9fff) - 0xdev
#### Continuous Integration
- (**github**) windows + experimental flatpak workflows for the mirror - (774b12f) - 0xdev
- (**woodpecker**) disable Windows cross-compile, mark step manual-only - (226dbc0) - 0xdev
- (**woodpecker**) run windows build on push too, temporarily - (7638c27) - 0xdev
- (**woodpecker**) add windows x86_64 build via cargo-xwin - (1a6a48c) - 0xdev
- (**woodpecker**) drop to runner in cargo-check + wasm-plugins too - (ea67cc5) - 0xdev
- (**woodpecker**) cd into plugins/dlsite-metadata to build wasm - (c8c3df0) - 0xdev
- (**woodpecker**) drop to non-root user for cargo-test - (ee3bc42) - 0xdev
- (**xwin**) pass --xwin-include-debug-libs to pull in extra SDK headers - (3d53fb4) - 0xdev
- add woodpecker pipeline for codeberg - (1e4559e) - 0xdev
#### Refactoring
- (**db**) extract perm helpers into arclain_app_fs crate - (8db40fd) - 0xdev
- (**plugins**) make TokenBucket testable via Clock trait - (013402a) - 0xdev
- (**workspace**) bump+centralize thiserror (1→2) and rand (0.8→0.9) - (037b5c9) - 0xdev
- (**workspace**) centralize common deps via [workspace.dependencies] - (c2e99c7) - 0xdev
- (**workspace**) collapse versions via [workspace.package] - (617124a) - 0xdev
- move AppDirectories from arclain_core into arclain_app_fs - (a5f0fbd) - 0xdev
#### Miscellaneous Chores
- document clean up - (ed9d48c) - 0xdev
- refresh Cargo.lock for gameta v0.5.0 - (3554565) - 0xdev
- update Cargo.lock for gameta 0.4.3 integration - (921344f) - 0xdev

- - -

## 2.1.0 - 2026-05-21
### Packages
- dlsite-metadata locked to dlsite-metadata-0.8.7
### Global changes
#### Features
- (**core**) add OutputArtifact::Folder for no-pack pipeline output - (4d8e6e1) - piperun
- (**core**) stream hash progress as StepStart/StepProgress events - (e22b5fd) - piperun
- (**core**) thread app-default collision policy through executor - (75b7d90) - piperun
- (**core**) wire executor to pipeline_runs DB for dedup + audit - (20665d8) - piperun
- (**core**) add OutputCollisionPolicy gate to pipeline executor - (d6d48e6) - piperun
- (**core**) recursive flatten with max_depth and safety caps - (39cbe35) - piperun
- (**core**) add pipeline preset save/load with builtin presets - (f908b17) - piperun
- (**core**) implement Organize pipeline step using RuleEngine - (17143b6) - piperun
- (**core**) apply organization plan moves to extracted work dir - (cc8764f) - piperun
- (**core**) add blocking pipeline executor with progress events - (269faa7) - piperun
- (**core**) add pure pipeline preview builder - (506a580) - piperun
- (**core**) add Pipeline types for archive processing workflows - (f1a01ee) - piperun
- (**core**) extend convert backend to accept format + compression level - (b00fec9) - piperun
- (**core**) add flatten_nested_archives for unnesting mod archives - (6283fb1) - piperun
- (**core**) add ConvertOptions types and longest_common_prefix helper - (0b23217) - piperun
- (**core**) wire GametaClient initialization from UserConfig - (a562274) - piperun
- (**core,plugin**) host-side list_cached_entries cache, drop WASM memo (Path D step 1) - (25e2b9d) - piperun
- (**core,ui**) warn on missing screenshots, orphan addons, duplicate previews - (13bb37d) - piperun
- (**data**) add ServerResolver for gameta server API routing - (7094b2a) - piperun
- (**db**) add pipeline_runs table + repo for idempotent pipeline dedup - (b9bdde4) - piperun
- (**db**) add gameta server config fields to UserConfig - (8842e0a) - piperun
- (**network**) add GametaClient for server API communication - (7b65fdd) - piperun
- (**network,data,plugins**) streaming downloads with range/resume for fetch_to_cache - (e14aea0) - piperun
- (**pipeline**) output filename falls back to detected product code - (0e7c032) - piperun
- (**pipeline**) output naming uses selected metadata title - (b489108) - piperun
- (**plugins**) route plugin logs into per-plugin files - (cd48397) - piperun
- (**plugins**) periodic drop summary to arclain.log - (afe9f31) - piperun
- (**plugins**) byte cap on per-plugin log files - (7c0eba1) - piperun
- (**plugins**) per-plugin file logger with daily rotation - (bda71b6) - piperun
- (**plugins**) add token bucket for per-plugin log rate limit - (e504de6) - piperun
- (**plugins**) WIT get-metadata export — plugins self-report instead of "Unknown" - (e0cd8b7) - piperun
- (**plugins**) play_cached_blob — hand cached videos to OS default app - (bfe1bc7) - piperun
- (**plugins**) fetch_to_cache host fn — store blobs without crossing WASM boundary - (ab3a2dd) - piperun
- (**plugins**) add GroupBegin/GroupEnd markers + theme plugin buttons - (22ac530) - piperun
- (**plugins**) route metadata fetches through gameta server when available - (de29881) - piperun
- (**scripts**) `debug` subcommand for fast iteration builds - (a7af527) - piperun
- (**theme**) add typography scale + theme file-type icon colors - (46de97a) - piperun
- (**theme,widgets**) add spacing module + extend Chips/SearchBar - (811dbe5) - piperun
- (**ui**) auto-retry file extraction after password unlock - (6967489) - piperun
- (**ui**) tab debug overlay — add inner-padding + centering guides - (42f70dd) - piperun
- (**ui**) tab debug overlay — paint_widget_rect_debug per tab - (f97db4e) - piperun
- (**ui**) drop-overlay Ctrl hint, plugin-tab nav fix, close-modal kittests - (cd06a5e) - piperun
- (**ui**) tab UX polish — overflow popup, context menu, pinned tabs - (7642709) - piperun
- (**ui**) reopen-closed-tab, middle-click close, drag-to-reorder - (7949fe9) - piperun
- (**ui**) wire PasswordDialog.pending_tab_id for multi-drop retry - (3f2b4cf) - piperun
- (**ui**) wire AskEachTime drop-behavior modal - (73fde57) - piperun
- (**ui**) wire per-tab in_flight_ops counter and tab_cancel checks - (175dce6) - piperun
- (**ui**) per-tab archive state foundation (single hardcoded tab) - (0b3cdcf) - piperun
- (**ui**) status bar shows 'Item selected' chip with click-to-detail - (08aaa74) - piperun
- (**ui**) toggle Logs/Plugins/Settings header buttons - (6ced3d1) - piperun
- (**ui**) extract dispatch_plugin_event helper - (2de4891) - piperun
- (**ui**) add default-collision-policy dropdown to Archives settings - (d73e1eb) - piperun
- (**ui**) surface skipped count + interrupted-run banner on Process page - (9446db5) - piperun
- (**ui**) expose collision policy picker on Process page - (60e7c2e) - piperun
- (**ui**) expose recursive-flatten toggle in step config - (66fba93) - piperun
- (**ui**) promote Process page progress to modal dialog - (2ba6793) - piperun
- (**ui**) add preset dropdown with save/delete on Process page - (5a88b54) - piperun
- (**ui**) add rule picker dropdown for Organize step config - (08e2e4f) - piperun
- (**ui**) wire Process page Execute button with background runner and progress - (bc01b35) - piperun
- (**ui**) implement Process page layout with pipeline builder and preview - (854dd1a) - piperun
- (**ui**) add process page skeleton with state and empty view - (99d500f) - piperun
- (**ui**) add AppPage::Process navigation variant - (b92a2cd) - piperun
- (**ui**) add batch convert toolbar action and folder scanning - (3b571cb) - piperun
- (**ui**) wire convert dialog through toolbar and convert_archive flow - (53c626e) - piperun
- (**ui**) add convert options dialog with format picker and flatten toggle - (389e5fb) - piperun
- (**ui**) add show_labels toggle for toolbar buttons - (2349f4a) - piperun
- (**ui**) redesign toolbar with icon-only buttons and group separators - (f88bfe0) - piperun
- (**ui**) redesign grid view with vertical cards and file type icons - (bd85c46) - piperun
- (**ui**) add server connection status indicator - (4e9a49b) - piperun
- (**ui**) add gameta server settings page - (9e7c742) - piperun
- (**ui**) improve network log page with filtering, severity colors, and export - (0efb8a8) - piperun
- (**ui,core,db**) tab UI, drop overlay, drop_behavior setting - (d3c9eab) - piperun
- (**ui,db**) tab persistence, cancel tokens, and TabPluginPool scaffold - (25280b0) - piperun
- (**widgets**) EGUI_UI_DEBUG_GUIDELINES env var — project-wide UI debug toggle - (9eea8e6) - piperun
- (**widgets**) visually-centered text via mesh_bounds + debug overlay helpers - (bc777b6) - piperun
- (**widgets**) Chips supports clickable mode - (8919aea) - piperun
- address three deferred TODOs (native progress, cache action, element update) - (105b652) - piperun
- decouple metadata fetch from plugin mutex via RequestFetch action - (3c14e70) - piperun
#### Bug Fixes
- (**audit**) defensive cleanups for medium findings (M1, M2, M6, M7) - (ce3bd44) - piperun
- (**audit**) surface previously-swallowed errors (H1, H2, H3, H7) - (1334a18) - piperun
- (**cog**) scope version-replace sed to [package] section only - (89b6f43) - piperun
- (**core**) use parking_lot::RwLock for title FILTER_CACHE (M3) - (9926711) - piperun
- (**core**) panic-free unix_seconds helper (H5, H6) - (8651f3b) - piperun
- (**core**) propagate set_config errors in vault operations (C5) - (6b699a5) - piperun
- (**core**) propagate move_dir remove failure (C3) - (7d92365) - piperun
- (**core**) use native unrar for filenames, CLI only for packed_size - (fd7bc38) - piperun
- (**core**) route RAR listing through UnRAR CLI to recover packed_size - (1f3839c) - piperun
- (**core**) unwrap single-root folder after flatten extraction - (467f43e) - piperun
- (**core**) suppress Windows console window on subprocess spawns - (5adacf1) - piperun
- (**core**) flatten_nested_archives walks tree and places folders next to source - (fd299e9) - piperun
- (**data**) surface resolver IoErrors instead of generic 'no source' msg - (32e44ee) - piperun
- (**db**) abort cache recovery when WAL or SHM removal fails (C6) - (01b1577) - piperun
- (**db**) abort migration when backup fails (C4) - (747c436) - piperun
- (**flatten**) use modinfo.ini name= as folder name + abort broken strips - (a8ae6f4) - piperun
- (**network,ui**) surface invalid SOCKS5 addresses to the user (M4) - (05711d0) - piperun
- (**plugins**) release plugins.read() before per-instance work (C1) - (ce3fd8c) - piperun
- (**plugins**) release instance lock during gameta HTTP fetch (C2) - (eabac78) - piperun
- (**plugins**) add native HTTP fallback when gameta server is absent - (a765553) - piperun
- (**plugins**) always notify plugin when background fetch completes - (d90590e) - piperun
- (**ui**) show password dialog when extraction fails on encrypted content - (da158b9) - piperun
- (**ui**) nested archive clicks open inside arclain, not Windows Explorer - (f14b299) - piperun
- (**ui**) auto-bind per-tab signals to ctx-repaint (root-cause fix) - (1f4fcf8) - piperun
- (**ui**) drop-zip — request_repaint after background archive load - (fd567c4) - piperun
- (**ui**) tab badge — env-gated debug overlay + cleaner layout/paint - (c79b763) - piperun
- (**ui**) tab bar badge — mesh-bounds visual centering + tighter badge shape - (a4efedd) - piperun
- (**ui**) top tab bar numeric badge — readable contrast + proper pill rendering - (8c2f15e) - piperun
- (**ui**) migrate archive_browser_test.rs to per-tab signal access - (9590630) - piperun
- (**ui**) item-selected chip is clickable and vertically centered - (f6ce5b5) - piperun
- (**ui**) item-selected popup toggles correctly on chip click - (16225b0) - piperun
- (**ui**) plugin page/dialog keep last layout while worker holds lock - (5a064df) - piperun
- (**ui**) __page_init uses try_lock to avoid 15s UI freeze on plugin page - (935985d) - piperun
- (**ui**) plugin domain access labels use explicit on_surface color - (68d2c0b) - piperun
- (**ui**) make plugin-domain approval atomic (DB then in-memory) - (d088a75) - piperun
- (**ui**) clear in-progress flag on native-dispatch panic (M5) - (d4d0867) - piperun
- (**ui**) propagate config-write errors in apply_preferences (H4) - (6ea4aa8) - piperun
- (**ui**) drain plugin actions across frames in detail_view (C7) - (5ffc77c) - piperun
- (**ui**) trigger network fetch when carousel images miss the cache - (5147b56) - piperun
- (**ui**) thread shared_state through process_plugin_actions - (e676437) - piperun
- (**ui**) wire shared_state into plugin dialog/page callbacks - (8a0dfd0) - piperun
- (**ui**) default encrypted-CRC policy to on-access (kills 6-minute hangs) - (e765b5a) - piperun
- (**ui**) make archive file list adapt to narrow widths - (0f2ef89) - piperun
- (**ui**) add horizontal margin to grid view cards - (6ae63e7) - piperun
- (**ui**) use on_primary for checkmark contrast on all themes - (c40c966) - piperun
- (**ui**) add top padding and selection checkmark to grid view - (187fd21) - piperun
- (**ui**) allow navigation to Logs/Settings while plugin page is open - (e67f111) - piperun
- (**ui,plugins**) try_lock plugin reads to prevent UI freeze during fetches - (7d540db) - piperun
- (**ui-tests**) update Switch and arclain_db references - (c6dbe39) - piperun
- resolve field drift, dead code, and safety issues from deep scan - (fa99af3) - piperun
- fully decouple archive open from plugin mutex - (939a984) - piperun
#### Performance Improvements
- (**core**) O(1) DLL dedup; preallocate archive listing capacity - (c9c24c9) - piperun
- (**data**) release ServerResolver lock during gameta HTTP (P6) - (21a0e3c) - piperun
- (**network,ui**) notify-driven HTTP completion (P2) - (88dd979) - piperun
- (**plugins**) dirty-bit short-circuit on get_all_settings (audit P14) - (888873d) - piperun
- (**plugins**) batch get_metadata_summaries — N+1 → 1 SQL query - (2888133) - piperun
- (**plugins**) invalidate top tabs cache on load/unload (P3 follow-up) - (e59a2e7) - piperun
- (**plugins,ui**) cache plugin counts and top tabs (P5, P3) - (610081b) - piperun
- (**ui**) virtualize file list and grid views - (6e906e8) - piperun
- (**ui**) cache MainPage plugin layout in detail_view (P4) - (85afd21) - piperun
- (**ui,plugins**) tokio + per-plugin settings for UI events (P7) - (639e7fc) - piperun
- medium-impact wins (P10/P12/P19) + cancel-removes-entry - (09c038a) - piperun
#### Documentation
- (**db**) explain strategy-(b) survival of get_config dual API - (73f4337) - piperun
- (**ui**) B3 slices 3+4+5 — document set_if_changed rationale (kept as correct) - (e780274) - piperun
- (**ui**) document why background_fetch direct send_ui_event is OK - (95809d7) - piperun
- introduce ARCZIP format spec, defer CARE as R&D - (c354922) - piperun
#### Tests
- (**audit**) regression tests for critical findings C1/C5/C6 - (347eab1) - piperun
- (**audit**) regression tests for critical findings C2/C3/C4 - (6af0ee6) - piperun
- (**core**) end-to-end idempotency + overwrite verification - (6274876) - piperun
- (**network**) add integration tests for GametaClient - (d4fa0e5) - piperun
- (**ui**) align encrypted_crc_policy_default with current default - (fc09975) - piperun
- add integration test for apply_plan_to_workdir - (3371059) - piperun
- add integration tests for pipeline preview and execution scaffold - (4d346b4) - piperun
- add integration tests for archive conversion flatten - (51383f0) - piperun
- add regression tests for deep scan fixes and fix completeness_score - (41dd9dd) - piperun
- add regression tests for metadata field naming bugs - (13785e9) - piperun
- add regression tests for gameta port fixes and new features - (292b42e) - piperun
#### Build system
- (**core**) drop native-tls from reqwest, align with network's rustls - (52fedf4) - piperun
#### Refactoring
- (**arch**) break data->core cycle via IoC traits - (b91d3f1) - piperun
- (**core**) R2 — epoch-guard LibraryService cache insert against invalidation race - (27c4b68) - piperun
- (**core**) split engine.rs into engine/{mod,plan_builder,tree}.rs - (57eff70) - piperun
- (**core**) split RuleEngine::create_plan into focused helpers - (aed9d7c) - piperun
- (**core**) introduce PipelineContext for executor service access - (0ce551c) - piperun
- (**data**) drop unused anyhow::Result re-export - (c8457cd) - piperun
- (**db**) hide ReDb + diesel_schema (pub(crate)) - (3e6a4fe) - piperun
- (**db**) collapse dual CRUD — checksum + title (7 fns) - (2b7342f) - piperun
- (**db**) collapse dual CRUD — ui (12 fns + dead-code) - (39f3e44) - piperun
- (**db**) collapse dual CRUD — domain_whitelist (10 fns) - (ce6046e) - piperun
- (**db**) collapse dual CRUD — profiles + rules (10 fns) - (328393b) - piperun
- (**db**) audit A5 — drop orphaned last_opened_archive column - (a080bba) - piperun
- (**db**) split lib.rs bootstrap, extract cache types, rename archive_profiles - (b9431bd) - piperun
- (**db**) split ui/config.rs into 4 cohesive sub-modules - (e5f33fa) - piperun
- (**db**) drop legacy module, keep only used cache_index helpers - (d43eda6) - piperun
- (**db**) extract diesel_err helper, kill 49x boilerplate - (2d7360e) - piperun
- (**network**) hoist HTTP timeout magic numbers to crate consts - (2c00ba0) - piperun
- (**network**) extract gameta API path constants - (d8b6392) - piperun
- (**network**) extract dlsite-header injection helper - (ed044d2) - piperun
- (**plugins**) flatten get_product_metadata lazy-repair nesting - (90ccc01) - piperun
- (**plugins**) move wit_rules From impls to conversions.rs - (795efaf) - piperun
- (**plugins**) extract WIT conversions to conversions.rs - (9843201) - piperun
- (**plugins**) collapse PluginManager dispatch APIs to one canonical path - (83a1d2f) - piperun
- (**plugins**) extract enabled_plugin_snapshot helper - (d9b7077) - piperun
- (**signals**) R7 — snapshot listener Vec before invoking callbacks - (1b99c77) - piperun
- (**ui**) move trigger_image_fetch to shared/ — close carousel→features leak - (57a3a8f) - piperun
- (**ui**) restore core/⊥features/ invariant — relocate BrowserViewState to core/tabs - (6515bc1) - piperun
- (**ui**) restore shared/⊥features/ invariant — plugin-rendering callback - (cdfa528) - piperun
- (**ui**) R4 — hoist backend_selector clone out of per-file closures - (7aa0c09) - piperun
- (**ui**) audit Tier 2 item 7 — drop StatusBarInfo mirrored fields, restore status_bar write - (bf3b707) - piperun
- (**ui**) audit Tier 2 item 6 — archive_info as Computed<ArchiveInfo> - (79cda96) - piperun
- (**ui**) audit Tier 2 item 8 — archive_loaded as Computed<bool> - (a3cd9ba) - piperun
- (**ui**) drop dead app_coordinator.rs (audit §5 #14) - (23ee35a) - piperun
- (**ui**) audit B1 follow-up — bundle render_settings_content's 18 params into SettingsContentBorrows - (db0a2f9) - piperun
- (**ui**) audit B1 slice 5c — PluginsFeature owns settings list state + context bundle - (55951e9) - piperun
- (**ui**) audit B1 slice 5b — PasswordManagementFeature owns password_rules_dialog - (96f7b86) - piperun
- (**ui**) audit B1 slice 5a — HotkeysFeature owns keyboard_mouse_state - (f796b7d) - piperun
- (**ui**) audit B1 slice 4 — drop dead SettingsFeatureState + PasswordFeatureState - (5ce8aea) - piperun
- (**ui**) audit B1 slice 3 — move org rules + archive profiles pages to organization feature - (d6d581f) - piperun
- (**ui**) audit B1 slice 2 — move plugins settings page to plugins feature - (8cc87f3) - piperun
- (**ui**) audit B1 slice 1 — move keyboard_mouse settings page to hotkeys feature - (5c7029e) - piperun
- (**ui**) panel + page rendering use async dispatch - (4f61a9a) - piperun
- (**ui**) toolbar plugin events go through async dispatch - (5f32722) - piperun
- (**ui**) plugin dialog/page callbacks use async dispatch - (b2e2634) - piperun
- (**ui**) detail_view uses dispatch_plugin_event helper - (7458383) - piperun
- (**ui**) move pending_plugin_actions to SharedState - (f6f8817) - piperun
- (**ui**) split plugin widgets.rs into form/containers/display - (a678816) - piperun
- (**ui**) migrate to canonical widgets + theme tokens - (5d97e5d) - piperun
- (**ui**) trim render_list_view by extracting repeated col patterns - (190dcc7) - piperun
- (**ui**) split OrganizePanel::render — extract tab bar, dedupe guard - (038d9e7) - piperun
- (**ui**) split render_list_item into focused helpers - (b5b503d) - piperun
- (**ui**) centralize error_label helper in shared/components - (f584c80) - piperun
- (**ui**) consolidate Switch into ToggleSwitch widget - (85d9351) - piperun
- (**ui**) bring Process page in line with app's theme widgets - (29ac2d7) - piperun
- (**ui**) rewire Convert/Batch toolbar buttons to Process page, delete old dialog - (84c62e4) - piperun
- (**ui**) convert network log viewer from dialog to page - (97b3a25) - piperun
- dead-code Tier 2 + Computed-migration loose ends - (a11af32) - piperun
- dead-code audit Tier 1 — 3 safe deletions (~39 LOC) - (0989f6f) - piperun
- deps audit quick wins — 3 pub demotions + cfg-gate debug helpers - (083056e) - piperun
- concurrency audit R6 + R9 — fix poison handling in SqliteDb + GametaClient - (cf4fdaa) - piperun
- B3 reframed slice 2 — progress_dialogs to per-tab TabState - (bd30a4c) - piperun
- B3 reframed slice 1 — password_dialog to per-tab TabState - (0f667e6) - piperun
- audit B2 remaining — migrate merge_dialog + lightbox_state to TabState - (7bff240) - piperun
- audit B2 partial — migrate pending_open_file + file_edit_dialog to TabState - (8623cc4) - piperun
- audit A3 — collapse 3 progress-dialog signals via ProgressDialogs slot-struct + proxy - (9975481) - piperun
- audit Tier A — concurrency (R3, R8) + shared/plugins boundary - (59c084a) - piperun
- audit Tier S cleanup — fix R1 concurrency, drop dead code - (8113fdf) - piperun
- clean Debug-formatted user-facing log lines - (0748961) - piperun
- nuke mini-orm crate, fold SqliteDb into arclain_db - (07360f0) - piperun
- split fat files per audit recommendations - (dc983f0) - piperun
- move cache_index + flatten tests to sibling files - (2793848) - piperun
- swap PathBuf.to_string_lossy().to_string() for .into_owned() - (2213301) - piperun
- route UI's db access through arclain_core re-exports - (a7de1c5) - piperun
- name magic-number duration constants - (7b0cb1f) - piperun
- downgrade FPS counter and signal repaint to trace level - (637570c) - piperun
- downgrade noisy log levels to debug/trace - (742aab8) - piperun
#### Miscellaneous Chores
- (**ci**) un-gitignore CHANGELOG.md and track current state - (aaa02e3) - piperun
- (**core**) add flatten_demo example for spot-checking real data - (82a52e1) - piperun
- (**deps**) sync Cargo.lock for tracing-test dev-dep - (ef3342e) - piperun
- (**deps**) refresh Cargo.lock after package version bumps - (47aaafe) - piperun
- (**deps**) introduce [workspace.dependencies] for shared deps - (2ff9062) - piperun
- (**deps**) remove unused dependencies - (a02f0f6) - piperun
- (**plugin-sdk**) drop orphaned dlsite-plugin example - (cc6511c) - piperun
- (**scripts**) add -v/-t short aliases for release skip flags - (78bc1d0) - piperun
- (**ui**) move preview_tree count helpers inside test module - (f1a0761) - piperun
- (**ui**) trim dead code from panel, toolbar, and app rendering - (5b48dfb) - piperun
- (**version**) harmonize workspace at v2.0.0 - (5de3e94) - piperun
- (**version**) arclain_ui-1.7.0 - (9751b83) - piperun
- (**version**) arclain_plugins-0.13.0 - (152e6bc) - piperun
- (**version**) arclain_db-0.9.0 - (6275675) - piperun
- (**version**) arclain_core-0.12.0 - (478cfcb) - piperun
- (**version**) realign core + db Cargo.toml with latest tags - (a0b9a7e) - piperun
- (**version**) arclain_db-0.6.0 - (63e203b) - piperun
- (**version**) arclain_core-0.9.0 - (0ed5c16) - piperun
- (**version**) bump packages - (911ba47) - piperun
- (**version**) bump packages - (1e9fc9b) - piperun
- (**version**) bump packages - (c3b3d1b) - piperun
- (**version**) bump packages - (4805007) - piperun
- (**version**) arclain_ui-1.2.0 - (3146dd4) - piperun
- (**version**) arclain_ui-1.1.0 - (316ca1d) - piperun
- (**version**) arclain_ui-1.0.0 - (ffff4e2) - piperun
- (**version**) 0.2.0 - (8fc1edb) - piperun
- clippy nits — drop useless format! and Copy clones - (11a63d8) - piperun
- remove freya-spike from workspace + drop dead type aliases - (fb201a6) - piperun
- remove root CHANGELOG.md - (bcc457a) - piperun
- remove stray src/main.rs and .versions.json - (f97b7c7) - piperun
- remove dead dlsite-fetch alias and spike artifact - (1564eff) - piperun
- fold ad-hoc shell scripts into release.py subcommands - (3bd90d0) - piperun
- update Cargo.lock after 1.3 work - (68435b3) - piperun
- migrate cog.toml to 7.0 monorepo.packages format - (4a77cca) - piperun
- update Cargo.lock after conversion redesign - (2b3cacd) - piperun
- remove timing instrumentation from archive open - (0d04923) - piperun
#### Style
- (**ui**) route plugin badge colors through theme tokens - (2c7350a) - piperun
- (**ui**) use theme tokens for error/success indicators - (12a987c) - piperun

- - -

Changelog generated by [cocogitto](https://github.com/cocogitto/cocogitto).