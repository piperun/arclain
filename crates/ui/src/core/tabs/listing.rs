//! Per-tab archive listing state, in the application facade's own
//! vocabulary.
//!
//! Before this module a tab navigated `arclain_core::archive::
//! NavigationState` over a flat `Vec<arclain_core::ArchiveEntry>`: the
//! "current folder" was a bare `String`, and showing that folder meant
//! re-filtering the whole archive's entry list on every navigation. The
//! application facade already models the same thing properly -- a
//! directory-scoped, sorted, filtered, paginated
//! [`ListEntriesRequest`] answered with the session's own
//! `arclain_app::archive::ArchiveEntryDto` rows -- so a tab holds *that*
//! instead, and navigation is simply which directory the request names.
//!
//! [`ArchiveNavigation`] keeps the pre-facade breadcrumb and
//! back/forward semantics exactly (its tests pin each one against the
//! behavior `NavigationState` had); what changes is the type of the
//! cursor: an [`ArchivePath`], validated once, instead of a string
//! re-normalized at every call site.

use arclain_app::archive::{ArchivePath, ListEntriesRequest};
use arclain_app::error::ApplicationError;
use arclain_app::ids::ArchiveSessionId;
use std::sync::Arc;

/// Normalizes a path fragment the way `arclain_core::archive::
/// NavigationState::normalize_path` did: split on either separator, drop
/// empty segments, rejoin with `/`.
///
/// Kept as a distinct step *before* [`ArchivePath::parse`] rather than
/// folded into it: `parse` deliberately preserves what it is given
/// (minus backslash normalization) so an archive entry's path survives a
/// round trip, whereas a navigation target arrives from a breadcrumb
/// click or a folder row and may carry the redundant leading, trailing,
/// or doubled separators the pre-facade code silently collapsed. Doing
/// it here keeps that leniency at the navigation boundary, where it
/// belongs, instead of loosening the shared path type for everyone.
fn normalize_fragment(path: &str) -> String {
    path.split(['/', '\\'])
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// The directory portion of an already-valid [`ArchivePath`].
///
/// `ArchivePath::parse` cannot fail on a prefix of a value it already
/// accepted (a prefix of a relative, traversal-free, NUL-free path taken
/// at a `/` boundary is all three as well), so the fallback to
/// [`ArchivePath::root`] is unreachable rather than a silent
/// substitution.
fn parent_of(path: &ArchivePath) -> ArchivePath {
    let raw = path.as_str();
    let parent = match raw.rfind('/') {
        Some(position) => &raw[..position],
        None => "",
    };
    ArchivePath::parse(parent.to_string()).unwrap_or_else(|_| ArchivePath::root())
}

/// Where one tab is browsing inside its open archive, plus the
/// back/forward history the toolbar arrows walk.
///
/// Replaces `arclain_core::archive::NavigationState`. Every navigation
/// rule that type had is reproduced here (see this module's tests, which
/// pin them one by one), with two deliberate differences:
///
/// * The cursor is an [`ArchivePath`], so a target that is not a legal
///   archive-relative path (absolute, or carrying a `..` traversal
///   segment) is refused instead of navigated to. `NavigationState`
///   accepted such a value and then simply matched no entries; refusing
///   it means a crafted breadcrumb or folder name can never become the
///   `directory` of a [`ListEntriesRequest`] at all.
/// * Every mutator reports whether it moved. The old type returned `()`
///   from `navigate_to`/`navigate_to_absolute` and left callers to
///   re-read `current_path` to find out.
///
/// The flat-list filtering `NavigationState` also carried
/// (`filter_entries`, `get_all_folders`) is deliberately *not* here:
/// scoping a listing to one directory is what
/// [`ListEntriesRequest::directory`] means, and the facade's own entry
/// index does the folder synthesis and aggregation that filtering used
/// to redo on every navigation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveNavigation {
    current: ArchivePath,
    back: Vec<ArchivePath>,
    forward: Vec<ArchivePath>,
}

