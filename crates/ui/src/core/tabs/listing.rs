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

use arclain_app::archive::{
    ArchiveEntryDto, ArchivePath, EntryPage, EntrySortKey, ListEntriesRequest, SortDirection,
};
use arclain_app::error::ApplicationError;
use arclain_app::ids::ArchiveSessionId;
use std::sync::Arc;

/// How many entries a directory-scoped listing asks for when the caller
/// wants the whole directory rather than one window of it.
///
/// The archive browser renders a complete directory today (its
/// virtualization is renderer-side, over an already-materialized row
/// list), and a directory's own entry count has no upper bound worth
/// naming more precisely than "effectively unbounded". The `offset`/
/// `limit` pair on [`TabListing`]'s request becomes a real window when
/// the browser panel starts paging through it.
pub const ALL_ENTRIES_IN_ONE_DIRECTORY: u32 = u32::MAX;

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

/// What the session has said about the directory a tab is browsing.
///
/// Four distinct states rather than one `Option<EntryPage>`, because
/// "there are no rows to show" has four different causes and only one of
/// them means the folder is empty. Collapsed into an `Option`, a listing
/// that *failed* is indistinguishable from a directory that is genuinely
/// empty -- so a backend error, a wrong password, or a revoked session
/// would render as a normal empty folder, with nothing anywhere in the
/// render tree able to tell the difference. That is the silent-empty-view
/// failure mode, arriving by construction rather than by accident.
///
/// Kept explicit now, while nothing renders this yet: the alternative is
/// discovering it after the render tree is built on top of an `Option`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PageState {
    /// Nothing has been asked. No archive open, or navigation just moved
    /// and the request for the new directory has not been sent yet.
    #[default]
    Absent,
    /// A listing for the current request is in flight.
    Loading,
    /// The session answered. Zero rows here means the directory really is
    /// empty -- which is exactly what [`Self::Failed`] does not mean.
    Loaded(Arc<EntryPage>),
    /// The session refused. The directory's contents are *unknown*, not
    /// empty. Behind an `Arc` for the same per-frame-clone reason
    /// [`Self::Loaded`] is.
    Failed(Arc<ApplicationError>),
}

