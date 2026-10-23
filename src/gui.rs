use std::{
    collections::{HashMap, VecDeque},
    fs,
    net::{IpAddr, SocketAddr, TcpStream},
    num::NonZeroUsize,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use eframe::egui;
use eyre::Result;
use network_interface::NetworkInterfaceConfig;
use serde::{Deserialize, Serialize};
use tracing_subscriber::prelude::*;

use crate::{
    dispatcher::{RawWeightedAddress, WeightedAddress},
    icon,
    net::get_valid_addresses,
    server::{spawn_server, ServerHandle, PHASE_FAILED, PHASE_RUNNING, PHASE_STOPPED},
    single_instance,
    sysproxy, wintray,
};

const LOG_LINES_LIMIT: usize = 500;

const APP_TITLE: &str = "Unknown Dispatch";

#[derive(Clone)]
pub struct LogBuffer(pub Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBufferWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut sink = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        sink.extend_from_slice(buf);
        if sink.len() > 64 * 1024 {
            if let Some(pos) = sink.iter().rposition(|&b| b == b'\n') {
                sink.drain(..=pos);
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub struct LogBufferWriter(Arc<Mutex<Vec<u8>>>);

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogBufferWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogBufferWriter(Arc::clone(&self.0))
    }
}

fn install_gui_logging(buffer: LogBuffer, verbose: bool) {
    let stdout_layer = tracing_subscriber::fmt::layer().with_target(false);
    let buffer_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_ansi(false)
        .with_writer(buffer);
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(if verbose { "debug" } else { "info" })
    });
    tracing_subscriber::registry()
        .with(stdout_layer)
        .with(buffer_layer)
        .with(filter)
        .with(tracing_error::ErrorLayer::default())
        .init();
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SavedInterface {
    name: String,
    weight: u32,
    enabled: bool,
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ProxyMode {
    BuiltIn,
    External,
}

#[derive(Serialize, Deserialize)]
struct Settings {
    listen: String,
    port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<ProxyMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    http_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    system_proxy: Option<bool>,
    #[serde(default = "default_http_port")]
    http_port: u16,
    #[serde(default)]
    interfaces: Vec<SavedInterface>,
}

fn default_http_port() -> u16 {
    8080
}

struct InterfaceRow {
    name: String,
    v4: Option<String>,
    v6: Option<String>,
    enabled: bool,
    weight: u32,
}

#[derive(Clone, Copy, PartialEq)]
enum Status {
    Idle,
    Running,
    Stopping,
    Failed,
}

pub struct DispatchGui {
    #[allow(dead_code)]
    runtime: tokio::runtime::Runtime,
    rt_handle: tokio::runtime::Handle,
    log_sink: LogBuffer,
    pending_log: Vec<u8>,
    log_lines: VecDeque<String>,
    interfaces: Vec<InterfaceRow>,
    listen: String,
    port: u16,
    mode: ProxyMode,
    http_port: u16,
    sysproxy_active: Arc<AtomicBool>,
    session: Arc<AtomicUsize>,
    exit_flag: Arc<AtomicBool>,
    tray_installed: bool,
    status: Status,
    last_error: Option<String>,
    server: Option<ServerHandle>,
    config_path: Option<PathBuf>,
}

pub fn run(verbose: bool) -> Result<()> {
    // If another instance is already running (its window may merely be
    // hidden to the system tray), activate it and exit without creating a
    // second window, tray icon or proxy server.
    let _instance_guard = match single_instance::acquire(APP_TITLE) {
        single_instance::Acquisition::Acquired(guard) => Some(guard),
        single_instance::Acquisition::AlreadyRunning => {
            tracing::info!("Another instance is already running; brought its window to the front instead.");
            return Ok(());
        }
    };

    let runtime = tokio::runtime::Runtime::new()?;
    let rt_handle = runtime.handle().clone();

    let log_sink = LogBuffer(Arc::new(Mutex::new(Vec::new())));
    install_gui_logging(log_sink.clone(), verbose);

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP_TITLE)
        .with_inner_size([780.0, 620.0])
        .with_min_inner_size([600.0, 440.0]);
    if let Some((width, height, rgba)) = icon::load_app_icon_rgba() {
        viewport = viewport.with_icon(egui::IconData {
            width,
            height,
            rgba,
        });
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        APP_TITLE,
        options,
        Box::new(move |_cc| Box::new(DispatchGui::new(runtime, rt_handle, log_sink))),
    )
    .map_err(|err| eyre::eyre!("GUI error: {err}"))?;

    Ok(())
}

impl DispatchGui {
    fn new(
        runtime: tokio::runtime::Runtime,
        rt_handle: tokio::runtime::Handle,
        log_sink: LogBuffer,
    ) -> Self {
        let config_path = config_path();

        let mut app = Self {
            runtime,
            rt_handle,
            log_sink,
            pending_log: Vec::new(),
            log_lines: VecDeque::new(),
            interfaces: Vec::new(),
            listen: "127.0.0.1".to_string(),
            port: 1080,
            mode: ProxyMode::BuiltIn,
            http_port: 8080,
            sysproxy_active: Arc::new(AtomicBool::new(false)),
            session: Arc::new(AtomicUsize::new(0)),
            exit_flag: Arc::new(AtomicBool::new(false)),
            tray_installed: false,
            status: Status::Idle,
            last_error: None,
            server: None,
            config_path,
        };

        let saved = app.load_settings();
        app.refresh_interfaces();
        if let Some(settings) = saved {
            app.apply_settings(settings);
        }
        app.recover_stale_sysproxy();
        app
    }

    fn refresh_interfaces(&mut self) {
        let prior: HashMap<String, (bool, u32)> = self
            .interfaces
            .iter()
            .map(|row| (row.name.clone(), (row.enabled, row.weight)))
            .collect();

        self.interfaces.clear();

        match network_interface::NetworkInterface::show() {
            Ok(list) => {
                for iface in list {
                    if iface.addr.is_empty() {
                        continue;
                    }
                    let addrs = get_valid_addresses(&iface.addr);
                    if addrs.is_empty() {
                        continue;
                    }
                    let v4 = addrs.iter().find(|a| a.is_ipv4()).map(ToString::to_string);
                    let v6 = addrs.iter().find(|a| a.is_ipv6()).map(ToString::to_string);
                    let (enabled, weight) = prior.get(&iface.name).copied().unwrap_or((false, 1));
                    self.interfaces.push(InterfaceRow {
                        name: iface.name,
                        v4,
                        v6,
                        enabled,
                        weight: weight.max(1),
                    });
                }
            }
            Err(err) => {
                tracing::error!("Failed to list network interfaces: {err}");
                self.last_error = Some(format!("Failed to list network interfaces: {err}"));
            }
        }

        self.interfaces
            .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    }

    fn selected_count(&self) -> usize {
        self.interfaces.iter().filter(|row| row.enabled).count()
    }

    fn start(&mut self) {
        self.last_error = None;

        let listen: IpAddr = match self.listen.trim().parse() {
            Ok(ip) => ip,
            Err(_) => {
                self.last_error = Some(format!("Invalid listen IP address: `{}`", self.listen));
                tracing::error!("{}", self.last_error.as_deref().unwrap_or_default());
                return;
            }
        };

        if self.mode == ProxyMode::BuiltIn && self.http_port == self.port {
            self.last_error =
                Some("The HTTP port must be different from the SOCKS port.".to_string());
            tracing::error!("{}", self.last_error.as_deref().unwrap_or_default());
            return;
        }

        let raws: Vec<RawWeightedAddress> = self
            .interfaces
            .iter()
            .filter(|row| row.enabled)
            .map(|row| {
                RawWeightedAddress::new(
                    row.name.clone(),
                    NonZeroUsize::new(row.weight.max(1) as usize)
                        .expect("weight is clamped to >= 1"),
                )
            })
            .collect();

        if raws.is_empty() {
            self.last_error =
                Some("Select at least one network interface before starting.".to_string());
            tracing::error!("{}", self.last_error.as_deref().unwrap_or_default());
            return;
        }

        match WeightedAddress::resolve(raws) {
            Ok(addresses) => {
                let built_in = self.mode == ProxyMode::BuiltIn;
                let socks_addr = SocketAddr::new(listen, self.port);
                let http_addr = if built_in {
                    Some(SocketAddr::new(listen, self.http_port))
                } else {
                    None
                };
                tracing::info!(
                    "Starting proxies on {socks_addr} (SOCKS5){}",
                    http_addr
                        .map(|addr| format!(" and {addr} (HTTP)"))
                        .unwrap_or_default()
                );
                let handle = spawn_server(&self.rt_handle, socks_addr, http_addr, addresses);
                self.server = Some(handle);
                self.status = Status::Running;
                self.session.fetch_add(1, Ordering::SeqCst);
                if built_in {
                    self.spawn_system_proxy_watch(listen);
                }
                self.save_settings();
            }
            Err(err) => {
                self.last_error = Some(format!("{err:?}"));
                tracing::error!("{err:?}");
                self.status = Status::Failed;
            }
        }
    }

    fn stop(&mut self) {
        self.session.fetch_add(1, Ordering::SeqCst);
        self.clear_sysproxy_if_set();
        if let Some(handle) = &self.server {
            tracing::info!("Stopping proxies");
            handle.stop();
            self.status = Status::Stopping;
            self.save_settings();
        }
    }

    fn spawn_system_proxy_watch(&mut self, listen: IpAddr) {
        let session = Arc::clone(&self.session);
        let active = Arc::clone(&self.sysproxy_active);
        let my_session = session.load(Ordering::SeqCst);
        let http_port = self.http_port;

        let probe_ip = match listen {
            IpAddr::V4(v4) if v4.is_unspecified() => IpAddr::from([127, 0, 0, 1]),
            IpAddr::V6(v6) if v6.is_unspecified() => IpAddr::from([0, 0, 0, 0, 0, 0, 0, 1]),
            other => other,
        };
        let proxy_host = match listen {
            IpAddr::V4(v4) if v4.is_unspecified() || v4.is_loopback() => "127.0.0.1".to_string(),
            IpAddr::V6(v6) if v6.is_unspecified() || v6.is_loopback() => "[::1]".to_string(),
            other => other.to_string(),
        };

        let spawned = std::thread::Builder::new()
            .name("sysproxy-watch".into())
            .spawn(move || {
                let target = format!("{proxy_host}:{http_port}");
                let target_addr = SocketAddr::new(probe_ip, http_port);

                tracing::info!(
                    "[~] Waiting for the HTTP proxy on {target} before changing the Windows System Proxy..."
                );
                let deadline = Instant::now() + Duration::from_secs(8);
                let mut ready = false;
                while Instant::now() < deadline {
                    if session.load(Ordering::SeqCst) != my_session {
                        return;
                    }
                    if TcpStream::connect_timeout(&target_addr, Duration::from_millis(400)).is_ok()
                    {
                        ready = true;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(150));
                }
                if !ready {
                    tracing::warn!(
                        "[-] The HTTP proxy did not become ready in time. Aborting - the Windows System Proxy was NOT changed."
                    );
                    return;
                }
                if session.load(Ordering::SeqCst) != my_session {
                    return;
                }
                if sysproxy::set_system_proxy(&target, &sysproxy::bypass_list()) {
                    active.store(true, Ordering::SeqCst);
                    tracing::info!(
                        "[+] Windows System Proxy enabled: {target} (local addresses bypassed)."
                    );
                } else {
                    tracing::error!("[-] Failed to set the Windows System Proxy.");
                }
            });
        if let Err(err) = spawned {
            tracing::error!("[-] Could not spawn the system proxy watcher: {err}");
        }
    }

    fn clear_sysproxy_if_set(&mut self) {
        if self.sysproxy_active.swap(false, Ordering::SeqCst) {
            if sysproxy::clear_system_proxy() {
                tracing::info!("[+] System Proxy reset. Internet restored.");
            } else {
                tracing::error!(
                    "[-] Failed to clear the system proxy! If internet is broken: Windows Settings > Network & Internet > Proxy > turn 'Use a proxy server' OFF."
                );
            }
        }
    }

    fn recover_stale_sysproxy(&mut self) {
        if let Some((enabled, server)) = sysproxy::current_proxy() {
            if enabled && server.trim() == format!("127.0.0.1:{}", self.http_port) {
                if sysproxy::clear_system_proxy() {
                    tracing::info!(
                        "[~] Cleared a stale system proxy left behind by a previous session."
                    );
                }
            }
        }
    }

    fn exit_completely(&mut self) {
        tracing::info!("[~] Exiting...");
        self.session.fetch_add(1, Ordering::SeqCst);
        self.clear_sysproxy_if_set();
        if let Some(handle) = &self.server {
            handle.stop();
        }
        self.save_settings();
        wintray::remove();
        std::process::exit(0);
    }

    fn poll_server(&mut self) {
        let phase = self
            .server
            .as_ref()
            .map(|handle| handle.phase.load(Ordering::SeqCst));

        match phase {
            Some(PHASE_STOPPED) => {
                self.server = None;
                if self.status != Status::Failed {
                    self.status = Status::Idle;
                }
            }
            Some(PHASE_FAILED) => {
                let failure = self
                    .server
                    .as_ref()
                    .and_then(|handle| {
                        handle
                            .failure
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .clone()
                    })
                    .unwrap_or_else(|| "Unknown server error".to_string());
                self.last_error = Some(failure);
                self.server = None;
                self.status = Status::Failed;
                self.clear_sysproxy_if_set();
            }
            Some(PHASE_RUNNING) => {
                if !matches!(self.status, Status::Running | Status::Stopping) {
                    self.status = Status::Running;
                }
            }
            _ => {}
        }
    }

    fn drain_logs(&mut self) {
        {
            let mut sink = self
                .log_sink
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !sink.is_empty() {
                self.pending_log.extend(sink.drain(..));
            }
        }

        if self.pending_log.is_empty() {
            return;
        }

        if let Some(pos) = self.pending_log.iter().rposition(|&b| b == b'\n') {
            let complete: Vec<u8> = self.pending_log.drain(..=pos).collect();
            let text = String::from_utf8_lossy(&complete);
            for line in text.lines() {
                self.log_lines.push_back(line.to_string());
            }
            while self.log_lines.len() > LOG_LINES_LIMIT {
                self.log_lines.pop_front();
            }
        }
    }

    fn load_settings(&self) -> Option<Settings> {
        let path = self.config_path.as_ref()?;
        let contents = fs::read_to_string(path).ok()?;
        match serde_json::from_str::<Settings>(&contents) {
            Ok(settings) => Some(settings),
            Err(err) => {
                tracing::warn!("Ignoring malformed settings file {path:?}: {err}");
                None
            }
        }
    }

    fn apply_settings(&mut self, settings: Settings) {
        if !settings.listen.trim().is_empty() {
            self.listen = settings.listen;
        }
        if settings.port > 0 {
            self.port = settings.port;
        }
        self.mode =
            settings
                .mode
                .unwrap_or_else(|| match (settings.http_enabled, settings.system_proxy) {
                    (Some(false), _) => ProxyMode::External,
                    (_, Some(false)) => ProxyMode::External,
                    _ => ProxyMode::BuiltIn,
                });
        if settings.http_port > 0 {
            self.http_port = settings.http_port;
        }
        let saved: HashMap<String, (bool, u32)> = settings
            .interfaces
            .into_iter()
            .map(|iface| (iface.name, (iface.enabled, iface.weight.max(1))))
            .collect();
        for row in &mut self.interfaces {
            if let Some((enabled, weight)) = saved.get(&row.name) {
                row.enabled = *enabled;
                row.weight = *weight;
            }
        }
    }

    fn save_settings(&self) {
        let Some(path) = self.config_path.as_ref() else {
            return;
        };
        let settings = Settings {
            listen: self.listen.clone(),
            port: self.port,
            mode: Some(self.mode),
            http_enabled: None,
            system_proxy: None,
            http_port: self.http_port,
            interfaces: self
                .interfaces
                .iter()
                .map(|row| SavedInterface {
                    name: row.name.clone(),
                    weight: row.weight,
                    enabled: row.enabled,
                })
                .collect(),
        };
        match serde_json::to_string_pretty(&settings) {
            Ok(json) => {
                if let Err(err) = fs::write(path, json) {
                    tracing::warn!("Failed to save settings to {path:?}: {err}");
                }
            }
            Err(err) => tracing::warn!("Failed to serialize settings: {err}"),
        }
    }

    fn status_color(&self) -> egui::Color32 {
        match self.status {
            Status::Idle => egui::Color32::GRAY,
            Status::Running => egui::Color32::from_rgb(76, 175, 80),
            Status::Stopping => egui::Color32::from_rgb(255, 170, 60),
            Status::Failed => egui::Color32::from_rgb(230, 80, 80),
        }
    }

    fn log_line_color(line: &str) -> Option<egui::Color32> {
        if line.contains("ERROR") {
            Some(egui::Color32::from_rgb(230, 80, 80))
        } else if line.contains(" WARN") {
            Some(egui::Color32::from_rgb(255, 170, 60))
        } else if line.contains("DEBUG") || line.contains("TRACE") {
            Some(egui::Color32::GRAY)
        } else {
            None
        }
    }
}

impl eframe::App for DispatchGui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_server();
        self.drain_logs();

        if !self.tray_installed {
            if let Some(hwnd) = native_hwnd(ctx) {
                let wake_ctx = ctx.clone();
                let sysproxy_active = Arc::clone(&self.sysproxy_active);
                let session = Arc::clone(&self.session);
                let config = wintray::TrayConfig {
                    exit_flag: Arc::clone(&self.exit_flag),
                    on_exit: Arc::new(move || {
                        tracing::info!("[~] Exiting...");
                        session.fetch_add(1, Ordering::SeqCst);
                        if sysproxy_active.swap(false, Ordering::SeqCst) {
                            if sysproxy::clear_system_proxy() {
                                tracing::info!("[+] System Proxy reset. Internet restored.");
                            } else {
                                tracing::error!(
                                    "[-] Failed to clear the system proxy! If internet is broken: Windows Settings > Network & Internet > Proxy > turn 'Use a proxy server' OFF."
                                );
                            }
                        }
                        wintray::remove();
                        std::process::exit(0);
                    }),
                    on_wake: Arc::new(move || wake_ctx.request_repaint()),
                    tooltip: APP_TITLE.to_string(),
                };
                if !wintray::install(hwnd, config) {
                    tracing::warn!("System tray integration is unavailable in this session.");
                }
                self.tray_installed = true;
            }
        }

        if self.exit_flag.load(Ordering::SeqCst) {
            self.exit_completely();
            return;
        }

        if ctx.input(|input| input.viewport().close_requested()) {
            self.clear_sysproxy_if_set();
            if let Some(handle) = &self.server {
                handle.stop();
            }
            self.save_settings();
            return;
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Unknown Dispatch");
            ui.add_space(6.0);

            egui::Grid::new("controls")
                .num_columns(4)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Listen IP:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.listen)
                            .desired_width(240.0)
                            .hint_text("127.0.0.1"),
                    );
                    ui.label("SOCKS port:");
                    ui.add(
                        egui::DragValue::new(&mut self.port)
                            .clamp_range(1..=65535)
                            .speed(1.0),
                    );
                    ui.end_row();

                    ui.label("Proxy mode:");
                    ui.radio_value(&mut self.mode, ProxyMode::BuiltIn, "Built-in (System-wide)");
                    ui.radio_value(&mut self.mode, ProxyMode::External, "External");
                    ui.end_row();

                    if self.mode == ProxyMode::BuiltIn {
                        ui.label("HTTP port:");
                        ui.add(
                            egui::DragValue::new(&mut self.http_port)
                                .clamp_range(1..=65535)
                                .speed(1.0),
                        );
                        ui.end_row();
                    }
                });
            ui.add_space(6.0);

            let can_start = self.server.is_none() && self.selected_count() > 0;
            let can_stop = self.server.is_some();

            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        can_start,
                        egui::Button::new(egui::RichText::new("Start").strong()),
                    )
                    .clicked()
                {
                    self.start();
                }
                if ui
                    .add_enabled(
                        can_stop,
                        egui::Button::new(egui::RichText::new("Stop").strong()),
                    )
                    .clicked()
                {
                    self.stop();
                }

                ui.separator();

                ui.label(egui::RichText::new("●").color(self.status_color()));
                ui.label(match self.status {
                    Status::Idle => "Stopped".to_string(),
                    Status::Running => {
                        let http = if self.mode == ProxyMode::BuiltIn {
                            format!("  ·  HTTP on {}:{}", self.listen.trim(), self.http_port)
                        } else {
                            String::new()
                        };
                        let system_wide = if self.sysproxy_active.load(Ordering::Relaxed) {
                            "  ·  system-wide ON"
                        } else {
                            ""
                        };
                        format!(
                            "Running  ·  SOCKS on {}:{}{}{}",
                            self.listen.trim(),
                            self.port,
                            http,
                            system_wide
                        )
                    }
                    Status::Stopping => "Stopping...".to_string(),
                    Status::Failed => "Failed".to_string(),
                });

                if let Some(handle) = &self.server {
                    ui.separator();
                    let active = handle.stats.active.load(Ordering::Relaxed);
                    let total = handle.stats.total_connections.load(Ordering::Relaxed);
                    let bytes = handle.stats.total_bytes.load(Ordering::Relaxed);
                    ui.label(format!(
                        "connections: {active} active, {total} total, {} relayed",
                        humanize_bytes(bytes)
                    ));
                }
            });

            if let Some(error) = &self.last_error {
                ui.add_space(4.0);
                ui.colored_label(
                    egui::Color32::from_rgb(230, 80, 80),
                    format!("Error: {error}"),
                );
            }

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                ui.heading("Network interfaces");
                if ui.button("Refresh").clicked() {
                    self.refresh_interfaces();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{} selected", self.selected_count()));
                });
            });
            ui.add_space(2.0);

            egui::ScrollArea::vertical()
                .id_source("interfaces")
                .auto_shrink([false, false])
                .max_height(170.0)
                .show(ui, |ui| {
                    egui::Grid::new("interfaces")
                        .striped(true)
                        .num_columns(4)
                        .spacing([14.0, 6.0])
                        .show(ui, |ui| {
                            for row in &mut self.interfaces {
                                ui.checkbox(&mut row.enabled, "");
                                ui.strong(&row.name);
                                ui.monospace(row.v4.as_deref().unwrap_or("—"));
                                ui.monospace(row.v6.as_deref().unwrap_or("—"));
                                ui.add_space(12.0);
                                ui.horizontal(|ui| {
                                    ui.label("weight");
                                    ui.add(
                                        egui::DragValue::new(&mut row.weight)
                                            .clamp_range(1..=16)
                                            .speed(1.0)
                                            .suffix("x"),
                                    );
                                });
                                ui.end_row();
                            }
                        });
                });

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(4.0);

            ui.heading("Log");
            egui::ScrollArea::vertical()
                .id_source("log")
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &self.log_lines {
                        let color = Self::log_line_color(line);
                        match color {
                            Some(color) => ui.monospace(egui::RichText::new(line).color(color)),
                            None => ui.monospace(line),
                        };
                    }
                });
        });

        if self.server.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }
}

#[cfg(windows)]
fn native_hwnd(_ctx: &egui::Context) -> Option<*mut core::ffi::c_void> {
    wintray::find_window_by_title(APP_TITLE)
}

#[cfg(not(windows))]
fn native_hwnd(_ctx: &egui::Context) -> Option<*mut core::ffi::c_void> {
    None
}

fn humanize_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn config_path() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "unknown-dispatch")?;
    let dir = dirs.data_local_dir().join("gui");
    fs::create_dir_all(&dir).ok()?;
    Some(dir.join("settings.json"))
}
