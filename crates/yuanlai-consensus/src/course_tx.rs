use core::fmt;
use std::collections::BTreeSet;

use yuanlai_types::params::{
    CANCEL_LIMIT, CANCEL_LIMIT_WINDOW, EXPIRY_BLOCKS, MAX_FILES_PER_COURSE, MAX_RENEWALS,
};
use yuanlai_types::{
    AddLock, AppendFiles, BeginCourse, Course, CoursePath, Credit, FileEntry, Hash, Height,
    OutPoint, PubKey, SessionRef, Source, UserTx, Utxo,
};

use crate::course::{
    PathError, Session, cancel_fee, check_path, course_cost, course_root, lock_bounds,
};
use crate::delta::Overlay;
use crate::state::window_start;
use crate::user_tx::{TxError, burn, consume_inputs};

/// A course rule violated (`spec/05-course-session.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CourseError {
    /// CRS-BEGIN-01
    NoIdentity,
    /// CRS-BEGIN-02
    SessionAlreadyOpen,
    /// CRS-BEGIN-03
    BeginDeclaredFiles,
    /// CRS-BEGIN-04
    BeginLockBounds,
    /// CRS-BEGIN-05
    InvalidParent,
    /// CRS-BEGIN-06
    BeginRenewals,
    /// CRS-APPEND-01
    AppendSession,
    /// CRS-PATH-01 – CRS-PATH-03 (via CRS-APPEND-02)
    Path(PathError),
    /// CRS-APPEND-03
    DuplicatePath,
    /// CRS-APPEND-04
    DuplicateContent,
    /// CRS-APPEND-05
    TooManyFiles,
    /// CRS-APPEND-06
    EmptyAppend,
    /// CRS-APPEND-07
    SizeOverflow,
    /// CRS-ADDLOCK-01
    AddLockSession,
    /// CRS-ADDLOCK-02
    AddLockDeclaredFiles,
    /// CRS-ADDLOCK-03
    AddLockBounds,
    /// CRS-ADDLOCK-04
    AddLockRenewals,
    /// CRS-ADDLOCK-05
    AddLockZero,
    /// CRS-FIN-01
    FinalizeSession,
    /// CRS-FIN-02
    NoFiles,
    /// CRS-FIN-03
    ParentNotHeadOrUnchanged,
    /// CRS-FIN-04
    InsufficientLock,
    /// CRS-FIN-05
    DuplicateRoot,
    /// CRS-CANCEL-01
    CancelSession,
    /// CRS-CANCEL-02
    CancelLimit,
}

impl CourseError {
    /// The spec rule identifier.
    #[must_use]
    pub const fn rule_id(&self) -> &'static str {
        match self {
            Self::NoIdentity => "CRS-BEGIN-01",
            Self::SessionAlreadyOpen => "CRS-BEGIN-02",
            Self::BeginDeclaredFiles => "CRS-BEGIN-03",
            Self::BeginLockBounds => "CRS-BEGIN-04",
            Self::InvalidParent => "CRS-BEGIN-05",
            Self::BeginRenewals => "CRS-BEGIN-06",
            Self::AppendSession => "CRS-APPEND-01",
            Self::Path(PathError::Shape) => "CRS-PATH-01",
            Self::Path(PathError::DotSegment) => "CRS-PATH-02",
            Self::Path(PathError::Backslash) => "CRS-PATH-03",
            Self::DuplicatePath => "CRS-APPEND-03",
            Self::DuplicateContent => "CRS-APPEND-04",
            Self::TooManyFiles => "CRS-APPEND-05",
            Self::EmptyAppend => "CRS-APPEND-06",
            Self::SizeOverflow => "CRS-APPEND-07",
            Self::AddLockSession => "CRS-ADDLOCK-01",
            Self::AddLockDeclaredFiles => "CRS-ADDLOCK-02",
            Self::AddLockBounds => "CRS-ADDLOCK-03",
            Self::AddLockRenewals => "CRS-ADDLOCK-04",
            Self::AddLockZero => "CRS-ADDLOCK-05",
            Self::FinalizeSession => "CRS-FIN-01",
            Self::NoFiles => "CRS-FIN-02",
            Self::ParentNotHeadOrUnchanged => "CRS-FIN-03",
            Self::InsufficientLock => "CRS-FIN-04",
            Self::DuplicateRoot => "CRS-FIN-05",
            Self::CancelSession => "CRS-CANCEL-01",
            Self::CancelLimit => "CRS-CANCEL-02",
        }
    }
}

