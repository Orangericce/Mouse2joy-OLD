use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, Device, EventSummary, InputEventKind, KeyCode,
    RelativeAxisCode, UinputAbsSetup,
};
use evdev::uinput::VirtualDevice;
use crossterm::event::{Event, KeyCode, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::{Block, Borders, Gauge, Paragraph, Widget};
use ratatui::Terminal;
use std::io::stdout;
use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, CheckButton, Label, LevelBar,
    Orientation, Scale,
};
use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::Direction;
use libpulse_simple_binding::Simple;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const AXIS_MIN: i32 = -32767;
const AXIS_MAX: i32 = 32767;
const CENTER_TICK_HZ: u64 = 60;
const APP_ID: &str = "dev.local.mouse2joy";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Config {
    mouse_device: String,
    grab_button: u16, // evdev key code
    sensitivity: f64,
    center_strength_x: f64,
    center_strength_y: f64,
    center_while_grabbed: bool,
    audio_volume: f64, // 0.0..1.0
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

#[derive(Clone, Copy, Debug)]
enum AudioCue {
    Grab,
    Ungrab,
}

struct AudioPlayer {
    spec: Spec,
}

impl AudioPlayer {
    fn new() -> Option<Self> {
        let spec = Spec {
            format: Format::S16le,
            channels: 1,
            rate: 44100,
        };
        // Just verify we can create a stream; keep it simple by creating per-cue.
        Some(Self { spec })
    }

    fn play(&self, cue: AudioCue, volume: f64) {
        let (freq, ms) = match cue {
            AudioCue::Grab => (880.0_f64, 90_u32),
            AudioCue::Ungrab => (440.0_f64, 110_u32),
        };
        let vol = volume.clamp(0.0, 1.0);
        if vol <= 0.001 {
            return;
        }
        let spec = self.spec;
        thread::spawn(move || {
            let Ok(pa) = Simple::new(
                None,
                "mouse2joy",
                Direction::Playback,
                None,
                "cue",
                &spec,
                None,
                None,
            ) else {
                return;
            };
            let rate = spec.rate as f64;
            let samples = ((rate * ms as f64) / 1000.0) as usize;
            let mut buf = Vec::with_capacity(samples * 2);
            for i in 0..samples {
                let t = i as f64 / rate;
                // Simple envelope to avoid clicks
                let env = if i < 64 {
                    i as f64 / 64.0
                } else if i + 64 > samples {
                    (samples.saturating_sub(i)) as f64 / 64.0
                } else {
                    1.0
                };
                let s = (2.0 * std::f64::consts::PI * freq * t).sin();
                let v = (s * env * vol * i16::MAX as f64) as i16;
                buf.extend_from_slice(&v.to_le_bytes());
            }
            let _ = pa.write(&buf);
            let _ = pa.drain();
        });
    }
}

struct Shared {
    x: AtomicI32, // fixed-point * 1000 for atomics simplicity? easier: store as i32 joystick units
    y: AtomicI32,
    grabbed: AtomicBool,
    running: AtomicBool,
    sensitivity_fp: AtomicU32,      // f32 bits
    center_x_fp: AtomicU32,         // f32 bits
    center_y_fp: AtomicU32,         // f32 bits
    center_while_grabbed: AtomicBool,
    volume_fp: AtomicU32,           // f32 bits
    show_window: AtomicBool,
    quit_requested: AtomicBool,
}

impl Shared {
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
            show_window: AtomicBool::new(true),
            quit_requested: AtomicBool::new(false),
        }
    }
    fn f32_load(a: &AtomicU32) -> f32 {
        f32::from_bits(a.load(Ordering::Relaxed))
    }
    fn f32_store(a: &AtomicU32, v: f32) {
        a.store(v.to_bits(), Ordering::Relaxed);
    }
}

fn clamp_i32(v: i32, lo: i32, hi: i32) -> i32 {
    v.max(lo).min(hi)
}

