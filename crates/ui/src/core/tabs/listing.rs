//! Per-tab archive listing state, in the application facade's own
//! vocabulary.
//!
//! Before this module a tab navigated `arclain_core::archive::
//! NavigationState` over a flat `Vec<arclain_core::ArchiveEntry>`: the
//! "current folder" was a bare `String`, and showing that folder meant
//! re-filtering the whole archive's entry list on every navigation. The
//! application facade already models the same thing properly -- a
//! directory-scoped, sorted, filtered, paginated
//! [`ListEntriesRequest`] answered with an [`EntryPage`] of
//! [`arclain_app::archive::ArchiveEntryDto`] rows -- so a tab holds
//! *that* instead, and navigation is simply which directory the request
//! names.
//!
//! [`ArchiveNavigation`] keeps the pre-facade breadcrumb and
//! back/forward semantics exactly (its tests pin each one against the
//! behavior `NavigationState` had); what changes is the type of the
//! cursor: an [`ArchivePath`], validated once, instead of a string
//! re-normalized at every call site.

use arclain_app::archive::{ArchiveEntryDto, ArchivePath, EntryPage, ListEntriesRequest};
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

/// What the tab's most recent listing request is doing.
///
/// Deliberately separate from *whether the tab holds rows* (see
/// [`TabListing::page`]), because those are independent facts and the
/// states that matter are their combinations:
///
/// | rows | status | means |
/// | --- | --- | --- |
/// | none | `Idle` | nothing asked yet -- no archive open, or navigation just moved |
/// | none | `Loading` | first listing of this directory in flight |
/// | none | `Failed` | listing this directory failed; its contents are *unknown* |
/// | some | `Idle` | the session answered; zero rows means the folder really is empty |
/// | some | `Loading` | refreshing, with the previous answer still on screen |
/// | some | `Failed` | a refresh failed; these rows are the last good answer |
///
/// Collapsing the two axes into one enum -- which an earlier shape of this
/// did -- makes the bottom two rows unnameable. A renderer then cannot
/// draw a spinner over existing rows, and cannot mark rows as
/// "couldn't refresh", even in principle: the failure has nowhere to live
/// once the decision is made to keep the rows. And with a single
/// `Option<EntryPage>` the top row and the fourth become the same value,
/// so a listing that *failed* renders as an ordinary empty folder -- the
/// silent-empty-view failure mode, arriving by construction rather than by
/// accident.
///
/// Kept explicit now, while nothing renders any of this yet: the
/// alternative is discovering it after the render tree is built on top of
/// the collapsed shape.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RequestStatus {
    /// No request is outstanding: either none has been made, or the last
    /// one was answered.
    #[default]
    Idle,
    /// A listing for the current request is in flight.
    Loading,
    /// The last listing of this directory failed. Behind an `Arc` for the
    /// same per-frame-clone reason the rows are: the whole envelope
    /// (kind, recoverability, suggested action, correlation id) is what a
    /// renderer needs to offer a retry, not just a summary string.
    Failed(Arc<ApplicationError>),
}

/// Names one listing request so its eventual reply -- success or failure
/// -- can be told apart from a newer request's. Minted by
/// [`TabListing::begin_loading`] and handed back to
/// [`TabListing::adopt_page`]/[`TabListing::fail`]; a reply carrying a
/// superseded token is dropped rather than applied.
///
/// Identity is scoped to one `TabListing` *value*: the counter restarts
/// when a tab rebinds to a new session via [`TabListing::for_session`],
/// so a token minted against the old value can numerically collide with
/// the new value's counter. That is deliberate and safe -- the session
/// guard [`TabListing::adopt_page`] applies after the generation check
/// refuses any cross-session reply regardless, so the token only needs
/// to order requests *within* one binding, which a per-value counter
/// does exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListingGeneration(u64);

