//! Native checkpoint file durability. The regression suite uses actual files.
#[cfg(not(windows))]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::Path;

pub fn sync_written_file(path: &Path) -> io::Result<()> {
    OpenOptions::new().write(true).open(path)?.sync_all()
}

pub fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Opening a directory requires FILE_FLAG_BACKUP_SEMANTICS; flushing
        // it requires a write-capable handle, just like an ordinary file.
        OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(0x02000000)
            .open(path)?
            .sync_all()
    }
    #[cfg(not(windows))]
    {
        File::open(path)?.sync_all()
    }
}

pub fn replace_durable_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)?;
    sync_directory(
        destination.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "destination needs a parent")
        })?,
    )
}
