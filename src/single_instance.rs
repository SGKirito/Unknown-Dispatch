#[cfg(windows)]
mod imp {
    type HANDLE = *mut core::ffi::c_void;
    type HWND = *mut core::ffi::c_void;
    type BOOL = i32;
    type DWORD = u32;

    const ERROR_ALREADY_EXISTS: DWORD = 183;

    const SW_SHOW: i32 = 5;
    const SW_RESTORE: i32 = 9;

    const FLASHW_ALL: u32 = 0x3;
    const FLASHW_TIMERNOFG: u32 = 0xC;

    const FIND_WINDOW_ATTEMPTS: u32 = 20;
    const FIND_WINDOW_DELAY_MS: u64 = 100;

    const MUTEX_NAME: &str = "Local\\UnknownDispatch.SingleInstance.Mutex";

    #[repr(C)]
    struct FLASHWINFO {
        cb_size: u32,
        hwnd: HWND,
        dw_flags: u32,
        u_count: u32,
        dw_timeout: DWORD,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateMutexW(
            attributes: *mut core::ffi::c_void,
            initial_owner: BOOL,
            name: *const u16,
        ) -> HANDLE;
        fn ReleaseMutex(handle: HANDLE) -> BOOL;
        fn CloseHandle(handle: HANDLE) -> BOOL;
        fn GetLastError() -> DWORD;
    }

    #[link(name = "user32")]
    extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> HWND;
        fn IsIconic(hwnd: HWND) -> BOOL;
        fn IsWindowVisible(hwnd: HWND) -> BOOL;
        fn ShowWindow(hwnd: HWND, command: i32) -> BOOL;
        fn SetForegroundWindow(hwnd: HWND) -> BOOL;
        fn FlashWindowEx(info: *mut FLASHWINFO) -> BOOL;
    }

    pub enum Acquisition {
        Acquired(Guard),
        AlreadyRunning,
    }

    pub struct Guard {
        handle: HANDLE,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                unsafe {
                    ReleaseMutex(self.handle);
                    CloseHandle(self.handle);
                }
            }
        }
    }

    pub fn acquire(window_title: &str) -> Acquisition {
        let mutex_name = wide(MUTEX_NAME);
        let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 0, mutex_name.as_ptr()) };
        if handle.is_null() {
            tracing::warn!("Could not create the single-instance mutex.");
            return Acquisition::Acquired(Guard {
                handle: std::ptr::null_mut(),
            });
        }

        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe { CloseHandle(handle) };
            activate_existing_window(window_title);
            return Acquisition::AlreadyRunning;
        }

        Acquisition::Acquired(Guard { handle })
    }

    fn activate_existing_window(title: &str) {
        let title_w = wide(title);
        for _ in 0..FIND_WINDOW_ATTEMPTS {
            let hwnd = unsafe { FindWindowW(std::ptr::null(), title_w.as_ptr()) };
            if !hwnd.is_null() {
                unsafe { activate(hwnd) };
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(FIND_WINDOW_DELAY_MS));
        }
        tracing::warn!("Another instance is running, but its window could not be found.");
    }

    unsafe fn activate(hwnd: HWND) {
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            } else if IsWindowVisible(hwnd) == 0 {
                ShowWindow(hwnd, SW_SHOW);
            }
            if SetForegroundWindow(hwnd) == 0 {
                let mut info: FLASHWINFO = core::mem::zeroed();
                info.cb_size = core::mem::size_of::<FLASHWINFO>() as u32;
                info.hwnd = hwnd;
                info.dw_flags = FLASHW_ALL | FLASHW_TIMERNOFG;
                info.u_count = 3;
                info.dw_timeout = 0;
                FlashWindowEx(&mut info);
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(not(windows))]
mod imp {
    pub enum Acquisition {
        Acquired(Guard),
        #[allow(dead_code)]
        AlreadyRunning,
    }

    pub struct Guard;

    pub fn acquire(_window_title: &str) -> Acquisition {
        Acquisition::Acquired(Guard)
    }
}

pub use imp::{acquire, Acquisition};
