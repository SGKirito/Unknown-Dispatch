use std::sync::{atomic::AtomicBool, Arc};
#[cfg(windows)]
use std::sync::{
    atomic::{AtomicIsize, Ordering},
    OnceLock,
};

#[allow(dead_code)]
pub struct TrayConfig {
    pub exit_flag: Arc<AtomicBool>,
    pub on_exit: Arc<dyn Fn() + Send + Sync>,
    pub on_wake: Arc<dyn Fn() + Send + Sync>,
    pub tooltip: String,
}

#[cfg(windows)]
mod imp {
    use super::{Arc, AtomicBool, AtomicIsize, OnceLock, Ordering, TrayConfig};

    type HWND = *mut core::ffi::c_void;
    type HICON = *mut core::ffi::c_void;
    type HMENU = *mut core::ffi::c_void;
    type BOOL = i32;
    type LRESULT = isize;
    type WPARAM = usize;
    type LPARAM = isize;

    const WM_NULL: u32 = 0x0000;
    const WM_DESTROY: u32 = 0x0002;
    const WM_CLOSE: u32 = 0x0010;
    const WM_APP: u32 = 0x8000;
    const WM_TRAYICON: u32 = WM_APP + 1;
    const WM_LBUTTONUP: u32 = 0x0202;
    const WM_LBUTTONDBLCLK: u32 = 0x0203;
    const WM_RBUTTONUP: u32 = 0x0205;
    const SW_HIDE: i32 = 0;
    const SW_RESTORE: i32 = 9;

    const NIM_ADD: u32 = 0;
    const NIM_MODIFY: u32 = 1;
    const NIM_DELETE: u32 = 2;
    const NIF_MESSAGE: u32 = 0x1;
    const NIF_ICON: u32 = 0x2;
    const NIF_TIP: u32 = 0x4;
    const NIF_INFO: u32 = 0x10;
    const NIIF_INFO: u32 = 0x1;

    const GWLP_WNDPROC: i32 = -4;
    const TPM_RETURNCMD: u32 = 0x0100;
    const TPM_RIGHTBUTTON: u32 = 0x0002;
    const TPM_NONOTIFY: u32 = 0x0080;
    const MF_STRING: u32 = 0x0000;
    const MF_SEPARATOR: u32 = 0x0800;
    const LR_DEFAULTCOLOR: u32 = 0x0000;

    const MENU_SHOW: usize = 1;
    const MENU_EXIT: usize = 2;
    const TRAY_ID: u32 = 1;

    const BALLOON_TEXT: &str =
        "Running in the background. Right-click the tray icon to exit completely.";

    #[repr(C)]
    struct POINT {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    #[repr(C)]
    struct NOTIFYICONDATAW {
        cb_size: u32,
        hwnd: HWND,
        uid: u32,
        flags: u32,
        callback_message: u32,
        icon: HICON,
        tip: [u16; 128],
        state: u32,
        state_mask: u32,
        info: [u16; 256],
        version: u32,
        info_title: [u16; 64],
        info_flags: u32,
        guid_item: Guid,
        balloon_icon: HICON,
    }

    #[link(name = "shell32")]
    extern "system" {
        fn Shell_NotifyIconW(message: u32, data: *const NOTIFYICONDATAW) -> BOOL;
    }

    #[link(name = "user32")]
    extern "system" {
        fn ShowWindow(hwnd: HWND, command: i32) -> BOOL;
        fn SetForegroundWindow(hwnd: HWND) -> BOOL;
        fn CreatePopupMenu() -> HMENU;
        fn AppendMenuW(menu: HMENU, flags: u32, id: usize, text: *const u16) -> BOOL;
        fn TrackPopupMenuEx(
            menu: HMENU,
            flags: u32,
            x: i32,
            y: i32,
            hwnd: HWND,
            params: *mut core::ffi::c_void,
        ) -> BOOL;
        fn DestroyMenu(menu: HMENU) -> BOOL;
        fn GetCursorPos(point: *mut POINT) -> BOOL;
        fn PostMessageW(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> BOOL;
        fn SetWindowLongPtrW(hwnd: HWND, index: i32, value: isize) -> isize;
        fn CallWindowProcW(
            prev: isize,
            hwnd: HWND,
            msg: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT;
        fn CreateIconFromResourceEx(
            bits: *const u8,
            size: u32,
            icon: BOOL,
            version: u32,
            desired_x: i32,
            desired_y: i32,
            flags: u32,
        ) -> HICON;
        fn LoadIconW(instance: HWND, name: *const u16) -> HICON;
        fn FindWindowW(class: *const u16, title: *const u16) -> HWND;
    }

    static OLD_WNDPROC: AtomicIsize = AtomicIsize::new(0);
    static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    static EXIT_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    static ON_EXIT: OnceLock<Arc<dyn Fn() + Send + Sync>> = OnceLock::new();
    static ON_WAKE: OnceLock<Arc<dyn Fn() + Send + Sync>> = OnceLock::new();

    pub fn install(hwnd: *mut core::ffi::c_void, config: TrayConfig) -> bool {
        if INSTALLED.load(Ordering::SeqCst) {
            return true;
        }
        unsafe {
            let icon = load_app_icon();
            if icon.is_null() {
                tracing::warn!("Could not load an icon for the system tray.");
                return false;
            }

            let prev = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, subclass_proc as *const () as isize);
            if prev == 0 {
                return false;
            }
            OLD_WNDPROC.store(prev, Ordering::SeqCst);

            EXIT_FLAG.get_or_init(|| Arc::clone(&config.exit_flag));
            ON_EXIT.get_or_init(|| Arc::clone(&config.on_exit));
            ON_WAKE.get_or_init(|| Arc::clone(&config.on_wake));

            let mut nid: NOTIFYICONDATAW = core::mem::zeroed();
            nid.cb_size = core::mem::size_of::<NOTIFYICONDATAW>() as u32;
            nid.hwnd = hwnd;
            nid.uid = TRAY_ID;
            nid.flags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            nid.callback_message = WM_TRAYICON;
            nid.icon = icon;
            fill_utf16(&mut nid.tip, &config.tooltip);

            if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
                SetWindowLongPtrW(hwnd, GWLP_WNDPROC, prev);
                OLD_WNDPROC.store(0, Ordering::SeqCst);
                tracing::warn!("Failed to create the system tray icon.");
                return false;
            }

            TRAY_HWND.store(hwnd as isize, Ordering::SeqCst);
            INSTALLED.store(true, Ordering::SeqCst);
            true
        }
    }

