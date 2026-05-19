# Changelog
All notable changes to this project will be documented in this file. See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

- - -
## arclain_db-0.9.0 - 2026-05-19
#### Features
- (**db**) add pipeline_runs table + repo for idempotent pipeline dedup - (b9bdde4) - piperun
- (**db**) add gameta server config fields to UserConfig - (8842e0a) - piperun
- (**ui**) add batch convert toolbar action and folder scanning - (3b571cb) - piperun
- (**ui,core,db**) tab UI, drop overlay, drop_behavior setting - (d3c9eab) - piperun
- (**ui,db**) tab persistence, cancel tokens, and TabPluginPool scaffold - (25280b0) - piperun
#### Bug Fixes
- (**db**) abort cache recovery when WAL or SHM removal fails (C6) - (01b1577) - piperun
- (**db**) abort migration when backup fails (C4) - (747c436) - piperun
#### Tests
- (**audit**) regression tests for critical findings C1/C5/C6 - (347eab1) - piperun
- (**audit**) regression tests for critical findings C2/C3/C4 - (6af0ee6) - piperun
- add regression tests for deep scan fixes and fix completeness_score - (41dd9dd) - piperun
- add regression tests for metadata field naming bugs - (13785e9) - piperun
- add regression tests for gameta port fixes and new features - (292b42e) - piperun
#### Refactoring
- (**db**) split lib.rs bootstrap, extract cache types, rename archive_profiles - (b9431bd) - piperun
- (**db**) split ui/config.rs into 4 cohesive sub-modules - (e5f33fa) - piperun
- (**db**) drop legacy module, keep only used cache_index helpers - (d43eda6) - piperun
- (**db**) extract diesel_err helper, kill 49x boilerplate - (2d7360e) - piperun
- nuke mini-orm crate, fold SqliteDb into arclain_db - (07360f0) - piperun
- split fat files per audit recommendations - (dc983f0) - piperun
- move cache_index + flatten tests to sibling files - (2793848) - piperun
- swap PathBuf.to_string_lossy().to_string() for .into_owned() - (2213301) - piperun
#### Miscellaneous Chores
- (**ci**) un-gitignore CHANGELOG.md and track current state - (aaa02e3) - piperun
- (**deps**) introduce [workspace.dependencies] for shared deps - (2ff9062) - piperun
- (**deps**) remove unused dependencies - (a02f0f6) - piperun
- (**version**) realign core + db Cargo.toml with latest tags - (a0b9a7e) - piperun
- (**version**) arclain_db-0.6.0 - (63e203b) - piperun
- (**version**) bump packages - (911ba47) - piperun
- (**version**) bump packages - (c3b3d1b) - piperun
- (**version**) bump packages - (4805007) - piperun
- (**version**) 0.2.0 - (8fc1edb) - piperun

- - -

