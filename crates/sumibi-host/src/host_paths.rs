//! Binary-only Windows path boundary; the Host library remains safe Rust.
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use windows::core::{GUID, HRESULT, PWSTR};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::FOLDERID_LocalAppData;

#[link(name = "shell32")]
extern "system" {
    // Keep the output pointer even on failure so it is always freed, as the
    // native API contract requires. The generated wrapper returns it on S_OK.
    #[link_name = "SHGetKnownFolderPath"]
    fn known_folder_path(
        id: *const GUID,
        flags: u32,
        token: *mut std::ffi::c_void,
        path: *mut PWSTR,
    ) -> HRESULT;
}

pub fn settings_path() -> Result<PathBuf, ()> {
    struct Buffer(PWSTR);
    impl Drop for Buffer {
        fn drop(&mut self) {
            // SAFETY: the pointer is null or the task-allocated output of the
            // Shell API, and this guard frees it exactly once on every path.
            unsafe { CoTaskMemFree(Some(self.0 .0.cast())) }
        }
    }
    let mut buffer = Buffer(PWSTR::null());
    // SAFETY: valid folder ID and output pointer; a null token selects the
    // current user. No CREATE flag is supplied, so no folder is created.
    unsafe {
        known_folder_path(
            &FOLDERID_LocalAppData,
            0,
            std::ptr::null_mut(),
            &mut buffer.0,
        )
    }
    .ok()
    .map_err(|_| ())?;
    if buffer.0.is_null() {
        return Err(());
    }
    // SAFETY: on success Windows supplies a NUL-terminated UTF-16 path whose
    // task-allocated storage is retained by buffer until after the copy.
    let local = unsafe { OsString::from_wide(buffer.0.as_wide()) };
    append_settings_path(Path::new(&local))
}

fn append_settings_path(local: &Path) -> Result<PathBuf, ()> {
    if !local.is_absolute() {
        return Err(());
    }
    Ok(local.join("Sumibi").join("settings.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_folder_resolution_is_read_only_and_uses_fixed_suffix() {
        let resolved = settings_path().expect("current user's LocalAppData");
        assert!(resolved.is_absolute());
        assert!(resolved.ends_with(Path::new(r"Sumibi\settings.json")));
        assert!(append_settings_path(Path::new("relative")).is_err());
        assert_eq!(
            append_settings_path(Path::new(r"D:\profile\ローカル")).unwrap(),
            PathBuf::from(r"D:\profile\ローカル\Sumibi\settings.json")
        );
    }
}
