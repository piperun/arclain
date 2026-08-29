# Changelog
All notable changes to this project will be documented in this file. See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

- - -
## 3.0.2 - 2026-08-29
### Packages
- dlsite-metadata locked to dlsite-metadata-0.12.0
### Global changes
#### Bug Fixes
- (**core**) back up a read-only directory instead of failing on it - (e9e8610) - 0xdev
#### Continuous Integration
- report every test failure in a run, not just the first - (b9b939f) - 0xdev
- install 7-Zip so the extraction tests can run - (48d56c2) - 0xdev

- - -

## 3.0.1 - 2026-08-29
### Packages
- dlsite-metadata locked to dlsite-metadata-0.12.0
### Global changes
#### Bug Fixes
- (**core**) find a split archive's parts whatever their case - (8ccd0bc) - 0xdev
#### Build system
- depend on gameta 0.6.0 - (8f1131c) - 0xdev

- - -

## 3.0.0 - 2026-08-28
### Package updates
- dlsite-metadata bumped to dlsite-metadata-0.12.0
### Global changes
#### Features
- (**app**) make plugin hosting optional - (11f4c39) - 0xdev
- (**app**) default-on gameta feature completes the optional metadata stack - (ee63afe) - 0xdev
- (**app**) add the remaining plugin-management facade surfaces - (664083d) - 0xdev
- (**app**) read one entry's text through the session that owns it - (7ad7123) - 0xdev
- (**app**) expose product metadata as a facade read model - (b324e8b) - 0xdev
- (**app**) own display-image fetching and the host image namespace - (4a522f0) - 0xdev
- (**app**) make a preview and its run fail to compile when they drift - (f95bded) - 0xdev
- (**app**) add the Process page's preset, preview and run-history surface - (b93c2b5) - 0xdev
- (**app**) hand the legacy egui composition the session's archive handle - (c39b47d) - 0xdev
- (**app**) own the encrypted-CRC backfill behind the facade - (f48455f) - 0xdev
- (**app**) report archive-level encryption on the session snapshot - (8e5648f) - 0xdev
- (**app**) list an archive session's whole entry tree - (a8769db) - 0xdev
- (**app**) add the drag-stage surface for OS drag-out sources - (596481a) - 0xdev
- (**app**) merge a split archive through the application facade - (9e92a65) - 0xdev
- (**app**) serve the chrome layout from the application - (65e984f) - 0xdev
- (**app**) report which organization rules apply to an open archive - (172301c) - 0xdev
- (**app**) enumerate the output formats a profile may name - (3400331) - 0xdev
- (**app**) apply the organize plan that was previewed - (1b94365) - 0xdev
- (**app**) report an open archive's whole file-path list - (677b32b) - 0xdev
- (**app**) report the computed default vault locations - (484d274) - 0xdev
- (**app**) broaden narrow auto-saved password rules at bootstrap - (95a4c6c) - 0xdev
- (**app**) resolve the per-plugin proxy routing map on the network DTO - (99ff6f1) - 0xdev
- (**app**) carry the pipeline collision default on the settings surface - (40cc12b) - 0xdev
- (**app**) report the whole network probe, direct path included - (d455c7e) - 0xdev
- (**app**) probe candidate SOCKS5 proxies, and report what gameta answered - (e8cc906) - 0xdev
- (**app**) serve domain access and gameta health checks from the facade - (0105907) - 0xdev
- (**app**) add the organization rule, profile, and preview surface - (7a3f03f) - 0xdev
- (**app**) expose log locations and archive-extension checks - (60cbcd5) - 0xdev
- (**app**) resolve interactive RequestFetch through the shared policy - (f23c2e0) - 0xdev
- (**app**) publish session events from ArchiveContextBridge - (b580e3a) - 0xdev
- (**app**) add a session-event broadcast channel - (8bf76a5) - 0xdev
- (**app**) express ad-hoc pipelines in the application contract - (4ff1beb) - 0xdev
- (**app,ui**) carry every output of a layout to the surfaces - (88d3f76) - 0xdev
- (**core**) seed the mod-manager layout beside the product one - (9de5d06) - 0xdev
- (**core**) make a rule's layout data rather than a boolean - (a283eef) - 0xdev
- (**core**) fill each output from its layout's placements - (33f8205) - 0xdev
- (**core**) resolve a layout to named outputs - (d4077c9) - 0xdev
- (**core**) add the layout vocabulary - (a5d2363) - 0xdev
- (**core**) resolve a plan's downloads before it is applied - (c9a49f8) - 0xdev
- (**core**) gate LibraryService, dlsite detection and pipeline metadata behind default-on gameta - (4fabad2) - 0xdev
- (**core**) recover interrupted proxy settings saves - (054922d) - 0xdev
- (**data**) name why the cache refused to store something - (1f7a5fa) - 0xdev
- (**data**) stop requesting arclain_db's default features through the edge - (daffc69) - 0xdev
- (**data**) gate MetadataReader and the metadata resolver behind default-on gameta - (5a716c9) - 0xdev
- (**db**) make the gameta type re-export an optional default-on feature - (5fc8dad) - 0xdev
- (**db**) add atomic secret mutation batches - (d39f8f8) - 0xdev
- (**network**) add a bounded buffered GET that keeps the response type - (b7f842b) - 0xdev
- (**plugins**) gate the metadata host-function engine behind default-on gameta - (496911d) - 0xdev
- (**ui**) give a tab a facade-typed whole-archive inventory - (9101c03) - 0xdev
- (**ui**) route drag-out through the facade drag-stage surface - (2068947) - 0xdev
- (**ui**) tell an empty archive folder apart from one that failed to list - (9f19a1e) - 0xdev
- (**ui**) render facade plugin documents as a real tree - (e9f0119) - 0xdev
- (**ui**) add a facade-backed plugin session registry - (779a9e7) - 0xdev
- (**ui**) trace session-event metadata delivery - (fb3b226) - 0xdev
- (**ui**) consume session events alongside operation events - (fc53a92) - 0xdev
- (**ui**) report the active tab's archive session to the facade - (975dfb3) - 0xdev
- <span style="background-color: #d73a49; color: white; padding: 2px 6px; border-radius: 3px; font-weight: bold; font-size: 0.85em;">BREAKING</span>(**wirt**) a badge names what it means, not what colour to be - (f3f56ab) - 0xdev
- <span style="background-color: #d73a49; color: white; padding: 2px 6px; border-radius: 3px; font-weight: bold; font-size: 0.85em;">BREAKING</span>(**wirt**) a split names how wide its sidebar should be - (78a95a1) - 0xdev
- <span style="background-color: #d73a49; color: white; padding: 2px 6px; border-radius: 3px; font-weight: bold; font-size: 0.85em;">BREAKING</span>(**wirt**) media elements name a size step, not a pixel height - (6e40031) - 0xdev
- <span style="background-color: #d73a49; color: white; padding: 2px 6px; border-radius: 3px; font-weight: bold; font-size: 0.85em;">BREAKING</span>(**wirt**) a label names its role, not its point size - (216dd6d) - 0xdev
- <span style="background-color: #d73a49; color: white; padding: 2px 6px; border-radius: 3px; font-weight: bold; font-size: 0.85em;">BREAKING</span>(**wirt**) spacing is a step the host sizes, not a pixel count - (db7b2c2) - 0xdev
- expose legacy network settings safely - (7857912) - 0xdev
- accept host-owned plugin routing - (1c380cb) - 0xdev
- add transactional plugin uninstall - (43459f5) - 0xdev
- add bounded persistent plugin data - (61bcc3b) - 0xdev
- add revisioned plugin settings snapshots - (2543116) - 0xdev
- package bundled plugins as Wirt archives - (cb11990) - 0xdev
- quarantine resource-abusing Wirt plugins - (202fbbd) - 0xdev
- review Wirt permissions before installation - (a76b3e2) - 0xdev
- install Wirt packages transactionally - (febf37a) - 0xdev
- add the Wirt developer command - (776285e) - 0xdev
- add deterministic Wirt package validation - (910d2ff) - 0xdev
- establish neutral Wirt crate boundary - (9cbc9b8) - 0xdev
- add atomic password-rule replacement - (c6c6f57) - 0xdev
- add operational CLI commands - (2802446) - 0xdev
- add inspect and list CLI commands - (ee9e19d) - 0xdev
- add cancellable application operation registry - (80652a0) - 0xdev
- add stable application facade value types - (a7e957e) - 0xdev
- add session-aware diagnostics logs - (17d6e66) - 0xdev
#### Bug Fixes
- (**app**) treat a full disk as the user's to clear, not a retry - (ce26fb1) - 0xdev
- (**app**) refuse a batch organize input before extracting it - (cc2e5fd) - 0xdev
- (**app**) bootstrap without 7-Zip instead of refusing to start - (d50369d) - 0xdev
- (**app**) take a plugin's settings at exit, not only at the next guest entry - (7f0b078) - 0xdev
- (**app**) close the windows where a disabled plugin still ran - (ee7ca1d) - 0xdev
- (**app**) persist the settings a plugin writes about itself - (3d5ca04) - 0xdev
- (**app**) refuse to run a plugin the user disabled - (c5b9cc4) - 0xdev
- (**app**) name the file a failed plugin install was pointed at - (6c601cc) - 0xdev
- (**app**) stop preset writes re-persisting stored convert passwords - (9acd24f) - 0xdev
- (**app**) clamp plugin-declared text before it reaches ui_items - (00d7dbc) - 0xdev
- (**app**) root the content cache at the bootstrap's own cache_dir - (33480d9) - 0xdev
- (**app**) make the display-options save atomic - (8afef29) - 0xdev
- (**app**) stop the launch sync from resetting top-tab customization - (a1fc6cd) - 0xdev
- (**app**) close the storage-scoped key door into plugin cache rows - (ff6813b) - 0xdev
- (**app**) keep host images at the ceiling they already had - (63ae2a8) - 0xdev
- (**app**) stop an empty pipeline batch carrying its own copy of the summary - (9d1680d) - 0xdev
- (**app**) make a pipeline preview describe the run it predicts - (67d8df6) - 0xdev
- (**app**) never lose a split archive's parts to a late cancellation - (9df6e66) - 0xdev
- (**app**) make ArchiveContextBridge's metadata and rename writes real - (f101a1f) - 0xdev
- (**app**) fix materialization lease-store bugs found in review - (7fd278d) - 0xdev
- (**app**) dispatch archive-open plugin events only once completion is confirmed - (3b6f5f1) - 0xdev
- (**app**) stop empty extraction selections widening scope - (7558261) - 0xdev
- (**app**) add missing extract_runner_override after rebase - (2fd9ac2) - 0xdev
- (**app,ui,core**) refuse to run a plan that would write nothing - (420a5a9) - 0xdev
- (**checksum**) validate Merkle UNC and root payloads - (21852d9) - 0xdev
- (**checksum**) harden Merkle path and decoding boundaries - (0bd8be4) - 0xdev
- (**checksum**) bind Merkle leaves to file identity - (93d2714) - 0xdev
- (**checksum**) propagate traversal and hash errors - (280f093) - 0xdev
- (**ci**) pin the gameta revision Cargo.lock was resolved against - (5bde869) - 0xdev
- (**ci**) install rustfmt before the format check - (484c51e) - 0xdev
- (**ci**) align Arclain with Gameta's Rust 1.98 floor - (52df847) - 0xdev
- (**cli**) scope a test-only import to the tests that use it - (8b6084b) - 0xdev
- (**cli,ui**) stop pushing metadata into a panel that is not organizing it - (16e9c82) - 0xdev
- (**core**) report why a pipeline file failed, not only where - (7495d70) - 0xdev
- (**core**) scope an output by its root spelled exactly - (df177bd) - 0xdev
- (**core**) report an output that would carry no file - (eff3653) - 0xdev
- (**core**) refuse to apply a plan that staged nothing - (54ccbfd) - 0xdev
- (**core**) refuse a marker root nested inside another - (592a9e8) - 0xdev
- (**core**) refuse a wrapperless output beside a sibling - (9067114) - 0xdev
- (**core**) reproduce shipped screenshot names, refuse collisions - (1809537) - 0xdev
- (**core**) resolve a tied content-root score the same way every run - (b89c0dd) - 0xdev
- (**core**) keep a screenshot fetch on the public internet - (e374172) - 0xdev
- (**core**) score the folder names the content check was written for - (ddc1282) - 0xdev
- (**core**) write the layered metadata document, not the struct - (49824f0) - 0xdev
- (**core**) follow a bounded redirect chain when fetching a screenshot - (c3b98d3) - 0xdev
- (**core**) bound the screenshot fetch and cache what it downloads - (eb15b73) - 0xdev
- (**core**) fetch the screenshots a pipeline's organize step schedules - (8f0b6b0) - 0xdev
- (**core**) randomize download staging directory and tolerate write failures - (a2734ef) - 0xdev
- (**core**) parse the screenshot URLs every product actually carries - (b556af9) - 0xdev
- (**core**) flag a 7z entry encrypted only when its block is AES-coded - (85ab9cb) - 0xdev
- (**core**) store every entry time as the UTC instant it denotes - (5fc247e) - 0xdev
- (**core**) name the password when no 7-Zip CLI is installed - (65d33cc) - 0xdev
- (**core**) normalize the times the CLI tiers read back from their tools - (ee96d8d) - 0xdev
- (**core**) describe RAR entry times per format, not as one encoding - (6243c82) - 0xdev
- (**core**) fail extraction that writes no file, naming the password - (81217f3) - 0xdev
- (**core**) report the instant a 7z header records - (7a15b7c) - 0xdev
- (**core**) report the modification time a RAR header records - (8009762) - 0xdev
- (**core**) log 7-Zip absence at debug in the detector - (c3e0033) - 0xdev
- (**core**) stop requiring 7-Zip to select a native archive backend - (9680f93) - 0xdev
- (**core**) parse product metadata with its plugin json shape - (d6666e3) - 0xdev
- (**core**) proxy bundled dlsite plugin by default - (a183d6f) - 0xdev
- (**core**) fail closed when proxy transport is unavailable - (19930b7) - 0xdev
- (**core**) serialize aliased pipeline destinations - (51ce0a0) - 0xdev
- (**core**) serialize pipeline output promotion - (f073d5c) - 0xdev
- (**core**) atomically replace pipeline outputs - (ff05412) - 0xdev
- (**core**) contain and report organization workspaces - (ad7bc6b) - 0xdev
- (**core**) own unique organization workspaces - (d38da32) - 0xdev
- (**core**) finalize organization metadata transactionally - (8ce9291) - 0xdev
- (**core**) preserve organization staging metadata and cleanup - (c2fdec6) - 0xdev
- (**core**) make organization application transactional - (4ffac78) - 0xdev
- (**core**) preserve organization sources on staging failure - (9eaeb08) - 0xdev
- (**core**) harden organization plan application - (1976929) - 0xdev
- (**core**) preserve native 7z extraction semantics - (e096ade) - 0xdev
- (**core**) contain native 7z extraction paths - (a41562f) - 0xdev
- (**core**) harden checked path containment - (4c03eb5) - 0xdev
- (**core**) add checked relative path boundary - (a28941b) - 0xdev
- (**core,app**) keep a blank metadata title out of the output name - (1b81da9) - 0xdev
- (**core,app**) prove a derived output name is a plain file name - (9bea5f7) - 0xdev
- (**data**) bound legacy data request retention - (53a9f8e) - 0xdev
- (**data**) remove completed download locks race-free - (641e5ee) - 0xdev
- (**data**) revoke resume metadata before append - (7db7ff3) - 0xdev
- (**data**) serialize and bind resumable downloads - (113da94) - 0xdev
- (**data**) snapshot resolver registry entries - (038c1ef) - 0xdev
- (**db**) escape trigger identifiers when dropping stale sync triggers - (9ca95a5) - 0xdev
- (**db**) repair stale cr-sqlite triggers where the shared database is opened - (bc565de) - 0xdev
- (**db**) propagate metadata store initialization failures - (3239306) - 0xdev
- (**db**) reject migration identity collisions - (7568bc3) - 0xdev
- (**db**) make library migration transactional - (a86ce17) - 0xdev
- (**network**) bound buffered plugin response bodies - (19286c4) - 0xdev
- (**network**) centralize proxy authority validation - (83e1587) - 0xdev
- (**network**) reject proxy address userinfo - (aa2ae42) - 0xdev
- (**network**) redact proxy credentials - (188ea3d) - 0xdev
- (**network**) scope DLsite policy to canonical hosts - (2093629) - 0xdev
- (**network**) authorize exact plugin hostnames - (e285df9) - 0xdev
- (**network**) close plugin HTTP authority bypasses - (50e2957) - 0xdev
- (**network**) centralize plugin HTTP authorization - (3a444a1) - 0xdev
- (**network**) bound partial response writes - (0744012) - 0xdev
- (**network**) validate resumable response ranges - (789c1c2) - 0xdev
- (**network**) store request completion state - (9b7febb) - 0xdev
- (**plugins**) ask for the spacing boldness used to imply - (d3e34f6) - 0xdev
- (**plugins**) fail closed on quarantine restore errors - (3fe695e) - 0xdev
- (**plugins**) enforce Wirt adapter shape at compile time - (a73584b) - 0xdev
- (**plugins**) scale the plugin epoch deadline to a liveness backstop - (7763514) - 0xdev
- (**plugins**) align bundled manifest versions - (8304582) - 0xdev
- (**plugins**) route all plugin fetches through checked HTTP - (8933c90) - 0xdev
- (**plugins**) bound top tab rendering - (05f4c3d) - 0xdev
- (**plugins**) close WebAssembly quota bypasses - (84dd872) - 0xdev
- (**plugins**) bound WebAssembly resource consumption - (98f109c) - 0xdev
- (**plugins**) restrict metadata validation host imports - (3c78c7d) - 0xdev
- (**plugins**) defer logging until plugin ID validation - (ae317b9) - 0xdev
- (**plugins**) validate plugin IDs before filesystem use - (f186a77) - 0xdev
- (**signals**) release signal guard before callbacks - (c430e9d) - 0xdev
- (**theme**) always register bundled icon font - (0284b5a) - 0xdev
- (**ui**) stop two more narrow-pane subtractions going negative - (5d738d2) - 0xdev
- (**ui**) keep a short window from asking egui for a negative height - (b3cb684) - 0xdev
- (**ui**) suffix the float literals rustc will stop accepting - (34b0961) - 0xdev
- (**ui**) a sparse sidebar keeps the width the host assigned - (acc760e) - 0xdev
- (**ui**) show entry times on the viewer's own clock - (395bd4d) - 0xdev
- (**ui**) release a plugin's main-page session when it is deselected - (2890e32) - 0xdev
- (**ui**) tell an archive nobody has listed from an empty one - (8c67fd8) - 0xdev
- (**ui**) draw a failed listing instead of the previous archive's rows - (12ccbd2) - 0xdev
- (**ui**) keep a Process page click ahead of the frame's own work - (cb3a1ba) - 0xdev
- (**ui**) surface a rejected preview instead of blanking the panel - (1a79d41) - 0xdev
- (**ui**) resolve an entry by kind as well as name - (87f9168) - 0xdev
- (**ui**) stop failed organization loads from re-firing every frame - (39bb8db) - 0xdev
- (**ui**) stop re-fetching an image the application refused permanently - (3fe2ecc) - 0xdev
- (**ui**) guard fail against foreign sessions and drop the reader-less archive handle - (b3bb948) - 0xdev
- (**ui**) drop a superseded listing request's late reply instead of applying it - (44cd777) - 0xdev
- (**ui**) stop dropping selections past 100 000 entries in one directory - (68f720b) - 0xdev
- (**ui**) never act on placeholder settings at startup - (18f5fa3) - 0xdev
- (**ui**) pin the cross-plugin image guards, and pin panels to their own archive - (5369929) - 0xdev
- (**ui**) give the layout editor a probe, and let a panel follow its archive - (1235d2c) - 0xdev
- (**ui**) correct plugin panel probing, checkbox revert, and split extent - (40c4ba5) - 0xdev
- (**ui**) recover plugin actions after a lagged broadcast, and redact their errors - (e879c1c) - 0xdev
- (**ui**) keep plugin-authored ids out of the session-open trace - (f8f053b) - 0xdev
- (**ui**) apply archive_path from session events, and reconcile on stamp - (c4546b4) - 0xdev
- (**ui**) restore both-view invalidation on plugin enable toggle - (f36f735) - 0xdev
- (**ui**) register materialize actions before the origin, not after - (a9a99b2) - 0xdev
- (**ui**) wire real shutdown and fix external-open lease edge cases - (3c3e962) - 0xdev
- (**ui**) harden the operation bridge lifecycle - (555cd4e) - 0xdev
- (**ui**) fail loudly instead of silently extracting the whole archive - (ab0322d) - 0xdev
- (**ui**) run archive file edits on workers - (4d8c0ec) - 0xdev
- (**ui**) route plugin jobs through origin-aware queue - (fc8d346) - 0xdev
- (**ui**) cache archive tree projections between frames - (156c8c0) - 0xdev
- (**ui**) show effective plugin proxy routing - (8c024e9) - 0xdev
- (**ui**) keep live plugin proxy routing atomic - (147b147) - 0xdev
- (**ui**) limit filtered toolbar deletes to visible rows - (3b80fe4) - 0xdev
- (**ui**) synchronize live plugin proxy routing - (3a9e8c4) - 0xdev
- (**ui**) validate proxy before persistence - (80516b6) - 0xdev
- (**wirt**) pin the Wirt guest toolchain - (c540179) - 0xdev
- (**wirt**) enforce release and dependency pins - (bfb41b6) - 0xdev
- (**wirt**) package the fixture from the manifest the repo checks out - (b24728a) - 0xdev
- (**wirt**) bound the escaped rendering, not the text that feeds it - (6918794) - 0xdev
- (**wirt**) name the version and the remedy when a plugin is refused - (e9dbb74) - 0xdev
- (**wirt-cli**) break the manifest the round trip means to reject - (9eddd94) - 0xdev
- bound plugin discovery - (b3901d9) - 0xdev
- bound legacy plugin routing inspection - (f6e9a49) - 0xdev
- skip standalone proxy recovery for host routing - (feb614c) - 0xdev
- harden legacy storage inspection - (03d5021) - 0xdev
- document routing facade lint exception - (79f3698) - 0xdev
- canonicalize every plugin settings writer - (8ba05ea) - 0xdev
- collapse legacy plugin settings aliases - (e009764) - 0xdev
- fail closed when uninstall settings rollback fails - (ef1d0df) - 0xdev
- retain session closes during uninstall rollback - (251a566) - 0xdev
- canonicalize plugin settings identities - (e90e177) - 0xdev
- synchronize standalone Wirt SDK lockfile - (21da695) - 0xdev
- refresh cache classification during upsert - (232c3ed) - 0xdev
- serialize plugin settings flush snapshots - (457e953) - 0xdev
- preserve rendered plugin action identity - (5922f0c) - 0xdev
- reject stale plugin UI actions - (8d0102e) - 0xdev
- refresh gameta core lock dependencies - (c355436) - 0xdev
- keep UI independent of Wirt internals - (2c01f1b) - 0xdev
- confine bundled Wirt compatibility inputs - (5f28f8d) - 0xdev
- accept canonical Wirt interface subsets - (13ce207) - 0xdev
- retain Wirt project copy roots - (9b74fa3) - 0xdev
- confine Wirt developer file operations - (cecbaf7) - 0xdev
- reject oversized Wirt names before sorting - (529c1de) - 0xdev
- harden Wirt contract type hashing - (56eda43) - 0xdev
- bind Wirt contract identities - (d30c047) - 0xdev
- harden Wirt package preflight - (d40060f) - 0xdev
- preserve grouped Wirt absolute roots - (2a36843) - 0xdev
- reject absolute internal Wirt globs - (92c7c07) - 0xdev
- accept concrete Wirt self globs - (fac75ab) - 0xdev
- complete Wirt boundary provenance - (4167015) - 0xdev
- parse Wirt boundary attributes structurally - (32c52de) - 0xdev
- harden Wirt boundary provenance - (a378e60) - 0xdev
- tighten Wirt source boundary - (a403acd) - 0xdev
- simplify Wirt bindgen boundary - (299badb) - 0xdev
- close Wirt bindgen provenance gaps - (2632a1d) - 0xdev
- resolve Wirt bindgen aliases - (d31f0e0) - 0xdev
- parse Wirt boundary declarations - (166c03a) - 0xdev
- close Wirt WIT guard bypasses - (5fe0ff2) - 0xdev
- harden Wirt ABI boundary - (8dc0e22) - 0xdev
- accept valid Rust byte escapes - (ea95aed) - 0xdev
- complete Rust literal boundary scanning - (e480210) - 0xdev
- close Wirt boundary bypasses - (529da6b) - 0xdev
- rebuild malicious fixture for Wirt ABI - (500c817) - 0xdev
- reject renamed product dependencies - (5620ecd) - 0xdev
- honor profile overrides in database defaults - (470e4be) - 0xdev
- publish post-init plugin page document - (9a9672c) - 0xdev
- make the image recovery write run, and confine it to one namespace - (88dee20) - 0xdev
- land plugin-originated writes where their readers look - (eef1b15) - 0xdev
- serve the session bridge from any thread - (d1e01d5) - 0xdev
- make the CLI cancellable and boundable in every phase - (94ace72) - 0xdev
- make plugin intents and sessions consumable across renderers - (ebd38e8) - 0xdev
- never write settings rows from the cache - (c769d7c) - 0xdev
- re-read config and close vault handles for every owner - (639f665) - 0xdev
- keep the lease root private and multi-instance safe - (8e22c79) - 0xdev
- replace leaked temporary files with leases - (1d4a1ec) - 0xdev
- mark desync before releasing the mutation lock - (9795763) - 0xdev
- make mutation failure states honest and refreshes single - (65c4a89) - 0xdev
- restore organize password flow and pipeline artifact fidelity - (918cd70) - 0xdev
- close superseded archive sessions at the completion choke point - (46b655b) - 0xdev
- keep entry ids unique across file and directory namespaces - (59196f4) - 0xdev
- make cancelled opens leak-free and entry ids path-stable - (6ed2c18) - 0xdev
- keep legacy plugin dispatch until open flow migrates - (6688da0) - 0xdev
- keep the runtime owner armed through shutdown - (40afee7) - 0xdev
- preserve first-run seeding order and make shutdown drop-safe - (7afef63) - 0xdev
- publish operation events under the record lock - (6acfed2) - 0xdev
- stop flagging egui/eframe mentions inside doc comments - (d6dda4f) - 0xdev
- harden plugin, cache, and network boundaries - (f7acc10) - 0xdev
- prevent archive rename collisions - (1df5c28) - 0xdev
- run warning gate on Rust 1.97 - (4823f12) - 0xdev
- resolve Rust 1.97 future incompatibilities - (6595145) - 0xdev
- close archive state and rendering gaps - (e9cff9f) - 0xdev
- move plugin UI work off the render thread - (b2c9fca) - 0xdev
- propagate plugin cleanup failures - (6ee2c54) - 0xdev
- fail closed on unreadable proxy routing - (1559b07) - 0xdev
- apply proxy routing atomically - (2e99668) - 0xdev
- recover proxy persistence before activation - (b46d0f4) - 0xdev
- publish a single release archive - (7b3c9f9) - 0xdev
- preserve archive entry identity across navigation - (3dc94bd) - 0xdev
- close checked fetch lifecycle gaps - (3eb3f6a) - 0xdev
- keep live proxy routing consistent - (595d704) - 0xdev
- make proxy settings persistence atomic - (88ad1e1) - 0xdev
- validate organization plan paths - (682da5c) - 0xdev
- package current host with bundled plugins - (abf7b3e) - 0xdev
#### Performance Improvements
- (**app**) only rebuild the entry list for rules that read it - (21c078f) - 0xdev
- (**network**) avoid cloning completed responses - (71c05c7) - 0xdev
- (**theme**) retain borrowed icon font bytes - (3406207) - 0xdev
- (**theme**) avoid cloning system font bytes - (be27534) - 0xdev
- (**ui**) stop rebuilding what the organize views only read - (eef2898) - 0xdev
- (**ui**) load and virtualize logs asynchronously - (e1c6505) - 0xdev
- (**ui**) unify async image asset ownership - (d797919) - 0xdev
- (**ui**) avoid cloning settled archive search text - (75f2bac) - 0xdev
- (**ui**) avoid idle archive filter allocation - (3019ca5) - 0xdev
- (**ui**) make archive selection clones constant time - (b6b8db3) - 0xdev
- (**ui**) cache archive selection projections - (97627d9) - 0xdev
- (**ui**) cache archive entry projections - (68f0082) - 0xdev
#### Documentation
- (**app**) finish the comment repair for bootstrap-without-7-Zip - (6f7994d) - 0xdev
- (**app**) correct two claims in the drag-stage worker - (e3e9af8) - 0xdev
- (**app**) name the probe surface where its tests and report live - (8751ad9) - 0xdev
- (**app**) name the domain-access read model in the plugins module doc - (c9b7d49) - 0xdev
- (**app**) reunite file_paths with its doc comment - (dd637f7) - 0xdev
- (**core**) say that a screenshot path loses a leading ./ - (8947b7c) - 0xdev
- (**core**) say which file variables arrive unsanitised - (ead3673) - 0xdev
- (**core**) say what a translated rule stopped doing - (34dfc6e) - 0xdev
- (**core**) correct comments naming the deleted flatten walk - (e317c61) - 0xdev
- (**core**) say why the organized screenshots assertion went - (a202063) - 0xdev
- (**core**) describe the RAR time word by the policy that now reads it - (4714bd3) - 0xdev
- (**ui**) stop describing a per-tab page the browser no longer holds - (c0ef252) - 0xdev
- (**ui**) correct comments that still name deleted conversion state - (08b0a62) - 0xdev
- (**ui**) stop describing the deleted duplicate listing pipeline - (689d3ff) - 0xdev
- (**ui**) name the one bootstrap copy the test fixture mirrors - (26d79ef) - 0xdev
- (**ui**) record the per-extension-point plugin migration plan - (bff4fef) - 0xdev
- (**wirt**) say what the code requires, not what a document said - (1120494) - 0xdev
- (**wirt**) state the ABI the host actually speaks - (668a2f3) - 0xdev
- document Wirt development and security contracts - (e435506) - 0xdev
- add network domains to manifest examples - (1a7e86e) - 0xdev
- align comments with the optional metadata stack - (3968a8f) - 0xdev
- clarify legacy composition boundary comment - (912416f) - 0xdev
#### Tests
- (**app**) prove archive-only Arclain embedding - (f80a1c9) - 0xdev
- (**app**) keep the refused-title guard covered without the gameta feature - (b03bcdf) - 0xdev
- (**app**) pin the exit sweep and the write-then-trap pull - (e67e10b) - 0xdev
- (**app**) give the fixture a plugin that writes its own settings - (d7ddf51) - 0xdev
- (**app**) pin what disabling does to a plugin's sessions - (4e44974) - 0xdev
- (**app**) stop the lease-expiry test racing the sweeper it enables - (b0bfd2b) - 0xdev
- (**app**) poll for a terminal operation state inside the runtime - (6c8e18d) - 0xdev
- (**app**) pin the read-back gate that vetoes a cancelled merge's cleanup - (75e3b64) - 0xdev
- (**app**) pin the wire shape of the two new operation variants - (1aa9c0c) - 0xdev
- (**app**) pin what a merge does with an encrypted set - (94fc909) - 0xdev
- (**app**) prove the listing gate fired, and refuse an empty destination - (85c88de) - 0xdev
- (**app**) verify ArchiveContextBridge degrades safely off-runtime - (8df125f) - 0xdev
- (**app**) supply the new BootstrapConfig fields in the plugin-session bootstrap helper - (62b0d7d) - 0xdev
- (**app**) widen a too-tight wait budget found during rebase - (6b100ef) - 0xdev
- (**app,ui**) stop cache tests failing on the machine's free space - (bdea421) - 0xdev
- (**core**) tell two outputs apart by content, not by name - (26485c2) - 0xdev
- (**core**) pin the whole-plan transaction across outputs - (eff4771) - 0xdev
- (**core**) pin the layout output rules nothing else held - (ac38e75) - 0xdev
- (**core**) skip when the source archive will not list either - (8ad2398) - 0xdev
- (**core**) build pipeline test contexts by functional update - (fef7875) - 0xdev
- (**core**) lock the feature-off fallback tiers with explicit coverage - (57c7571) - 0xdev
- (**core**) stabilize Windows access-time verification - (d5c0982) - 0xdev
- (**core**) stabilize Windows timestamp fixture - (e57f29f) - 0xdev
- (**data**) cover checked streaming resume end to end - (97cb83c) - 0xdev
- (**network**) budget the routing-race waits for a loaded machine - (76b5965) - 0xdev
- (**network**) prevent routing test cleanup deadlock - (eb17815) - 0xdev
- (**network**) cover unavailable routing overlap - (360a1e6) - 0xdev
- (**network**) measure completion status clones - (299c0b6) - 0xdev
- (**plugins**) give the facade test fixture a dialog extension point - (c59b5ce) - 0xdev
- (**plugins**) sharpen the epoch dead-man doctrine and its sizing floor - (dc92ad2) - 0xdev
- (**plugins**) give the facade fixture a top tab, log line and capabilities - (1c6537f) - 0xdev
- (**plugins**) cover validation WASI isolation - (3aa85ea) - 0xdev
- (**signals**) make guard reentry regression deterministic - (064957f) - 0xdev
- (**theme**) lock system font byte ownership - (809d144) - 0xdev
- (**ui**) prove the type scale reaches the screen, not just the table - (6728b9e) - 0xdev
- (**ui**) stabilize dialog facade interactions - (b461c32) - 0xdev
- (**ui**) enforce image dependency boundary - (684dad5) - 0xdev
- (**ui**) align MainPage facade fixture - (ab32927) - 0xdev
- (**ui**) bound the no-manager-lock assertion by scheduling latency - (e882f68) - 0xdev
- (**ui**) keep the logs harness alive through the blocked-read handshake - (4946104) - 0xdev
- (**ui**) warm the slot before asserting a disabled plugin draws nothing - (6bc7585) - 0xdev
- (**ui**) pin the toolbar's facade-backed plugin buttons - (261f942) - 0xdev
- (**ui**) pin the session-backed relist pipeline end to end - (b559862) - 0xdev
- (**ui**) pin metadata to the session its panel is organizing - (fec694d) - 0xdev
- (**ui**) pin the guard on the startup session-deletion path - (2fc468a) - 0xdev
- (**ui**) drop unused plugin-routing test scaffolding - (7211bba) - 0xdev
- (**ui**) cover the session-event bridge swap end to end - (0c96fe0) - 0xdev
- (**ui**) add an on-demand end-to-end smoke test for open-file-from-archive - (2cd2881) - 0xdev
- (**ui**) stabilize concurrent UI worker tests - (8f93c25) - 0xdev
- (**ui**) keep proxy test ports reserved - (84ad0ee) - 0xdev
- (**wirt**) pin the Wirt extraction contract - (ce674c4) - 0xdev
- cover IPv6 proxy authorities - (c450685) - 0xdev
- characterize proxy routing before Porxi - (59afeee) - 0xdev
- use maintained UI plugin fixtures - (8e03dab) - 0xdev
- use tracked Wirt test fixtures - (a72799a) - 0xdev
- refresh Wirt CLI starter contract - (e1a7596) - 0xdev
- prove Wirt ABI round trip through Arclain - (29e15ba) - 0xdev
- cover neutral Wirt runtime contracts - (277721f) - 0xdev
- parse component imports structurally - (5973234) - 0xdev
- budget the two wall-clock waits for a cold, loaded machine - (f3c2794) - 0xdev
- add gate-lean recipe asserting lean builds exclude the gameta crates - (c598105) - 0xdev
- strengthen facade boundary regressions - (caa2203) - 0xdev
- settle async dialog layout before clicks - (2a62110) - 0xdev
- pin the Process page's facade path and convert cancellation - (844a6d5) - 0xdev
- cover arclain_cli in the owned-formatting expectations - (266d428) - 0xdev
- cover arclain_app in the owned-formatting expectations - (154923e) - 0xdev
- refresh trybuild snapshot for serde diagnostic drift - (aa885d7) - 0xdev
- add frontend dependency boundary guard - (f765877) - 0xdev
- enforce fmt-check recipe boundary - (c83f47f) - 0xdev
- strengthen owned formatting contracts - (13da0b6) - 0xdev
- prove plugin cleanup continues after failure - (725f017) - 0xdev
- run non-UI integration tests in CI - (747d1b9) - 0xdev
#### Build system
- pin the toolchain every recipe runs against - (ab977a2) - 0xdev
#### Refactoring
- (**app**) extract the plugin top-tab ui_items sync from bootstrap - (8f64154) - 0xdev
- (**app**) read the saved presets through one shared loader - (10ebb1c) - 0xdev
- (**app**) make the contract own the whole-directory request shape - (0427269) - 0xdev
- (**app**) keep the proxy probe's diagnostic honest when it has nothing to say - (4868802) - 0xdev
- (**app**) move processing workflows behind application facade - (a1364e2) - 0xdev
- (**app**) expose extraction as application operation - (3825bb4) - 0xdev
- (**app,core**) align the display-options docs and surface with the shipped shape - (f88580d) - 0xdev
- (**app,ui**) derive retryable from recoverability, not beside it - (60e9875) - 0xdev
- (**app,ui**) tidy the layout surface after review - (b12a570) - 0xdev
- (**core**) assert one filled output per located one - (ffa3133) - 0xdev
- (**core**) parse modinfo from a string, not only a folder - (7b022c1) - 0xdev
- (**core**) drop the copier the flattener left behind - (6a0a63c) - 0xdev
- (**core**) remove the second content-root detection - (84e1657) - 0xdev
- (**core**) drop the organizer nothing reached - (449f88e) - 0xdev
- (**core**) apply an organization plan in one place - (5ebb569) - 0xdev
- (**core**) drop an assert that could not fail - (c0575da) - 0xdev
- (**core**) drop the opener that shared one temp directory - (3c7a7ea) - 0xdev
- (**core**) keep the primary tier's reason on fallback failure - (b133926) - 0xdev
- (**core**) pin the container capabilities to the switches themselves - (c748c26) - 0xdev
- (**core**) validate a derived file name with the one validator - (f1ccceb) - 0xdev
- (**core,app**) seed the default rules from one place - (d3a1cd8) - 0xdev
- (**core,app**) decide once whether a plan stages nothing - (4971289) - 0xdev
- (**core,app**) let the format decide what a container can honour - (75a44a8) - 0xdev
- (**plugins**) inject the library service post-instantiate like every other optional service - (93dd44d) - 0xdev
- (**plugins**) give a plugin event the entry paths it can actually read - (bbcac99) - 0xdev
- (**ui**) move the spacing scale in beside its siblings - (02b271d) - 0xdev
- (**ui**) give the host's scales their own module - (1c61548) - 0xdev
- (**ui**) route plugin management through facade - (0b2a003) - 0xdev
- (**ui**) render plugin pages through the facade - (13d88a8) - 0xdev
- (**ui**) render the plugin main page through the facade - (437f672) - 0xdev
- (**ui**) draw the plugin dialog from its facade session - (2e01c6e) - 0xdev
- (**ui**) draw the toolbar's plugin buttons from their facade session - (9b65769) - 0xdev
- (**ui**) retire the browser listing's write-only page fetch - (7575207) - 0xdev
- (**ui**) render the top tab bar from the facade chrome DTOs - (c1a7a1e) - 0xdev
- (**ui**) drop the legacy pipeline-metadata projection - (1412dcb) - 0xdev
- (**ui**) run the Process page through the application facade - (73130a6) - 0xdev
- (**ui**) draw the archive browser from the session's own rows - (6974619) - 0xdev
- (**ui**) read a file for editing through its archive session - (1ef4d9d) - 0xdev
- (**ui**) read product metadata through the facade - (6e84769) - 0xdev
- (**ui**) resolve and fetch images through the application only - (6c23dc6) - 0xdev
- (**ui**) relist archives through the facade session instead of a second backend listing - (30012e3) - 0xdev
- (**ui**) split a listing's rows from what its request is doing - (7f0e821) - 0xdev
- (**ui**) bind a tab's listing to the session whose pages it may seat - (4a2b618) - 0xdev
- (**ui**) browse a tab's archive through the facade's listing model - (9ff2ac9) - 0xdev
- (**ui**) key the tree projection on its own entry type - (9101604) - 0xdev
- (**ui**) merge split archives through the facade - (a67d419) - 0xdev
- (**ui**) draw and edit the chrome layout through the facade - (c99b226) - 0xdev
- (**ui**) reach the organization surface through the facade - (84fbb60) - 0xdev
- (**ui**) read and write settings through the application's own shapes - (e5ad58b) - 0xdev
- (**ui**) stop running a persisted-secrets migration from the frontend - (feb8b81) - 0xdev
- (**ui**) render the probe's own trace, and honour the proxy toggle - (0df63a1) - 0xdev
- (**ui**) send the proxy probe through the facade too - (814e250) - 0xdev
- (**ui**) reach domain access and the gameta probe through the facade - (c325760) - 0xdev
- (**ui**) read log paths and archive checks from the facade - (560288f) - 0xdev
- (**ui**) render the archive-browser plugin panel through the facade - (43a7123) - 0xdev
- (**ui**) install the facade's active-tab bridge in production - (0704c0c) - 0xdev
- (**ui**) make session.json the operational tab-restore driver - (4a23fd1) - 0xdev
- (**ui**) route archive opening and extraction through the facade - (34744ca) - 0xdev
- (**wirt**) consume the shared Wirt platform - (97826f9) - 0xdev
- route proxy state through Porxi - (41cebbd) - 0xdev
- route Wirt execution through messages - (00126d0) - 0xdev
- establish the canonical Wirt SDK and ABI - (283749d) - 0xdev
- serve Wirt model through application facade - (2c4c423) - 0xdev
- move secure plugin loading into Wirt - (ded4577) - 0xdev
- adapt Arclain host state to Wirt runtime - (ee9cb1e) - 0xdev
- extract generic Wirt runtime kernel - (45740e9) - 0xdev
- move neutral plugin model into Wirt - (f7729b7) - 0xdev
- move plugin ABI under Wirt namespace - (30d09db) - 0xdev
- collapse invariant checks into `just check <subject>` - (f3af57b) - 0xdev
- rename gate-lean to check-gameta - (4db018b) - 0xdev
- remove core dependency from frontend - (0a4f52a) - 0xdev
- retire legacy frontend composition - (cea9a31) - 0xdev
- keep saved passwords behind facade - (f297461) - 0xdev
- move cache maintenance behind facade - (c3ce127) - 0xdev
- project settings through facade - (30a4f8c) - 0xdev
- move plugin visibility behind facade - (95dd124) - 0xdev
- expose frontend support through facade - (415129c) - 0xdev
- isolate frontend runtime from core services - (9e38ea0) - 0xdev
- move plugin domain policy behind facade - (6d42568) - 0xdev
- move plugin bridge ownership behind facade - (aa14089) - 0xdev
- drop the dead content-cache parameter threading - (5b5f589) - 0xdev
- correct the parts-ordering contract and drop the last dead field - (eba14ea) - 0xdev
- tighten the settings surface after review - (abe54d2) - 0xdev
- remove three direct headless dependencies from arclain_ui - (9f396c3) - 0xdev
- route every settings writer through the application facade - (90fa673) - 0xdev
- expose renderer-neutral plugin sessions - (394fdd7) - 0xdev
- move settings and persistence behind facade - (821593f) - 0xdev
- expose archive mutations through facade - (2934a31) - 0xdev
- move archive sessions behind application facade - (e779db0) - 0xdev
- move runtime composition into application facade - (595a5a3) - 0xdev
- consolidate release helper behavior - (2bc2658) - 0xdev
- release.py builds+packages only; CI owns test gating - (b652b83) - 0xdev
#### Miscellaneous Chores
- (**core**) drop the base64 dependency nothing uses - (1772492) - 0xdev
- (**ui**) deny the f32 float-literal fallback in this crate - (2b3aa88) - 0xdev
- (**ui**) drop the rotted pre-signals navigation module - (77d05e9) - 0xdev
- (**ui**) drop a dead import comment, restate a doc reference - (024d5d8) - 0xdev
- drop an unreachable theme and describe tests in their own terms - (a883618) - 0xdev
- refresh vendored just exclude - (07c216b) - 0xdev
- vendor just exclude for analyzer worktree exclusions - (a9f09fb) - 0xdev
- ignore worktree copies in search and language servers - (0a17080) - 0xdev
- keep the facade test fixture out of release packages - (f0d6b90) - 0xdev
- cover arclain_app under the shared format gate - (8dd3609) - 0xdev
- own repository formatting - (dcc75d9) - 0xdev
- pin gameta revision across builds - (9ff215c) - 0xdev
- ignore subagent scratch files - (8ad25c9) - 0xdev
- configure local workspaces - (b082754) - 0xdev
- adopt shared just library - (565be47) - 0xdev
- format Rust sources - (ec225f9) - 0xdev
- remove trailing whitespace - (a331d4e) - 0xdev
#### Style
- (**plugins**) rustfmt the epoch deadline test helpers - (545ea2f) - 0xdev
- apply rustfmt and fix a stale bridge doc reference - (25aa211) - 0xdev

- - -

## 2.3.2 - 2026-05-28
### Packages
- dlsite-metadata locked to dlsite-metadata-0.11.0
### Global changes
#### Bug Fixes
- (**ui**) respect keyboard focus and popups for app hotkeys - (bfa5f6d) - 0xdev
#### Refactoring
- (**ui**) group header and palette render args into structs - (62b6455) - 0xdev

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