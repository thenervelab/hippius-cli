//! Opening a file whose bytes `hipc` sends, refusing what a caller did not mean to send: a
//! symlink, which could point at the config file and upload its tokens, and anything but a
//! regular file, such as a FIFO, whose read would wait for a writer that never comes.
//!
//! `O_NOFOLLOW` covers only the last component of the path: a symlink earlier in it, such as
//! `/tmp` on macOS or a linked directory, is followed. The refusal guards against a link
//! named as the file itself, not against every path that ends at a file through one.

use std::fs::File;
use std::io;
use std::path::Path;

use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;

/// Why [`open_regular`] refused a path.
#[derive(Debug)]
pub enum OpenRegularError {
    /// The last component of the path is a symlink, which is never followed.
    Symlink,
    /// The path names a directory, a FIFO, a device, or a socket.
    NotRegular,
    /// The open or the check on the open descriptor failed.
    Io(io::Error),
}

/// Opens `path` for reading without following a symlink as its last component, and checks
/// on the open descriptor that it is a regular file, so the check and the read are of the
/// same file.
///
/// # Errors
///
/// [`OpenRegularError::Symlink`] for a symlink, [`OpenRegularError::NotRegular`] for
/// anything but a regular file, and [`OpenRegularError::Io`] when the open or the check
/// fails, a missing file included.
pub fn open_regular(path: &Path) -> Result<File, OpenRegularError> {
    // `O_NONBLOCK`: opening a FIFO returns at once instead of waiting for a writer, so the
    // check below refuses it. A regular file ignores the flag.
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let file = match rustix::fs::open(path, flags, Mode::empty()) {
        Ok(fd) => File::from(fd),
        Err(Errno::LOOP) => return Err(OpenRegularError::Symlink),
        Err(e) => return Err(OpenRegularError::Io(e.into())),
    };
    let metadata = file.metadata().map_err(OpenRegularError::Io)?;
    if metadata.is_file() {
        Ok(file)
    } else {
        Err(OpenRegularError::NotRegular)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    use std::path::PathBuf;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hipc-regular-{name}-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    #[test]
    fn a_regular_file_opens_and_reads() {
        let dir = temp_dir("file");
        let path = dir.join("compose.yaml");
        std::fs::write(&path, b"services: {}\n").unwrap();
        let mut text = String::new();
        open_regular(&path)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "services: {}\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_symlink_is_refused_even_to_a_regular_file() {
        let dir = temp_dir("link");
        std::fs::write(dir.join("target"), b"token").unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink("target", &link).unwrap();
        assert!(matches!(
            open_regular(&link),
            Err(OpenRegularError::Symlink)
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_directory_and_a_fifo_are_not_regular() {
        let dir = temp_dir("special");
        let fifo = dir.join("fifo");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(made.success());
        assert!(matches!(
            open_regular(&dir),
            Err(OpenRegularError::NotRegular)
        ));
        assert!(matches!(
            open_regular(&fifo),
            Err(OpenRegularError::NotRegular)
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_file_is_the_io_error() {
        let dir = temp_dir("missing");
        let opened = open_regular(&dir.join("missing"));
        assert!(
            matches!(&opened, Err(OpenRegularError::Io(e)) if e.kind() == io::ErrorKind::NotFound)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
