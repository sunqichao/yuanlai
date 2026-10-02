use std::collections::BTreeMap;

use yuanlai_codec::Encode;
use yuanlai_crypto::merkle_root;
use yuanlai_types::params::{
    CANCEL_FEE_PERCENT, CANCEL_MIN_FEE, COST_COURSE_BASE, COST_PER_FILE, COST_PER_GIB,
    COST_UPDATE_BASE, GIB, LOCK_HEADROOM_PERCENT, SESSION_IDLE_BLOCKS, SESSION_MAX_BLOCKS,
};
use yuanlai_types::{
    AutoCancelRecord, Course, CoursePath, CourseTitle, CourseVersion, Credit, FileEntry, Hash,
    Height, PubKey,
};

use crate::journal::{Change, Journal, replay};

/// An open course write session (`spec/05-course-session.md` §2). Files are stored separately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// `BeginCourse` txid.
    pub id: Hash,
    /// Owner.
    pub owner: PubKey,
    /// Root of the previous version, if this is an update.
    pub parent_root: Option<Hash>,
    /// Title.
    pub title: CourseTitle,
    /// Version label.
    pub version: CourseVersion,
    /// Declared (changed) files.
    pub declared_files: u32,
    /// Declared (changed) bytes.
    pub declared_bytes: u64,
    /// Locked credit.
    pub locked: Credit,
    /// Maximum hops of the locked credit.
    pub max_hops: u8,
    /// Maximum renewals of the locked credit.
    pub max_renewals: u8,
    /// Files appended so far.
    pub file_count: u32,
    /// Total size of appended files.
    pub total_size: u64,
    /// Height of `BeginCourse`.
    pub started_height: Height,
    /// Height of the last `BeginCourse`, `AppendFiles` or `AddLock`.
    pub last_activity_height: Height,
}

impl Session {
    /// `min(started + SESSION_MAX_BLOCKS, last_activity + SESSION_IDLE_BLOCKS)`.
    #[must_use]
    pub fn deadline(&self) -> Height {
        Height(
            self.started_height
                .0
                .saturating_add(SESSION_MAX_BLOCKS)
                .min(
                    self.last_activity_height
                        .0
                        .saturating_add(SESSION_IDLE_BLOCKS),
                ),
        )
    }

    /// Estimated cost from the declared figures.
    #[must_use]
    pub fn estimate(&self) -> Option<Credit> {
        course_cost(
            self.parent_root.is_some(),
            u64::from(self.declared_files),
            self.declared_bytes,
        )
    }
}

/// `⌈bytes / GIB⌉`.
#[must_use]
pub const fn gib_units(bytes: u64) -> u64 {
    bytes.div_ceil(GIB)
}

/// Course cost (`spec/05-course-session.md` §3); `files` and `bytes` count changed files for an
/// update.
#[must_use]
pub fn course_cost(is_update: bool, files: u64, bytes: u64) -> Option<Credit> {
    let base = if is_update {
        COST_UPDATE_BASE
    } else {
        COST_COURSE_BASE
    };
    let per_file = files.checked_mul(COST_PER_FILE.0)?;
    let per_gib = gib_units(bytes).checked_mul(COST_PER_GIB.0)?;
    base.0
        .checked_add(per_file)?
        .checked_add(per_gib)
        .map(Credit)
}

/// `[estimate, estimate + ⌊estimate × LOCK_HEADROOM_PERCENT / 100⌋]`.
#[must_use]
pub fn lock_bounds(estimate: Credit) -> Option<(Credit, Credit)> {
    let headroom = estimate
        .0
        .checked_mul(LOCK_HEADROOM_PERCENT)?
        .checked_div(100)?;
    Some((estimate, Credit(estimate.0.checked_add(headroom)?)))
}

/// `min(locked, max(CANCEL_MIN_FEE, ⌊locked × CANCEL_FEE_PERCENT / 100⌋))`.
#[must_use]
pub fn cancel_fee(locked: Credit) -> Option<Credit> {
    let proportional = locked.0.checked_mul(CANCEL_FEE_PERCENT)?.checked_div(100)?;
    Some(locked.min(CANCEL_MIN_FEE.max(Credit(proportional))))
}

/// A path rule violated (`spec/05-course-session.md` §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathError {
    /// CRS-PATH-01
    Shape,
    /// CRS-PATH-02
    DotSegment,
    /// CRS-PATH-03
    Backslash,
}

/// Checks CRS-PATH-01 – CRS-PATH-03.
///
/// # Errors
///
/// Returns the first violated rule.
pub fn check_path(path: &str) -> Result<(), PathError> {
    if path.contains('\\') {
        return Err(PathError::Backslash);
    }
    // Leading, trailing and doubled separators all produce an empty segment, as does "".
    for segment in path.split('/') {
        if segment.is_empty() {
            return Err(PathError::Shape);
        }
        if segment == "." || segment == ".." {
            return Err(PathError::DotSegment);
        }
    }
    Ok(())
}

