// final replacement for the entire file
use evdev::{AbsoluteAxisCode, Device, KeyCode, UinputAbsSetup, AttributeSet};
use evdev::uinput::VirtualDevice;
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Box as GtkBox, Orientation, Scale, Entry, Switch, Button,
    Label, Grid, ListBox, ListBoxRow, Frame, ScrolledWindow, HeaderBar,
    FileFilter, Adjustment, MessageDialog, DialogFlags,
    ButtonsType, CssProvider, gio, glib, DrawingArea};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use dirs;
use toml;

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
    // First check if a specific path is configured
    if !configured.is_empty() && Path::new(configured).exists() {
        return Some(PathBuf::from(configured));
    }

    // Try multiple locations for mouse device detection
    let search_locations = vec![
        Path::new("/dev/input/by-id"),
        Path::new("/dev/input/event"),
        Path::new("/dev/input/"),
    ];

    for dir_path in search_locations {
        if let Ok(entries) = fs::read_dir(dir_path) {
            let mut candidates: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    if let Some(filename) = p.file_name().and_then(|n| n.to_str()) {
                        filename.contains("event-mouse") ||
                        filename.contains("-event-") ||
                        (filename.starts_with("event") && filename.chars().last().unwrap_or('0').is_ascii_digit())
                    } else {
                        false
                    }
                })
                .collect();
            candidates.sort();
            if let Some(p) = candidates.first() {
                return Some(p.clone());
            }
        }
    }

    None
}

fn list_all_mouse_devices() -> Vec<(String, PathBuf)> {
    let mut devices = Vec::new();
    let search_locations = vec![
        Path::new("/dev/input/by-id"),
        Path::new("/dev/input/event"),
        Path::new("/dev/input/"),
    ];

    for dir_path in search_locations {
        if let Ok(entries) = fs::read_dir(dir_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.contains("event-mouse") ||
                   name.contains("-event-") ||
                   (name.starts_with("event") && name.chars().last().unwrap_or('0').is_ascii_digit()) {
                    devices.push((name.to_string(), path));
                }
            }
        }
    }

    devices.sort_by(|a, b| a.0.cmp(&b.0));
    devices
}

#[derive(Clone, Debug)]
enum UiEvent {
    Grabbed(()),
    RawButton(u16),
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
    pos_x: AtomicI32,
    pos_y: AtomicI32,
    display_dx: AtomicI32,
    display_dy: AtomicI32,
    grabbed: AtomicBool,
    running: AtomicBool,
    sensitivity_fp: AtomicU32,
    center_x_fp: AtomicU32,
    center_y_fp: AtomicU32,
    center_while_grabbed: AtomicBool,
    volume_fp: AtomicU32,
    binding_mode: AtomicBool,
    toggle_request: AtomicBool,
}

