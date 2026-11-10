use evdev::{AbsoluteAxisCode, Device, KeyCode, UinputAbsSetup, AttributeSet};
use evdev::uinput::VirtualDevice;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Clear},
    Terminal,
};
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use crossterm::{
    event::{self, Event, KeyCode as CrossKeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use std::io::{stdout, Write};

const AXIS_MIN: i32 = -32767;
const AXIS_MAX: i32 = 32767;
const CENTER_TICK_HZ: u64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Config {
    mouse_device: String,
    grab_button: u16,
    sensitivity: f64,
    center_strength_x: f64,
    center_strength_y: f64,
    center_while_grabbed: bool,
    audio_volume: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mouse_device: String::new(),
            grab_button: KeyCode::BTN_SIDE.0,
            sensitivity: 1000.0,
            center_strength_x: 0.04,
            center_strength_y: 0.02,
            center_while_grabbed: true,
            audio_volume: 0.5,
        }
    }
}

fn config_path() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("mouse2joy").join("config.toml")
}

fn load_config() -> Config {
    let path = config_path();
    if let Ok(s) = fs::read_to_string(&path) {
        if let Ok(cfg) = toml::from_str::<Config>(&s) {
            return cfg;
        }
    }
    Config::default()
}

fn save_config(cfg: &Config) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(s) = toml::to_string_pretty(cfg) {
        let _ = fs::write(path, s);
    }
}

fn detect_mouse_device(configured: &str) -> Option<PathBuf> {
    if !configured.is_empty() && Path::new(configured).exists() {
        return Some(PathBuf::from(configured));
    }
    let by_id = Path::new("/dev/input/by-id");
    if let Ok(entries) = fs::read_dir(by_id) {
        let mut candidates: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                name.contains("event-mouse") || (name.contains("-event-") && name.contains("Mouse"))
            })
            .collect();
        candidates.sort();
        if let Some(p) = candidates.first() {
            return Some(p.clone());
        }
    }
    None
}

fn list_all_mouse_devices() -> Vec<(String, PathBuf)> {
    let mut devices = Vec::new();
    let by_id = Path::new("/dev/input/by-id");
    if let Ok(entries) = fs::read_dir(by_id) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.contains("event-mouse") || (name.contains("-event-") && name.contains("Mouse")) {
                devices.push((name.to_string(), path));
            }
        }
    }
    devices.sort_by(|a, b| a.0.cmp(&b.0));
    devices
}

#[derive(Clone, Debug)]
enum UiEvent {
    Grabbed,
    RawButton(u16),
    DeviceError(String),
}

struct AudioPlayer {
    spec: libpulse_binding::sample::Spec,
}

impl AudioPlayer {
    fn new() -> Option<Self> {
        Some(AudioPlayer {
            spec: libpulse_binding::sample::Spec {
                format: libpulse_binding::sample::Format::S16le,
                channels: 1,
                rate: 44100,
            },
        })
    }

    fn play(&self, cue: u8, volume: f64) {
        let vol = volume.clamp(0.0, 1.0);
        if vol <= 0.001 { return; }
        let spec = self.spec;
        let rate = spec.rate as f64;
        let samples = ((rate * 0.1) as u32) as usize;
        thread::spawn(move || {
            if let Ok(pa) = libpulse_simple_binding::Simple::new(
                None,
                "mouse2joy",
                libpulse_binding::stream::Direction::Playback,
                None,
                "cue",
                &spec,
                None,
                None,
            ) {
                let mut buf = Vec::with_capacity(samples * 2);
                for i in 0..samples {
                    let t = i as f64 / rate;
                    let freq = if cue == 1 { 880.0 } else { 440.0 };
                    let env = if i < 8 { i as f64 / 8.0 } else if i + 8 > samples { (samples - i) as f64 / 8.0 } else { 1.0 };
                    let s = (2.0 * std::f64::consts::PI * freq * t).sin() * env;
                    let v = (s * vol * i16::MAX as f64) as i16;
                    buf.extend_from_slice(&v.to_le_bytes());
                }
                let _ = pa.write(&buf);
                let _ = pa.drain();
            }
        });
    }
}

