use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;

const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
const DWMSBT_MAINWINDOW: u32 = 2;

pub fn enable_dark_titlebar(hwnd: isize) -> bool {
    let flag: u32 = 1;
    unsafe {
        DwmSetWindowAttribute(
            hwnd as *mut _,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &flag as *const _ as *const _,
            std::mem::size_of::<u32>() as u32,
        ) == 0
    }
}

pub fn enable_mica(hwnd: isize) -> bool {
    unsafe {
        DwmSetWindowAttribute(
            hwnd as *mut _,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &DWMSBT_MAINWINDOW as *const _ as *const _,
            std::mem::size_of::<u32>() as u32,
        ) == 0
    }
}
