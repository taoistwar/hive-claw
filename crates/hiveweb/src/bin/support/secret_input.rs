use std::fs::File;
use std::io::Read;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

/// Opens and validates a private input file using the opened descriptor.
///
/// On Unix, `O_NOFOLLOW` closes the symlink race before the file is opened.
/// All remaining checks use `File::metadata`, so replacing the pathname after
/// the open cannot redirect the read to a different inode.
#[cfg(unix)]
fn open_private_file(path: &Path, input_name: &str) -> anyhow::Result<File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| anyhow::anyhow!("failed to open private {input_name} file"))?;
    let metadata = file
        .metadata()
        .map_err(|_| anyhow::anyhow!("failed to inspect private {input_name} file"))?;
    anyhow::ensure!(
        metadata.file_type().is_file(),
        "private {input_name} file must be a regular file"
    );
    anyhow::ensure!(
        metadata.uid() == unsafe { libc::geteuid() },
        "private {input_name} file must be owned by current uid"
    );
    anyhow::ensure!(
        metadata.permissions().mode() & 0o077 == 0,
        "private {input_name} file permissions must be 0600 or stricter"
    );
    anyhow::ensure!(
        metadata.nlink() == 1,
        "private {input_name} file must not have hard links"
    );
    Ok(file)
}

#[cfg(not(unix))]
fn open_private_file(_path: &Path, input_name: &str) -> anyhow::Result<File> {
    anyhow::bail!(
        "private {input_name} files are unavailable on this platform; use the stdin input mode"
    )
}

pub(crate) fn read_private_file(
    path: &Path,
    max_bytes: u64,
    input_name: &str,
) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_private_file(path, input_name)?
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("failed to read private {input_name} file"))?;
    anyhow::ensure!(
        bytes.len() <= max_bytes as usize,
        "private {input_name} file is too large"
    );
    Ok(bytes)
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;

    use super::open_private_file;

    fn private_file(prefix: &str, contents: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::write(&path, contents).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        path
    }

    #[test]
    fn rejects_symbolic_and_hard_links() {
        let path = private_file("hiveweb-private-input", b"original");
        let symlink = path.with_extension("symlink");
        let hard_link = path.with_extension("hard-link");
        std::os::unix::fs::symlink(&path, &symlink).unwrap();
        std::fs::hard_link(&path, &hard_link).unwrap();

        assert!(open_private_file(&symlink, "test input").is_err());
        assert!(open_private_file(&path, "test input").is_err());

        std::fs::remove_file(symlink).unwrap();
        std::fs::remove_file(hard_link).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn pathname_replacement_cannot_redirect_an_opened_descriptor() {
        let path = private_file("hiveweb-private-input", b"original");
        let moved = path.with_extension("opened");
        let mut opened = open_private_file(&path, "test input").unwrap();

        std::fs::rename(&path, &moved).unwrap();
        std::fs::write(&path, b"replacement").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let mut contents = String::new();
        opened.read_to_string(&mut contents).unwrap();
        assert_eq!(contents, "original");

        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(moved).unwrap();
    }
}