impl fmt::Display for CourseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {self:?}", self.rule_id())
    }
}

impl From<CourseError> for TxError {
    fn from(e: CourseError) -> Self {
        Self::Course(e)
    }
}

fn session_of(
    ov: &Overlay<'_>,
    id: &Hash,
    sender: PubKey,
    err: CourseError,
) -> Result<Session, TxError> {
    ov.courses
        .sessions
        .get(id)
        .filter(|s| s.owner == sender)
        .cloned()
        .ok_or(TxError::Course(err))
}

fn in_lock_bounds(is_update: bool, files: u32, bytes: u64, locked: Credit) -> bool {
    course_cost(is_update, u64::from(files), bytes)
        .and_then(lock_bounds)
        .is_some_and(|(lo, hi)| lo <= locked && locked <= hi)
}

fn store_session(ov: &mut Overlay<'_>, before: Option<&Session>, after: Session) {
    if let Some(old) = before {
        ov.courses.deadlines.remove((old.deadline(), old.id));
    }
    ov.courses
        .deadlines
        .insert((after.deadline(), after.id), ());
    ov.courses.sessions.insert(after.id, after);
}

fn session_file_keys(ov: &Overlay<'_>, id: Hash) -> Vec<((Hash, CoursePath), FileEntry)> {
    ov.courses
        .session_files
        .scan(&(id, CoursePath::empty()), |(s, _)| *s == id)
}

/// Removes a session and all its indexes; its files are returned in path order.
fn remove_session(ov: &mut Overlay<'_>, session: &Session) -> Vec<FileEntry> {
    let files = session_file_keys(ov, session.id);
    for (key, entry) in &files {
        ov.courses.session_files.remove(key.clone());
        ov.courses
            .session_contents
            .remove((session.id, entry.content_hash));
    }
    ov.courses.sessions.remove(session.id);
    ov.courses.open_by_owner.remove(session.owner);
    ov.courses
        .deadlines
        .remove((session.deadline(), session.id));
    files.into_iter().map(|(_, entry)| entry).collect()
}

fn renewed_utxo(
    session: &Session,
    amount: Credit,
    source: Source,
    height: Height,
) -> Result<Utxo, TxError> {
    Ok(Utxo {
        owner: session.owner,
        amount,
        source,
        acquired_height: height,
        spendable_from: height,
        expire_height: height.checked_add(EXPIRY_BLOCKS).ok_or(TxError::Overflow)?,
        hops: session.max_hops,
        renewals: session
            .max_renewals
            .checked_add(1)
            .ok_or(TxError::Overflow)?,
    })
}

/// Ends a session by cancellation (manual or automatic): burns the fee and restores the rest as
/// output `restored_at`. Returns `(fee, restored)`.
pub(crate) fn end_session(
    ov: &mut Overlay<'_>,
    session: &Session,
    restored_at: OutPoint,
    height: Height,
) -> Result<(Credit, Credit), TxError> {
    let fee = cancel_fee(session.locked).ok_or(TxError::Overflow)?;
    let restored = session.locked.checked_sub(fee).ok_or(TxError::Overflow)?;
    ov.unlock(session.owner, session.locked)?;
    burn(ov, fee)?;
    if restored > Credit::ZERO {
        let utxo = renewed_utxo(session, restored, Source::Cancelled, height)?;
        ov.create(restored_at, utxo)?;
    }
    remove_session(ov, session);
    Ok((fee, restored))
}

pub(crate) fn apply_begin(
    ov: &mut Overlay<'_>,
    tx: &UserTx<BeginCourse>,
    txid: Hash,
    height: Height,
) -> Result<(), TxError> {
    let sender = tx.sender;
    let b = &tx.body;
    if ov.identity(&sender).is_none() {
        return Err(CourseError::NoIdentity.into());
    }
    if ov.courses.open_by_owner.contains(&sender) {
        return Err(CourseError::SessionAlreadyOpen.into());
    }
    if !(1..=MAX_FILES_PER_COURSE).contains(&b.declared_files) {
        return Err(CourseError::BeginDeclaredFiles.into());
    }
    if !in_lock_bounds(
        b.parent_root.is_some(),
        b.declared_files,
        b.declared_bytes,
        b.lock,
    ) {
        return Err(CourseError::BeginLockBounds.into());
    }
    if let Some(parent) = b.parent_root {
        let owned = ov.courses.owner_roots.contains(&(sender, parent));
        let superseded = ov.courses.children.contains(&(sender, parent));
        if !owned || superseded {
            return Err(CourseError::InvalidParent.into());
        }
    }
    let consumed = consume_inputs(ov, sender, b.inputs.as_slice(), b.lock, height, txid, 0)?;
    if consumed.renewals >= MAX_RENEWALS {
        return Err(CourseError::BeginRenewals.into());
    }
    ov.lock(sender, b.lock)?;
    let session = Session {
        id: txid,
        owner: sender,
        parent_root: b.parent_root,
        title: b.title.clone(),
        version: b.version.clone(),
        declared_files: b.declared_files,
        declared_bytes: b.declared_bytes,
        locked: b.lock,
        max_hops: consumed.hops,
        max_renewals: consumed.renewals,
        file_count: 0,
        total_size: 0,
        started_height: height,
        last_activity_height: height,
    };
    ov.courses.open_by_owner.insert(sender, txid);
    store_session(ov, None, session);
    Ok(())
}

