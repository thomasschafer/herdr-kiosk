use std::{borrow::Cow, path::Path};

/// Compare filesystem paths using the host platform's path semantics.
pub fn equivalent(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        windows_path_key(&left.to_string_lossy()) == windows_path_key(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

/// Return whether `path` is `base` or is nested below it.
pub fn starts_with(path: &Path, base: &Path) -> bool {
    #[cfg(windows)]
    {
        let path = windows_path_key(&path.to_string_lossy());
        let base = windows_path_key(&base.to_string_lossy());
        path == base
            || path
                .strip_prefix(&base)
                .is_some_and(|rest| base.ends_with('/') || rest.starts_with('/'))
    }
    #[cfg(not(windows))]
    {
        path.starts_with(base)
    }
}

/// Hide Windows' internal verbatim prefix in user-facing paths.
pub fn display(path: &Path) -> Cow<'_, str> {
    let value = path.to_string_lossy();
    #[cfg(windows)]
    {
        Cow::Owned(strip_windows_verbatim_prefix(&value).into_owned())
    }
    #[cfg(not(windows))]
    {
        value
    }
}

#[cfg(any(windows, test))]
fn strip_windows_verbatim_prefix(value: &str) -> Cow<'_, str> {
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        Cow::Owned(format!(r"\\{rest}"))
    } else if let Some(rest) = value.strip_prefix(r"\\?\") {
        Cow::Borrowed(rest)
    } else {
        Cow::Borrowed(value)
    }
}

#[cfg(any(windows, test))]
fn windows_path_key(value: &str) -> String {
    let value = strip_windows_verbatim_prefix(value);
    let mut key = value.replace('\u{5c}', "/").to_lowercase();
    while key.len() > 1 && key.ends_with('/') && !is_windows_drive_root(&key) {
        key.pop();
    }
    key
}

#[cfg(any(windows, test))]
fn is_windows_drive_root(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 3 && bytes[0].is_ascii_alphabetic() && bytes[1..] == *b":/"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_keys_ignore_verbatim_prefix_case_and_separator_style() {
        assert_eq!(
            windows_path_key(r"\\?\C:\Users\Tom\Repo\"),
            windows_path_key("c:/users/tom/repo")
        );
        assert_eq!(
            windows_path_key(r"\\?\UNC\Server\Share\Repo"),
            windows_path_key(r"\\server\share\repo")
        );
    }

    #[test]
    fn windows_display_hides_only_the_verbatim_prefix() {
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\C:\Users\Tom"),
            r"C:\Users\Tom"
        );
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\UNC\Server\Share"),
            r"\\Server\Share"
        );
    }
}