/// One tab's archive listing: which session it is listing, where it is
/// browsing, the request that describes what its browser is showing, the
/// rows the session last returned for it, and what the latest request is
/// doing.
///
/// `request.directory` always equals `navigation.current()` -- callers
/// change it by navigating, never by editing the request -- and every
/// navigation discards both rows and status, because a reply answers the
/// directory it was requested for and nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabListing {
    /// The archive session this listing belongs to, `None` before the
    /// tab has an archive open. Load-bearing rather than informational:
    /// it is what [`Self::adopt_page`] checks a page against, and an
    /// `EntryId` is only unique *within* its own session -- a page from
    /// a superseded session could otherwise seat rows whose ids name
    /// entirely different entries in the session the tab now holds, and
    /// those ids are what an extract or a delete is addressed by.
    session: Option<ArchiveSessionId>,
    navigation: ArchiveNavigation,
    request: ListEntriesRequest,
    /// The rows the session last returned for `request.directory`, behind
    /// an `Arc` so a renderer reading this signal every frame clones a
    /// refcount rather than every row of the directory it is showing.
    rows: Option<Arc<EntryPage>>,
    status: RequestStatus,
    /// Which request the listing currently cares about -- bumped by
    /// [`Self::begin_loading`] (a new fetch) and by every successful
    /// navigation (whatever was in flight no longer answers what is
    /// being browsed). The reply-ordering guard [`Self::adopt_page`] and
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
            rows: None,
            status: RequestStatus::Idle,
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

    /// The request this tab's browser is showing the answer to -- what a
    /// caller hands `ArclainApp::list_entries` to refresh it.
    pub fn request(&self) -> &ListEntriesRequest {
        &self.request
    }

    /// What the latest listing request for this directory is doing.
    ///
    /// Orthogonal to whether rows are held -- a renderer must consult both
    /// before deciding what to draw. See [`RequestStatus`] for the full
    /// table of combinations.
    pub fn status(&self) -> &RequestStatus {
        &self.status
    }

    /// The rows the session last returned for the directory being browsed,
    /// or `None` when it has not returned any yet.
    ///
    /// `Some` with zero entries means the folder really is empty. A `None`
    /// here says only "no rows"; [`Self::status`] says *why*.
    pub fn page(&self) -> Option<&EntryPage> {
        self.rows.as_deref()
    }

    /// The error the last listing of this directory failed with, if it
    /// failed. Independent of whether rows are held: a refresh that fails
    /// over a directory already on screen reports both.
    pub fn failure(&self) -> Option<&ApplicationError> {
        match &self.status {
            RequestStatus::Failed(error) => Some(error),
            RequestStatus::Idle | RequestStatus::Loading => None,
        }
    }

    /// Whether a listing for the current request is in flight -- which says
    /// nothing about whether rows are on screen while it runs.
    pub fn is_loading(&self) -> bool {
        matches!(self.status, RequestStatus::Loading)
    }

    /// The current directory's rows -- empty until the session returns
    /// some.
    ///
    /// TRANSITIONAL(4c): an un-migrated consumer sees exactly what it saw
    /// when this was an `Option<EntryPage>`, which is what keeps its
    /// behavior unchanged. A consumer that *renders* a directory must not
    /// keep that reading: an empty slice here is not "this folder is
    /// empty" unless [`Self::page`] is also `Some`, and [`Self::status`]
    /// is what turns the other cases into a spinner or an error instead of
    /// a blank folder.
    pub fn entries(&self) -> &[ArchiveEntryDto] {
        match &self.rows {
            Some(page) => &page.entries,
            None => &[],
        }
    }

    /// Records that a listing for the current request is in flight,
    /// without touching the rows, and mints the [`ListingGeneration`]
    /// naming this attempt -- the token its eventual
    /// [`Self::adopt_page`]/[`Self::fail`] must present.
    ///
    /// Whether the previous answer stays on screen while it runs is the
    /// caller's choice, not this type's: rows and status are independent
    /// fields, so "refreshing, previous rows still shown" is a state the
    /// model can hold. Navigation is the one thing that clears rows on its
    /// own, because a new directory's listing has nothing to keep.
    pub fn begin_loading(&mut self) -> ListingGeneration {
        self.generation += 1;
        self.status = RequestStatus::Loading;
        ListingGeneration(self.generation)
    }

    /// Stores `page` as the answer to the request `generation` names.
    ///
    /// Refuses (reporting `false`) any page that cannot be the answer to
    /// what is being browsed now:
    ///
    /// * one whose `generation` a newer [`Self::begin_loading`] or a
    ///   navigation has superseded -- the late reply of an overtaken
    ///   request. Without this, a slow request's success could land
    ///   *after* a newer request already answered (silently replacing
    ///   fresh rows and erasing the newer request's own status), purely
    ///   on reply order;
    /// * one from a session this listing does not belong to -- a reply
    ///   for the archive the tab held *before* the current one, whose
    ///   `EntryId`s are meaningless (and actively dangerous, being
    ///   session-scoped) against the session it holds now. This guard is
    ///   also what makes the per-value generation counter sufficient:
    ///   a token minted against a previous binding cannot smuggle a
    ///   foreign page in through a numeric collision;
    /// * one listing a different directory, from an in-flight reply
    ///   racing the very navigation that superseded it;
    /// * one older than the rows already held, from a refresh overtaken
    ///   by a newer one that already answered.
    ///
    /// An accepted page also returns the status to
    /// [`RequestStatus::Idle`]: a successful listing supersedes whatever
    /// the previous attempt was doing, including a failure.
    pub fn adopt_page(&mut self, generation: ListingGeneration, page: EntryPage) -> bool {
        if generation.0 != self.generation {
            return false;
        }
        if self.session != Some(page.session_id) {
            return false;
        }
        if page.directory != self.request.directory {
            return false;
        }
        if let Some(held) = &self.rows {
            if page.revision < held.revision {
                return false;
            }
        }
        self.rows = Some(Arc::new(page));
        self.status = RequestStatus::Idle;
        true
    }

    /// Records that the listing attempt `generation` names -- made
    /// against `session_id` -- failed for `directory`, and reports
    /// whether that failure is about what is being browsed now.
    ///
    /// Refused (`false`) when a newer [`Self::begin_loading`] or a
    /// navigation has superseded `generation`, when `session_id` is not
    /// the session this listing is bound to (the same guard
    /// [`Self::adopt_page`] applies, for the same reason: after a rebind
    /// the fresh value's generation counter restarts, so a numerically
    /// colliding token from the previous binding must not let the old
    /// session's failure deface the new session's status), or when
    /// `directory` is no longer the one being browsed. The generation
    /// guard is what [`Self::adopt_page`] has always needed mirrored
    /// here: an `ApplicationError` carries no revision, so without a
    /// request identity a superseded request's late failure would mark
    /// rows a *newer* request just refreshed as failed -- and its
    /// mirror, a late failure erasing a newer request's genuine
    /// `Loading`, is the same hole from the other side.
    ///
    /// **Rows already held are kept.** They are the session's last
    /// successful answer for this exact directory, and replacing them with
    /// "contents unknown" because a *refresh* failed loses information the
    /// user can still act on -- while the failure itself is recorded in the
    /// status either way, so a renderer can mark the rows stale rather than
    /// having to choose between showing them and reporting the error.
    ///
    /// Keeping them is safe, not merely convenient: acting on a stale row
    /// cannot reach the wrong entry, because the `EntryId` it carries is
    /// validated against the owning session on every facade call that takes
    /// one. A superseded-revision id resolves to nothing rather than to
    /// some other entry, so the worst case is a refused operation, not a
    /// wrong-file delete.
    pub fn fail(
        &mut self,
        generation: ListingGeneration,
        session_id: ArchiveSessionId,
        directory: &ArchivePath,
        error: ApplicationError,
    ) -> bool {
        if generation.0 != self.generation {
            return false;
        }
        if self.session != Some(session_id) {
            return false;
        }
        if directory != &self.request.directory {
            return false;
        }
        self.status = RequestStatus::Failed(Arc::new(error));
        true
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
    /// new directory and discards whatever answered the old one -- rows,
    /// an in-flight marker, or a failure alike, since none of them say
    /// anything about the directory now being browsed. The generation
    /// advances too: a reply still in flight for the old directory is a
    /// superseded request's reply now, dropped by its own token rather
    /// than relying solely on the directory comparison (which a
    /// navigate-away-and-back would defeat).
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
        self.rows = None;
        self.status = RequestStatus::Idle;
        self.generation += 1;
        true
    }
}

#[cfg(test)]
#[path = "listing_tests.rs"]
mod tests;
