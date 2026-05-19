# Changelog
All notable changes to this project will be documented in this file. See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

- - -
## arclain_plugins-0.13.0 - 2026-05-09
#### Features
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
- decouple metadata fetch from plugin mutex via RequestFetch action - (3c14e70) - piperun
#### Bug Fixes
- (**audit**) defensive cleanups for medium findings (M1, M2, M6, M7) - (ce3bd44) - piperun
- (**audit**) surface previously-swallowed errors (H1, H2, H3, H7) - (1334a18) - piperun
- (**plugins**) release plugins.read() before per-instance work (C1) - (ce3fd8c) - piperun
- (**plugins**) release instance lock during gameta HTTP fetch (C2) - (eabac78) - piperun
- (**plugins**) add native HTTP fallback when gameta server is absent - (a765553) - piperun
- (**ui,plugins**) try_lock plugin reads to prevent UI freeze during fetches - (7d540db) - piperun
- resolve field drift, dead code, and safety issues from deep scan - (fa99af3) - piperun
- fully decouple archive open from plugin mutex - (939a984) - piperun
#### Performance Improvements
- (**plugins**) dirty-bit short-circuit on get_all_settings (audit P14) - (888873d) - piperun
- (**plugins**) batch get_metadata_summaries — N+1 → 1 SQL query - (2888133) - piperun
- (**plugins**) invalidate top tabs cache on load/unload (P3 follow-up) - (e59a2e7) - piperun
- (**plugins,ui**) cache plugin counts and top tabs (P5, P3) - (610081b) - piperun
- (**ui,plugins**) tokio + per-plugin settings for UI events (P7) - (639e7fc) - piperun
#### Tests
- (**audit**) regression tests for critical findings C1/C5/C6 - (347eab1) - piperun
#### Refactoring
- (**arch**) break data->core cycle via IoC traits - (b91d3f1) - piperun
- (**plugins**) flatten get_product_metadata lazy-repair nesting - (90ccc01) - piperun
- (**plugins**) move wit_rules From impls to conversions.rs - (795efaf) - piperun
- (**plugins**) extract WIT conversions to conversions.rs - (9843201) - piperun
- (**plugins**) collapse PluginManager dispatch APIs to one canonical path - (83a1d2f) - piperun
- (**plugins**) extract enabled_plugin_snapshot helper - (d9b7077) - piperun
- swap PathBuf.to_string_lossy().to_string() for .into_owned() - (2213301) - piperun
- downgrade noisy log levels to debug/trace - (742aab8) - piperun
#### Miscellaneous Chores
- (**ci**) un-gitignore CHANGELOG.md and track current state - (aaa02e3) - piperun
- (**deps**) introduce [workspace.dependencies] for shared deps - (2ff9062) - piperun
- (**deps**) remove unused dependencies - (a02f0f6) - piperun
- (**version**) bump packages - (911ba47) - piperun
- (**version**) bump packages - (1e9fc9b) - piperun
- (**version**) 0.2.0 - (8fc1edb) - piperun

- - -

## arclain_plugins-0.12.0 - 2026-05-07
#### Features
- (**plugins**) WIT get-metadata export — plugins self-report instead of "Unknown" - (210919e) - piperun
- (**plugins**) play_cached_blob — hand cached videos to OS default app - (58873bc) - piperun
- (**plugins**) fetch_to_cache host fn — store blobs without crossing WASM boundary - (cb05b93) - piperun
#### Bug Fixes
- (**audit**) defensive cleanups for medium findings (M1, M2, M6, M7) - (f5df66b) - piperun
- (**audit**) surface previously-swallowed errors (H1, H2, H3, H7) - (be51153) - piperun
- (**plugins**) release plugins.read() before per-instance work (C1) - (4a650e1) - piperun
- (**plugins**) release instance lock during gameta HTTP fetch (C2) - (8ba504e) - piperun
- (**plugins**) add native HTTP fallback when gameta server is absent - (4aab72d) - piperun
#### Performance Improvements
- (**plugins**) dirty-bit short-circuit on get_all_settings (audit P14) - (553eae9) - piperun
- (**plugins**) batch get_metadata_summaries — N+1 → 1 SQL query - (aed4eed) - piperun
- (**plugins**) invalidate top tabs cache on load/unload (P3 follow-up) - (ea71c01) - piperun
- (**plugins,ui**) cache plugin counts and top tabs (P5, P3) - (9671b7b) - piperun
- (**ui,plugins**) tokio + per-plugin settings for UI events (P7) - (e216ac3) - piperun
#### Tests
- (**audit**) regression tests for critical findings C1/C5/C6 - (62bca76) - piperun
#### Refactoring
- (**arch**) break data->core cycle via IoC traits - (aec9783) - piperun
- (**plugins**) flatten get_product_metadata lazy-repair nesting - (8bc2363) - piperun
- (**plugins**) move wit_rules From impls to conversions.rs - (8b53439) - piperun
- (**plugins**) extract WIT conversions to conversions.rs - (17432b4) - piperun
- (**plugins**) collapse PluginManager dispatch APIs to one canonical path - (ea2452a) - piperun
- (**plugins**) extract enabled_plugin_snapshot helper - (63084bf) - piperun
- swap PathBuf.to_string_lossy().to_string() for .into_owned() - (efdf209) - piperun
#### Miscellaneous Chores
- (**ci**) un-gitignore CHANGELOG.md and track current state - (e300c30) - piperun
- (**deps**) introduce [workspace.dependencies] for shared deps - (6b069e8) - piperun
- (**deps**) remove unused dependencies - (c05cc12) - piperun

- - -

## arclain_plugins-0.11.0 - 2026-05-02
#### Features
- (**plugins**) add GroupBegin/GroupEnd markers + theme plugin buttons - (44a644c) - piperun

- - -

## arclain_plugins-0.10.0 - 2026-03-26
#### Features
- (**plugins**) route metadata fetches through gameta server when available - (de29881) - piperun
- decouple metadata fetch from plugin mutex via RequestFetch action - (3c14e70) - piperun
#### Bug Fixes
- resolve field drift, dead code, and safety issues from deep scan - (7648cfa) - piperun
- fully decouple archive open from plugin mutex - (939a984) - piperun
#### Refactoring
- downgrade noisy log levels to debug/trace - (742aab8) - piperun

- - -

## arclain_plugins-0.9.7 - 2026-03-26
#### Bug Fixes
- resolve metadata persistence and image display after gameta port - (e107c7b) - piperun
#### Performance Improvements
- use sort_by_cached_key and consolidate lock acquisition - (db68abc) - piperun
#### Refactoring
- remove dead code across UI, plugins, and signals - (2298cff) - piperun

- - -

Changelog generated by [cocogitto](https://github.com/cocogitto/cocogitto).