pub(crate) fn apply_append(
    ov: &mut Overlay<'_>,
    tx: &UserTx<AppendFiles>,
    height: Height,
) -> Result<(), TxError> {
    let b = &tx.body;
    let session = session_of(ov, &b.session_id, tx.sender, CourseError::AppendSession)?;
    if b.entries.is_empty() {
        return Err(CourseError::EmptyAppend.into());
    }
    let mut paths = BTreeSet::new();
    let mut contents = BTreeSet::new();
    let mut total = session.total_size;
    for entry in &b.entries {
        check_path(entry.path.as_str()).map_err(CourseError::Path)?;
        if !paths.insert(&entry.path)
            || ov
                .courses
                .session_files
                .contains(&(session.id, entry.path.clone()))
        {
            return Err(CourseError::DuplicatePath.into());
        }
        if !contents.insert(entry.content_hash)
            || ov
                .courses
                .session_contents
                .contains(&(session.id, entry.content_hash))
        {
            return Err(CourseError::DuplicateContent.into());
        }
        total = total
            .checked_add(entry.size)
            .ok_or(CourseError::SizeOverflow)?;
    }
    let file_count = session
        .file_count
        .checked_add(b.entries.len())
        .filter(|n| *n <= MAX_FILES_PER_COURSE)
        .ok_or(CourseError::TooManyFiles)?;

    for entry in &b.entries {
        ov.courses
            .session_files
            .insert((session.id, entry.path.clone()), entry.clone());
        ov.courses
            .session_contents
            .insert((session.id, entry.content_hash), ());
    }
    let updated = Session {
        file_count,
        total_size: total,
        last_activity_height: height,
        ..session.clone()
    };
    store_session(ov, Some(&session), updated);
    Ok(())
}

pub(crate) fn apply_add_lock(
    ov: &mut Overlay<'_>,
    tx: &UserTx<AddLock>,
    txid: Hash,
    height: Height,
) -> Result<(), TxError> {
    let b = &tx.body;
    let session = session_of(ov, &b.session_id, tx.sender, CourseError::AddLockSession)?;
    if !(1..=MAX_FILES_PER_COURSE).contains(&b.declared_files) {
        return Err(CourseError::AddLockDeclaredFiles.into());
    }
    if b.amount == Credit::ZERO {
        return Err(CourseError::AddLockZero.into());
    }
    let locked = session
        .locked
        .checked_add(b.amount)
        .ok_or(TxError::Overflow)?;
    let is_update = session.parent_root.is_some();
    if !in_lock_bounds(is_update, b.declared_files, b.declared_bytes, locked) {
        return Err(CourseError::AddLockBounds.into());
    }
    let consumed = consume_inputs(
        ov,
        tx.sender,
        b.inputs.as_slice(),
        b.amount,
        height,
        txid,
        0,
    )?;
    if consumed.renewals >= MAX_RENEWALS {
        return Err(CourseError::AddLockRenewals.into());
    }
    ov.lock(tx.sender, b.amount)?;
    let updated = Session {
        declared_files: b.declared_files,
        declared_bytes: b.declared_bytes,
        locked,
        max_hops: session.max_hops.max(consumed.hops),
        max_renewals: session.max_renewals.max(consumed.renewals),
        last_activity_height: height,
        ..session.clone()
    };
    store_session(ov, Some(&session), updated);
    Ok(())
}

