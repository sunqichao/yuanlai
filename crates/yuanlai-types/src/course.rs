use yuanlai_codec::{BoundedText, BoundedVec, Decode, DecodeError, Encode, Reader};
use yuanlai_crypto::Hash;

use crate::params::{
    MAX_FILES_PER_APPEND, MAX_INPUTS, MAX_PATH_BYTES, MAX_TITLE_BYTES, MAX_VERSION_BYTES,
};
use crate::{Credit, Height, OutPoint, PubKey};

/// A file path inside a course. Validity (CRS-PATH-*) is checked by consensus, not on decode.
pub type CoursePath = BoundedText<MAX_PATH_BYTES>;
/// A course title.
pub type CourseTitle = BoundedText<MAX_TITLE_BYTES>;
/// A course version label.
pub type CourseVersion = BoundedText<MAX_VERSION_BYTES>;

/// One file of a course (`spec/05-course-session.md` §1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Relative path with `/` separators.
    pub path: CoursePath,
    /// Plain SHA-256 of the file content.
    pub content_hash: Hash,
    /// Size in bytes.
    pub size: u64,
}

impl Encode for FileEntry {
    fn encode_to(&self, out: &mut Vec<u8>) {
        self.path.encode_to(out);
        self.content_hash.encode_to(out);
        self.size.encode_to(out);
    }
}

impl Decode for FileEntry {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            path: BoundedText::decode_from(reader)?,
            content_hash: Hash::decode_from(reader)?,
            size: u64::decode_from(reader)?,
        })
    }
}

macro_rules! body {
    ($(#[$meta:meta])* $name:ident { $($(#[$fmeta:meta])* $field:ident : $ty:ty),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name {
            $($(#[$fmeta])* pub $field: $ty,)*
        }

        impl Encode for $name {
            fn encode_to(&self, out: &mut Vec<u8>) {
                $(self.$field.encode_to(out);)*
            }
        }

        impl Decode for $name {
            fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
                Ok(Self { $($field: <$ty>::decode_from(reader)?,)* })
            }
        }
    };
}

body!(
    /// `BeginCourse` body (`spec/05-course-session.md` §4).
    BeginCourse {
        /// Inputs in consumption order.
        inputs: BoundedVec<OutPoint, MAX_INPUTS>,
        /// Root of the previous version, for a new version of an existing course.
        parent_root: Option<Hash>,
        /// Course title.
        title: CourseTitle,
        /// Version label.
        version: CourseVersion,
        /// Declared number of (changed) files, used for the lock bounds.
        declared_files: u32,
        /// Declared total size of (changed) files, used for the lock bounds.
        declared_bytes: u64,
        /// Credit to lock.
        lock: Credit,
    }
);

body!(
    /// `AppendFiles` body (`spec/05-course-session.md` §5).
    AppendFiles {
        /// The session.
        session_id: Hash,
        /// Files to add.
        entries: BoundedVec<FileEntry, MAX_FILES_PER_APPEND>,
    }
);

body!(
    /// `AddLock` body (`spec/05-course-session.md` §5).
    AddLock {
        /// Inputs in consumption order.
        inputs: BoundedVec<OutPoint, MAX_INPUTS>,
        /// The session.
        session_id: Hash,
        /// New declared number of (changed) files.
        declared_files: u32,
        /// New declared total size of (changed) files.
        declared_bytes: u64,
        /// Additional credit to lock.
        amount: Credit,
    }
);

body!(
    /// `FinalizeCourse` and `CancelCourse` body (`spec/05-course-session.md` §6).
    SessionRef {
        /// The session.
        session_id: Hash,
    }
);

body!(
    /// Record of one automatically cancelled session (`spec/05-course-session.md` §7).
    AutoCancelRecord {
        /// The session.
        session_id: Hash,
        /// Its owner.
        owner: PubKey,
        /// Credit that was locked.
        locked: Credit,
        /// Burned fee.
        fee: Credit,
        /// Credit restored to the owner.
        restored: Credit,
    }
);

/// A registered course (`spec/05-course-session.md` §6). Its files are stored alongside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Course {
    /// Equal to the session ID.
    pub id: Hash,
    /// Merkle root over the file entries sorted by path.
    pub root: Hash,
    /// Registrant.
    pub owner: PubKey,
    /// Root of the previous version, if any.
    pub parent_root: Option<Hash>,
    /// Title.
    pub title: CourseTitle,
    /// Version label.
    pub version: CourseVersion,
    /// Number of files.
    pub file_count: u32,
    /// Total size of all files.
    pub total_size: u64,
    /// Height of the `FinalizeCourse` block.
    pub finalized_height: Height,
    /// Index of `FinalizeCourse` in that block's transactions.
    pub position: u32,
}
