#[cfg(windows)]
const INTERNET_OPTION_SETTINGS_CHANGED: u32 = 39;
#[cfg(windows)]
const INTERNET_OPTION_REFRESH: u32 = 37;

#[cfg_attr(not(windows), allow(dead_code))]
const INTERNET_SETTINGS_SUBKEY: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings";

pub fn bypass_list() -> String {
    let mut list = String::from("localhost;127.*;10.*");
    for i in 16..=31 {
        list.push_str(&format!(";172.{i}.*"));
    }
    list.push_str(";192.168.*;<local>");
    list
}

#[cfg(windows)]
pub fn set_system_proxy(server: &str, bypass: &str) -> bool {
    let server_w = wide(server);
    let bypass_w = wide(bypass);
    unsafe {
        let Some(key) = open_settings_key(true) else {
            return false;
        };
        let ok = set_dword(key, "ProxyEnable", 1)
            && set_sz(key, "ProxyServer", &server_w)
            && set_sz(key, "ProxyOverride", &bypass_w);
        ffi::RegCloseKey(key);
        if !ok {
            return false;
        }
        refresh_wininet();
        true
    }
}

#[cfg(windows)]
pub fn clear_system_proxy() -> bool {
    unsafe {
        let Some(key) = open_settings_key(true) else {
            return false;
        };
        let ok = set_dword(key, "ProxyEnable", 0);
        ffi::RegCloseKey(key);
        if !ok {
            return false;
        }
        refresh_wininet();
        true
    }
}

#[cfg(windows)]
pub fn current_proxy() -> Option<(bool, String)> {
    unsafe {
        let key = open_settings_key(false)?;
        let enabled = query_dword(key, "ProxyEnable").unwrap_or(0) == 1;
        let server = query_sz(key, "ProxyServer");
        ffi::RegCloseKey(key);
        server.map(|server| (enabled, server))
    }
}

#[cfg(not(windows))]
pub fn set_system_proxy(_server: &str, _bypass: &str) -> bool {
    false
}

#[cfg(not(windows))]
pub fn clear_system_proxy() -> bool {
    false
}

#[cfg(not(windows))]
pub fn current_proxy() -> Option<(bool, String)> {
    None
}

#[cfg(windows)]
mod ffi {
    pub type HKEY = *mut core::ffi::c_void;
    pub const HKEY_CURRENT_USER: HKEY = 0x8000_0001usize as HKEY;
    pub const KEY_SET_VALUE: u32 = 0x0002;
    pub const KEY_READ: u32 = 0x0002_0019;
    pub const ERROR_SUCCESS: i32 = 0;
    pub const REG_SZ: u32 = 1;
    pub const REG_DWORD: u32 = 4;

    #[link(name = "advapi32")]
    extern "system" {
        pub fn RegOpenKeyExW(
            key: HKEY,
            subkey: *const u16,
            options: u32,
            sam_desired: u32,
            result: *mut HKEY,
        ) -> i32;
        pub fn RegSetValueExW(
            key: HKEY,
            name: *const u16,
            reserved: u32,
            kind: u32,
            data: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
        pub fn RegQueryValueExW(
            key: HKEY,
            name: *const u16,
            reserved: *mut u32,
            kind: *mut u32,
            data: *mut core::ffi::c_void,
            size: *mut u32,
        ) -> i32;
        pub fn RegCloseKey(key: HKEY) -> i32;
    }

    #[link(name = "wininet")]
    extern "system" {
        pub fn InternetSetOptionW(
            internet: *mut core::ffi::c_void,
            option: u32,
            buffer: *mut core::ffi::c_void,
            buffer_len: u32,
        ) -> i32;
    }
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
unsafe fn open_settings_key(write: bool) -> Option<ffi::HKEY> {
    let subkey = wide(INTERNET_SETTINGS_SUBKEY);
    let mut key: ffi::HKEY = std::ptr::null_mut();
    let access = if write {
        ffi::KEY_SET_VALUE
    } else {
        ffi::KEY_READ
    };
    if ffi::RegOpenKeyExW(ffi::HKEY_CURRENT_USER, subkey.as_ptr(), 0, access, &mut key)
        != ffi::ERROR_SUCCESS
    {
        return None;
    }
    Some(key)
}

#[cfg(windows)]
unsafe fn set_dword(key: ffi::HKEY, name: &str, value: u32) -> bool {
    let name_w = wide(name);
    ffi::RegSetValueExW(
        key,
        name_w.as_ptr(),
        0,
        ffi::REG_DWORD,
        &value as *const u32 as *const core::ffi::c_void,
        4,
    ) == ffi::ERROR_SUCCESS
}

#[cfg(windows)]
unsafe fn set_sz(key: ffi::HKEY, name: &str, value_w: &[u16]) -> bool {
    let name_w = wide(name);
    let bytes = (value_w.len() * 2) as u32;
    ffi::RegSetValueExW(
        key,
        name_w.as_ptr(),
        0,
        ffi::REG_SZ,
        value_w.as_ptr() as *const core::ffi::c_void,
        bytes,
    ) == ffi::ERROR_SUCCESS
}

#[cfg(windows)]
unsafe fn query_dword(key: ffi::HKEY, name: &str) -> Option<u32> {
    let name_w = wide(name);
    let mut value: u32 = 0;
    let mut size = 4u32;
    let result = ffi::RegQueryValueExW(
        key,
        name_w.as_ptr(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        &mut value as *mut u32 as *mut core::ffi::c_void,
        &mut size,
    );
    if result == ffi::ERROR_SUCCESS && size == 4 {
        Some(value)
    } else {
        None
    }
}

#[cfg(windows)]
unsafe fn query_sz(key: ffi::HKEY, name: &str) -> Option<String> {
    let name_w = wide(name);
    let mut kind = 0u32;
    let mut size = 0u32;
    if ffi::RegQueryValueExW(
        key,
        name_w.as_ptr(),
        std::ptr::null_mut(),
        &mut kind,
        std::ptr::null_mut(),
        &mut size,
    ) != ffi::ERROR_SUCCESS
        || kind != ffi::REG_SZ
        || size == 0
    {
        return None;
    }
    let mut buf = vec![0u16; (size as usize + 1) / 2];
    let mut actual = size;
    if ffi::RegQueryValueExW(
        key,
        name_w.as_ptr(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        buf.as_mut_ptr() as *mut core::ffi::c_void,
        &mut actual,
    ) != ffi::ERROR_SUCCESS
    {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    buf.truncate(end);
    Some(String::from_utf16_lossy(&buf))
}

#[cfg(windows)]
unsafe fn refresh_wininet() {
    ffi::InternetSetOptionW(
        std::ptr::null_mut(),
        INTERNET_OPTION_SETTINGS_CHANGED,
        std::ptr::null_mut(),
        0,
    );
    ffi::InternetSetOptionW(
        std::ptr::null_mut(),
        INTERNET_OPTION_REFRESH,
        std::ptr::null_mut(),
        0,
    );
}
