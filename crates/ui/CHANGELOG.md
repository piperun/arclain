# Changelog
All notable changes to this project will be documented in this file. See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

- - -
## arclain_ui-1.7.0 - 2026-05-19
#### Features
- (**core**) add OutputArtifact::Folder for no-pack pipeline output - (4d8e6e1) - piperun
- (**core**) extend convert backend to accept format + compression level - (b00fec9) - piperun
- (**core,ui**) warn on missing screenshots, orphan addons, duplicate previews - (13bb37d) - piperun
- (**pipeline**) output naming uses selected metadata title - (b489108) - piperun
- (**plugins**) add GroupBegin/GroupEnd markers + theme plugin buttons - (22ac530) - piperun
- (**plugins**) route metadata fetches through gameta server when available - (de29881) - piperun
- (**theme**) add typography scale + theme file-type icon colors - (46de97a) - piperun
- (**theme,widgets**) add spacing module + extend Chips/SearchBar - (811dbe5) - piperun
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
- (**widgets**) visually-centered text via mesh_bounds + debug overlay helpers - (bc777b6) - piperun
- (**widgets**) Chips supports clickable mode - (8919aea) - piperun
- decouple metadata fetch from plugin mutex via RequestFetch action - (3c14e70) - piperun
#### Bug Fixes
- (**audit**) surface previously-swallowed errors (H1, H2, H3, H7) - (1334a18) - piperun
- (**core**) panic-free unix_seconds helper (H5, H6) - (8651f3b) - piperun
- (**network,ui**) surface invalid SOCKS5 addresses to the user (M4) - (05711d0) - piperun
- (**plugins**) add native HTTP fallback when gameta server is absent - (a765553) - piperun
- (**plugins**) always notify plugin when background fetch completes - (d90590e) - piperun
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
#### Performance Improvements
- (**network,ui**) notify-driven HTTP completion (P2) - (88dd979) - piperun
- (**plugins,ui**) cache plugin counts and top tabs (P5, P3) - (610081b) - piperun
- (**ui**) virtualize file list and grid views - (6e906e8) - piperun
- (**ui**) cache MainPage plugin layout in detail_view (P4) - (85afd21) - piperun
- (**ui,plugins**) tokio + per-plugin settings for UI events (P7) - (639e7fc) - piperun
- medium-impact wins (P10/P12/P19) + cancel-removes-entry - (09c038a) - piperun
- use sort_by_cached_key and consolidate lock acquisition - (db68abc) - piperun
#### Documentation
- (**ui**) document why background_fetch direct send_ui_event is OK - (95809d7) - piperun
#### Tests
- (**ui**) align encrypted_crc_policy_default with current default - (fc09975) - piperun
- add regression tests for gameta port fixes and new features - (292b42e) - piperun
#### Refactoring
- (**arch**) break data->core cycle via IoC traits - (b91d3f1) - piperun
- (**core**) introduce PipelineContext for executor service access - (0ce551c) - piperun
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
- split fat files per audit recommendations - (dc983f0) - piperun
- swap PathBuf.to_string_lossy().to_string() for .into_owned() - (2213301) - piperun
- route UI's db access through arclain_core re-exports - (a7de1c5) - piperun
- downgrade FPS counter and signal repaint to trace level - (637570c) - piperun
- remove dead code across UI, plugins, and signals - (2298cff) - piperun
- extract shared UI components into widgets crate - (2a89d10) - piperun
#### Miscellaneous Chores
- (**ci**) un-gitignore CHANGELOG.md and track current state - (aaa02e3) - piperun
- (**deps**) introduce [workspace.dependencies] for shared deps - (2ff9062) - piperun
- (**ui**) move preview_tree count helpers inside test module - (f1a0761) - piperun
- (**ui**) trim dead code from panel, toolbar, and app rendering - (5b48dfb) - piperun
- (**version**) bump packages - (911ba47) - piperun
- (**version**) bump packages - (1e9fc9b) - piperun
- (**version**) bump packages - (c3b3d1b) - piperun
- (**version**) bump packages - (4805007) - piperun
- (**version**) arclain_ui-1.2.0 - (3146dd4) - piperun
- (**version**) arclain_ui-1.1.0 - (316ca1d) - piperun
- (**version**) arclain_ui-1.0.0 - (ffff4e2) - piperun
- (**version**) 0.2.0 - (8fc1edb) - piperun
- remove freya-spike from workspace + drop dead type aliases - (fb201a6) - piperun
- remove timing instrumentation from archive open - (0d04923) - piperun
- clean up stale artifacts and modernize cog.toml - (c150815) - piperun
#### Style
- (**ui**) route plugin badge colors through theme tokens - (2c7350a) - piperun
- (**ui**) use theme tokens for error/success indicators - (12a987c) - piperun

