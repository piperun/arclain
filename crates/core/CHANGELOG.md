# Changelog
All notable changes to this project will be documented in this file. See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

- - -
## arclain_core-0.12.0 - 2026-05-19
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
- (**core,ui**) warn on missing screenshots, orphan addons, duplicate previews - (13bb37d) - piperun
- (**pipeline**) output filename falls back to detected product code - (0e7c032) - piperun
- (**pipeline**) output naming uses selected metadata title - (b489108) - piperun
- (**ui,core,db**) tab UI, drop overlay, drop_behavior setting - (d3c9eab) - piperun
#### Bug Fixes
- (**audit**) defensive cleanups for medium findings (M1, M2, M6, M7) - (ce3bd44) - piperun
- (**core**) use parking_lot::RwLock for title FILTER_CACHE (M3) - (9926711) - piperun
- (**core**) panic-free unix_seconds helper (H5, H6) - (8651f3b) - piperun
- (**core**) propagate set_config errors in vault operations (C5) - (6b699a5) - piperun
- (**core**) propagate move_dir remove failure (C3) - (7d92365) - piperun
- (**core**) use native unrar for filenames, CLI only for packed_size - (fd7bc38) - piperun
- (**core**) route RAR listing through UnRAR CLI to recover packed_size - (1f3839c) - piperun
- (**core**) unwrap single-root folder after flatten extraction - (467f43e) - piperun
- (**core**) suppress Windows console window on subprocess spawns - (5adacf1) - piperun
- (**core**) flatten_nested_archives walks tree and places folders next to source - (fd299e9) - piperun
- (**flatten**) use modinfo.ini name= as folder name + abort broken strips - (a8ae6f4) - piperun
- resolve field drift, dead code, and safety issues from deep scan - (fa99af3) - piperun
#### Performance Improvements
- (**core**) O(1) DLL dedup; preallocate archive listing capacity - (c9c24c9) - piperun
- (**plugins**) batch get_metadata_summaries — N+1 → 1 SQL query - (2888133) - piperun
#### Tests
- (**audit**) regression tests for critical findings C1/C5/C6 - (347eab1) - piperun
- (**audit**) regression tests for critical findings C2/C3/C4 - (6af0ee6) - piperun
- (**core**) end-to-end idempotency + overwrite verification - (6274876) - piperun
- add integration test for apply_plan_to_workdir - (3371059) - piperun
- add integration tests for pipeline preview and execution scaffold - (4d346b4) - piperun
- add integration tests for archive conversion flatten - (51383f0) - piperun
- add regression tests for gameta port fixes and new features - (292b42e) - piperun
#### Build system
- (**core**) drop native-tls from reqwest, align with network's rustls - (52fedf4) - piperun
#### Refactoring
- (**arch**) break data->core cycle via IoC traits - (b91d3f1) - piperun
- (**core**) split engine.rs into engine/{mod,plan_builder,tree}.rs - (57eff70) - piperun
- (**core**) split RuleEngine::create_plan into focused helpers - (aed9d7c) - piperun
- (**core**) introduce PipelineContext for executor service access - (0ce551c) - piperun
- (**db**) split lib.rs bootstrap, extract cache types, rename archive_profiles - (b9431bd) - piperun
- split fat files per audit recommendations - (dc983f0) - piperun
- move cache_index + flatten tests to sibling files - (2793848) - piperun
- swap PathBuf.to_string_lossy().to_string() for .into_owned() - (2213301) - piperun
- route UI's db access through arclain_core re-exports - (a7de1c5) - piperun
#### Miscellaneous Chores
- (**ci**) un-gitignore CHANGELOG.md and track current state - (aaa02e3) - piperun
- (**core**) add flatten_demo example for spot-checking real data - (82a52e1) - piperun
- (**deps**) introduce [workspace.dependencies] for shared deps - (2ff9062) - piperun
- (**deps**) remove unused dependencies - (a02f0f6) - piperun
- (**version**) realign core + db Cargo.toml with latest tags - (a0b9a7e) - piperun
- (**version**) arclain_core-0.9.0 - (0ed5c16) - piperun
- (**version**) bump packages - (911ba47) - piperun
- (**version**) bump packages - (c3b3d1b) - piperun
- (**version**) bump packages - (4805007) - piperun
- (**version**) 0.2.0 - (8fc1edb) - piperun

- - -