/// `MTH([FileEntry encoding, …])` over entries already sorted by path bytes.
#[must_use]
pub fn course_root<'a>(sorted: impl IntoIterator<Item = &'a FileEntry>) -> Hash {
    let leaves: Vec<Vec<u8>> = sorted.into_iter().map(Encode::encode).collect();
    merkle_root(&leaves)
}

/// Course and session state. Registration indexes are keyed so that iteration order is the
/// total order `(height, position)` required by INV-12.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[expect(
    clippy::zero_sized_map_values,
    reason = "sets are unit-valued maps so that one Journal type records undo for maps and sets"
)]
pub struct CourseState {
    pub(crate) sessions: BTreeMap<Hash, Session>,
    pub(crate) open_by_owner: BTreeMap<PubKey, Hash>,
    pub(crate) deadlines: BTreeMap<(Height, Hash), ()>,
    pub(crate) session_files: BTreeMap<(Hash, CoursePath), FileEntry>,
    pub(crate) session_contents: BTreeMap<(Hash, Hash), ()>,
    pub(crate) courses: BTreeMap<Hash, Course>,
    pub(crate) course_files: BTreeMap<(Hash, CoursePath), FileEntry>,
    /// `(owner, root)` → course ID (CRS-FIN-05).
    pub(crate) owner_roots: BTreeMap<(PubKey, Hash), Hash>,
    /// `(owner, parent root)` → the course that superseded it.
    pub(crate) children: BTreeMap<(PubKey, Hash), Hash>,
    pub(crate) root_index: BTreeMap<(Hash, Height, u32), Hash>,
    pub(crate) content_index: BTreeMap<(Hash, Height, u32), Hash>,
    /// Manual cancellations per `(owner, height)`.
    pub(crate) cancels: BTreeMap<(PubKey, Height), u32>,
}

/// One registration of a course root or file content, in time order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Registration {
    /// Height of `FinalizeCourse`.
    pub height: Height,
    /// Its position in the block.
    pub position: u32,
    /// The course.
    pub course_id: Hash,
}

impl CourseState {
    /// A session by ID.
    #[must_use]
    pub fn session(&self, id: &Hash) -> Option<&Session> {
        self.sessions.get(id)
    }

    /// The open session of `owner`.
    #[must_use]
    pub fn open_session(&self, owner: &PubKey) -> Option<&Session> {
        self.open_by_owner
            .get(owner)
            .and_then(|id| self.sessions.get(id))
    }

    /// All open sessions.
    pub fn sessions(&self) -> impl Iterator<Item = &Session> {
        self.sessions.values()
    }

    /// Files appended to a session, sorted by path.
    #[must_use]
    pub fn session_files(&self, id: &Hash) -> Vec<&FileEntry> {
        files_of(&self.session_files, id)
    }

    /// A course by ID.
    #[must_use]
    pub fn course(&self, id: &Hash) -> Option<&Course> {
        self.courses.get(id)
    }

    /// Files of a course, sorted by path.
    #[must_use]
    pub fn course_files(&self, id: &Hash) -> Vec<&FileEntry> {
        files_of(&self.course_files, id)
    }

    /// The course `owner` registered with `root`.
    #[must_use]
    pub fn course_by_owner_root(&self, owner: &PubKey, root: &Hash) -> Option<&Course> {
        self.owner_roots
            .get(&(*owner, *root))
            .and_then(|id| self.courses.get(id))
    }

    /// The newer version that superseded `owner`'s course `root`, if any.
    #[must_use]
    pub fn successor(&self, owner: &PubKey, root: &Hash) -> Option<&Course> {
        self.children
            .get(&(*owner, *root))
            .and_then(|id| self.courses.get(id))
    }

    /// Every registration of a course root, earliest first.
    #[must_use]
    pub fn registrations_of_root(&self, root: &Hash) -> Vec<Registration> {
        registrations(&self.root_index, root)
    }

    /// Every registration of a file content hash, earliest first.
    #[must_use]
    pub fn registrations_of_content(&self, content_hash: &Hash) -> Vec<Registration> {
        registrations(&self.content_index, content_hash)
    }
}

fn files_of<'a>(map: &'a BTreeMap<(Hash, CoursePath), FileEntry>, id: &Hash) -> Vec<&'a FileEntry> {
    map.range((*id, CoursePath::empty())..)
        .take_while(|((owner_id, _), _)| owner_id == id)
        .map(|(_, entry)| entry)
        .collect()
}

fn registrations(index: &BTreeMap<(Hash, Height, u32), Hash>, key: &Hash) -> Vec<Registration> {
    index
        .range((*key, Height(0), 0)..)
        .take_while(|((k, _, _), _)| k == key)
        .map(|((_, height, position), course_id)| Registration {
            height: *height,
            position: *position,
            course_id: *course_id,
        })
        .collect()
}