impl SharedState {
    fn new(cfg: &Config) -> Self {
        Self {
            x: AtomicI32::new(0),
            y: AtomicI32::new(0),
            pos_x: AtomicI32::new(0),
            pos_y: AtomicI32::new(0),
            display_dx: AtomicI32::new(0),
            display_dy: AtomicI32::new(0),
            grabbed: AtomicBool::new(false),
            running: AtomicBool::new(true),
            sensitivity_fp: AtomicU32::new((cfg.sensitivity as f32).to_bits()),
            center_x_fp: AtomicU32::new((cfg.center_strength_x as f32).to_bits()),
            center_y_fp: AtomicU32::new((cfg.center_strength_y as f32).to_bits()),
            center_while_grabbed: AtomicBool::new(cfg.center_while_grabbed),
            volume_fp: AtomicU32::new((cfg.audio_volume as f32).to_bits()),
            binding_mode: AtomicBool::new(false),
            toggle_request: AtomicBool::new(false),
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
        shared.pos_x.store(xi, Ordering::Relaxed);
        shared.pos_y.store(yi, Ordering::Relaxed);
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
    let _ = device.set_nonblocking(true);

    loop {
        if !shared.running.load(Ordering::Relaxed) { break; }

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
                            let _ = ui_tx.send(UiEvent::Grabbed(()));
                            if now_grabbed { let _ = device.grab(); } else { let _ = device.ungrab(); }
                        }
                    }
                }
                evdev::EventSummary::RelativeAxis(_, axis, value) => {
                    // Always track raw movement for status display, even when not grabbed
                    match axis {
                        evdev::RelativeAxisCode::REL_X => {
                            shared.display_dx.fetch_add(value, Ordering::Relaxed);
                        }
                        evdev::RelativeAxisCode::REL_Y => {
                            shared.display_dy.fetch_add(value, Ordering::Relaxed);
                        }
                        _ => {}
                    }

                    if !shared.grabbed.load(Ordering::Relaxed) { continue; }
                    let sens = SharedState::f32_load(&shared.sensitivity_fp);
                    let delta = ((value as f32) * sens / 100.0) as i32;
                    match axis {
                        evdev::RelativeAxisCode::REL_X => {
                            shared.x.fetch_add(delta, Ordering::Relaxed);
                        }
                        evdev::RelativeAxisCode::REL_Y => {
                            shared.y.fetch_add(delta, Ordering::Relaxed);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    let _ = device.ungrab();
}

fn binding_listener_loop(
    shared: Arc<SharedState>,
    ui_tx: mpsc::Sender<UiEvent>,
) {
    while shared.running.load(Ordering::Relaxed) {
        if !shared.binding_mode.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(50));
            continue;
        }

        let devices = list_all_mouse_devices();
        if devices.is_empty() {
            thread::sleep(Duration::from_millis(500));
            continue;
        }

        let mut open_devices: Vec<Device> = Vec::new();
        let mut open_device_paths: Vec<PathBuf> = Vec::new();

        for (_, path) in &devices {
            match Device::open(path) {
                Ok(dev) => {
                    let _ = dev.set_nonblocking(true);
                    open_devices.push(dev);
                    open_device_paths.push(path.clone());
                }
                Err(_) => continue,
            }
        }

        if open_devices.is_empty() {
            thread::sleep(Duration::from_millis(500));
            continue;
        }

        let mut detected = false;
        while shared.running.load(Ordering::Relaxed) && shared.binding_mode.load(Ordering::Relaxed) && !detected {
            for dev in &mut open_devices {
                if let Ok(events) = dev.fetch_events() {
                    for ev in events {
                        if let evdev::EventSummary::Key(_, key_code, value) = ev.destructure() {
                            if value == 1 {
                                let _ = ui_tx.send(UiEvent::RawButton(key_code.0));
                                detected = true;
                                break;
                            }
                        }
                    }
                }
                if detected { break; }
            }
            if !detected {
                thread::sleep(Duration::from_millis(20));
            }
        }

        for mut dev in open_devices.drain(..) {
            let _ = dev.ungrab();
        }

        if detected {
            thread::sleep(Duration::from_millis(200));
        }
    }
}

fn keycode_to_name(code: u16) -> String {
    match code {
        0x110 => "Left Click".to_string(),
        0x111 => "Right Click".to_string(),
        0x112 => "Middle Click".to_string(),
        0x113 => "Side (Btn 4)".to_string(),
        0x114 => "Extra (Btn 5)".to_string(),
        0x115 => "Forward".to_string(),
        0x116 => "Back".to_string(),
        0x117 => "Task".to_string(),
        c if c >= 0x100 && c < 0x120 => format!("Mouse Btn {}", c - 0x100 + 1),
        c if c >= 0x01 && c < 0x72 => format!("Key {}", c),
        _ => format!("Unknown (0x{:04X})", code),
    }
}

struct AppState {
    config: Arc<Mutex<Config>>,
    shared: Arc<SharedState>,
    grab_button: Arc<AtomicU32>,
    uinput: Arc<Mutex<VirtualDevice>>,
    audio: Option<Arc<AudioPlayer>>,
    ui_tx: mpsc::Sender<UiEvent>,
    ui_rx: Arc<Mutex<mpsc::Receiver<UiEvent>>>,

    // UI widgets
    window: Option<ApplicationWindow>,
    status_label: Option<Label>,
    status_movement_label: Option<Label>,
    drawing_area: Option<DrawingArea>,
    x_scale: Option<Scale>,
    y_scale: Option<Scale>,
    x_value_label: Option<Label>,
    y_value_label: Option<Label>,
    trigger_btn: Option<Button>,
    sensitivity_scale: Option<Scale>,
    sensitivity_entry: Option<Entry>,
    center_x_scale: Option<Scale>,
    center_x_entry: Option<Entry>,
    center_y_scale: Option<Scale>,
    center_y_entry: Option<Entry>,
    volume_scale: Option<Scale>,
    volume_entry: Option<Entry>,
    center_grabbed_switch: Option<Switch>,
    device_listbox: Option<ListBox>,
    selected_device_label: Option<Label>,
    export_btn: Option<Button>,
    import_btn: Option<Button>,

    last_save: Arc<Mutex<Instant>>,
}

impl AppState {
    fn new() -> Self {
        let cfg = load_config();
        let grab_button_shared = Arc::new(AtomicU32::new(cfg.grab_button as u32));
        let cfg_shared = Arc::new(Mutex::new(cfg.clone()));
        let shared = Arc::new(SharedState::new(&cfg));
        let audio = AudioPlayer::new().map(Arc::new);
        let (ui_tx, ui_rx) = mpsc::channel::<UiEvent>();
        let ui_rx = Arc::new(Mutex::new(ui_rx));

        let device_path = match detect_mouse_device(&cfg.mouse_device) {
            Some(p) => p,
            None => {
                eprintln!("No mouse device found. The application will continue running and will wait for a mouse to be connected.");
                // Use a fallback path that will be handled in the reconnection logic
                PathBuf::from("/dev/null")
            }
        };

        let device = match Device::open(&device_path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Could not open mouse device {:?}: {}", device_path, e);
                eprintln!("The application will continue running and will wait for a valid mouse device.");
                // Create a dummy device using open on /dev/null (will fail but we'll handle reconnection)
                // Use the first available event device as fallback, or create a non-functional placeholder
                let fallback = Device::open("/dev/null").unwrap_or_else(|_| {
                    // Last resort: open the first event device we can find
                    for entry in fs::read_dir("/dev/input").unwrap_or_else(|_| fs::read_dir(".").unwrap()).flatten() {
                        if let Ok(dev) = Device::open(entry.path()) {
                            return dev;
                        }
                    }
                    // If all fails, panic - this shouldn't happen in normal circumstances
                    panic!("No input device available for fallback");
                });
                fallback
            }
        };