fn create_uinput() -> Result<UInputDevice, Box<dyn std::error::Error>> {
    let abs_x = UinputAbsSetup::new(
        AbsoluteAxisCode::ABS_X,
        AbsInfo::new(0, AXIS_MIN, AXIS_MAX, 0, 0, 0),
    );
    let abs_y = UinputAbsSetup::new(
        AbsoluteAxisCode::ABS_Y,
        AbsInfo::new(0, AXIS_MIN, AXIS_MAX, 0, 0, 0),
    );
    let mut keys = AttributeSet::<KeyCode>::new();
    keys.insert(KeyCode::BTN_TRIGGER);
    keys.insert(KeyCode::BTN_THUMB);
    keys.insert(KeyCode::BTN_THUMB2);

    let dev = VirtualDeviceBuilder::new()?
        .name("mouse2joy Virtual Joystick")
        .with_absolute_axis(&abs_x)?
        .with_absolute_axis(&abs_y)?
        .with_keys(&keys)?
        .build()?;
    Ok(dev)
}

fn centering_loop(shared: Arc<Shared>, uinput: Arc<Mutex<UInputDevice>>) {
    let interval = Duration::from_micros(1_000_000 / CENTER_TICK_HZ);
    // Use f32 accumulators locally for smooth physics
    let mut x = 0.0f32;
    let mut y = 0.0f32;
    while shared.running.load(Ordering::Relaxed) {
        let start = Instant::now();
        let grabbed = shared.grabbed.load(Ordering::Relaxed);
        let cwg = shared.center_while_grabbed.load(Ordering::Relaxed);
        let cx = Shared::f32_load(&shared.center_x_fp);
        let cy = Shared::f32_load(&shared.center_y_fp);

        // Pull in external deltas accumulated by input thread
        let dx = shared.x.swap(0, Ordering::Relaxed) as f32;
        let dy = shared.y.swap(0, Ordering::Relaxed) as f32;
        x = (x + dx).clamp(AXIS_MIN as f32, AXIS_MAX as f32);
        y = (y + dy).clamp(AXIS_MIN as f32, AXIS_MAX as f32);

        let should_center = cwg || !grabbed;
        if should_center {
            x -= x * cx;
            y -= y * cy;
            // snap tiny values to zero to avoid drift
            if x.abs() < 0.5 {
                x = 0.0;
            }
            if y.abs() < 0.5 {
                y = 0.0;
            }
        }

        let xi = x as i32;
        let yi = y as i32;
        if let Ok(mut ui) = uinput.lock() {
            // evdev 0.13: emit takes events; use write_event
            let _ = ui.emit(&[
                evdev::InputEvent::new_now(evdev::EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_X.0, xi),
                evdev::InputEvent::new_now(evdev::EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Y.0, yi),
                evdev::InputEvent::new_now(evdev::EventType::SYNCHRONIZATION.0, 0, 0),
            ]);
        }

        let elapsed = start.elapsed();
        if elapsed < interval {
            thread::sleep(interval - elapsed);
        }
    }
    // center on exit
    if let Ok(mut ui) = uinput.lock() {
        let _ = ui.emit(&[
            evdev::InputEvent::new_now(evdev::EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_X.0, 0),
            evdev::InputEvent::new_now(evdev::EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Y.0, 0),
            evdev::InputEvent::new_now(evdev::EventType::SYNCHRONIZATION.0, 0, 0),
        ]);
    }
}

enum UiEvent {
    Grabbed(bool),
    Axes(i32, i32),
    Quit,
}

struct MouseTray {
    shared: Arc<Shared>,
    ui_tx: glib::Sender<UiEvent>,
}

