use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GetSystemMetrics, LoadImageW, SendMessageW, HICON, ICON_BIG, ICON_SMALL,
    IMAGE_FLAGS, IMAGE_ICON, SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, WM_SETICON,
};

// Resource ID in assets/Sumibi.rc.
const SUMIBI_ICON: PCWSTR = PCWSTR(101 as *const u16);

pub struct Icon(pub HICON);

impl Drop for Icon {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyIcon(self.0);
        }
    }
}

fn load(instance: HINSTANCE, small: bool) -> Option<Icon> {
    let (width, height) = if small {
        (SM_CXSMICON, SM_CYSMICON)
    } else {
        (SM_CXICON, SM_CYICON)
    };
    unsafe {
        LoadImageW(
            Some(instance),
            SUMIBI_ICON,
            IMAGE_ICON,
            GetSystemMetrics(width),
            GetSystemMetrics(height),
            IMAGE_FLAGS(0),
        )
        .ok()
        .map(|handle| Icon(HICON(handle.0)))
    }
}

pub fn small(instance: HINSTANCE) -> Option<Icon> {
    load(instance, true)
}

pub fn set_window_icons(hwnd: HWND, instance: HINSTANCE) -> [Option<Icon>; 2] {
    let icons = [load(instance, false), small(instance)];
    for (kind, icon) in [ICON_BIG, ICON_SMALL].into_iter().zip(&icons) {
        if let Some(Icon(icon)) = icon {
            unsafe {
                SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(kind as usize)),
                    Some(LPARAM(icon.0 as isize)),
                );
            }
        }
    }
    icons
}
