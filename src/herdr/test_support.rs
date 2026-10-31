//! Fake programs for the tests that drive real `herdr`, `herdr-app` and `hc` invocations.

use std::{fs, os::unix::fs::PermissionsExt, path::Path, thread, time::Duration};

use super::{HerdrError, PaneOpenError};

/// Writes an executable `/bin/sh` script with `body` to `path`.
pub fn write_script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// An error that may be `ETXTBSY` from executing a script written moments before.
pub trait TextFileBusy {
    fn is_text_file_busy(&self) -> bool;
}

impl TextFileBusy for HerdrError {
    fn is_text_file_busy(&self) -> bool {
        matches!(self, HerdrError::Invocation(message)
            if message.contains("Text file busy") || message.contains("os error 26"))
    }
}

impl TextFileBusy for PaneOpenError {
    fn is_text_file_busy(&self) -> bool {
        match self {
            PaneOpenError::Split(error) | PaneOpenError::Run(error) => error.is_text_file_busy(),
        }
    }
}

/// Runs `invoke` again while it fails with `ETXTBSY`, returning its first other result.
///
/// Tests run on parallel threads, so another test can fork while a script is still open for
/// writing here. The child holds that descriptor until it execs, and until then the kernel
/// refuses to execute the script. The window is short, so retrying is enough.
pub fn retry_while_text_file_busy<T, E: TextFileBusy>(
    mut invoke: impl FnMut() -> Result<T, E>,
) -> Result<T, E> {
    const ATTEMPTS: usize = 100;
    let mut attempt = 1;
    loop {
        match invoke() {
            Err(error) if error.is_text_file_busy() && attempt < ATTEMPTS => {
                attempt += 1;
                thread::sleep(Duration::from_millis(1));
            }
            result => return result,
        }
    }
}
