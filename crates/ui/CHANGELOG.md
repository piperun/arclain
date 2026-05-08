# Changelog
All notable changes to this project will be documented in this file. See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

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