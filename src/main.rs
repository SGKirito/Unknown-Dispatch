#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::{net::IpAddr, str::FromStr};

use clap::Parser;
use debug::LogStrategy;
use dispatcher::{RawWeightedAddress, WeightedAddress};
use eyre::Result;

mod debug;
mod dispatcher;
mod gui;
mod http_bridge;
mod icon;
mod list;
mod net;
mod server;
mod single_instance;
mod socks;
mod sysproxy;
mod wintray;

#[cfg(windows)]
mod winconsole {
    type Handle = *mut core::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
        fn SetStdHandle(which: u32, handle: Handle) -> i32;
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *mut u8,
            disposition: u32,
            flags: u32,
            template: Handle,
        ) -> Handle;
    }

    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
    const STD_INPUT_HANDLE: u32 = 0xFFFF_FFF6;
    const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;
    const STD_ERROR_HANDLE: u32 = 0xFFFF_FFF4;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;
    const OPEN_EXISTING: u32 = 0x3;

    pub fn attach() {
        unsafe {
            if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                return;
            }
            fn wide(s: &str) -> Vec<u16> {
                s.encode_utf16().chain(std::iter::once(0)).collect()
            }
            let conout = wide("CONOUT$");
            let conin = wide("CONIN$");
            let out = CreateFileW(
                conout.as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            let input = CreateFileW(
                conin.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            if out != -1isize as Handle {
                SetStdHandle(STD_OUTPUT_HANDLE, out);
                SetStdHandle(STD_ERROR_HANDLE, out);
                SetStdHandle(STD_INPUT_HANDLE, input);
            }
        }
    }
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Opt {
    #[arg(short, long)]
    debug: bool,
    #[arg(long)]
    gui: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Parser, Debug)]
enum Command {
    List,
    Start {
        #[arg(default_value = "127.0.0.1", long)]
        ip: IpAddr,
        #[arg(default_value = "1080", long)]
        port: u16,
        #[arg(long)]
        http_port: Option<u16>,
        #[arg(required = true, value_parser = RawWeightedAddress::from_str)]
        addresses: Vec<RawWeightedAddress>,
    },
}

fn main() -> Result<()> {
    let opt = Opt::parse();

    #[cfg(windows)]
    winconsole::attach();

    match opt.command {
        Some(Command::List) => {
            let _guard = debug::install(
                if opt.debug {
                    LogStrategy::Stdout
                } else {
                    LogStrategy::File
                },
                opt.debug,
            )?;
            list::list();
        }
        Some(Command::Start {
            ip,
            port,
            http_port,
            addresses,
        }) => {
            let _guard = debug::install(
                if opt.debug {
                    LogStrategy::Stdout
                } else {
                    LogStrategy::File
                },
                opt.debug,
            )?;
            let addresses = WeightedAddress::resolve(addresses)?;
            server::server(ip, port, http_port, addresses)?
        }
        None => gui::run(opt.debug)?,
    }

    Ok(())
}