pub(crate) fn apply_finalize(
    ov: &mut Overlay<'_>,
    tx: &UserTx<SessionRef>,
    txid: Hash,
    height: Height,
    position: u32,
) -> Result<(), TxError> {
    let sender = tx.sender;
    let session = session_of(
        ov,
        &tx.body.session_id,
        sender,
        CourseError::FinalizeSession,
    )?;
    if session.file_count == 0 {
        return Err(CourseError::NoFiles.into());
    }
    let files: Vec<FileEntry> = session_file_keys(ov, session.id)
        .into_iter()
        .map(|(_, e)| e)
        .collect();

    let (changed_files, changed_bytes) = match session.parent_root {
        None => (u64::from(session.file_count), session.total_size),
        Some(parent) => {
            let parent_id = ov
                .courses
                .owner_roots
                .get(&(sender, parent))
                .copied()
                .ok_or(CourseError::ParentNotHeadOrUnchanged)?;
            if ov.courses.children.contains(&(sender, parent)) {
                return Err(CourseError::ParentNotHeadOrUnchanged.into());
            }
            let parent_contents: BTreeSet<Hash> = ov
                .courses
                .course_files
                .scan(&(parent_id, CoursePath::empty()), |(c, _)| *c == parent_id)
                .into_iter()
                .map(|(_, e)| e.content_hash)
                .collect();
            let changed: Vec<&FileEntry> = files
                .iter()
                .filter(|f| !parent_contents.contains(&f.content_hash))
                .collect();
            if changed.is_empty() {
                return Err(CourseError::ParentNotHeadOrUnchanged.into());
            }
            let bytes = changed
                .iter()
                .try_fold(0_u64, |acc, f| acc.checked_add(f.size))
                .ok_or(TxError::Overflow)?;
            (
                u64::try_from(changed.len()).map_err(|_| TxError::Overflow)?,
                bytes,
            )
        }
    };
    let actual = course_cost(session.parent_root.is_some(), changed_files, changed_bytes)
        .ok_or(TxError::Overflow)?;
    if actual > session.locked {
        return Err(CourseError::InsufficientLock.into());
    }
    let root = course_root(&files);
    if ov.courses.owner_roots.contains(&(sender, root)) {
        return Err(CourseError::DuplicateRoot.into());
    }

    ov.unlock(sender, session.locked)?;
    burn(ov, actual)?;
    let released = session
        .locked
        .checked_sub(actual)
        .ok_or(TxError::Overflow)?;
    if released > Credit::ZERO {
        let utxo = renewed_utxo(&session, released, Source::Released, height)?;
        ov.create(OutPoint { txid, index: 0 }, utxo)?;
    }

    let id = session.id;
    for entry in remove_session(ov, &session) {
        ov.courses
            .content_index
            .insert((entry.content_hash, height, position), id);
        ov.courses
            .course_files
            .insert((id, entry.path.clone()), entry);
    }
    ov.courses.root_index.insert((root, height, position), id);
    ov.courses.owner_roots.insert((sender, root), id);
    if let Some(parent) = session.parent_root {
        ov.courses.children.insert((sender, parent), id);
    }
    ov.courses.courses.insert(
        id,
        Course {
            id,
            root,
            owner: sender,
            parent_root: session.parent_root,
            title: session.title,
            version: session.version,
            file_count: session.file_count,
            total_size: session.total_size,
            finalized_height: height,
            position,
        },
    );
    Ok(())
}

pub(crate) fn apply_cancel(
    ov: &mut Overlay<'_>,
    tx: &UserTx<SessionRef>,
    txid: Hash,
    height: Height,
) -> Result<(), TxError> {
    let sender = tx.sender;
    let session = session_of(ov, &tx.body.session_id, sender, CourseError::CancelSession)?;
    let lo = window_start(height, CANCEL_LIMIT_WINDOW);
    let recent = ov
        .courses
        .cancels
        .scan(&(sender, lo), |(owner, h)| *owner == sender && *h <= height)
        .into_iter()
        .try_fold(0_u32, |acc, (_, n)| acc.checked_add(n))
        .ok_or(TxError::Overflow)?;
    if recent >= CANCEL_LIMIT {
        return Err(CourseError::CancelLimit.into());
    }
    end_session(ov, &session, OutPoint { txid, index: 0 }, height)?;
    let count = ov
        .courses
        .cancels
        .get(&(sender, height))
        .copied()
        .unwrap_or(0);
    ov.courses.cancels.insert(
        (sender, height),
        count.checked_add(1).ok_or(TxError::Overflow)?,
    );
    Ok(())
}