    pub fn remove() {
        let hwnd = TRAY_HWND.load(Ordering::SeqCst);
        if hwnd != 0 {
            unsafe { remove_icon(hwnd as HWND) };
        }
    }

    pub fn find_window_by_title(title: &str) -> Option<*mut core::ffi::c_void> {
        let title_w: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        let hwnd = unsafe { FindWindowW(std::ptr::null(), title_w.as_ptr()) };
        if hwnd.is_null() {
            None
        } else {
            Some(hwnd)
        }
    }

    unsafe extern "system" fn subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let old = OLD_WNDPROC.load(Ordering::SeqCst);
        match msg {
            WM_CLOSE => {
                ShowWindow(hwnd, SW_HIDE);
                show_balloon();
                return 0;
            }
            WM_TRAYICON => {
                match lparam as u32 {
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK => restore_window(hwnd),
                    WM_RBUTTONUP => show_menu(hwnd),
                    _ => {}
                }
                return 0;
            }
            WM_DESTROY => remove_icon(hwnd),
            _ => {}
        }
        CallWindowProcW(old, hwnd, msg, wparam, lparam)
    }

    unsafe fn restore_window(hwnd: HWND) {
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd);
    }

    unsafe fn show_menu(hwnd: HWND) {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        let show_label = wide("Show Manager");
        let exit_label = wide("Exit Completely");
        AppendMenuW(menu, MF_STRING, MENU_SHOW, show_label.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(menu, MF_STRING, MENU_EXIT, exit_label.as_ptr());

        let mut point = POINT { x: 0, y: 0 };
        GetCursorPos(&mut point);
        SetForegroundWindow(hwnd);
        let command = TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            point.x,
            point.y,
            hwnd,
            std::ptr::null_mut(),
        ) as usize;
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);

        match command {
            MENU_SHOW => restore_window(hwnd),
            MENU_EXIT => {
                if let Some(flag) = EXIT_FLAG.get() {
                    flag.store(true, Ordering::SeqCst);
                }

                if let Some(exit) = ON_EXIT.get() {
                    exit();
                }
                if let Some(wake) = ON_WAKE.get() {
                    wake();
                }
            }
            _ => {}
        }
    }

    unsafe fn show_balloon() {
        let hwnd = TRAY_HWND.load(Ordering::SeqCst);
        if hwnd == 0 {
            return;
        }
        let mut nid: NOTIFYICONDATAW = core::mem::zeroed();
        nid.cb_size = core::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hwnd = hwnd as HWND;
        nid.uid = TRAY_ID;
        nid.flags = NIF_INFO;
        fill_utf16(&mut nid.info, BALLOON_TEXT);
        fill_utf16(&mut nid.info_title, "Unknown Dispatch");
        nid.info_flags = NIIF_INFO;
        Shell_NotifyIconW(NIM_MODIFY, &nid);
    }

    unsafe fn remove_icon(hwnd: HWND) {
        let mut nid: NOTIFYICONDATAW = core::mem::zeroed();
        nid.cb_size = core::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hwnd = hwnd;
        nid.uid = TRAY_ID;
        Shell_NotifyIconW(NIM_DELETE, &nid);
        TRAY_HWND.store(0, Ordering::SeqCst);
        INSTALLED.store(false, Ordering::SeqCst);
    }

    unsafe fn load_app_icon() -> HICON {
        if let Some(entry) = crate::icon::best_entry(crate::icon::LOGO_ICO) {
            let icon = CreateIconFromResourceEx(
                entry.data.as_ptr(),
                entry.data.len() as u32,
                1,
                0x0003_0000,
                0,
                0,
                LR_DEFAULTCOLOR,
            );
            if !icon.is_null() {
                return icon;
            }
        }
        LoadIconW(std::ptr::null_mut(), 32512usize as *const u16)
    }

    fn fill_utf16(slot: &mut [u16], text: &str) {
        let taken: Vec<u16> = text.encode_utf16().take(slot.len() - 1).collect();
        slot[..taken.len()].copy_from_slice(&taken);
        slot[taken.len()] = 0;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(not(windows))]
mod imp {
    use super::TrayConfig;

    pub fn install(_hwnd: *mut core::ffi::c_void, _config: TrayConfig) -> bool {
        false
    }

    pub fn remove() {}

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn find_window_by_title(_title: &str) -> Option<*mut core::ffi::c_void> {
        None
    }
}

#[cfg(windows)]
pub use imp::find_window_by_title;
pub use imp::{install, remove};
