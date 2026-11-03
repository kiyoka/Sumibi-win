//! Private minimal environment for suspended diagnostics, never inherited.
use super::ProbeError;
use windows::Win32::System::SystemInformation::{GetSystemDirectoryW, GetSystemWindowsDirectoryW};

const MAX_UNITS: usize = 32767;

pub(super) fn minimal() -> Result<Vec<u16>, ProbeError> {
    let mut windows = vec![0u16; MAX_UNITS + 1];
    let mut system = vec![0u16; MAX_UNITS + 1];
    // SAFETY: the APIs receive bounded writable UTF-16 buffers. Do not consult
    // the parent's SystemRoot, WINDIR or PATH, including in failure paths.
    let windows_len = unsafe { GetSystemWindowsDirectoryW(Some(&mut windows)) } as usize;
    let system_len = unsafe { GetSystemDirectoryW(Some(&mut system)) } as usize;
    if windows_len == 0
        || windows_len >= windows.len()
        || system_len == 0
        || system_len >= system.len()
    {
        return Err(ProbeError::EnvironmentUnavailable);
    }
    windows.truncate(windows_len);
    system.truncate(system_len);
    // GetSystemWindowsDirectory may return just a drive for a root install.
    if windows.len() == 2 {
        windows.push(b'\\' as u16);
    }
    from_directories(&windows, &system)
}

fn from_directories(windows: &[u16], system: &[u16]) -> Result<Vec<u16>, ProbeError> {
    validate_directory(windows)?;
    validate_directory(system)?;
    let mut block = Vec::new();
    // Fixed case-insensitive variable-name order. No
    // credentials, proxy controls, drive-current-directory entries or user PATH.
    for (name, value) in [
        ("PATH", system),
        ("SystemDrive", &windows[..2]),
        ("SystemRoot", windows),
        ("WINDIR", windows),
    ] {
        block.extend(name.encode_utf16());
        block.push(b'=' as u16);
        block.extend_from_slice(value);
        block.push(0);
    }
    block.push(0);
    if block.len() > MAX_UNITS {
        return Err(ProbeError::EnvironmentUnavailable);
    }
    Ok(block)
}

fn validate_directory(path: &[u16]) -> Result<(), ProbeError> {
    if path.len() < 3
        || path.len() > MAX_UNITS
        || !u8::try_from(path[0]).is_ok_and(|letter| letter.is_ascii_alphabetic())
        || path[1] != b':' as u16
        || path[2] != b'\\' as u16
        || path
            .iter()
            .any(|unit| *unit < 32 || b"\";/<>|*?".iter().any(|byte| *unit == u16::from(*byte)))
        || path[2..].contains(&(b':' as u16))
        || (path.len() > 3
            && path[3..].split(|unit| *unit == b'\\' as u16).any(|part| {
                part.is_empty()
                    || part
                        .last()
                        .is_some_and(|last| *last == b'.' as u16 || *last == b' ' as u16)
            }))
    {
        return Err(ProbeError::EnvironmentUnavailable);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().collect()
    }

    #[test]
    fn unicode_environment_has_only_four_sorted_os_path_entries() {
        let windows = wide(r"C:\Windows_日本語");
        let system = wide(r"C:\Windows_日本語\System32");
        let block = from_directories(&windows, &system).unwrap();
        assert_eq!(&block[block.len() - 2..], &[0, 0]);
        let entries: Vec<_> = block[..block.len() - 2]
            .split(|unit| *unit == 0)
            .map(|units| String::from_utf16(units).unwrap())
            .collect();
        assert_eq!(
            entries,
            [
                r"PATH=C:\Windows_日本語\System32",
                "SystemDrive=C:",
                r"SystemRoot=C:\Windows_日本語",
                r"WINDIR=C:\Windows_日本語",
            ]
        );
        let actual = minimal().expect("OS paths must provide a valid explicit environment");
        assert_eq!(actual.iter().filter(|unit| **unit == 0).count(), 5);
    }

    #[test]
    fn invalid_paths_cannot_inject_environment_or_extra_path_entries() {
        for invalid in [
            "",
            "Windows",
            "C:Windows",
            r"\\server\share",
            r"C:\Win;C:\evil",
            "C:\\Win\0EVIL=yes",
            "C:\\Win\nEVIL=yes",
            r"C:\Win\..\evil",
            r"C:\Win\.",
            r"C:\Win:ads",
            r"C:\Win\\evil",
            r"C:\Win\.. \evil",
            r"C:\Windows.\System32",
            "C:\\Win\"evil",
        ] {
            assert_eq!(
                from_directories(&wide(invalid), &wide(r"C:\Windows\System32")),
                Err(ProbeError::EnvironmentUnavailable)
            );
            assert_eq!(
                from_directories(&wide(r"C:\Windows"), &wide(invalid)),
                Err(ProbeError::EnvironmentUnavailable)
            );
        }
        let huge = wide(&format!(r"C:\{}", "x".repeat(MAX_UNITS)));
        assert_eq!(
            from_directories(&huge, &huge),
            Err(ProbeError::EnvironmentUnavailable)
        );
        let combined = wide(&format!(r"C:\{}", "x".repeat(MAX_UNITS / 2)));
        assert_eq!(
            from_directories(&combined, &combined),
            Err(ProbeError::EnvironmentUnavailable)
        );
        assert!(from_directories(&wide(r"C:\"), &wide(r"C:\System32")).is_ok());
    }
}