impl Default for ArchiveNavigation {
    fn default() -> Self {
        Self {
            current: ArchivePath::root(),
            back: Vec::new(),
            forward: Vec::new(),
        }
    }
}

impl ArchiveNavigation {
    /// The directory currently being browsed;
    /// [`ArchivePath::root`]-equal at the archive root.
    pub fn current(&self) -> &ArchivePath {
        &self.current
    }

    /// The current directory as the slash-separated string the
    /// breadcrumb renders (empty at the archive root).
    pub fn current_path(&self) -> &str {
        self.current.as_str()
    }

    /// Descends into `folder`, interpreted relative to the current
    /// directory. A multi-segment fragment descends the whole way in one
    /// step; an empty (or all-separator) fragment is a no-op.
    pub fn descend(&mut self, folder: &str) -> bool {
        let segment = normalize_fragment(folder);
        if segment.is_empty() {
            return false;
        }
        let joined = if self.current.as_str().is_empty() {
            segment
        } else {
            format!("{}/{}", self.current.as_str(), segment)
        };
        let Ok(next) = ArchivePath::parse(joined) else {
            return false;
        };
        self.push_history();
        self.current = next;
        self.forward.clear();
        true
    }

    /// Jumps to `path`, interpreted from the archive root. Navigating to
    /// the directory already displayed is a no-op that leaves the
    /// history untouched.
    pub fn go_to(&mut self, path: &str) -> bool {
        let Ok(next) = ArchivePath::parse(normalize_fragment(path)) else {
            return false;
        };
        self.go_to_directory(next)
    }

    /// [`Self::go_to`] once the target has been validated -- split out
    /// so the equality/history rules live in one place regardless of
    /// where the [`ArchivePath`] came from.
    fn go_to_directory(&mut self, directory: ArchivePath) -> bool {
        if directory == self.current {
            return false;
        }
        self.push_history();
        self.current = directory;
        self.forward.clear();
        true
    }

    /// Steps back through the history, then (once the history is empty)
    /// out to the archive root.
    pub fn back(&mut self) -> bool {
        if let Some(previous) = self.back.pop() {
            self.forward
                .push(std::mem::replace(&mut self.current, previous));
            return true;
        }
        if self.current.as_str().is_empty() {
            return false;
        }
        self.forward
            .push(std::mem::replace(&mut self.current, ArchivePath::root()));
        true
    }

    /// Steps forward through whatever [`Self::back`] walked out of.
    pub fn forward(&mut self) -> bool {
        let Some(next) = self.forward.pop() else {
            return false;
        };
        self.back.push(std::mem::replace(&mut self.current, next));
        true
    }

