//! Append-only block storage for YuanLai nodes.
//!
//! One file of records `MAGIC ‖ len: u32 LE ‖ block ‖ checksum`, where `checksum` is the first
//! four bytes of `SHA256(block)`. There is deliberately no database: on startup every block is
//! read back and fully re-validated by consensus, so the file never has to be trusted.

use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::{error, fmt};

use yuanlai_codec::{Encode, decode_exact};
use yuanlai_crypto::sha256;
use yuanlai_types::Block;
use yuanlai_types::params::MAX_BLOCK_SIZE;

mod inbox;

pub use inbox::{Inbox, Pending};

const MAGIC: [u8; 4] = *b"YLBK";
const HEADER_LEN: u64 = 8;
const CHECKSUM_LEN: u64 = 4;

/// Storage failure.
#[derive(Debug)]
pub enum StoreError {
    /// Operating-system I/O error.
    Io(io::Error),
    /// A complete record at `offset` is malformed. Not repaired automatically, since that could
    /// silently discard data.
    Corrupt {
        /// Byte offset of the record.
        offset: u64,
        /// What is wrong.
        reason: &'static str,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "block store I/O error: {e}"),
            Self::Corrupt { offset, reason } => {
                write!(f, "block store corrupt at byte {offset}: {reason}")
            }
        }
    }
}

impl error::Error for StoreError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Corrupt { .. } => None,
        }
    }
}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// An open block file positioned for appending.
#[derive(Debug)]
pub struct BlockStore {
    file: File,
    path: PathBuf,
}

/// Result of [`BlockStore::open`].
#[derive(Debug)]
pub struct Opened {
    /// The store, ready for appends.
    pub store: BlockStore,
    /// Stored blocks in append order. They are *not* validated here.
    pub blocks: Vec<Block>,
    /// Bytes of an incomplete final record removed during recovery (0 if none).
    pub truncated_bytes: u64,
}

fn checksum(payload: &[u8]) -> [u8; 4] {
    let [a, b, c, d, ..] = sha256(payload).0;
    [a, b, c, d]
}

/// Reads exactly `buf.len()` bytes. Returns `false` on end of file before the buffer is full.
fn read_full(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<bool> {
    match reader.read_exact(buf) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e),
    }
}

enum Record {
    Block(Block, u64),
    /// Incomplete final record: a torn write.
    Torn,
    End,
}

fn read_record(reader: &mut impl Read, offset: u64) -> Result<Record, StoreError> {
    let corrupt = |reason| StoreError::Corrupt { offset, reason };
    let mut header = [0_u8; 8];
    let mut first = [0_u8; 1];
    if !read_full(reader, &mut first)? {
        return Ok(Record::End);
    }
    header[0] = first[0];
    if !read_full(reader, header.get_mut(1..).unwrap_or_default())? {
        return Ok(Record::Torn);
    }
    let (magic, len) = header.split_at(4);
    if magic != MAGIC {
        return Err(corrupt("bad magic"));
    }
    let len_bytes: [u8; 4] = len.try_into().map_err(|_| corrupt("bad length"))?;
    let len = u32::from_le_bytes(len_bytes);
    let len_usize = usize::try_from(len).map_err(|_| corrupt("bad length"))?;
    if len_usize > MAX_BLOCK_SIZE {
        return Err(corrupt("record longer than MAX_BLOCK_SIZE"));
    }
    let mut payload = vec![0_u8; len_usize];
    let mut sum = [0_u8; 4];
    if !read_full(reader, &mut payload)? || !read_full(reader, &mut sum)? {
        return Ok(Record::Torn);
    }
    if sum != checksum(&payload) {
        return Err(corrupt("checksum mismatch"));
    }
    let block = decode_exact::<Block>(&payload).map_err(|_| corrupt("undecodable block"))?;
    let size = HEADER_LEN
        .saturating_add(u64::from(len))
        .saturating_add(CHECKSUM_LEN);
    Ok(Record::Block(block, size))
}