## arclain_db-0.6.0 - 2026-05-19
#### Features
- (**db**) add pipeline_runs table + repo for idempotent pipeline dedup - (b9bdde4) - piperun
- (**db**) add gameta server config fields to UserConfig - (8842e0a) - piperun
- (**ui**) add batch convert toolbar action and folder scanning - (3b571cb) - piperun
- (**ui,core,db**) tab UI, drop overlay, drop_behavior setting - (d3c9eab) - piperun
- (**ui,db**) tab persistence, cancel tokens, and TabPluginPool scaffold - (25280b0) - piperun
#### Bug Fixes
- (**db**) abort cache recovery when WAL or SHM removal fails (C6) - (01b1577) - piperun
- (**db**) abort migration when backup fails (C4) - (747c436) - piperun
#### Tests
- (**audit**) regression tests for critical findings C1/C5/C6 - (347eab1) - piperun
- (**audit**) regression tests for critical findings C2/C3/C4 - (6af0ee6) - piperun
- add regression tests for deep scan fixes and fix completeness_score - (41dd9dd) - piperun
- add regression tests for metadata field naming bugs - (13785e9) - piperun
- add regression tests for gameta port fixes and new features - (292b42e) - piperun
#### Refactoring
- (**db**) split lib.rs bootstrap, extract cache types, rename archive_profiles - (b9431bd) - piperun
- (**db**) split ui/config.rs into 4 cohesive sub-modules - (e5f33fa) - piperun
- (**db**) drop legacy module, keep only used cache_index helpers - (d43eda6) - piperun
- (**db**) extract diesel_err helper, kill 49x boilerplate - (2d7360e) - piperun
- nuke mini-orm crate, fold SqliteDb into arclain_db - (07360f0) - piperun
- split fat files per audit recommendations - (dc983f0) - piperun
- move cache_index + flatten tests to sibling files - (2793848) - piperun
- swap PathBuf.to_string_lossy().to_string() for .into_owned() - (2213301) - piperun
#### Miscellaneous Chores
- (**ci**) un-gitignore CHANGELOG.md and track current state - (aaa02e3) - piperun
- (**deps**) introduce [workspace.dependencies] for shared deps - (2ff9062) - piperun
- (**deps**) remove unused dependencies - (a02f0f6) - piperun
- (**version**) bump packages - (911ba47) - piperun
- (**version**) bump packages - (c3b3d1b) - piperun
- (**version**) bump packages - (4805007) - piperun
- (**version**) 0.2.0 - (8fc1edb) - piperun

- - -

## arclain_db-0.8.1 - 2026-05-07
#### Bug Fixes
- (**db**) abort cache recovery when WAL or SHM removal fails (C6) - (89e0056) - piperun
- (**db**) abort migration when backup fails (C4) - (b5e7828) - piperun
#### Tests
- (**audit**) regression tests for critical findings C1/C5/C6 - (62bca76) - piperun
- (**audit**) regression tests for critical findings C2/C3/C4 - (e9d0e67) - piperun
#### Refactoring
- (**db**) split lib.rs bootstrap, extract cache types, rename archive_profiles - (c3f11e5) - piperun
- (**db**) split ui/config.rs into 4 cohesive sub-modules - (52269c7) - piperun
- (**db**) drop legacy module, keep only used cache_index helpers - (7f0f7a9) - piperun
- (**db**) extract diesel_err helper, kill 49x boilerplate - (c73383d) - piperun
- nuke mini-orm crate, fold SqliteDb into arclain_db - (8eeb38b) - piperun
- split fat files per audit recommendations - (3631f76) - piperun
- move cache_index + flatten tests to sibling files - (d34b00a) - piperun
- swap PathBuf.to_string_lossy().to_string() for .into_owned() - (efdf209) - piperun
#### Miscellaneous Chores
- (**ci**) un-gitignore CHANGELOG.md and track current state - (e300c30) - piperun
- (**deps**) introduce [workspace.dependencies] for shared deps - (6b069e8) - piperun
- (**deps**) remove unused dependencies - (c05cc12) - piperun

- - -

## arclain_db-0.8.0 - 2026-04-18
#### Features
- (**db**) add pipeline_runs table + repo for idempotent pipeline dedup - (aa13994) - piperun

- - -

## arclain_db-0.7.0 - 2026-04-17
#### Features
- (**ui**) add batch convert toolbar action and folder scanning - (6091966) - piperun

- - -

## arclain_db-0.6.0 - 2026-03-26
#### Features
- (**db**) add gameta server config fields to UserConfig - (8842e0a) - piperun
#### Tests
- add regression tests for deep scan fixes and fix completeness_score - (7404a15) - piperun
- add regression tests for metadata field naming bugs - (4738142) - piperun
- add regression tests for gameta port fixes and new features - (292b42e) - piperun

- - -

## arclain_db-0.5.5 - 2026-03-26
#### Bug Fixes
- resolve metadata persistence and image display after gameta port - (e107c7b) - piperun
#### Tests
- improve test infrastructure and parallel safety - (b141785) - piperun
#### Miscellaneous Chores
- clean up stale artifacts and modernize cog.toml - (c150815) - piperun

- - -

Changelog generated by [cocogitto](https://github.com/cocogitto/cocogitto).