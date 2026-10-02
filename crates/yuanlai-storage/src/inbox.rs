//! Transaction inbox: signed transactions handed from the CLI to a local node through the data
//! directory, until the network API exists. Files are written atomically (temp file + rename).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use yuanlai_codec::{Encode, decode_exact};
use yuanlai_types::Tx;

const EXTENSION: &str = "tx";

/// The `inbox/` directory of a node's data directory.
#[derive(Debug, Clone)]
pub struct Inbox {
    dir: PathBuf,
    rejected: PathBuf,
}

/// A pending transaction and the file it came from.
#[derive(Debug, Clone)]
pub struct Pending {
    /// Source file.
    pub path: PathBuf,
    /// The decoded transaction.
    pub tx: Tx,
}

impl Inbox {
    /// Opens (creating if needed) `datadir/inbox` and `datadir/inbox/rejected`.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the directories cannot be created.
    pub fn open(datadir: &Path) -> io::Result<Self> {
        let dir = datadir.join("inbox");
        let rejected = dir.join("rejected");
        fs::create_dir_all(&rejected)?;
        Ok(Self { dir, rejected })
    }

    /// Writes `tx` as `<txid>.tx`. Returns the file path.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the file cannot be written.
    pub fn submit(&self, tx: &Tx) -> io::Result<PathBuf> {
        let name = tx.txid().to_string();
        let final_path = self.dir.join(format!("{name}.{EXTENSION}"));
        let temp_path = self.dir.join(format!("{name}.tmp"));
        fs::write(&temp_path, tx.encode())?;
        fs::rename(&temp_path, &final_path)?;
        Ok(final_path)
    }

    /// All decodable pending transactions, sorted by file name. Undecodable files are moved to
    /// `rejected/`.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the directory cannot be read.
    pub fn pending(&self) -> io::Result<Vec<Pending>> {
        let mut paths: Vec<PathBuf> = fs::read_dir(&self.dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == EXTENSION))
            .collect();
        paths.sort();
        let mut out = Vec::with_capacity(paths.len());
        for path in paths {
            match fs::read(&path).map(|bytes| decode_exact::<Tx>(&bytes)) {
                Ok(Ok(tx)) => out.push(Pending { path, tx }),
                Ok(Err(e)) => self.reject(&path, &format!("undecodable: {e}"))?,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    /// Removes a transaction that was included in a block.
    ///
    /// # Errors
    ///
    /// Returns an I/O error other than "not found".
    pub fn remove(&self, path: &Path) -> io::Result<()> {
        match fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Moves a transaction to `rejected/` with a `.reason` file next to it.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the file cannot be moved.
    pub fn reject(&self, path: &Path, reason: &str) -> io::Result<()> {
        let Some(name) = path.file_name() else {
            return Ok(());
        };
        let target = self.rejected.join(name);
        fs::rename(path, &target)?;
        fs::write(target.with_extension("reason"), format!("{reason}\n"))
    }
}