    /// Moves to the parent directory, clearing the forward history the
    /// way any other fresh navigation does.
    pub fn up(&mut self) -> bool {
        if self.current.as_str().is_empty() {
            return false;
        }
        let parent = parent_of(&self.current);
        self.back.push(std::mem::replace(&mut self.current, parent));
        self.forward.clear();
        true
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty() || !self.current.as_str().is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn can_go_up(&self) -> bool {
        !self.current.as_str().is_empty()
    }

    /// Records the current directory as the place [`Self::back`] returns
    /// to. The archive root is deliberately not recorded: `back`'s own
    /// "history empty, so step out to the root" fallback already covers
    /// it, and pushing it would make one `back` press land on the root
    /// twice.
    fn push_history(&mut self) {
        if !self.current.as_str().is_empty() {
            self.back.push(self.current.clone());
        }
    }
}

/// What the tab knows about the directory it is browsing, and what the
/// fetch behind it is doing.
///
/// Six states, which are three pairs of the *same* fetch state over the
/// two answers to the question that decides everything the browser draws:
/// **have this archive's contents ever been listed?**
///
/// | contents | nothing running | fetch in flight | last fetch failed |
/// | --- | --- | --- | --- |
/// | **unknown** | [`Unlisted`] | [`Loading`] | [`Unlistable`] |
/// | **known** | [`Listed`] | [`Refreshing`] | [`Stale`] |
///
/// The top row is the one that used to be missing. An earlier shape had a
/// single `Idle` covering both "nothing has been asked" and "the session
/// answered", with the browser inferring which by asking whether the tab
/// had any rows -- and no rows plus nothing running is exactly what a tab
/// looks like both when its archive is empty and when it has never listed
/// anything at all. So a tab that had only ever been *pointed* at an
/// archive drew as a loaded, empty one, and stayed that way for good once
/// the open behind it failed: there was nowhere for that outcome to land.
/// Naming the unknown row makes the two tellable apart from this value
/// alone, with no second signal to consult and no row count to read.
///
/// The same missing fact is why [`Stale`] and [`Unlistable`] are separate
/// variants rather than one `Failed`. A failure over a directory that
/// *has* been answered strands the last good answer on screen, and a
/// directory whose last good answer was "nothing here" is no exception --
/// keying that on "are there rows" reported a failed refresh of a
/// genuinely empty folder as contents-unknown.
///
/// The rows themselves stay off this type. They are the browser rows
/// published for the browsed directory (`TabState::browser_entries`,
/// scoped out of the whole-archive [`TabInventory`] by
/// `crate::core::operations::browser_rows`); this is what the fetch behind
/// them is doing.
///
/// `crate::features::archive_browser::presentation::views::browser_page::
/// browser_body` is the reader that turns this table into what the browser
/// panel draws.
///
/// [`Unlisted`]: RequestStatus::Unlisted
/// [`Loading`]: RequestStatus::Loading
/// [`Unlistable`]: RequestStatus::Unlistable
/// [`Listed`]: RequestStatus::Listed
/// [`Refreshing`]: RequestStatus::Refreshing
/// [`Stale`]: RequestStatus::Stale
/// [`TabInventory`]: super::inventory::TabInventory
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RequestStatus {
    /// Nothing has ever been listed here and nothing is running. The tab
    /// may already *name* an archive -- reopen-closed, duplicate, session
    /// restore and replace-active all point a fresh tab at a path before
    /// the open behind it has begun -- but naming one is not knowing what
    /// is in it.
    #[default]
    Unlisted,
    /// A listing is in flight and nothing has answered before it, so the
    /// contents are still unknown.
    Loading,
    /// A listing answered. The published rows *are* that answer, and zero
    /// of them means the directory really is empty.
    Listed,
    /// A listing is in flight over contents that have already been
    /// answered. Whatever is on screen is still the last good answer while
    /// it runs.
    Refreshing,
    /// A listing failed with nothing ever answered before it: the contents
    /// are **unknown**, which is a different claim from *empty*.
    ///
    /// Behind an `Arc` for the same per-frame-clone reason the rows are:
    /// the whole envelope (kind, recoverability, suggested action,
    /// correlation id) is what a renderer needs to offer a retry, not just
    /// a summary string.
    Unlistable(Arc<ApplicationError>),
    /// A listing failed over contents that had already been answered:
    /// what is on screen is the last good answer, and this is why it is
    /// not a fresh one.
    Stale(Arc<ApplicationError>),
}

impl RequestStatus {
    /// Whether a listing has ever answered for this binding -- which half
    /// of [`RequestStatus`]'s own table this value sits in.
    ///
    /// The one fact every transition below needs and no caller should have
    /// to reconstruct. Matched exhaustively with no `_` arm on purpose: a
    /// new variant has to say which half it belongs to rather than
    /// defaulting into "unknown" and quietly reintroducing the conflation
    /// this type exists to prevent.
    pub fn contents_known(&self) -> bool {
        match self {
            Self::Unlisted | Self::Loading | Self::Unlistable(_) => false,
            Self::Listed | Self::Refreshing | Self::Stale(_) => true,
        }
    }
}

/// Names one listing request so its eventual reply -- success or failure
/// -- can be told apart from a newer request's. Minted by
/// [`TabListing::begin_loading`] and handed back to
/// [`TabListing::succeed`]/[`TabListing::fail`]; a reply carrying a
/// superseded token is dropped rather than applied.
///
/// Identity is scoped to one `TabListing` *value*: the counter restarts
/// when a tab rebinds to a new session via [`TabListing::for_session`],
/// so a token minted against the old value can numerically collide with
/// the new value's counter. That is deliberate and safe -- the session
/// guard both replies apply after the generation check refuses any
/// cross-session reply regardless, so the token only needs to order
/// requests *within* one binding, which a per-value counter does exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListingGeneration(u64);

/// One tab's archive listing: which session it is listing, where it is
/// browsing, the request that describes what its browser is showing, and
/// what the fetch behind that is doing.
///
/// The rows themselves are deliberately *not* here. A tab holds its
/// archive's whole entry tree once ([`TabInventory`]) and the browser
/// draws one folder by scoping it, which is what makes navigation
/// repaint without a round trip; `crates/ui/tests/tab_archive_model_test.rs`
/// asserts that scoping equals the session's own answer to
/// [`Self::request`] row for row, field for field, which is what licenses
/// the arrangement. An earlier shape kept a second, per-relist
/// `list_entries` page here as well -- nothing ever read it, and the
/// status axis never depended on it, because the same
/// [`Self::begin_loading`]/[`Self::fail`] pair brackets the inventory
/// fetch.
///
/// `request.directory` always equals `navigation.current()` -- callers
/// change it by navigating, never by editing the request -- and every
/// navigation discards the status, because a reply answers the directory
/// it was requested for and nothing else.
///
/// [`TabInventory`]: super::inventory::TabInventory
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabListing {
    /// The archive session this listing belongs to, `None` before the
    /// tab has an archive open. Load-bearing rather than informational:
    /// it is what [`Self::succeed`] and [`Self::fail`] check a reply
    /// against, so a listing made for the archive the tab held *before*
    /// this one can neither clear nor deface the status of the one it
    /// holds now.
    session: Option<ArchiveSessionId>,
    navigation: ArchiveNavigation,
    request: ListEntriesRequest,
    status: RequestStatus,
    /// Which request the listing currently cares about -- bumped by
    /// [`Self::begin_loading`] (a new fetch) and by every successful
    /// navigation (whatever was in flight no longer answers what is
    /// being browsed). The reply-ordering guard [`Self::succeed`] and
    /// [`Self::fail`] compare their token against.
    generation: u64,
}

impl Default for TabListing {
    fn default() -> Self {
        Self::for_session(None)
    }
}

impl TabListing {
    /// A fresh listing at the archive root, bound to `session` -- what a
    /// tab starts browsing a newly opened archive from, and (with
    /// `None`) what a tab with no archive open holds.
    ///
    /// The whole-directory request shape itself lives on the contract
    /// ([`ListEntriesRequest::whole_directory`]) rather than here: every
    /// frontend that resolves paths back to `EntryId`s needs it, not
    /// only this one.
    pub fn for_session(session: Option<ArchiveSessionId>) -> Self {
        Self {
            session,
            navigation: ArchiveNavigation::default(),
            request: ListEntriesRequest::whole_directory(ArchivePath::root()),
            // Nothing has been asked for this binding yet, and a rebind to
            // a different archive knows nothing about the new one however
            // much it knew about the old.
            status: RequestStatus::Unlisted,
            generation: 0,
        }
    }