impl Tray for MouseTray {
    fn id(&self) -> String {
        "mouse2joy".into()
    }
    fn title(&self) -> String {
        "mouse2joy".into()
    }
    fn icon_name(&self) -> String {
        if self.shared.grabbed.load(Ordering::Relaxed) {
            "input-mouse".into()
        } else {
            "input-mouse".into()
        }
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        let grabbed = self.shared.grabbed.load(Ordering::Relaxed);
        ksni::ToolTip {
            title: if grabbed {
                "mouse2joy: Grabbed".into()
            } else {
                "mouse2joy: Released".into()
            },
            ..Default::default()
        }
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        vec![
            StandardItem {
                label: "Show Window".into(),
                activate: Box::new(|this: &mut Self| {
                    this.shared.show_window.store(true, Ordering::Relaxed);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|this: &mut Self| {
                    this.shared.quit_requested.store(true, Ordering::Relaxed);
                    this.shared.running.store(false, Ordering::Relaxed);
                    let _ = this.ui_tx.send(UiEvent::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

fn input_loop(
    mut device: Device,
    shared: Arc<Shared>,
    grab_button: u16,
    ui_tx: glib::Sender<UiEvent>,
    audio: Option<Arc<AudioPlayer>>,
) {
    while shared.running.load(Ordering::Relaxed) {
        // fetch_events blocks; evdev 0.13 API
        let events = match device.fetch_events() {
            Ok(ev) => ev.collect::<Vec<_>>(),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        for ev in events {
            if !shared.running.load(Ordering::Relaxed) {
                break;
            }
            match ev.destructure() {
                EventSummary::Key(_, key_code, value) if key_code.as_u16() == grab_button => {
                    if value == 1 {
                        let now_grabbed = !shared.grabbed.load(Ordering::Relaxed);
                        shared.grabbed.store(now_grabbed, Ordering::Relaxed);
                        if now_grabbed {
                            let _ = device.grab();
                        } else {
                            let _ = device.ungrab();
                        }
                        let vol = Shared::f32_load(&shared.volume_fp) as f64;
                        if let Some(a) = &audio {
                            a.play(
                                if now_grabbed { AudioCue::Grab } else { AudioCue::Ungrab },
                                vol,
                            );
                        }
                        let _ = ui_tx.send(UiEvent::Grabbed(now_grabbed));
                    }
                }
                EventSummary::RelativeAxis(_, axis, value) => {
                    if !shared.grabbed.load(Ordering::Relaxed) {
                        continue;
                    }
                    let sens = Shared::f32_load(&shared.sensitivity_fp);
                    let delta = ((value as f32) * sens / 100.0) as i32;
                    match axis {
                        RelativeAxisCode::REL_X => {
                            shared.x.fetch_add(delta, Ordering::Relaxed);
                        }
                        RelativeAxisCode::REL_Y => {
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

fn build_ui(app: &Application, shared: Arc<Shared>, cfg: Rc<RefCell<Config>>, ui_rx: glib::Receiver<UiEvent>) {
    let window = ApplicationWindow::builder()
        .application(app)
        .title("mouse2joy")
        .resizable(true)
        .build();
    // Adaptive sizing for tiling WMs (Hyprland): let WM decide; set a reasonable default
    window.set_default_size(420, 320);

    let root = GtkBox::new(Orientation::Vertical, 8);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    root.set_margin_start(12);
    root.set_margin_end(12);

    // Status
    let status_label = Label::new(Some("Status: Released"));
    status_label.set_halign(Align::Start);
    status_label.add_css_class("title-4");

    // Axis display
    let x_row = GtkBox::new(Orientation::Horizontal, 8);
    let x_lbl = Label::new(Some("X"));
    x_lbl.set_width_chars(2);
    let x_bar = LevelBar::builder()
        .min_value(AXIS_MIN as f64)
        .max_value(AXIS_MAX as f64)
        .value(0.0)
        .hexpand(true)
        .build();
    let x_val = Label::new(Some("0"));
    x_val.set_width_chars(7);
    x_row.append(&x_lbl);
    x_row.append(&x_bar);
    x_row.append(&x_val);

    let y_row = GtkBox::new(Orientation::Horizontal, 8);
    let y_lbl = Label::new(Some("Y"));
    y_lbl.set_width_chars(2);
    let y_bar = LevelBar::builder()
        .min_value(AXIS_MIN as f64)
        .max_value(AXIS_MAX as f64)
        .value(0.0)
        .hexpand(true)
        .build();
    let y_val = Label::new(Some("0"));
    y_val.set_width_chars(7);
    y_row.append(&y_lbl);
    y_row.append(&y_bar);
    y_row.append(&y_val);

    // Settings
    let mk_slider = |label: &str, min: f64, max: f64, value: f64| -> (GtkBox, Scale) {
        let row = GtkBox::new(Orientation::Vertical, 2);
        let lbl = Label::new(Some(label));
        lbl.set_halign(Align::Start);
        let scale = Scale::with_range(Orientation::Horizontal, min, max, (max - min) / 200.0);
        scale.set_value(value);
        scale.set_draw_value(true);
        scale.set_value_pos(gtk4::PositionType::Right);
        scale.set_hexpand(true);
        row.append(&lbl);
        row.append(&scale);
        (row, scale)
    };

    let c0 = cfg.borrow();
    let (sens_row, sens_scale) = mk_slider("Sensitivity", 100.0, 5000.0, c0.sensitivity);
    let (cx_row, cx_scale) = mk_slider("Center Strength X (roll)", 0.001, 0.2, c0.center_strength_x);
    let (cy_row, cy_scale) = mk_slider("Center Strength Y (pitch)", 0.001, 0.2, c0.center_strength_y);
    let (vol_row, vol_scale) = mk_slider("Sound Volume", 0.0, 1.0, c0.audio_volume);
    let cwg_check = CheckButton::with_label("Center while grabbed");
    cwg_check.set_active(c0.center_while_grabbed);
    drop(c0);

    // Wire settings
    {
        let shared = shared.clone();
        let cfg = cfg.clone();
        sens_scale.connect_value_changed(move |s| {
            let v = s.value();
            Shared::f32_store(&shared.sensitivity_fp, v as f32);
            cfg.borrow_mut().sensitivity = v;
        });
    }
    {
        let shared = shared.clone();
        let cfg = cfg.clone();
        cx_scale.connect_value_changed(move |s| {
            let v = s.value();
            Shared::f32_store(&shared.center_x_fp, v as f32);
            cfg.borrow_mut().center_strength_x = v;
        });
    }
    {
        let shared = shared.clone();
        let cfg = cfg.clone();
        cy_scale.connect_value_changed(move |s| {
            let v = s.value();
            Shared::f32_store(&shared.center_y_fp, v as f32);
            cfg.borrow_mut().center_strength_y = v;
        });
    }
    {
        let shared = shared.clone();
        let cfg = cfg.clone();
        vol_scale.connect_value_changed(move |s| {
            let v = s.value();
            Shared::f32_store(&shared.volume_fp, v as f32);
            cfg.borrow_mut().audio_volume = v;
        });
    }
    {
        let shared = shared.clone();
        let cfg = cfg.clone();
        cwg_check.connect_toggled(move |c| {
            let v = c.is_active();
            shared.center_while_grabbed.store(v, Ordering::Relaxed);
            cfg.borrow_mut().center_while_grabbed = v;
        });
    }

    // Bottom buttons
    let btn_row = GtkBox::new(Orientation::Horizontal, 8);
    let hide_btn = Button::with_label("Hide to Tray");
    let quit_btn = Button::with_label("Quit");
    btn_row.append(&hide_btn);
    btn_row.append(&quit_btn);

    {
        let window = window.clone();
        hide_btn.connect_clicked(move |_| {
            window.set_visible(false);
        });
    }
    {
        let shared = shared.clone();
        let cfg = cfg.clone();
        let window = window.clone();
        quit_btn.connect_clicked(move |_| {
            save_config(&cfg.borrow());
            shared.quit_requested.store(true, Ordering::Relaxed);
            shared.running.store(false, Ordering::Relaxed);
            window.close();
        });
    }

    // Close button -> hide to tray (keep running)
    {
        let shared = shared.clone();
        let cfg = cfg.clone();
        window.connect_close_request(move |w| {
            if shared.quit_requested.load(Ordering::Relaxed) {
                save_config(&cfg.borrow());
                glib::Propagation::Proceed
            } else {
                w.set_visible(false);
                glib::Propagation::Stop
            }
        });
    }

    root.append(&status_label);
    root.append(&x_row);
    root.append(&y_row);
    root.append(&sens_row);
    root.append(&cx_row);
    root.append(&cy_row);
    root.append(&vol_row);
    root.append(&cwg_check);
    root.append(&btn_row);
    window.set_child(Some(&root));

    // Periodic UI refresh: axes + show/hide + quit
    {
        let shared = shared.clone();
        let window = window.clone();
        let status_label = status_label.clone();
        let x_bar = x_bar.clone();
        let y_bar = y_bar.clone();
        let x_val = x_val.clone();
        let y_val = y_val.clone();
        let cfg = cfg.clone();
        glib::timeout_add_local(Duration::from_millis(33), move || {
            if shared.quit_requested.load(Ordering::Relaxed) {
                save_config(&cfg.borrow());
                window.close();
                return ControlFlow::Break;
            }
            if shared.show_window.swap(false, Ordering::Relaxed) {
                window.set_visible(true);
                window.present();
            }
            let grabbed = shared.grabbed.load(Ordering::Relaxed);
            status_label.set_text(if grabbed {
                "Status: GRABBED"
            } else {
                "Status: Released"
            });
            // We don't have direct axis readback from centering thread's local accumulators;
            // approximate by showing 0 when not grabbed and stable, or track via shared if needed.
            // Better: store emitted axes back to shared for UI. Add fields? Simpler: keep as-is,
            // UI shows grab state; axes bars can be driven by a separate atomic pair.
            // (See axes_shared below in main wiring - handled via ui_rx events.)
            ControlFlow::Continue
        });
    }

    // Handle events from input thread (grab state confirmed, axes if sent)
    {
        let status_label = status_label.clone();
        let x_bar = x_bar.clone();
        let y_bar = y_bar.clone();
        let x_val = x_val.clone();
        let y_val = y_val.clone();
        let window = window.clone();
        ui_rx.attach(None, move |ev| {
            match ev {
                UiEvent::Grabbed(g) => {
                    status_label.set_text(if g { "Status: GRABBED" } else { "Status: Released" });
                }
                UiEvent::Axes(x, y) => {
                    x_bar.set_value(x as f64);
                    y_bar.set_value(y as f64);
                    x_val.set_text(&x.to_string());
                    y_val.set_text(&y.to_string());
                }
                UiEvent::Quit => {
                    window.close();
                }
            }
            ControlFlow::Continue
        });
    }

    window.present();
}

fn main() {
    let cfg = load_config();
    let device_path = match detect_mouse_device(&cfg.mouse_device) {
        Some(p) => p,
        None => {
            eprintln!(
                "mouse2joy: could not find a mouse device. Set mouse.device in {} to a /dev/input/by-id/*-event-mouse path.",
                config_path().display()
            );
            // Try to list devices for help
            let by_id = Path::new("/dev/input/by-id");
            if let Ok(entries) = fs::read_dir(by_id) {
                eprintln!("Available by-id devices:");
                for e in entries.flatten() {
                    eprintln!("  {}", e.path().display());
                }
            }
            std::process::exit(1);
        }
    };

    let device = match Device::open(&device_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("mouse2joy: failed to open {}: {e}", device_path.display());
            eprintln!("Hint: you need read access (input group) or run with appropriate permissions.");
            std::process::exit(1);
        }
    };
    eprintln!("mouse2joy: reading from {}", device_path.display());

    let uinput = match create_uinput() {
        Ok(u) => Arc::new(Mutex::new(u)),
        Err(e) => {
            eprintln!("mouse2joy: failed to create uinput device: {e}");
            eprintln!("Hint: ensure /dev/uinput exists and you have write access (uinput module loaded, correct permissions).");
            std::process::exit(1);
        }
    };
    eprintln!("mouse2joy: virtual joystick 'mouse2joy Virtual Joystick' created");

    let shared = Arc::new(Shared::new(&cfg));
    let audio = AudioPlayer::new().map(Arc::new);

    // GTK app + channel for UI events
    let app = Application::builder().application_id(APP_ID).build();
    let shared_for_app = shared.clone();
    let uinput_for_app = uinput.clone();
    let cfg_rc = Rc::new(RefCell::new(cfg.clone()));

    app.connect_activate(move |app| {
        // Hold the application so closing/hiding the window doesn't exit while tray is alive
        let hold_guard = app.hold();

        let (ui_tx, ui_rx) = mpsc::channel::<UiEvent>();

        // Tray service
        let tray = MouseTray {
            shared: shared_for_app.clone(),
            ui_tx: ui_tx.clone(),
        };
        let tray_service = TrayService::new(tray);
        let tray_handle: Handle<MouseTray> = tray_service.handle();
        thread::spawn(move || {
            tray_service.run();
        });
        // Periodically update tray tooltip/icon state
        {
            let shared = shared_for_app.clone();
            glib::timeout_add_local(Duration::from_millis(250), move || {
                if !shared.running.load(Ordering::Relaxed) {
                    return ControlFlow::Break;
                }
                tray_handle.update(|_| {});
                ControlFlow::Continue
            });
        }

        // Centering thread (emits axes to uinput; also send axis snapshots to UI)
        {
            let shared = shared_for_app.clone();
            let uinput = uinput_for_app.clone();
            let ui_tx_axes = ui_tx.clone();
            thread::spawn(move || {
                // Wrap centering_loop but also report axes: easiest to inline a variant here
                let interval = Duration::from_micros(1_000_000 / CENTER_TICK_HZ);
                let mut x = 0.0f32;
                let mut y = 0.0f32;
                let mut last_ui = Instant::now();
                while shared.running.load(Ordering::Relaxed) {
                    let start = Instant::now();
                    let grabbed = shared.grabbed.load(Ordering::Relaxed);
                    let cwg = shared.center_while_grabbed.load(Ordering::Relaxed);
                    let cx = Shared::f32_load(&shared.center_x_fp);
                    let cy = Shared::f32_load(&shared.center_y_fp);
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
                    if last_ui.elapsed() >= Duration::from_millis(33) {
                        let _ = ui_tx_axes.send(UiEvent::Axes(xi, yi));
                        last_ui = Instant::now();
                    }
                    let elapsed = start.elapsed();
                    if elapsed < interval {
                        thread::sleep(interval - elapsed);
                    }
                }
                if let Ok(mut ui) = uinput.lock() {
                    let _ = ui.emit(&[
                        evdev::InputEvent::new_now(evdev::EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_X.0, 0),
                        evdev::InputEvent::new_now(evdev::EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Y.0, 0),
                        evdev::InputEvent::new_now(evdev::EventType::SYNCHRONIZATION.0, 0, 0),
                    ]);
                }
            });
        }

        // Input thread
        {
            let shared = shared_for_app.clone();
            let grab_button = cfg_rc.borrow().grab_button;
            let dev = Device::open(&device_path).expect("reopen device");
            let audio = audio.clone();
            thread::spawn(move || {
                input_loop(dev, shared, grab_button, ui_tx, audio);
            });
        }

        build_ui(app, shared_for_app.clone(), cfg_rc.clone(), ui_rx);

        // Keep hold_guard alive until app quits
        let _hold = hold_guard;
        // Leak the hold in effect by stashing in a static-ish way? GTK holds while app runs;
        // easier: store in window data or just forget. But we returned from activate;
        // hold_guard drops here which would release — bad for tray-only mode.
        // Fix: use std::mem::forget.
        std::mem::forget(_hold);
    });

    // Run with no args so GTK doesn't parse ours
    app.run_with_args::<&str>(&[]);
}