/// Journals over every [`CourseState`] map.
pub(crate) struct CourseJournal<'a> {
    pub(crate) sessions: Journal<'a, Hash, Session>,
    pub(crate) open_by_owner: Journal<'a, PubKey, Hash>,
    pub(crate) deadlines: Journal<'a, (Height, Hash), ()>,
    pub(crate) session_files: Journal<'a, (Hash, CoursePath), FileEntry>,
    pub(crate) session_contents: Journal<'a, (Hash, Hash), ()>,
    pub(crate) courses: Journal<'a, Hash, Course>,
    pub(crate) course_files: Journal<'a, (Hash, CoursePath), FileEntry>,
    pub(crate) owner_roots: Journal<'a, (PubKey, Hash), Hash>,
    pub(crate) children: Journal<'a, (PubKey, Hash), Hash>,
    pub(crate) root_index: Journal<'a, (Hash, Height, u32), Hash>,
    pub(crate) content_index: Journal<'a, (Hash, Height, u32), Hash>,
    pub(crate) cancels: Journal<'a, (PubKey, Height), u32>,
}

/// Recorded changes to [`CourseState`] made by one block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[expect(
    missing_docs,
    reason = "each field mirrors the CourseState map of the same name"
)]
pub struct CourseDelta {
    pub sessions: Vec<Change<Hash, Session>>,
    pub open_by_owner: Vec<Change<PubKey, Hash>>,
    pub deadlines: Vec<Change<(Height, Hash), ()>>,
    pub session_files: Vec<Change<(Hash, CoursePath), FileEntry>>,
    pub session_contents: Vec<Change<(Hash, Hash), ()>>,
    pub courses: Vec<Change<Hash, Course>>,
    pub course_files: Vec<Change<(Hash, CoursePath), FileEntry>>,
    pub owner_roots: Vec<Change<(PubKey, Hash), Hash>>,
    pub children: Vec<Change<(PubKey, Hash), Hash>>,
    pub root_index: Vec<Change<(Hash, Height, u32), Hash>>,
    pub content_index: Vec<Change<(Hash, Height, u32), Hash>>,
    pub cancels: Vec<Change<(PubKey, Height), u32>>,
    /// Sessions ended by `AutoCancel` in this block, kept for queries.
    pub auto_cancelled: Vec<AutoCancelRecord>,
}

macro_rules! course_maps {
    ($m:ident) => {
        $m!(
            sessions,
            open_by_owner,
            deadlines,
            session_files,
            session_contents,
            courses,
            course_files,
            owner_roots,
            children,
            root_index,
            content_index,
            cancels
        )
    };
}

impl<'a> CourseJournal<'a> {
    pub(crate) fn new(base: &'a CourseState) -> Self {
        macro_rules! build {
            ($($f:ident),*) => { Self { $($f: Journal::new(&base.$f),)* } };
        }
        course_maps!(build)
    }

    pub(crate) fn finish(self, auto_cancelled: Vec<AutoCancelRecord>) -> CourseDelta {
        macro_rules! build {
            ($($f:ident),*) => { CourseDelta { $($f: self.$f.finish(),)* auto_cancelled } };
        }
        course_maps!(build)
    }
}

impl CourseState {
    pub(crate) fn replay(&mut self, delta: &CourseDelta, forward: bool) {
        macro_rules! apply {
            ($($f:ident),*) => { $(replay(&mut self.$f, &delta.$f, forward);)* };
        }
        course_maps!(apply);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_rules() {
        for ok in [
            "a",
            "a/b",
            "第1章/视频.mp4",
            "a b/c.d",
            ".hidden",
            "a/..b",
            "...",
        ] {
            assert_eq!(check_path(ok), Ok(()), "{ok}");
        }
        let bad = [
            ("", PathError::Shape),
            ("/a", PathError::Shape),
            ("a/", PathError::Shape),
            ("a//b", PathError::Shape),
            (".", PathError::DotSegment),
            ("a/../b", PathError::DotSegment),
            ("a/./b", PathError::DotSegment),
            ("a\\b", PathError::Backslash),
        ];
        for (path, err) in bad {
            assert_eq!(check_path(path), Err(err), "{path}");
        }
    }

    #[test]
    fn pricing() {
        assert_eq!(gib_units(0), 0);
        assert_eq!(gib_units(1), 1);
        assert_eq!(gib_units(GIB), 1);
        assert_eq!(gib_units(GIB + 1), 2);
        assert_eq!(course_cost(false, 10, 2 * GIB), Some(Credit(50)));
        assert_eq!(course_cost(false, 40, 12 * GIB), Some(Credit(130)));
        assert_eq!(course_cost(true, 3, 1), Some(Credit(23)));
        assert_eq!(lock_bounds(Credit(50)), Some((Credit(50), Credit(60))));
        assert_eq!(lock_bounds(Credit(34)), Some((Credit(34), Credit(40))));
        assert_eq!(cancel_fee(Credit(1)), Some(Credit(1)));
        assert_eq!(cancel_fee(Credit(30)), Some(Credit(2)));
        assert_eq!(cancel_fee(Credit(60)), Some(Credit(3)));
        assert_eq!(course_cost(false, u64::MAX, 0), None);
    }
}