    /// The archive session this listing belongs to.
    pub fn session(&self) -> Option<ArchiveSessionId> {
        self.session
    }

    /// The directory being browsed -- equivalently
    /// `request().directory`.
    pub fn directory(&self) -> &ArchivePath {
        &self.request.directory
    }

    /// The current directory as the slash-separated string the
    /// breadcrumb renders (empty at the archive root).
    pub fn current_path(&self) -> &str {
        self.navigation.current_path()
    }

    /// The request this tab's browser is showing the answer to -- the
    /// exact [`ListEntriesRequest`] a caller hands
    /// `ArclainApp::list_entries` to get the session's own answer for the
    /// browsed directory.
    ///
    /// Nothing on the render path issues it, because the browser draws
    /// the same answer by scoping the whole-archive inventory (see this
    /// type's own doc comment). It is what that equivalence is stated
    /// *against*: `tab_archive_model_test` lists this request for real and
    /// compares it to the rows the browser draws, so a drift between the
    /// two is a test failure rather than a silent display regression.
    pub fn request(&self) -> &ListEntriesRequest {
        &self.request
    }

    /// What the latest listing request for this directory is doing.
    ///
    /// Orthogonal to whether the tab has rows to show -- a renderer must
    /// consult both before deciding what to draw. See [`RequestStatus`]
    /// for the full table of combinations.
    pub fn status(&self) -> &RequestStatus {
        &self.status
    }