- - -

## arclain_ui-1.6.0 - 2026-05-07
#### Features
- (**theme**) add typography scale + theme file-type icon colors - (2edc4cf) - piperun
- (**theme,widgets**) add spacing module + extend Chips/SearchBar - (0dcb8c0) - piperun
#### Bug Fixes
- (**audit**) surface previously-swallowed errors (H1, H2, H3, H7) - (be51153) - piperun
- (**core**) panic-free unix_seconds helper (H5, H6) - (638e0dc) - piperun
- (**network,ui**) surface invalid SOCKS5 addresses to the user (M4) - (4eb7e36) - piperun
- (**plugins**) add native HTTP fallback when gameta server is absent - (4aab72d) - piperun
- (**ui**) make plugin-domain approval atomic (DB then in-memory) - (b49112d) - piperun
- (**ui**) clear in-progress flag on native-dispatch panic (M5) - (db54fa0) - piperun
- (**ui**) propagate config-write errors in apply_preferences (H4) - (ac4a4e9) - piperun
- (**ui**) drain plugin actions across frames in detail_view (C7) - (064fdde) - piperun
- (**ui**) trigger network fetch when carousel images miss the cache - (4d85f54) - piperun
- (**ui**) thread shared_state through process_plugin_actions - (a0e558e) - piperun
- (**ui**) wire shared_state into plugin dialog/page callbacks - (91fb76f) - piperun
- (**ui**) default encrypted-CRC policy to on-access (kills 6-minute hangs) - (72c3bcd) - piperun
#### Performance Improvements
- (**network,ui**) notify-driven HTTP completion (P2) - (fadaa04) - piperun
- (**plugins,ui**) cache plugin counts and top tabs (P5, P3) - (9671b7b) - piperun
- (**ui**) cache MainPage plugin layout in detail_view (P4) - (f426196) - piperun
- (**ui,plugins**) tokio + per-plugin settings for UI events (P7) - (e216ac3) - piperun
- medium-impact wins (P10/P12/P19) + cancel-removes-entry - (7473ef5) - piperun
#### Tests
- (**ui**) align encrypted_crc_policy_default with current default - (dcdb1d0) - piperun
#### Refactoring
- (**arch**) break data->core cycle via IoC traits - (aec9783) - piperun
- (**ui**) split plugin widgets.rs into form/containers/display - (243479f) - piperun
- (**ui**) migrate to canonical widgets + theme tokens - (4c64c95) - piperun
- (**ui**) trim render_list_view by extracting repeated col patterns - (6dd4084) - piperun
- (**ui**) split OrganizePanel::render — extract tab bar, dedupe guard - (a73fba2) - piperun
- (**ui**) split render_list_item into focused helpers - (901845e) - piperun
- (**ui**) centralize error_label helper in shared/components - (70b9802) - piperun
- (**ui**) consolidate Switch into ToggleSwitch widget - (bf88d37) - piperun
- split fat files per audit recommendations - (3631f76) - piperun
- swap PathBuf.to_string_lossy().to_string() for .into_owned() - (efdf209) - piperun
- route UI's db access through arclain_core re-exports - (5665518) - piperun
#### Miscellaneous Chores
- (**ci**) un-gitignore CHANGELOG.md and track current state - (e300c30) - piperun
- (**deps**) introduce [workspace.dependencies] for shared deps - (6b069e8) - piperun
- (**ui**) move preview_tree count helpers inside test module - (d65b331) - piperun
- (**ui**) trim dead code from panel, toolbar, and app rendering - (2dc2ac5) - piperun
- remove freya-spike from workspace + drop dead type aliases - (7fab1cb) - piperun
#### Style
- (**ui**) route plugin badge colors through theme tokens - (9c6e9c5) - piperun
- (**ui**) use theme tokens for error/success indicators - (d4f0ff1) - piperun

- - -

## arclain_ui-1.5.0 - 2026-05-02
#### Features
- (**plugins**) add GroupBegin/GroupEnd markers + theme plugin buttons - (44a644c) - piperun
#### Bug Fixes
- (**plugins**) always notify plugin when background fetch completes - (ae1e655) - piperun