/// Reads all complete blocks without modifying the file, for readers running alongside a node.
/// A missing file yields no blocks; an incomplete final record is ignored.
///
/// # Errors
///
/// Returns [`StoreError::Corrupt`] for a malformed complete record, or an I/O error.
pub fn read_blocks(path: &Path) -> Result<Vec<Block>, StoreError> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut reader = BufReader::new(file);
    let mut blocks = Vec::new();
    let mut offset = 0_u64;
    while let Record::Block(block, size) = read_record(&mut reader, offset)? {
        blocks.push(block);
        offset = offset.saturating_add(size);
    }
    Ok(blocks)
}

impl BlockStore {
    /// Opens or creates the block file at `path` and reads all stored blocks.
    ///
    /// An incomplete final record, left by a crash during [`Self::append`], is truncated.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Corrupt`] for a malformed complete record, or an I/O error.
    pub fn open(path: &Path) -> Result<Opened, StoreError> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let file_len = file.metadata()?.len();

        let mut blocks = Vec::new();
        let mut offset = 0_u64;
        {
            let mut reader = BufReader::new(&mut file);
            while let Record::Block(block, size) = read_record(&mut reader, offset)? {
                blocks.push(block);
                offset = offset.saturating_add(size);
            }
        }

        let truncated_bytes = file_len.saturating_sub(offset);
        if truncated_bytes > 0 {
            file.set_len(offset)?;
            file.sync_all()?;
        }
        file.seek(SeekFrom::End(0))?;

        Ok(Opened {
            store: Self {
                file,
                path: path.to_path_buf(),
            },
            blocks,
            truncated_bytes,
        })
    }

    /// Appends `block` and flushes it to stable storage before returning.
    ///
    /// # Errors
    ///
    /// Returns an I/O error; the record may then be torn and is repaired by the next [`Self::open`].
    pub fn append(&mut self, block: &Block) -> Result<(), StoreError> {
        let payload = block.encode();
        let len = u32::try_from(payload.len()).map_err(|_| StoreError::Corrupt {
            offset: 0,
            reason: "block too large to store",
        })?;
        let mut record = Vec::with_capacity(payload.len().saturating_add(12));
        record.extend_from_slice(&MAGIC);
        record.extend_from_slice(&len.to_le_bytes());
        record.extend_from_slice(&payload);
        record.extend_from_slice(&checksum(&payload));
        self.file.write_all(&record)?;
        self.file.sync_data()?;
        Ok(())
    }

    /// Atomically replaces the stored active chain with `blocks`.
    ///
    /// The replacement is fully written and synced to a sibling temporary file before rename, so
    /// interruption before the rename preserves the previous chain. This is intended for an
    /// already-validated reorganization; every block is still revalidated on the next startup.
    ///
    /// # Errors
    ///
    /// Returns an I/O error or a storage error if a block cannot be represented in a record.
    pub fn replace_all(&mut self, blocks: &[Block]) -> Result<(), StoreError> {
        let temp_path = self.path.with_extension("replace.tmp");
        let mut replacement = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp_path)?;
        let result = (|| {
            for block in blocks {
                let payload = block.encode();
                let len = u32::try_from(payload.len()).map_err(|_| StoreError::Corrupt {
                    offset: 0,
                    reason: "block too large to store",
                })?;
                let mut record = Vec::with_capacity(payload.len().saturating_add(12));
                record.extend_from_slice(&MAGIC);
                record.extend_from_slice(&len.to_le_bytes());
                record.extend_from_slice(&payload);
                record.extend_from_slice(&checksum(&payload));
                replacement.write_all(&record)?;
            }
            replacement.sync_all()?;
            std::fs::rename(&temp_path, &self.path)?;
            Ok::<(), StoreError>(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp_path);
        }
        result?;
        self.file = replacement;
        Ok(())
    }

    /// Path of the block file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}