    /// Whether nothing has ever been asked of this listing -- stricter
    /// than the negation of [`RequestStatus::contents_known`], which also
    /// holds for a listing that is running and for one that failed.
    ///
    /// The guard [`Self::fail_unlisted`] applies, named once here because
    /// two callers need it: that method, and the one that has to make the
    /// *other* decision about a tab that never listed anything -- a
    /// cancelled open has no failure to record and takes the archive off
    /// the tab instead.
    pub fn is_unlisted(&self) -> bool {
        matches!(self.status, RequestStatus::Unlisted)
    }

    /// The error the last listing of this directory failed with, if it
    /// failed. Independent of whether the tab has rows to show: a refresh
    /// that fails over a directory already on screen reports both.
    pub fn failure(&self) -> Option<&ApplicationError> {
        match &self.status {
            RequestStatus::Unlistable(error) | RequestStatus::Stale(error) => Some(error),
            RequestStatus::Unlisted
            | RequestStatus::Loading
            | RequestStatus::Listed
            | RequestStatus::Refreshing => None,
        }
    }

    /// Whether a listing for the current request is in flight -- which says
    /// nothing about whether rows are on screen while it runs.
    pub fn is_loading(&self) -> bool {
        matches!(
            self.status,
            RequestStatus::Loading | RequestStatus::Refreshing
        )
    }

    /// Records that a listing for the current request is in flight and
    /// mints the [`ListingGeneration`] naming this attempt -- the token
    /// its eventual [`Self::succeed`]/[`Self::fail`] must present.
    ///
    /// Which in-flight state it lands in carries the fact the reply will
    /// need: a fetch over contents nothing has ever answered is
    /// [`RequestStatus::Loading`] (there is nothing to show while it
    /// runs), one over contents already answered is
    /// [`RequestStatus::Refreshing`] (what is on screen is still the last
    /// good answer). That is also what lets [`Self::fail`] tell an
    /// unlistable archive from a stale one without counting rows.
    ///
    /// Whether the rows already on screen stay there while it runs is the
    /// caller's choice, not this type's: the rows live on the tab's own
    /// signals, so "refreshing, previous answer still shown" is a state
    /// the model can hold. A relist that rebinds the tab to a *different*
    /// archive clears them, because none of them describe it -- and
    /// rebinding resets the status to [`RequestStatus::Unlisted`], so the
    /// fetch that follows is a first listing again.
    pub fn begin_loading(&mut self) -> ListingGeneration {
        self.generation += 1;
        self.status = if self.status.contents_known() {
            RequestStatus::Refreshing
        } else {
            RequestStatus::Loading
        };
        ListingGeneration(self.generation)
    }

    /// Records that the listing attempt `generation` names -- made
    /// against `session_id` for `directory` -- answered, settling the
    /// status at [`RequestStatus::Listed`]. A successful listing
    /// supersedes whatever the previous attempt was doing, including a
    /// failure, and it is the only transition that makes the contents
    /// known.
    ///
    /// Refused (`false`) under [`Self::answers_current_request`]'s three
    /// guards, exactly as [`Self::fail`] is.
    pub fn succeed(
        &mut self,
        generation: ListingGeneration,
        session_id: ArchiveSessionId,
        directory: &ArchivePath,
    ) -> bool {
        if !self.answers_current_request(generation, session_id, directory) {
            return false;
        }
        self.status = RequestStatus::Listed;
        true
    }

