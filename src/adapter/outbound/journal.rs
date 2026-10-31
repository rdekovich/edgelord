//! Exclusive, fsynced append-only execution ledger. Corruption fails closed.
use crate::{
    error::{Error, Result},
    port::outbound::journal::{ExecutionJournal, ExecutionRecord},
};
use parking_lot::Mutex;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::fd::AsRawFd,
    path::Path,
};

pub struct FileExecutionJournal {
    file: Mutex<File>,
}
impl FileExecutionJournal {
    pub fn open(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)?;
        // A second process must never submit against the same ledger concurrently.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(Error::Io(std::io::Error::last_os_error()));
        }
        file.sync_all()?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        File::open(parent)?.sync_all()?;
        let journal = Self {
            file: Mutex::new(file),
        };
        journal.records()?;
        Ok(journal)
    }
}
impl ExecutionJournal for FileExecutionJournal {
    fn append(&self, record: &ExecutionRecord) -> Result<()> {
        let mut bytes = serde_json::to_vec(record)?;
        bytes.push(b'\n');
        let mut file = self.file.lock();
        file.write_all(&bytes)?;
        file.sync_all()?;
        Ok(())
    }
    fn records(&self) -> Result<Vec<ExecutionRecord>> {
        let mut file = self.file.lock();
        file.seek(SeekFrom::Start(0))?;
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        if !text.is_empty() && !text.ends_with('\n') {
            return Err(Error::Parse(
                "Incomplete execution ledger; reconcile before trading".into(),
            ));
        }
        text.lines()
            .map(|line| serde_json::from_str(line).map_err(Error::from))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn journal_is_exclusive_and_rejects_truncated_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger");
        let journal = FileExecutionJournal::open(&path).unwrap();
        assert!(FileExecutionJournal::open(&path).is_err());
        drop(journal);
        std::fs::write(&path, "{\"Intent\":").unwrap();
        assert!(FileExecutionJournal::open(&path).is_err());
    }
}