- - -

## arclain_ui-1.4.0 - 2026-04-18
#### Features
- (**core**) add OutputArtifact::Folder for no-pack pipeline output - (258e85b) - piperun
- (**ui**) add default-collision-policy dropdown to Archives settings - (7dce1a9) - piperun
- (**ui**) surface skipped count + interrupted-run banner on Process page - (a452c64) - piperun
- (**ui**) expose collision policy picker on Process page - (a66093f) - piperun
- (**ui**) expose recursive-flatten toggle in step config - (47473bf) - piperun
#### Bug Fixes
- (**ui**) make archive file list adapt to narrow widths - (b2910ee) - piperun

- - -

## arclain_ui-1.3.0 - 2026-04-18
#### Features
- (**ui**) promote Process page progress to modal dialog - (67bf98f) - piperun
- (**ui**) add preset dropdown with save/delete on Process page - (0ab2509) - piperun
- (**ui**) add rule picker dropdown for Organize step config - (b599c76) - piperun
#### Refactoring
- (**core**) introduce PipelineContext for executor service access - (2b69b7f) - piperun
- (**ui**) bring Process page in line with app's theme widgets - (df7c11e) - piperun

- - -

## arclain_ui-1.2.0 - 2026-04-17
#### Features
- (**ui**) wire Process page Execute button with background runner and progress - (f2ba4d3) - piperun
- (**ui**) implement Process page layout with pipeline builder and preview - (5c32a94) - piperun
- (**ui**) add process page skeleton with state and empty view - (b5bbb9f) - piperun
- (**ui**) add AppPage::Process navigation variant - (29338ee) - piperun
#### Refactoring
- (**ui**) rewire Convert/Batch toolbar buttons to Process page, delete old dialog - (d05dba1) - piperun

- - -

## arclain_ui-1.1.0 - 2026-04-17
#### Features
- (**core**) extend convert backend to accept format + compression level - (023bdf7) - piperun
- (**ui**) add batch convert toolbar action and folder scanning - (6091966) - piperun
- (**ui**) wire convert dialog through toolbar and convert_archive flow - (3207b5b) - piperun
- (**ui**) add convert options dialog with format picker and flatten toggle - (546a292) - piperun

- - -

## arclain_ui-1.0.0 - 2026-03-26
#### Miscellaneous Chores
- (**version**) 0.2.0 - (036763a) - piperun

- - -

## arclain_ui-0.12.0 - 2026-03-26
#### Features
- (**plugins**) route metadata fetches through gameta server when available - (de29881) - piperun
- (**ui**) add show_labels toggle for toolbar buttons - (449570f) - piperun
- (**ui**) redesign toolbar with icon-only buttons and group separators - (cb3a970) - piperun
- (**ui**) redesign grid view with vertical cards and file type icons - (bd85c46) - piperun
- (**ui**) add server connection status indicator - (4e9a49b) - piperun
- (**ui**) add gameta server settings page - (9e7c742) - piperun
- (**ui**) improve network log page with filtering, severity colors, and export - (0efb8a8) - piperun
- decouple metadata fetch from plugin mutex via RequestFetch action - (3c14e70) - piperun
#### Bug Fixes
- (**ui**) add horizontal margin to grid view cards - (6ae63e7) - piperun
- (**ui**) use on_primary for checkmark contrast on all themes - (c40c966) - piperun
- (**ui**) add top padding and selection checkmark to grid view - (187fd21) - piperun
- (**ui**) allow navigation to Logs/Settings while plugin page is open - (e67f111) - piperun
- resolve field drift, dead code, and safety issues from deep scan - (7648cfa) - piperun
#### Performance Improvements
- use sort_by_cached_key and consolidate lock acquisition - (db68abc) - piperun
#### Tests
- add regression tests for gameta port fixes and new features - (292b42e) - piperun
#### Refactoring
- (**ui**) convert network log viewer from dialog to page - (97b3a25) - piperun
- downgrade FPS counter and signal repaint to trace level - (637570c) - piperun
- remove dead code across UI, plugins, and signals - (2298cff) - piperun
- extract shared UI components into widgets crate - (2a89d10) - piperun
#### Miscellaneous Chores
- remove timing instrumentation from archive open - (987ccf4) - piperun
- clean up stale artifacts and modernize cog.toml - (c150815) - piperun

- - -

Changelog generated by [cocogitto](https://github.com/cocogitto/cocogitto).