    /// Records that the listing attempt `generation` names -- made
    /// against `session_id` -- failed for `directory`, and reports
    /// whether that failure is about what is being browsed now.
    ///
    /// Refused (`false`) under [`Self::answers_current_request`]'s three
    /// guards, exactly as [`Self::succeed`] is.
    ///
    /// Lands on [`RequestStatus::Stale`] when a listing had already
    /// answered for this binding and [`RequestStatus::Unlistable`] when
    /// none ever had -- the distinction between "these are the last known
    /// contents" and "the contents are unknown". It is read off the
    /// in-flight state [`Self::begin_loading`] minted rather than off the
    /// tab's row count, which is why a failed refresh of a directory that
    /// was answered as *empty* still reads as a failed refresh.
    ///
    /// **Rows already on screen are left alone.** They are the session's
    /// last successful answer for this exact directory, and blanking them
    /// because a *refresh* failed loses information the user can still act
    /// on -- while the failure itself is recorded in the status either
    /// way, so a renderer can mark the rows stale rather than having to
    /// choose between showing them and reporting the error.
    ///
    /// Keeping them is safe, not merely convenient: acting on a stale row
    /// cannot reach the wrong entry, because the `EntryId` it carries is
    /// validated against the owning session on every facade call that takes
    /// one. A superseded-revision id resolves to nothing rather than to
    /// some other entry, so the worst case is a refused operation, not a
    /// wrong-file delete. That reasoning covers a *stale* row of the same
    /// archive; it is not a licence to leave a previous *archive's* rows up
    /// under a new archive's name, which is why the relist clears them when
    /// it rebinds the tab.
    pub fn fail(
        &mut self,
        generation: ListingGeneration,
        session_id: ArchiveSessionId,
        directory: &ArchivePath,
        error: ApplicationError,
    ) -> bool {
        if !self.answers_current_request(generation, session_id, directory) {
            return false;
        }
        let error = Arc::new(error);
        self.status = if self.status.contents_known() {
            RequestStatus::Stale(error)
        } else {
            RequestStatus::Unlistable(error)
        };
        true
    }

    /// Records why a listing that was never even asked for is never going
    /// to be answered, and reports whether it was recorded.
    ///
    /// The companion to [`Self::fail`] for the failure that happens
    /// *before* any listing: an archive open that never reached the point
    /// of minting a session has no attempt to close, no session to check a
    /// reply against and no directory to name, so it cannot present the
    /// token `fail` demands -- yet it decides the same thing, that this
    /// tab's contents are unknown, permanently.
    ///
    /// Without it the terminal outcome had nowhere to land. A tab pointed
    /// at an archive by reopen-closed, duplicate, session restore or
    /// replace-active sits at [`RequestStatus::Unlisted`] while its open
    /// runs; when that open fails there is no in-flight marker left to
    /// observe and nothing to distinguish the tab from one whose archive
    /// was listed and found empty, so the browser drew an empty archive
    /// and kept drawing it.
    ///
    /// Refused (`false`) unless the listing is still
    /// [`RequestStatus::Unlisted`]. That guard is what keeps a failed open
    /// off a tab with an archive already on screen: opening B into a tab
    /// showing A and having it fail says nothing about A, whose rows,
    /// inventory and listing all still describe it correctly. It equally
    /// refuses to deface a listing that is genuinely in flight, whose own
    /// reply is the one entitled to settle it.
    pub fn fail_unlisted(&mut self, error: ApplicationError) -> bool {
        if !self.is_unlisted() {
            return false;
        }
        self.status = RequestStatus::Unlistable(Arc::new(error));
        true
    }

