use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Atomic file write: temp sibling + fsync + platform-aware replacement. On
/// failure, truncate the owned temp sibling instead of deleting it so callers
/// never need destructive cleanup.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    write_atomic_inner(path, bytes, false)
}

/// Atomic write for credential-bearing files.
///
/// Same publish contract as [`write_atomic`], but the staged sibling is created
/// owner-only (mode `0600` on unix, set at creation and re-applied before any
/// byte is written) so a secret is never readable by other users, neither in
/// the staged file nor in the published one. On Windows the staged file
/// inherits the protected per-user directory ACL, matching the previous
/// behavior of `secure_file`.
pub fn write_atomic_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    write_atomic_inner(path, bytes, true)
}

fn write_atomic_inner(path: &Path, bytes: &[u8], private: bool) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        anyhow::bail!(
            "output directory does not exist: {} (it is not created)",
            parent.display()
        );
    }
    let mut temp_name = path.as_os_str().to_os_string();
    temp_name.push(format!(
        ".tmp-{}.{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let temp = PathBuf::from(temp_name);
    let result = (|| -> anyhow::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::PermissionsExt;
            // Restore owner access even under a restrictive umask, before
            // writing any bytes.
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        #[cfg(not(unix))]
        let _ = private;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        replace_staged(&temp, path)?;
        Ok(())
    })();
    if result.is_err()
        && let Ok(file) = std::fs::OpenOptions::new().write(true).open(&temp)
    {
        let _truncated = file.set_len(0);
    }
    result
}

/// Replace `destination` with a fully-written sibling file.
///
/// The staged file is synced again here so callers that use a specialized
/// writer (for example, DPAPI-backed secure storage) get the same durability
/// boundary before replacement. Windows requires `ReplaceFileW` for an
/// existing destination; `std::fs::rename` does not provide that contract on
/// every supported Windows filesystem.
pub fn replace_staged(staged: &Path, destination: &Path) -> io::Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .open(staged)?
        .sync_all()?;
    replace_staged_platform(staged, destination)
}

#[cfg(not(windows))]
fn replace_staged_platform(staged: &Path, destination: &Path) -> io::Result<()> {
    std::fs::rename(staged, destination)?;
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(windows)]
fn replace_staged_platform(staged: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_WRITE_THROUGH, MoveFileExW, REPLACEFILE_WRITE_THROUGH, ReplaceFileW,
    };
    use windows::core::PCWSTR;

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// Win32 `ERROR_SHARING_VIOLATION`.
    const ERROR_SHARING_VIOLATION: u16 = 32;
    /// Win32 `ERROR_UNABLE_TO_REMOVE_REPLACED`: the destination is briefly held
    /// open by another handle (for example a virus scanner or indexer).
    const ERROR_UNABLE_TO_REMOVE_REPLACED: u16 = 1175;
    const REPLACE_ATTEMPTS: u32 = 8;

    // HRESULT_FROM_WIN32 wraps the Win32 code in the low word of a 0x8007xxxx value.
    fn is_transient_replace_error(error: &windows::core::Error) -> bool {
        let code = error.code().0 as u32;
        (code >> 16) == 0x8007
            && matches!(
                (code & 0xFFFF) as u16,
                ERROR_SHARING_VIOLATION | ERROR_UNABLE_TO_REMOVE_REPLACED
            )
    }

    let destination_exists = destination.exists();
    let staged = wide(staged);
    let destination = wide(destination);
    let mut attempt = 0;
    loop {
        // SAFETY: both vectors are NUL-terminated and remain alive for each API
        // call; the Windows APIs do not retain either pointer after returning.
        let result = unsafe {
            if destination_exists {
                ReplaceFileW(
                    PCWSTR(destination.as_ptr()),
                    PCWSTR(staged.as_ptr()),
                    PCWSTR::null(),
                    REPLACEFILE_WRITE_THROUGH,
                    None,
                    None,
                )
            } else {
                MoveFileExW(
                    PCWSTR(staged.as_ptr()),
                    PCWSTR(destination.as_ptr()),
                    MOVEFILE_WRITE_THROUGH,
                )
            }
        };
        match result {
            Ok(()) => return Ok(()),
            // The staged file is left in place by a failed replace, so a
            // short bounded retry is safe; the final error is still returned.
            Err(error)
                if destination_exists
                    && attempt + 1 < REPLACE_ATTEMPTS
                    && is_transient_replace_error(&error) =>
            {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(10 * u64::from(attempt)));
            }
            Err(error) => return Err(io::Error::other(error.to_string())),
        }
    }
}