/// One tab's archive listing: which session it is listing, where it is
/// browsing, the request that describes what its browser is showing, and
/// what that session said in reply.
///
/// `request.directory` always equals `navigation.current()` -- callers
/// change it by navigating, never by editing the request -- and every
/// navigation returns the page state to [`PageState::Absent`], because a
/// reply answers the directory it was requested for and nothing else.
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
    page: PageState,
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
    pub fn for_session(session: Option<ArchiveSessionId>) -> Self {
        Self {
            session,
            navigation: ArchiveNavigation::default(),
            request: Self::whole_directory_request(ArchivePath::root()),
            page: PageState::Absent,
        }
    }

    /// The request for everything in one directory, in the order and
    /// scope the archive browser has always shown a freshly-opened
    /// archive in: name-ascending, unfiltered, from the first row, with
    /// no cap.
    ///
    /// The one place that shape is written down. Every caller that
    /// resolves a selection of paths back to `EntryId`s needs exactly it
    /// (extraction, add/replace-text, delete), and each used to spell it
    /// out itself -- including one that capped `limit` at a literal
    /// `100_000` and therefore dropped every selected row past that in a
    /// larger directory.
    ///
    /// Sorting, filtering, and paging still happen renderer-side today
    /// (over `TabState::browser_entries`); moving each onto these fields
    /// is what lets the session do that work instead.
    pub fn whole_directory_request(directory: ArchivePath) -> ListEntriesRequest {
        ListEntriesRequest {
            directory,
            sort_key: EntrySortKey::Name,
            sort_direction: SortDirection::Ascending,
            name_filter: None,
            offset: 0,
            limit: ALL_ENTRIES_IN_ONE_DIRECTORY,
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

    /// What the session has said about the directory being browsed.
    ///
    /// The accessor a renderer must consult before treating an empty row
    /// list as an empty folder -- see [`PageState`].
    pub fn page_state(&self) -> &PageState {
        &self.page
    }

    /// The page the session answered with, or `None` when it has not
    /// answered (nothing asked yet, a listing in flight, or a listing that
    /// failed -- [`Self::page_state`] distinguishes those).
    pub fn page(&self) -> Option<&EntryPage> {
        match &self.page {
            PageState::Loaded(page) => Some(page),
            PageState::Absent | PageState::Loading | PageState::Failed(_) => None,
        }
    }

    /// Why the current directory has no rows, when the reason is that
    /// listing it failed.
    pub fn failure(&self) -> Option<&ApplicationError> {
        match &self.page {
            PageState::Failed(error) => Some(error),
            PageState::Absent | PageState::Loading | PageState::Loaded(_) => None,
        }
    }

    /// Whether a listing for the current request is in flight.
    pub fn is_loading(&self) -> bool {
        matches!(self.page, PageState::Loading)
    }

    /// The current directory's rows -- empty unless the session has
    /// actually answered.
    ///
    /// TRANSITIONAL(4c): an un-migrated consumer sees exactly what it saw
    /// when this was an `Option<EntryPage>` -- an empty slice for every
    /// state but `Loaded` -- which is what keeps its behavior unchanged.
    /// A consumer that *renders* a directory must not keep that reading:
    /// ask [`Self::page_state`], so a failed listing shows an error rather
    /// than an empty folder.
    pub fn entries(&self) -> &[ArchiveEntryDto] {
        match &self.page {
            PageState::Loaded(page) => &page.entries,
            PageState::Absent | PageState::Loading | PageState::Failed(_) => &[],
        }
    }

    /// Records that a listing for the current request is in flight.
    ///
    /// A caller refreshing a directory whose rows are already on screen
    /// simply does not call this: [`Self::adopt_page`] swaps the rows in
    /// place, so the choice between "blank the list while reloading" and
    /// "keep showing the previous answer" stays with the caller rather
    /// than being baked in here.
    pub fn begin_loading(&mut self) {
        self.page = PageState::Loading;
    }

    /// Stores `page` as the answer to the current request.
    ///
    /// Refuses (reporting `false`) any page that cannot be the answer to
    /// what is being browsed now:
    ///
    /// * one from a session this listing does not belong to -- a reply
    ///   for the archive the tab held *before* the current one, whose
    ///   `EntryId`s are meaningless (and actively dangerous, being
    ///   session-scoped) against the session it holds now;
    /// * one listing a different directory, from an in-flight request
    ///   whose reply lands after navigation moved on;
    /// * one older than a page already loaded, from a refresh overtaken by
    ///   a newer one.
    ///
    /// Without these the browser would flip to a stale archive,
    /// directory, or revision purely on reply order.
    pub fn adopt_page(&mut self, page: EntryPage) -> bool {
        if self.session != Some(page.session_id) {
            return false;
        }
        if page.directory != self.request.directory {
            return false;
        }
        if let PageState::Loaded(held) = &self.page {
            if page.revision < held.revision {
                return false;
            }
        }
        self.page = PageState::Loaded(Arc::new(page));
        true
    }

    /// Records that listing `directory` failed.
    ///
    /// `directory` is what the failed request asked for, so a reply
    /// landing after navigation moved on is refused the same way
    /// [`Self::adopt_page`] refuses one.
    ///
    /// Also refused once rows are already loaded for that directory: they
    /// are the session's last successful answer for it, and replacing
    /// them with "contents unknown" because a *refresh* failed loses
    /// information the user can still act on. A failed refresh reaches the
    /// user through the status bar, like every other operation failure.
    pub fn fail(&mut self, directory: &ArchivePath, error: ApplicationError) -> bool {
        if directory != &self.request.directory {
            return false;
        }
        if matches!(self.page, PageState::Loaded(_)) {
            return false;
        }
        self.page = PageState::Failed(Arc::new(error));
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
    /// anything about the directory now being browsed.
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
        self.page = PageState::Absent;
        true
    }
}

#[cfg(test)]
#[path = "listing_tests.rs"]
mod tests;