    /// Whether a reply presenting `generation`, made against `session_id`
    /// for `directory`, is the answer to what is being browsed now.
    ///
    /// Three guards, each closing a distinct hole:
    ///
    /// * `generation` -- a newer [`Self::begin_loading`] or a navigation
    ///   has superseded this request, so its reply is an overtaken one.
    ///   An `ApplicationError` carries no revision to order failures by,
    ///   so without a request identity a slow attempt's late failure would
    ///   mark a listing a *newer* attempt just refreshed as failed; and
    ///   its mirror, a late success erasing a newer attempt's genuine
    ///   `Loading`, is the same hole from the other side.
    /// * `session_id` -- a reply for the archive the tab held *before*
    ///   this one says nothing about the one it holds now. This is also
    ///   what makes the per-value generation counter sufficient: a rebind
    ///   restarts that counter, so a numerically colliding token from the
    ///   previous binding must not be able to clear or deface the new
    ///   session's status.
    /// * `directory` -- an in-flight reply racing the very navigation that
    ///   superseded it.
    fn answers_current_request(
        &self,
        generation: ListingGeneration,
        session_id: ArchiveSessionId,
        directory: &ArchivePath,
    ) -> bool {
        generation.0 == self.generation
            && self.session == Some(session_id)
            && directory == &self.request.directory
    }

    /// Descends into `folder`, relative to the current directory -- a
    /// folder row's own name, or a multi-segment fragment from the tree
    /// panel.
    pub fn descend(&mut self, folder: &str) -> bool {
        self.navigated(ArchiveNavigation::descend, folder)
    }

    /// Jumps to `path`, interpreted from the archive root -- a
    /// breadcrumb segment or a tree-panel selection.
    pub fn go_to(&mut self, path: &str) -> bool {
        self.navigated(ArchiveNavigation::go_to, path)
    }

    pub fn back(&mut self) -> bool {
        self.navigated(|navigation, ()| navigation.back(), ())
    }

    pub fn forward(&mut self) -> bool {
        self.navigated(|navigation, ()| navigation.forward(), ())
    }

    pub fn up(&mut self) -> bool {
        self.navigated(|navigation, ()| navigation.up(), ())
    }

    pub fn can_go_back(&self) -> bool {
        self.navigation.can_go_back()
    }

    pub fn can_go_forward(&self) -> bool {
        self.navigation.can_go_forward()
    }

    pub fn can_go_up(&self) -> bool {
        self.navigation.can_go_up()
    }

    /// Runs one navigation and, if it moved, re-points the request at the
    /// new directory and discards whatever answered the old one -- an
    /// in-flight marker or a failure alike, since neither says anything
    /// about the directory now being browsed. The generation advances
    /// too: a reply still in flight for the old directory is a superseded
    /// request's reply now, dropped by its own token rather than relying
    /// solely on the directory comparison (which a navigate-away-and-back
    /// would defeat).
    ///
    /// What is *not* discarded is whether this archive's contents are
    /// known. Moving between directories is answered from the whole-archive
    /// inventory the tab already holds, not by a new fetch, so a tab that
    /// has listed its archive still knows what is in the folder it just
    /// moved to -- including that the folder is empty. Dropping to
    /// [`RequestStatus::Unlisted`] on every move would make each
    /// navigation into an empty folder read as "contents unknown".
    ///
    /// The browser's rows are republished by the navigation service in the
    /// same step, out of that inventory, so a move never leaves the old
    /// directory's rows under the new directory's breadcrumb and never
    /// costs a frame of emptiness.
    fn navigated<A>(
        &mut self,
        navigate: impl FnOnce(&mut ArchiveNavigation, A) -> bool,
        argument: A,
    ) -> bool {
        if !navigate(&mut self.navigation, argument) {
            return false;
        }
        self.request.directory = self.navigation.current().clone();
        self.request.offset = 0;
        self.status = if self.status.contents_known() {
            RequestStatus::Listed
        } else {
            RequestStatus::Unlisted
        };
        self.generation += 1;
        true
    }
}

#[cfg(test)]
#[path = "listing_tests.rs"]
mod tests;