## arclain_core-0.9.0 - 2026-05-18
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
- (**core,ui**) warn on missing screenshots, orphan addons, duplicate previews - (13bb37d) - piperun
- (**pipeline**) output filename falls back to detected product code - (0e7c032) - piperun
- (**pipeline**) output naming uses selected metadata title - (b489108) - piperun
- (**ui,core,db**) tab UI, drop overlay, drop_behavior setting - (d3c9eab) - piperun
#### Bug Fixes
- (**audit**) defensive cleanups for medium findings (M1, M2, M6, M7) - (ce3bd44) - piperun
- (**core**) use parking_lot::RwLock for title FILTER_CACHE (M3) - (9926711) - piperun
- (**core**) panic-free unix_seconds helper (H5, H6) - (8651f3b) - piperun
- (**core**) propagate set_config errors in vault operations (C5) - (6b699a5) - piperun
- (**core**) propagate move_dir remove failure (C3) - (7d92365) - piperun
- (**core**) use native unrar for filenames, CLI only for packed_size - (fd7bc38) - piperun
- (**core**) route RAR listing through UnRAR CLI to recover packed_size - (1f3839c) - piperun
- (**core**) unwrap single-root folder after flatten extraction - (467f43e) - piperun
- (**core**) suppress Windows console window on subprocess spawns - (5adacf1) - piperun
- (**core**) flatten_nested_archives walks tree and places folders next to source - (fd299e9) - piperun
- (**flatten**) use modinfo.ini name= as folder name + abort broken strips - (a8ae6f4) - piperun
- resolve field drift, dead code, and safety issues from deep scan - (fa99af3) - piperun
#### Performance Improvements
- (**core**) O(1) DLL dedup; preallocate archive listing capacity - (c9c24c9) - piperun
- (**plugins**) batch get_metadata_summaries — N+1 → 1 SQL query - (2888133) - piperun
#### Tests
- (**audit**) regression tests for critical findings C1/C5/C6 - (347eab1) - piperun
- (**audit**) regression tests for critical findings C2/C3/C4 - (6af0ee6) - piperun
- (**core**) end-to-end idempotency + overwrite verification - (6274876) - piperun
- add integration test for apply_plan_to_workdir - (3371059) - piperun
- add integration tests for pipeline preview and execution scaffold - (4d346b4) - piperun
- add integration tests for archive conversion flatten - (51383f0) - piperun
- add regression tests for gameta port fixes and new features - (292b42e) - piperun
#### Build system
- (**core**) drop native-tls from reqwest, align with network's rustls - (52fedf4) - piperun
#### Refactoring
- (**arch**) break data->core cycle via IoC traits - (b91d3f1) - piperun
- (**core**) split engine.rs into engine/{mod,plan_builder,tree}.rs - (57eff70) - piperun
- (**core**) split RuleEngine::create_plan into focused helpers - (aed9d7c) - piperun
- (**core**) introduce PipelineContext for executor service access - (0ce551c) - piperun
- (**db**) split lib.rs bootstrap, extract cache types, rename archive_profiles - (b9431bd) - piperun
- split fat files per audit recommendations - (dc983f0) - piperun
- move cache_index + flatten tests to sibling files - (2793848) - piperun
- swap PathBuf.to_string_lossy().to_string() for .into_owned() - (2213301) - piperun
- route UI's db access through arclain_core re-exports - (a7de1c5) - piperun
#### Miscellaneous Chores
- (**ci**) un-gitignore CHANGELOG.md and track current state - (aaa02e3) - piperun
- (**core**) add flatten_demo example for spot-checking real data - (82a52e1) - piperun
- (**deps**) introduce [workspace.dependencies] for shared deps - (2ff9062) - piperun
- (**deps**) remove unused dependencies - (a02f0f6) - piperun
- (**version**) bump packages - (911ba47) - piperun
- (**version**) bump packages - (c3b3d1b) - piperun
- (**version**) bump packages - (4805007) - piperun
- (**version**) 0.2.0 - (8fc1edb) - piperun

- - -