        let uinput = Arc::new(create_uinput().expect("Could not create uinput device"));

        // Input loop thread - handle device reconnection
        let shared_input = shared.clone();
        let audio_clone = audio.clone();
        let grab_button_input = Arc::clone(&grab_button_shared);
        let ui_tx_input = ui_tx.clone();
        let initial_device_path = device_path.clone();
        thread::spawn(move || {
            let mut current_path = initial_device_path;
            let mut first_device: Option<Device> = Some(device);

            while shared_input.running.load(Ordering::Relaxed) {
                // Determine device to use
                let mut active_device = if let Some(dev) = first_device.take() {
                    Some(dev)
                } else {
                    // Try to detect/reopen device
                    if let Some(new_path) = detect_mouse_device(&current_path.to_string_lossy()) {
                        current_path = new_path.clone();
                        Device::open(&new_path).ok().map(|mut d| {
                            let _ = d.set_nonblocking(true);
                            d
                        })
                    } else if let Some(new_path) = detect_mouse_device("") {
                        current_path = new_path.clone();
                        Device::open(&new_path).ok().map(|mut d| {
                            let _ = d.set_nonblocking(true);
                            d
                        })
                    } else {
                        None
                    }
                };

                if let Some(dev) = active_device.take() {
                    eprintln!("Using mouse device: {:?}", current_path);
                    // Run input loop (blocks until device errors/disconnects or running=false)
                    input_loop(dev, shared_input.clone(), grab_button_input.clone(), ui_tx_input.clone(), audio_clone.clone());
                }

                // Device disconnected or unavailable - wait before retry
                if shared_input.running.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_secs(2));
                }
            }
        });

        // Binding listener thread (listens on all devices when binding mode active)
        let shared_bind = shared.clone();
        let ui_tx_bind = ui_tx.clone();
        thread::spawn(move || {
            binding_listener_loop(shared_bind, ui_tx_bind);
        });

        // Centering thread
        let shared_center = shared.clone();
        let uinput_center = uinput.clone();
        thread::spawn(move || {
            centering_loop(shared_center, uinput_center);
        });

        Self {
            config: cfg_shared,
            shared,
            grab_button: Arc::clone(&grab_button_shared),
            uinput,
            audio,
            ui_tx,
            ui_rx,
            window: None,
            status_label: None,
            status_movement_label: None,
            drawing_area: None,
            x_scale: None,
            y_scale: None,
            x_value_label: None,
            y_value_label: None,
            trigger_btn: None,
            sensitivity_scale: None,
            sensitivity_entry: None,
            center_x_scale: None,
            center_x_entry: None,
            center_y_scale: None,
            center_y_entry: None,
            volume_scale: None,
            volume_entry: None,
            center_grabbed_switch: None,
            device_listbox: None,
            selected_device_label: None,
            export_btn: None,
            import_btn: None,
            last_save: Arc::new(Mutex::new(Instant::now())),
        }
    }

    fn build_ui(&mut self, app: &Application) {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("mouse2joy")
            .default_width(520)
            .default_height(700)
            .resizable(true)
            .build();

        // Apply CSS
        let css_provider = CssProvider::new();
        css_provider.load_from_data(include_str!("style.css"));
        gtk4::style_context_add_provider_for_display(
            &gtk4::prelude::WidgetExt::display(&window),
            &css_provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        // Header bar
        let header = HeaderBar::new();
        header.set_show_title_buttons(true);
        header.set_title_widget(Some(&Label::builder()
            .label("mouse2joy")
            .css_classes(["title-1"])
            .build()));
        window.set_titlebar(Some(&header));

        // Main container
        let main_box = GtkBox::new(Orientation::Vertical, 0);
        main_box.set_margin_top(12);
        main_box.set_margin_bottom(12);
        main_box.set_margin_start(12);
        main_box.set_margin_end(12);

        // Status section - with movement display
        let status_frame = Frame::new(Some("Status"));
        let status_box = GtkBox::new(Orientation::Vertical, 8);
        status_box.set_margin_top(12);
        status_box.set_margin_bottom(12);
        status_box.set_margin_start(12);
        status_box.set_margin_end(12);
        status_box.set_halign(gtk4::Align::Center);

        let status_label = Label::builder()
            .label("RELEASED")
            .css_classes(["status-indicator", "status-released"])
            .halign(gtk4::Align::Center)
            .build();
        self.status_label = Some(status_label.clone());

        // Movement display - always visible (even when not grabbed)
        let movement_label = Label::builder()
            .label("Movement: 0 | 0")
            .css_classes(["movement-label"])
            .halign(gtk4::Align::Center)
            .build();
        self.status_movement_label = Some(movement_label.clone());

        status_box.append(&status_label);
        status_box.append(&movement_label);
        status_frame.set_child(Some(&status_box));
        main_box.append(&status_frame);

        // Circular XY indicator
        let drawing_area = DrawingArea::new();
        drawing_area.set_size_request(200, 200);
        drawing_area.set_hexpand(true);
        drawing_area.add_css_class("xy-indicator");
        self.drawing_area = Some(drawing_area.clone());

        let xy_frame = Frame::new(Some("XY Joystick Position"));
        let xy_box = GtkBox::new(Orientation::Vertical, 8);
        xy_box.set_margin_top(12);
        xy_box.set_margin_bottom(12);
        xy_box.set_margin_start(12);
        xy_box.set_margin_end(12);
        xy_box.set_halign(gtk4::Align::Center);
        xy_box.append(&drawing_area);
        xy_frame.set_child(Some(&xy_box));
        main_box.append(&xy_frame);

        // Settings section
        let settings_frame = Frame::new(Some("Settings"));
        let settings_grid = Grid::new();
        settings_grid.set_margin_top(12);
        settings_grid.set_margin_bottom(12);
        settings_grid.set_margin_start(12);
        settings_grid.set_margin_end(12);
        settings_grid.set_row_spacing(10);
        settings_grid.set_column_spacing(12);

        // Export/Import buttons
        let io_frame = Frame::new(Some("Configuration"));
        let io_box = GtkBox::new(Orientation::Horizontal, 12);
        io_box.set_margin_top(12);
        io_box.set_margin_bottom(12);
        io_box.set_margin_start(12);
        io_box.set_margin_end(12);
        io_box.set_halign(gtk4::Align::Center);

        let export_btn = Button::with_label("Export Config");
        let import_btn = Button::with_label("Import Config");
        self.export_btn = Some(export_btn.clone());
        self.import_btn = Some(import_btn.clone());
        io_box.append(&export_btn);
        io_box.append(&import_btn);
        io_frame.set_child(Some(&io_box));
        main_box.append(&io_frame);

        // Trigger Button
        let trigger_label = Label::new(Some("Trigger Button:"));
        trigger_label.set_xalign(0.0);
        let trigger_btn = Button::builder()
            .label(&keycode_to_name(self.grab_button.load(Ordering::Relaxed) as u16))
            .hexpand(true)
            .halign(gtk4::Align::Fill)
            .build();
        self.trigger_btn = Some(trigger_btn.clone());
        settings_grid.attach(&trigger_label, 0, 0, 1, 1);
        settings_grid.attach(&trigger_btn, 1, 0, 1, 1);

        // Sensitivity
        let sens_label = Label::new(Some("Sensitivity:"));
        sens_label.set_xalign(0.0);
        let sens_scale = Scale::new(Orientation::Horizontal, Some(&Adjustment::new(50.0, 50.0, 5000.0, 50.0, 100.0, 0.0)));
        sens_scale.set_draw_value(false);
        sens_scale.set_hexpand(true);
        sens_scale.set_digits(0);
        sens_scale.set_value(self.config.lock().unwrap().sensitivity);
        let sens_entry = Entry::builder()
            .placeholder_text("50-5000")
            .width_chars(8)
            .text(&format!("{:.0}", self.config.lock().unwrap().sensitivity))
            .build();
        self.sensitivity_scale = Some(sens_scale.clone());
        self.sensitivity_entry = Some(sens_entry.clone());
        let sens_box = GtkBox::new(Orientation::Horizontal, 8);
        sens_box.append(&sens_scale);
        sens_box.append(&sens_entry);
        settings_grid.attach(&sens_label, 0, 1, 1, 1);
        settings_grid.attach(&sens_box, 1, 1, 1, 1);

        // Center X
        let cx_label = Label::new(Some("Center X (Roll):"));
        cx_label.set_xalign(0.0);
        let cx_scale = Scale::new(Orientation::Horizontal, Some(&Adjustment::new(0.0, 0.0, 1.0, 0.005, 0.01, 0.0)));
        cx_scale.set_draw_value(false);
        cx_scale.set_hexpand(true);
        cx_scale.set_digits(3);
        cx_scale.set_value(self.config.lock().unwrap().center_strength_x);
        let cx_entry = Entry::builder()
            .placeholder_text("0.000-1.000")
            .width_chars(8)
            .text(&format!("{:.3}", self.config.lock().unwrap().center_strength_x))
            .build();
        self.center_x_scale = Some(cx_scale.clone());
        self.center_x_entry = Some(cx_entry.clone());
        let cx_box = GtkBox::new(Orientation::Horizontal, 8);
        cx_box.append(&cx_scale);
        cx_box.append(&cx_entry);
        settings_grid.attach(&cx_label, 0, 2, 1, 1);
        settings_grid.attach(&cx_box, 1, 2, 1, 1);

        // Center Y
        let cy_label = Label::new(Some("Center Y (Pitch):"));
        cy_label.set_xalign(0.0);
        let cy_scale = Scale::new(Orientation::Horizontal, Some(&Adjustment::new(0.0, 0.0, 1.0, 0.005, 0.01, 0.0)));
        cy_scale.set_draw_value(false);
        cy_scale.set_hexpand(true);
        cy_scale.set_digits(3);
        cy_scale.set_value(self.config.lock().unwrap().center_strength_y);
        let cy_entry = Entry::builder()
            .placeholder_text("0.000-1.000")
            .width_chars(8)
            .text(&format!("{:.3}", self.config.lock().unwrap().center_strength_y))
            .build();
        self.center_y_scale = Some(cy_scale.clone());
        self.center_y_entry = Some(cy_entry.clone());
        let cy_box = GtkBox::new(Orientation::Horizontal, 8);
        cy_box.append(&cy_scale);
        cy_box.append(&cy_entry);
        settings_grid.attach(&cy_label, 0, 3, 1, 1);
        settings_grid.attach(&cy_box, 1, 3, 1, 1);

        // Audio Volume
        let vol_label = Label::new(Some("Audio Volume:"));
        vol_label.set_xalign(0.0);
        let vol_scale = Scale::new(Orientation::Horizontal, Some(&Adjustment::new(0.0, 0.0, 1.0, 0.05, 0.1, 0.0)));
        vol_scale.set_draw_value(false);
        vol_scale.set_hexpand(true);
        vol_scale.set_digits(2);
        vol_scale.set_value(self.config.lock().unwrap().audio_volume);
        let vol_entry = Entry::builder()
            .placeholder_text("0.00-1.00")
            .width_chars(8)
            .text(&format!("{:.2}", self.config.lock().unwrap().audio_volume))
            .build();
        self.volume_scale = Some(vol_scale.clone());
        self.volume_entry = Some(vol_entry.clone());
        let vol_box = GtkBox::new(Orientation::Horizontal, 8);
        vol_box.append(&vol_scale);
        vol_box.append(&vol_entry);
        settings_grid.attach(&vol_label, 0, 4, 1, 1);
        settings_grid.attach(&vol_box, 1, 4, 1, 1);

        // Center while grabbed - smaller switch, left aligned
        let cwg_label = Label::new(Some("Center while grabbed:"));
        cwg_label.set_xalign(0.0);
        let cwg_switch = Switch::builder()
            .active(self.config.lock().unwrap().center_while_grabbed)
            .halign(gtk4::Align::Start)
            .valign(gtk4::Align::Center)
            .build();
        cwg_switch.add_css_class("compact-switch");
        self.center_grabbed_switch = Some(cwg_switch.clone());
        settings_grid.attach(&cwg_label, 0, 5, 1, 1);
        settings_grid.attach(&cwg_switch, 1, 5, 1, 1);

        settings_frame.set_child(Some(&settings_grid));
        main_box.append(&settings_frame);

        // Device Selection
        let device_frame = Frame::new(Some("Mouse Device (used for grab/movement)"));
        let device_box = GtkBox::new(Orientation::Vertical, 8);
        device_box.set_margin_top(12);
        device_box.set_margin_bottom(12);
        device_box.set_margin_start(12);
        device_box.set_margin_end(12);

        let selected_device_label = Label::builder()
            .label("Active device (grab/movement): detecting...")
            .xalign(0.0)
            .css_classes(["selected-device-label"])
            .build();
        self.selected_device_label = Some(selected_device_label.clone());
        device_box.append(&selected_device_label);

        let hint_label = Label::builder()
            .label("Select which mouse is grabbed for joystick movement. Trigger button binding listens on all mouse devices.")
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label"])
            .build();
        device_box.append(&hint_label);

        let device_listbox = ListBox::new();
        device_listbox.set_selection_mode(gtk4::SelectionMode::Single);
        self.populate_device_list(&device_listbox);
        self.device_listbox = Some(device_listbox.clone());

        let scrolled = ScrolledWindow::builder()
            .child(&device_listbox)
            .min_content_height(100)
            .max_content_height(160)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .build();
        device_box.append(&scrolled);
        device_frame.set_child(Some(&device_box));
        main_box.append(&device_frame);

        // Export/Import buttons
        let io_frame = Frame::new(Some("Configuration"));
        let io_box = GtkBox::new(Orientation::Horizontal, 12);
        io_box.set_margin_top(12);
        io_box.set_margin_bottom(12);
        io_box.set_margin_start(12);
        io_box.set_margin_end(12);
        io_box.set_halign(gtk4::Align::Center);

        let export_btn = Button::with_label("Export Config");
        let import_btn = Button::with_label("Import Config");
        self.export_btn = Some(export_btn.clone());
        self.import_btn = Some(import_btn.clone());
        io_box.append(&export_btn);
        io_box.append(&import_btn);
        io_frame.set_child(Some(&io_box));
        main_box.append(&io_frame);

        // Export/Import buttons
        let io_frame = Frame::new(Some("Configuration"));
        let io_box = GtkBox::new(Orientation::Horizontal, 12);
        io_box.set_margin_top(12);
        io_box.set_margin_bottom(12);
        io_box.set_margin_start(12);
        io_box.set_margin_end(12);
        io_box.set_halign(gtk4::Align::Center);

        let export_btn = Button::with_label("Export Config");
        let import_btn = Button::with_label("Import Config");
        self.export_btn = Some(export_btn.clone());
        self.import_btn = Some(import_btn.clone());
        io_box.append(&export_btn);
        io_box.append(&import_btn);
        io_frame.set_child(Some(&io_box));
        main_box.append(&io_frame);

        window.set_child(Some(&main_box));
        self.window = Some(window.clone());

        // Connect signals
        self.connect_signals(&window);

        // Start UI update loop
        self.start_ui_updates();

        window.present();
    }

    fn update_selected_device_label(&self) {
        if let Some(label) = &self.selected_device_label {
            let cfg = self.config.lock().unwrap();
            let devices = list_all_mouse_devices();
            let active = devices.iter().find(|(_, p)| p.to_string_lossy() == cfg.mouse_device);
            let text = if let Some((name, _)) = active {
                format!("Active device (grab/movement): {}", name)
            } else if !cfg.mouse_device.is_empty() {
                format!("Active device (grab/movement): {}", cfg.mouse_device)
            } else {
                "Active device (grab/movement): (auto-detect)".to_string()
            };
            label.set_label(&text);
        }
    }

    fn populate_device_list(&self, listbox: &ListBox) {
        while let Some(row) = listbox.row_at_index(0) {
            listbox.remove(&row);
        }
        let devices = list_all_mouse_devices();
        let current_device = self.config.lock().unwrap().mouse_device.clone();

        for (name, path) in devices {
            let row = ListBoxRow::new();
            let label = Label::builder()
                .label(&name)
                .xalign(0.0)
                .margin_start(12)
                .margin_top(6)
                .margin_bottom(6)
                .margin_end(12)
                .build();
            row.set_child(Some(&label));
            if path.to_string_lossy().to_string() == current_device {
                row.add_css_class("selected-device");
                listbox.select_row(Some(&row));
            }
            listbox.append(&row);
        }
    }

    fn connect_signals(&self, window: &ApplicationWindow) {
        // Trigger button
        if let Some(btn) = &self.trigger_btn {
            let shared = self.shared.clone();
            let grab_button = self.grab_button.clone();
            let btn_clone = btn.clone();

            btn.connect_clicked(move |_| {
                if shared.binding_mode.load(Ordering::Relaxed) {
                    shared.binding_mode.store(false, Ordering::Relaxed);
                    btn_clone.set_label(&keycode_to_name(grab_button.load(Ordering::Relaxed) as u16));
                    btn_clone.remove_css_class("binding");
                } else {
                    shared.binding_mode.store(true, Ordering::Relaxed);
                    btn_clone.set_label("Press any mouse button to bind...");
                    btn_clone.add_css_class("binding");
                }
            });
        }

        // Sensitivity
        self.connect_scale_entry(
            &self.sensitivity_scale,
            &self.sensitivity_entry,
            50.0, 5000.0,
            |cfg, v| cfg.sensitivity = v,
            |s| format!("{:.0}", s),
            |shared, v| shared.sensitivity_fp.store((v as f32).to_bits(), Ordering::Relaxed),
        );

        // Center X
        self.connect_scale_entry(
            &self.center_x_scale,
            &self.center_x_entry,
            0.0, 1.0,
            |cfg, v| cfg.center_strength_x = v,
            |s| format!("{:.3}", s),
            |shared, v| shared.center_x_fp.store((v as f32).to_bits(), Ordering::Relaxed),
        );

        // Center Y
        self.connect_scale_entry(
            &self.center_y_scale,
            &self.center_y_entry,
            0.0, 1.0,
            |cfg, v| cfg.center_strength_y = v,
            |s| format!("{:.3}", s),
            |shared, v| shared.center_y_fp.store((v as f32).to_bits(), Ordering::Relaxed),
        );

        // Volume
        self.connect_scale_entry(
            &self.volume_scale,
            &self.volume_entry,
            0.0, 1.0,
            |cfg, v| cfg.audio_volume = v,
            |s| format!("{:.2}", s),
            |shared, v| shared.volume_fp.store((v as f32).to_bits(), Ordering::Relaxed),
        );

        // Center while grabbed switch
        if let Some(switch) = &self.center_grabbed_switch {
            let config = self.config.clone();
            let shared = self.shared.clone();
            switch.connect_state_set(move |_, state| {
                let mut cfg = config.lock().unwrap();
                cfg.center_while_grabbed = state;
                shared.center_while_grabbed.store(state, Ordering::Relaxed);
                glib::Propagation::Proceed
            });
        }

        // Device listbox selection
        if let Some(listbox) = &self.device_listbox {
            let config = self.config.clone();
            let selected_device_label = self.selected_device_label.clone();
            let devices = list_all_mouse_devices();
            listbox.connect_row_selected(move |_, row| {
                if let Some(row) = row {
                    let index = row.index();
                    if index >= 0 {
                        let idx = index as usize;
                        if let Some((name, path)) = devices.get(idx) {
                            let mut cfg = config.lock().unwrap();
                            cfg.mouse_device = path.to_string_lossy().to_string();
                            save_config(&cfg);
                            if let Some(label) = &selected_device_label {
                                label.set_label(&format!("Active device (grab/movement): {}", name));
                            }
                        }
                    }
                }
            });
        }

        // Export button
        if let Some(btn) = &self.export_btn {
            let config = self.config.clone();
            let window_clone = window.clone();
            btn.connect_clicked(move |_| {
                let dialog = gtk4::FileChooserNative::new(
                    Some("Export Configuration"),
                    Some(&window_clone),
                    gtk4::FileChooserAction::Save,
                    Some("_Export"),
                    Some("_Cancel"),
                );
                dialog.set_modal(true);

                let filter = FileFilter::new();
                filter.set_name(Some("TOML files"));
                filter.add_pattern("*.toml");
                dialog.add_filter(&filter);

                let config_clone = config.clone();
                dialog.connect_response(move |d, response| {
                    if response == gtk4::ResponseType::Accept {
                        if let Some(file) = d.file() {
                            if let Some(path) = file.path() {
                                let cfg = config_clone.lock().unwrap();
                                if let Ok(s) = toml::to_string_pretty(&*cfg) {
                                    let _ = fs::write(path, s);
                                }
                            }
                        }
                    }
                    d.destroy();
                });
                dialog.show();
            });
        }

        // Import button
        if let Some(btn) = &self.import_btn {
            let config = self.config.clone();
            let shared = self.shared.clone();
            let grab_button = self.grab_button.clone();
            let window_clone = window.clone();
            btn.connect_clicked(move |_| {
                let dialog = gtk4::FileChooserNative::new(
                    Some("Import Configuration"),
                    Some(&window_clone),
                    gtk4::FileChooserAction::Open,
                    Some("_Import"),
                    Some("_Cancel"),
                );
                dialog.set_modal(true);

                let filter = FileFilter::new();
                filter.set_name(Some("TOML files"));
                filter.add_pattern("*.toml");
                dialog.add_filter(&filter);

                let config_clone = config.clone();
                let shared_clone = shared.clone();
                let grab_button_clone = grab_button.clone();
                let window_err = window_clone.clone();
                dialog.connect_response(move |d, response| {
                    if response == gtk4::ResponseType::Accept {
                        if let Some(file) = d.file() {
                            if let Some(path) = file.path() {
                                if let Ok(s) = fs::read_to_string(path) {
                                    if let Ok(new_cfg) = toml::from_str::<Config>(&s) {
                                        let mut cfg = config_clone.lock().unwrap();
                                        *cfg = new_cfg.clone();
                                        shared_clone.sensitivity_fp.store((cfg.sensitivity as f32).to_bits(), Ordering::Relaxed);
                                        shared_clone.center_x_fp.store((cfg.center_strength_x as f32).to_bits(), Ordering::Relaxed);
                                        shared_clone.center_y_fp.store((cfg.center_strength_y as f32).to_bits(), Ordering::Relaxed);
                                        shared_clone.volume_fp.store((cfg.audio_volume as f32).to_bits(), Ordering::Relaxed);
                                        shared_clone.center_while_grabbed.store(cfg.center_while_grabbed, Ordering::Relaxed);
                                        grab_button_clone.store(cfg.grab_button as u32, Ordering::Relaxed);
                                        save_config(&cfg);
                                    } else {
                                        Self::show_error_dialog(&window_err, "Invalid configuration file");
                                    }
                                } else {
                                    Self::show_error_dialog(&window_err, "Failed to read file");
                                }
                            }
                        }
                    }
                    d.destroy();
                });
                dialog.show();
            });
        }

        // Window close
        let shared = self.shared.clone();
        let config = self.config.clone();
        window.connect_close_request(move |_| {
            shared.running.store(false, Ordering::Relaxed);
            let cfg = config.lock().unwrap();
            save_config(&*cfg);
            glib::Propagation::Proceed
        });
    }

    fn connect_scale_entry<F1, F2, F3>(
        &self,
        scale: &Option<Scale>,
        entry: &Option<Entry>,
        min: f64, max: f64,
        config_set: F1,
        format: F2,
        shared_set: F3,
    ) where
        F1: Fn(&mut Config, f64) + Send + Sync + 'static + Clone,
        F2: Fn(f64) -> String + Send + Sync + 'static + Clone,
        F3: Fn(&Arc<SharedState>, f64) + Send + Sync + 'static + Clone,
    {
        if let (Some(scale), Some(entry)) = (scale, entry) {
            let config = self.config.clone();
            let shared = self.shared.clone();
            let scale_clone = scale.clone();
            let entry_clone = entry.clone();

            let syncing = std::rc::Rc::new(std::cell::Cell::new(false));
            let syncing_scale = syncing.clone();
            let syncing_entry = syncing;

            let config_set_s = config_set.clone();
            let format_s = format.clone();
            let shared_set_s = shared_set.clone();
            scale.connect_value_changed(move |s| {
                if syncing_scale.get() { return; }
                let val = s.value().clamp(min, max);
                syncing_scale.set(true);
                {
                    let mut cfg = config.lock().unwrap();
                    config_set_s(&mut cfg, val);
                }
                entry_clone.set_text(&format_s(val));
                shared_set_s(&shared, val);
                syncing_scale.set(false);
            });

            let config2 = self.config.clone();
            let shared2 = self.shared.clone();
            entry.connect_changed(move |e| {
                if syncing_entry.get() { return; }
                let text = e.text().to_string();
                if let Ok(val) = text.parse::<f64>() {
                    let clamped = val.clamp(min, max);
                    syncing_entry.set(true);
                    {
                        let mut cfg = config2.lock().unwrap();
                        config_set(&mut cfg, clamped);
                    }
                    scale_clone.set_value(clamped);
                    shared_set(&shared2, clamped);
                    syncing_entry.set(false);
                }
            });
        }
    }

    fn start_ui_updates(&self) {
        let status_label = self.status_label.clone();
        let status_movement_label = self.status_movement_label.clone();
        let trigger_btn = self.trigger_btn.clone();
        let grab_button = self.grab_button.clone();
        let shared = self.shared.clone();
        let ui_rx = self.ui_rx.clone();
        let config = self.config.clone();
        let sensitivity_scale = self.sensitivity_scale.clone();
        let sensitivity_entry = self.sensitivity_entry.clone();
        let center_x_scale = self.center_x_scale.clone();
        let center_x_entry = self.center_x_entry.clone();
        let center_y_scale = self.center_y_scale.clone();
        let center_y_entry = self.center_y_entry.clone();
        let volume_scale = self.volume_scale.clone();
        let volume_entry = self.volume_entry.clone();
        let center_grabbed_switch = self.center_grabbed_switch.clone();
        let last_save = self.last_save.clone();
        let drawing_area = self.drawing_area.clone();

        glib::timeout_add_local(Duration::from_millis(16), move || {
            // Handle UI events
            while let Ok(ev) = ui_rx.lock().unwrap().try_recv() {
                match ev {
                    UiEvent::RawButton(code) => {
                        if shared.binding_mode.load(Ordering::Relaxed) {
                            let mut cfg = config.lock().unwrap();
                            cfg.grab_button = code;
                            grab_button.store(code as u32, Ordering::Relaxed);
                            shared.binding_mode.store(false, Ordering::Relaxed);
                            if let Some(btn) = &trigger_btn {
                                btn.set_label(&keycode_to_name(code));
                                btn.remove_css_class("binding");
                            }
                            save_config(&cfg);
                        }
                    }
                    UiEvent::Grabbed(_) => {}
                }
            }

            // Update status label
            if let Some(label) = &status_label {
                let grabbed = shared.grabbed.load(Ordering::Relaxed);
                label.set_label(if grabbed { "GRABBED" } else { "RELEASED" });
                label.remove_css_class("status-grabbed");
                label.remove_css_class("status-released");
                label.add_css_class(if grabbed { "status-grabbed" } else { "status-released" });
            }

            // Update movement display - ALWAYS SHOW (even when not grabbed)
            // Shows recent raw mouse movement deltas to demonstrate input is working
            if let Some(movement_label) = &status_movement_label {
                let dx = shared.display_dx.swap(0, Ordering::Relaxed);
                let dy = shared.display_dy.swap(0, Ordering::Relaxed);
                movement_label.set_label(&format!("Movement: X={} Y={}", dx, dy));
            }

            // Draw circular indicator for recent movement
            if let Some(da) = &drawing_area {
                let _x = shared.pos_x.load(Ordering::Relaxed);
                let _y = shared.pos_y.load(Ordering::Relaxed);
                da.queue_draw();
            }

            // Draw circular XY indicator
            if let Some(da) = &drawing_area {
                let _x = shared.pos_x.load(Ordering::Relaxed);
                let _y = shared.pos_y.load(Ordering::Relaxed);
                da.queue_draw();
            }

            // Update UI controls from config (for import)
            let cfg = config.lock().unwrap();
            if let Some(scale) = &sensitivity_scale { if (scale.value() - cfg.sensitivity).abs() > 0.1 { scale.set_value(cfg.sensitivity); } }
            if let Some(entry) = &sensitivity_entry {
                if entry.text().to_string() != format!("{:.0}", cfg.sensitivity) && !entry.is_focus() {
                    entry.set_text(&format!("{:.0}", cfg.sensitivity));
                }
            }
            if let Some(scale) = &center_x_scale { if (scale.value() - cfg.center_strength_x).abs() > 0.001 { scale.set_value(cfg.center_strength_x); } }
            if let Some(entry) = &center_x_entry {
                if entry.text().to_string() != format!("{:.3}", cfg.center_strength_x) && !entry.is_focus() {
                    entry.set_text(&format!("{:.3}", cfg.center_strength_x));
                }
            }
            if let Some(scale) = &center_y_scale { if (scale.value() - cfg.center_strength_y).abs() > 0.001 { scale.set_value(cfg.center_strength_y); } }
            if let Some(entry) = &center_y_entry {
                if entry.text().to_string() != format!("{:.3}", cfg.center_strength_y) && !entry.is_focus() {
                    entry.set_text(&format!("{:.3}", cfg.center_strength_y));
                }
            }
            if let Some(scale) = &volume_scale { if (scale.value() - cfg.audio_volume).abs() > 0.001 { scale.set_value(cfg.audio_volume); } }
            if let Some(entry) = &volume_entry {
                if entry.text().to_string() != format!("{:.2}", cfg.audio_volume) && !entry.is_focus() {
                    entry.set_text(&format!("{:.2}", cfg.audio_volume));
                }
            }
            if let Some(switch) = &center_grabbed_switch {
                if switch.is_active() != cfg.center_while_grabbed {
                    switch.set_active(cfg.center_while_grabbed);
                }
            }

            // Auto-save
            if last_save.lock().unwrap().elapsed() > Duration::from_secs(30) {
                save_config(&*cfg);
                *last_save.lock().unwrap() = Instant::now();
            }

            glib::ControlFlow::Continue
        });
    }

    fn show_error_dialog(window: &ApplicationWindow, message: &str) {
        let dialog = MessageDialog::new(
            Some(window),
            DialogFlags::MODAL | DialogFlags::DESTROY_WITH_PARENT,
            gtk4::MessageType::Error,
            ButtonsType::Ok,
            message,
        );
        dialog.connect_response(|d, _| d.destroy());
        dialog.show();
    }
}

fn main() {
    let app = Application::builder()
        .application_id("org.mouse2joy.mouse2joy")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(|app| {
        let mut state = AppState::new();
        state.build_ui(app);
        Box::leak(Box::new(state));
    });

    app.run();
}