struct SharedState {
    x: AtomicI32,
    y: AtomicI32,
    grabbed: AtomicBool,
    running: AtomicBool,
    sensitivity_fp: AtomicU32,
    center_x_fp: AtomicU32,
    center_y_fp: AtomicU32,
    center_while_grabbed: AtomicBool,
    volume_fp: AtomicU32,
    binding_mode: AtomicBool,
}

impl SharedState {
    fn new(cfg: &Config) -> Self {
        Self {
            x: AtomicI32::new(0),
            y: AtomicI32::new(0),
            grabbed: AtomicBool::new(false),
            running: AtomicBool::new(true),
            sensitivity_fp: AtomicU32::new((cfg.sensitivity as f32).to_bits()),
            center_x_fp: AtomicU32::new((cfg.center_strength_x as f32).to_bits()),
            center_y_fp: AtomicU32::new((cfg.center_strength_y as f32).to_bits()),
            center_while_grabbed: AtomicBool::new(cfg.center_while_grabbed),
            volume_fp: AtomicU32::new((cfg.audio_volume as f32).to_bits()),
            binding_mode: AtomicBool::new(false),
        }
    }

    fn f32_load(a: &AtomicU32) -> f32 { f32::from_bits(a.load(Ordering::Relaxed)) }
}

fn create_uinput() -> Result<Mutex<VirtualDevice>, Box<dyn std::error::Error>> {
    let abs_x = UinputAbsSetup::new(
        AbsoluteAxisCode::ABS_X,
        evdev::AbsInfo::new(0, AXIS_MIN, AXIS_MAX, 0, 0, 0),
    );
    let abs_y = UinputAbsSetup::new(
        AbsoluteAxisCode::ABS_Y,
        evdev::AbsInfo::new(0, AXIS_MIN, AXIS_MAX, 0, 0, 0),
    );
    let mut keys = AttributeSet::<KeyCode>::new();
    keys.insert(KeyCode::BTN_TRIGGER);
    keys.insert(KeyCode::BTN_THUMB);
    keys.insert(KeyCode::BTN_THUMB2);

    let dev = VirtualDevice::builder()?
        .name("mouse2joy Virtual Joystick")
        .with_absolute_axis(&abs_x)?
        .with_absolute_axis(&abs_y)?
        .with_keys(&keys)?
        .build()?;

    Ok(Mutex::new(dev))
}

fn centering_loop(shared: Arc<SharedState>, uinput: Arc<Mutex<VirtualDevice>>) {
    let interval = Duration::from_micros(1_000_000 / CENTER_TICK_HZ);
    let mut x: f32 = 0.0;
    let mut y: f32 = 0.0;

    while shared.running.load(Ordering::Relaxed) {
        let start = Instant::now();
        let grabbed = shared.grabbed.load(Ordering::Relaxed);
        let cwg = shared.center_while_grabbed.load(Ordering::Relaxed);
        let cx = SharedState::f32_load(&shared.center_x_fp);
        let cy = SharedState::f32_load(&shared.center_y_fp);

        let dx = shared.x.swap(0, Ordering::Relaxed) as f32;
        let dy = shared.y.swap(0, Ordering::Relaxed) as f32;
        x = (x + dx).clamp(AXIS_MIN as f32, AXIS_MAX as f32);
        y = (y + dy).clamp(AXIS_MIN as f32, AXIS_MAX as f32);

        if cwg || !grabbed {
            x -= x * cx;
            y -= y * cy;
            if x.abs() < 0.5 { x = 0.0; }
            if y.abs() < 0.5 { y = 0.0; }
        }

        let xi = x as i32;
        let yi = y as i32;
        if let Ok(mut ui) = uinput.lock() {
            let _ = ui.emit(&[
                evdev::InputEvent::new_now(evdev::EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_X.0, xi),
                evdev::InputEvent::new_now(evdev::EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Y.0, yi),
                evdev::InputEvent::new_now(evdev::EventType::SYNCHRONIZATION.0, 0, 0),
            ]);
        }

        let elapsed = start.elapsed();
        if elapsed < interval { thread::sleep(interval - elapsed); }
    }
}