## arclain_core-0.11.1 - 2026-05-07
#### Bug Fixes
- (**audit**) defensive cleanups for medium findings (M1, M2, M6, M7) - (f5df66b) - piperun
- (**core**) use parking_lot::RwLock for title FILTER_CACHE (M3) - (480df2c) - piperun
- (**core**) panic-free unix_seconds helper (H5, H6) - (638e0dc) - piperun
- (**core**) propagate set_config errors in vault operations (C5) - (8b8dad3) - piperun
- (**core**) propagate move_dir remove failure (C3) - (48164a6) - piperun
- (**core**) use native unrar for filenames, CLI only for packed_size - (2514892) - piperun
- (**flatten**) use modinfo.ini name= as folder name + abort broken strips - (92131d5) - piperun
#### Performance Improvements
- (**core**) O(1) DLL dedup; preallocate archive listing capacity - (4d693c7) - piperun
- (**plugins**) batch get_metadata_summaries — N+1 → 1 SQL query - (aed4eed) - piperun
#### Tests
- (**audit**) regression tests for critical findings C1/C5/C6 - (62bca76) - piperun
- (**audit**) regression tests for critical findings C2/C3/C4 - (e9d0e67) - piperun
#### Build system
- (**core**) drop native-tls from reqwest, align with network's rustls - (2973c04) - piperun
#### Refactoring
- (**arch**) break data->core cycle via IoC traits - (aec9783) - piperun
- (**core**) split engine.rs into engine/{mod,plan_builder,tree}.rs - (fa45ce5) - piperun
- (**core**) split RuleEngine::create_plan into focused helpers - (0cd6641) - piperun
- (**db**) split lib.rs bootstrap, extract cache types, rename archive_profiles - (c3f11e5) - piperun
- split fat files per audit recommendations - (3631f76) - piperun
- move cache_index + flatten tests to sibling files - (d34b00a) - piperun
- swap PathBuf.to_string_lossy().to_string() for .into_owned() - (efdf209) - piperun
- route UI's db access through arclain_core re-exports - (5665518) - piperun
#### Miscellaneous Chores
- (**ci**) un-gitignore CHANGELOG.md and track current state - (e300c30) - piperun
- (**core**) add flatten_demo example for spot-checking real data - (4460a81) - piperun
- (**deps**) introduce [workspace.dependencies] for shared deps - (6b069e8) - piperun
- (**deps**) remove unused dependencies - (c05cc12) - piperun

- - -

## arclain_core-0.11.0 - 2026-04-18
#### Features
- (**core**) add OutputArtifact::Folder for no-pack pipeline output - (258e85b) - piperun
- (**core**) stream hash progress as StepStart/StepProgress events - (836b5c7) - piperun
- (**core**) thread app-default collision policy through executor - (ed8b8f2) - piperun
- (**core**) wire executor to pipeline_runs DB for dedup + audit - (cee2bf2) - piperun
- (**core**) add OutputCollisionPolicy gate to pipeline executor - (f9ea241) - piperun
- (**core**) recursive flatten with max_depth and safety caps - (3e911e6) - piperun
#### Bug Fixes
- (**core**) route RAR listing through UnRAR CLI to recover packed_size - (6b077cd) - piperun
#### Tests
- (**core**) end-to-end idempotency + overwrite verification - (9d9e140) - piperun

- - -

## arclain_core-0.10.0 - 2026-04-18
#### Features
- (**core**) add pipeline preset save/load with builtin presets - (a4d26c8) - piperun
- (**core**) implement Organize pipeline step using RuleEngine - (42fbe95) - piperun
- (**core**) apply organization plan moves to extracted work dir - (27dd22f) - piperun
- (**core**) add blocking pipeline executor with progress events - (c2fb5f0) - piperun
- (**core**) add pure pipeline preview builder - (b3fc3a3) - piperun
- (**core**) add Pipeline types for archive processing workflows - (e4cf8ce) - piperun
- (**core**) extend convert backend to accept format + compression level - (023bdf7) - piperun
- (**core**) add flatten_nested_archives for unnesting mod archives - (34d2a27) - piperun
- (**core**) add ConvertOptions types and longest_common_prefix helper - (1d86a32) - piperun
#### Bug Fixes
- (**core**) unwrap single-root folder after flatten extraction - (c68c90a) - piperun
- (**core**) suppress Windows console window on subprocess spawns - (8e81827) - piperun
- (**core**) flatten_nested_archives walks tree and places folders next to source - (6cee0c7) - piperun
#### Tests
- add integration test for apply_plan_to_workdir - (eda02c2) - piperun
- add integration tests for pipeline preview and execution scaffold - (271aa58) - piperun
- add integration tests for archive conversion flatten - (07b539f) - piperun
#### Refactoring
- (**core**) introduce PipelineContext for executor service access - (2b69b7f) - piperun

- - -

## arclain_core-0.9.0 - 2026-03-26
#### Features
- (**core**) wire GametaClient initialization from UserConfig - (a562274) - piperun
#### Bug Fixes
- resolve field drift, dead code, and safety issues from deep scan - (7648cfa) - piperun
#### Tests
- add regression tests for gameta port fixes and new features - (292b42e) - piperun

- - -

## arclain_core-0.8.9 - 2026-03-26
#### Bug Fixes
- resolve metadata persistence and image display after gameta port - (e107c7b) - piperun

- - -

## arclain_core-0.8.8 - 2026-03-26
#### Bug Fixes
- align dlsite plugin and organization engine with gameta API - (397a529) - piperun
#### Tests
- improve test infrastructure and parallel safety - (b141785) - piperun
#### Refactoring
- extract constants and helpers in 7z/unrar CLI backends - (bd81109) - piperun

- - -

Changelog generated by [cocogitto](https://github.com/cocogitto/cocogitto).