fn input_loop(
    mut device: Device,
    shared: Arc<SharedState>,
    grab_button: Arc<AtomicU32>,
    ui_tx: mpsc::Sender<UiEvent>,
    audio: Option<Arc<AudioPlayer>>,
) {
    while shared.running.load(Ordering::Relaxed) {
        let events: Vec<evdev::InputEvent> = match device.fetch_events() {
            Ok(ev) => ev.collect(),
            Err(_) => { thread::sleep(Duration::from_millis(50)); continue; }
        };
        for ev in events {
            if !shared.running.load(Ordering::Relaxed) { break; }
            match ev.destructure() {
                evdev::EventSummary::Key(_, key_code, value) => {
                    if shared.binding_mode.load(Ordering::Relaxed) {
                        if value == 1 {
                            let _ = ui_tx.send(UiEvent::RawButton(key_code.0));
                        }
                        continue;
                    }

                    if key_code.0 == grab_button.load(Ordering::Relaxed) as u16 {
                        if value == 1 {
                            let now_grabbed = !shared.grabbed.load(Ordering::Relaxed);
                            shared.grabbed.store(now_grabbed, Ordering::Relaxed);
                            if let Some(a) = &audio {
                                a.play(if now_grabbed { 1 } else { 0 }, SharedState::f32_load(&shared.volume_fp).into());
                            }
                            let _ = ui_tx.send(UiEvent::Grabbed);
                            if now_grabbed { let _ = device.grab(); } else { let _ = device.ungrab(); }
                        }
                    }
                }
                evdev::EventSummary::RelativeAxis(_, axis, value) => {
                    if !shared.grabbed.load(Ordering::Relaxed) { continue; }
                    let sens = SharedState::f32_load(&shared.sensitivity_fp);
                    let delta = ((value as f32) * sens / 100.0) as i32;
                    match axis {
                        evdev::RelativeAxisCode::REL_X => { shared.x.fetch_add(delta, Ordering::Relaxed); }
                        evdev::RelativeAxisCode::REL_Y => { shared.y.fetch_add(delta, Ordering::Relaxed); }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    let _ = device.ungrab();
}

#[derive(PartialEq)]
enum MenuState {
    Main,
    DeviceSelect,
    Binding,
}

fn ui_terminal(
    shared: Arc<SharedState>,
    cfg_shared: Arc<Mutex<Config>>,
    ui_rx: mpsc::Receiver<UiEvent>,
    uinput: Arc<Mutex<VirtualDevice>>,
    grab_button_shared: Arc<AtomicU32>,
) {
    let shared_center = shared.clone();
    let uinput_center = uinput.clone();
    let _centering_thread = thread::spawn(move || {
        centering_loop(shared_center, uinput_center);
    });

    let mut terminal = Terminal::new(CrosstermBackend::new(stdout())).unwrap();
    let _ = crossterm::terminal::enable_raw_mode();
    let _ = execute!(stdout(), crossterm::terminal::EnterAlternateScreen);

    let mut menu_state = MenuState::Main;
    let mut selected_idx = 0;
    let mut device_list = list_all_mouse_devices();
    let mut selected_device_idx = 0;
    let mut last_save = Instant::now();

    while shared.running.load(Ordering::Relaxed) {
        // Handle UI events
        while let Ok(ev) = ui_rx.try_recv() {
            match ev {
                UiEvent::RawButton(code) => {
                    if menu_state == MenuState::Binding {
                        let mut cfg = cfg_shared.lock().unwrap();
                        cfg.grab_button = code;
                        grab_button_shared.store(code as u32, Ordering::Relaxed);
                        shared.binding_mode.store(false, Ordering::Relaxed);
                        menu_state = MenuState::Main;
                    }
                }
                UiEvent::Grabbed => {}
                UiEvent::DeviceError(msg) => {
                    // TODO: show error in UI
                }
            }
        }

        // Handle keyboard input
        if event::poll(Duration::from_millis(50)).unwrap() {
            if let Event::Key(key) = event::read().unwrap() {
                if key.kind == KeyEventKind::Press {
                    match menu_state {
                        MenuState::Main => match key.code {
                            CrossKeyCode::Char('q') => shared.running.store(false, Ordering::Relaxed),
                            CrossKeyCode::Up => selected_idx = if selected_idx == 0 { 6 } else { selected_idx - 1 },
                            CrossKeyCode::Down => selected_idx = if selected_idx == 6 { 0 } else { selected_idx + 1 },
                            CrossKeyCode::Left | CrossKeyCode::Char('-') => {
                                let mut cfg = cfg_shared.lock().unwrap();
                                match selected_idx {
                                    1 => cfg.sensitivity = (cfg.sensitivity - 50.0).max(50.0),
                                    2 => cfg.center_strength_x = (cfg.center_strength_x - 0.005).max(0.0),
                                    3 => cfg.center_strength_y = (cfg.center_strength_y - 0.005).max(0.0),
                                    4 => cfg.audio_volume = (cfg.audio_volume - 0.05).max(0.0),
                                    _ => {}
                                }
                                shared.sensitivity_fp.store((cfg.sensitivity as f32).to_bits(), Ordering::Relaxed);
                                shared.center_x_fp.store((cfg.center_strength_x as f32).to_bits(), Ordering::Relaxed);
                                shared.center_y_fp.store((cfg.center_strength_y as f32).to_bits(), Ordering::Relaxed);
                                shared.volume_fp.store((cfg.audio_volume as f32).to_bits(), Ordering::Relaxed);
                            }
                            CrossKeyCode::Right | CrossKeyCode::Char('+') | CrossKeyCode::Char('=') => {
                                let mut cfg = cfg_shared.lock().unwrap();
                                match selected_idx {
                                    1 => cfg.sensitivity = (cfg.sensitivity + 50.0).min(5000.0),
                                    2 => cfg.center_strength_x = (cfg.center_strength_x + 0.005).min(1.0),
                                    3 => cfg.center_strength_y = (cfg.center_strength_y + 0.005).min(1.0),
                                    4 => cfg.audio_volume = (cfg.audio_volume + 0.05).min(1.0),
                                    _ => {}
                                }
                                shared.sensitivity_fp.store((cfg.sensitivity as f32).to_bits(), Ordering::Relaxed);
                                shared.center_x_fp.store((cfg.center_strength_x as f32).to_bits(), Ordering::Relaxed);
                                shared.center_y_fp.store((cfg.center_strength_y as f32).to_bits(), Ordering::Relaxed);
                                shared.volume_fp.store((cfg.audio_volume as f32).to_bits(), Ordering::Relaxed);
                            }
                            CrossKeyCode::Enter => {
                                match selected_idx {
                                    0 => {
                                        shared.binding_mode.store(true, Ordering::Relaxed);
                                        menu_state = MenuState::Binding;
                                    }
                                    5 => {
                                        let mut cfg = cfg_shared.lock().unwrap();
                                        cfg.center_while_grabbed = !cfg.center_while_grabbed;
                                        shared.center_while_grabbed.store(cfg.center_while_grabbed, Ordering::Relaxed);
                                    }
                                    6 => {
                                        device_list = list_all_mouse_devices();
                                        selected_device_idx = 0;
                                        menu_state = MenuState::DeviceSelect;
                                    }
                                    _ => {}
                                }
                            }
                            CrossKeyCode::Char('e') => {
                                let cfg = cfg_shared.lock().unwrap();
                                let export_path = dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("mouse2joy_export.txt");
                                if let Ok(s) = toml::to_string_pretty(&*cfg) {
                                    let _ = fs::write(&export_path, s);
                                }
                            }
                            CrossKeyCode::Char('i') => {
                                let import_path = dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("mouse2joy_export.txt");
                                if let Ok(s) = fs::read_to_string(&import_path) {
                                    if let Ok(new_cfg) = toml::from_str::<Config>(&s) {
                                        let mut cfg = cfg_shared.lock().unwrap();
                                        *cfg = new_cfg;
                                        shared.sensitivity_fp.store((cfg.sensitivity as f32).to_bits(), Ordering::Relaxed);
                                        shared.center_x_fp.store((cfg.center_strength_x as f32).to_bits(), Ordering::Relaxed);
                                        shared.center_y_fp.store((cfg.center_strength_y as f32).to_bits(), Ordering::Relaxed);
                                        shared.volume_fp.store((cfg.audio_volume as f32).to_bits(), Ordering::Relaxed);
                                        shared.center_while_grabbed.store(cfg.center_while_grabbed, Ordering::Relaxed);
                                        grab_button_shared.store(cfg.grab_button as u32, Ordering::Relaxed);
                                        // Note: device change requires restart or hot-swap in input loop
                                    }
                                }
                            }
                            _ => {}
                        },
                        MenuState::DeviceSelect => match key.code {
                            CrossKeyCode::Esc => menu_state = MenuState::Main,
                            CrossKeyCode::Up => selected_device_idx = if selected_device_idx == 0 { device_list.len() - 1 } else { selected_device_idx - 1 },
                            CrossKeyCode::Down => selected_device_idx = if selected_device_idx == device_list.len() - 1 { 0 } else { selected_device_idx + 1 },
                            CrossKeyCode::Enter => {
                                if let Some((_, path)) = device_list.get(selected_device_idx) {
                                    let mut cfg = cfg_shared.lock().unwrap();
                                    cfg.mouse_device = path.to_string_lossy().to_string();
                                    menu_state = MenuState::Main;
                                    // Note: restart required for device change currently
                                }
                            }
                            _ => {}
                        },
                        MenuState::Binding => match key.code {
                            CrossKeyCode::Esc => {
                                shared.binding_mode.store(false, Ordering::Relaxed);
                                menu_state = MenuState::Main;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // Auto-save every 30 seconds if modified
        if last_save.elapsed() > Duration::from_secs(30) {
            let cfg = cfg_shared.lock().unwrap();
            save_config(&*cfg);
            last_save = Instant::now();
        }

        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(2)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(5),
                    Constraint::Min(0),
                    Constraint::Length(1),
                ])
                .split(f.size());

            let status_text = if shared.grabbed.load(Ordering::Relaxed) { "Status: GRABBED" } else { "Status: RELEASED" };
            let status_style = if shared.grabbed.load(Ordering::Relaxed) { Style::default().fg(Color::Green).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Gray) };
            f.render_widget(Paragraph::new(status_text).style(status_style).block(Block::default().borders(Borders::ALL)), chunks[0]);

            let x_val = shared.x.load(Ordering::Relaxed);
            let y_val = shared.y.load(Ordering::Relaxed);
            let x_bar = (x_val as f32 - AXIS_MIN as f32) / (AXIS_MAX as f32 - AXIS_MIN as f32);
            let y_bar = (y_val as f32 - AXIS_MIN as f32) / (AXIS_MAX as f32 - AXIS_MIN as f32);

            let meters = Paragraph::new(vec![
                Line::from(format!("X: {:>6} [{}{}]", x_val, "█".repeat((x_bar * 40.0) as usize), " ".repeat((40.0 - x_bar * 40.0) as usize))),
                Line::from(format!("Y: {:>6} [{}{}]", y_val, "█".repeat((y_bar * 40.0) as usize), " ".repeat((40.0 - y_bar * 40.0) as usize))),
            ]).block(Block::default().borders(Borders::ALL).title("Axis Meters"));
            f.render_widget(meters, chunks[1]);

            let cfg = cfg_shared.lock().unwrap();
            let mut items = vec![
                format!("Trigger Button: {}", cfg.grab_button),
                format!("Sensitivity:    {:.0}", cfg.sensitivity),
                format!("Center X (Roll): {:.3}", cfg.center_strength_x),
                format!("Center Y (Pitch): {:.3}", cfg.center_strength_y),
                format!("Audio Volume:   {:.0}%", cfg.audio_volume * 100.0),
                format!("Center Grabbed: {}", if cfg.center_while_grabbed { "ON" } else { "OFF" }),
                format!("Mouse Device:   {}", cfg.mouse_device),
            ];

            let list_items: Vec<ListItem> = items.iter().enumerate().map(|(i, s)| {
                let style = if i == selected_idx { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
                ListItem::new(Span::styled(s, style))
            }).collect();

            f.render_widget(List::new(list_items).block(Block::default().borders(Borders::ALL).title("Settings (Up/Down/Enter/Left/Right)")), chunks[2]);

            f.render_widget(Paragraph::new("Q: Quit | E: Export | I: Import | (Log out/in if permission denied)").style(Style::default().fg(Color::DarkGray)), chunks[3]);

            if menu_state == MenuState::DeviceSelect {
                let area = centered_rect(60, 60, f.size());
                f.render_widget(Clear, area);
                let device_items: Vec<ListItem> = device_list.iter().enumerate().map(|(i, (name, _))| {
                    let style = if i == selected_device_idx { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default() };
                    ListItem::new(Span::styled(name, style))
                }).collect();
                f.render_widget(List::new(device_items).block(Block::default().borders(Borders::ALL).title("Select Mouse Device (Enter to select, Esc to cancel)")), area);
            }

            if menu_state == MenuState::Binding {
                let area = centered_rect(40, 20, f.size());
                f.render_widget(Clear, area);
                f.render_widget(Paragraph::new("\n\nPress any mouse button\n\nto bind it as trigger...\n\n(Esc to cancel)")
                    .alignment(ratatui::layout::Alignment::Center)
                    .block(Block::default().borders(Borders::ALL).title("Binding")), area);
            }
        }).unwrap();
    }

    let _ = crossterm::terminal::disable_raw_mode();
    let _ = execute!(stdout(), crossterm::terminal::LeaveAlternateScreen);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[macro_use]
extern crate crossterm;

fn main() {
    let cfg = load_config();
    let grab_button_shared = Arc::new(AtomicU32::new(cfg.grab_button as u32));
    let cfg_shared = Arc::new(Mutex::new(cfg.clone()));

    let device_path = match detect_mouse_device(&cfg.mouse_device) {
        Some(p) => p,
        None => {
            eprintln!("Could not find mouse device. Please check your config.");
            std::process::exit(1);
        }
    };

    let device = match Device::open(&device_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Could not open mouse device {:?}: {}", device_path, e);
            eprintln!("Try running with sudo or check uinput permissions.");
            std::process::exit(1);
        }
    };

    let uinput = Arc::new(create_uinput().expect("Could not create uinput device"));
    let shared = Arc::new(SharedState::new(&cfg));

    let audio = AudioPlayer::new().map(Arc::new);

    let (ui_tx, ui_rx) = mpsc::channel::<UiEvent>();
    let ui_tx_input = ui_tx.clone();
    let shared_input = shared.clone();
    let audio_clone = audio.clone();
    let grab_button_input = grab_button_shared.clone();

    thread::spawn(move || {
        input_loop(device, shared_input, grab_button_input, ui_tx_input, audio_clone);
    });

    ui_terminal(shared.clone(), cfg_shared.clone(), ui_rx, uinput, grab_button_shared);

    shared.running.store(false, Ordering::Relaxed);
    let final_cfg = cfg_shared.lock().unwrap();
    save_config(&*final_cfg);
}