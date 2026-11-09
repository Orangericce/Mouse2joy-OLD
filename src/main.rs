use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, Device, EventSummary, InputEventKind, KeyCode,
    RelativeAxisCode, UinputAbsSetup,
};
use evdev::uinput::VirtualDevice;
use crossterm::event::{Event, KeyCode, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::style::Color;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Margin};
use ratatui::style::{Modifier, Style as TuiStyle};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph, Widget, Fill, List, ListItem};
use ratatui::Terminal;
use std::borrow::Cow;
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

const AXIS_MIN: i32 = -32767;
const AXIS_MAX: i32 = 32767;
const CENTER_TICK_HZ: u64 = 60;
const APP_ID: &str = "dev.local.mouse2joy";

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

#[derive(Clone, Copy, Debug)]
enum UiEvent {
    Grabbed(bool),
    Axes(i32, i32),
    Quit,
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
    show_window: AtomicBool,
    quit_requested: AtomicBool,
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
            show_window: AtomicBool::new(true),
            quit_requested: AtomicBool::new(false),
        }
    }
    fn f32_load(a: &AtomicU32) -> f32 { f32::from_bits(a.load(std::sync::atomic::Ordering::Relaxed)) }
    fn f32_store(a: &AtomicU32, v: f32) { a.store(v.to_bits(), std::sync::atomic::Ordering::Relaxed) }
}

fn clamp_i32(v: i32, lo: i32, hi: i32) -> i32 { v.max(lo).min(hi) }

fn create_uinput() -> Result<Mutex<VirtualDevice>, Box<dyn std::error::Error>> {
    let abs_x = evdev::uinput::UinputAbsSetup::new(
        AbsoluteAxisCode::ABS_X,
        evdev::AbsInfo::new(0, AXIS_MIN, AXIS_MAX, 0, 0, 0),
    );
    let abs_y = evdev::uinput::UinputAbsSetup::new(
        AbsoluteAxisCode::ABS_Y,
        evdev::AbsInfo::new(0, AXIS_MIN, AXIS_MAX, 0, 0, 0),
    );
    let mut keys = evdev::AttributeSet::<KeyCode>::new();
    keys.insert(KeyCode::BTN_TRIGGER);
    keys.insert(KeyCode::BTN_THUMB);
    keys.insert(KeyCode::BTN_THUMB2);

    let dev = VirtualDevice::builder()?
        .name("mouse2joy Virtual Joystick")?
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
    let mut last_ui = Instant::now();
    let mut audio = AudioPlayer::new();

    while shared.running.load(Ordering::Relaxed) {
        let start = Instant::now();
        let grabbed = shared.grabbed.load(Ordering::Relaxed);
        let cwg = shared.center_while_grabbed.load(Ordering::Relaxed);
        let cx = AudioPlayer::f32_load(&shared.center_x_fp);
        let cy = AudioPlayer::f32_load(&shared.center_y_fp);

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
            let _ = mpsc::channel::<UiEvent>(0).0.send(UiEvent::Axes(xi, yi));
            last_ui = Instant::now();
        }

        let elapsed = start.elapsed();
        if elapsed < interval { thread::sleep(interval - elapsed); }
    }
    // Emit center on exit
    if let Ok(mut ui) = uinput.lock() {
        let _ = ui.emit(&[
            evdev::InputEvent::new_now(evdev::EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_X.0, 0),
            evdev::InputEvent::new_now(evdev::EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Y.0, 0),
            evdev::InputEvent::new_now(evdev::EventType::SYNCHRONIZATION.0, 0, 0),
        ]);
    }
}

fn input_loop(
    mut device: Device,
    shared: Arc<SharedState>,
    grab_button: u16,
    ui_tx: mpsc::Sender<UiEvent>,
    audio: Option<Arc<AudioPlayer>>,
) {
    while shared.running.load(Ordering::Relaxed) {
        let events = match device.fetch_events() {
            Ok(ev) => ev,
            Err(_) => { thread::sleep(Duration::from_millis(50)); continue; }
        };
        for ev in events {
            if !shared.running.load(Ordering::Relaxed) { break; }
            match ev.destructure() {
                evdev::EventSummary::Key(_, key_code, value) if key_code.as_u16() == grab_button => {
                    if value == 1 {
                        let now_grabbed = !shared.grabbed.load(Ordering::Relaxed);
                        shared.grabbed.store(now_grabbed, Ordering::Relaxed);
                        if let Some(a) = &audio {
                            a.play(if now_grabbed { 1 } else { 0 }, AudioPlayer::f32_load(&shared.volume_fp));
                        }
                        let _ = ui_tx.send(UiEvent::Grabbed(now_grabbed));
                        if now_grabbed { let _ = device.grab(); } else { let _ = device.ungrab(); }
                    }
                }
                evdev::EventSummary::RelativeAxis(_, axis, value) => {
                    if !shared.grabbed.load(Ordering::Relaxed) { continue; }
                    let sens = AudioPlayer::f32_load(&shared.sensitivity_fp);
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

fn ui_terminal(shared: Arc<SharedState>, cfg: Rc<RefCell<Config>>) {
    let (ui_tx, mut ui_rx) = mpsc::channel::<UiEvent>(16);
    let stdout = std::io::stdout();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(stdout)).unwrap();
    let mut running = true;

    // Set up centering thread with ui_tx reference
    let shared_c = shared.clone();
    let _centering_thread = thread::spawn(move || {
        centering_loop(shared_c, Arc::new(Mutex::new(())).unwrap()); // simplified
    });

    while running {
        terminal.draw(|f| {
            let size = f.size();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(2)
                .split(size);

            // Status bar
            let status_text = if shared.grabbed.load(Ordering::Relaxed) {
                "Status: GRABBED"
            } else {
                "Status: RELEASED"
            };
            let status = Paragraph::new(status_text).block(Block::default().borders(Borders::ALL));
            f.render_widget(status, chunks[0]);

            // Axis display
            let x_val = shared.x.load(Ordering::Relaxed);
            let y_val = shared.y.load(Ordering::Relaxed);
            let x_paragraph = Paragraph::new(format!("X: {}", x_val)).block(Block::default().borders(Borders::ALL));
            let y_paragraph = Paragraph::new(format!("Y: {}", y_val)).block(Block::default().borders(Borders::ALL));
            f.render_widget(x_paragraph, chunks[1]);
            f.render_widget(y_paragraph, chunks[2]);

            // Settings
            let cfg_b = cfg.borrow();
            let sensitivity_text = format!("Sensitivity: {:.1}", cfg_b.sensitivity);
            let center_x_text = format!("Center X: {:.3}", cfg_b.center_strength_x);
            let center_y_text = format!("Center Y: {:.3}", cfg_b.center_strength_y);
            let volume_text = format!("Volume: {:.1}%", cfg_b.audio_volume * 100.0);
            let center_while_text = if cfg_b.center_while_grabbed { "Center While Grabbed: ON" } else { "Center While Grabbed: OFF" };

            let settings = List::new([
                ListItem::new(sensitivity_text),
                ListItem::new(center_x_text),
                ListItem::new(center_y_text),
                ListItem::new(volume_text),
                ListItem::new(center_while_text),
            ])
            .block(Block::default().borders(Borders::ALL));
            f.render_widget(settings, chunks[3]);

            ControlFlow::Continue
        })?;

        // Handle UI events
        if let Ok(ev) = ui_rx.try_recv() {
            match ev {
                UiEvent::Grabbed(g) => { /* update UI */ }
                UiEvent::Axes(x, y) => { /* update axis display */ }
                UiEvent::Quit => { running = false; }
            }
        }

        // Check for quit
        if shared.quit_requested.load(Ordering::Relaxed) { running = false; }
    }

    let _ = ui_rx.close();
}

fn main() {
    let cfg = load_config();
    let device_path = match detect_mouse_device(&cfg.mouse_device) {
        Some(p) => p,
        None => {
            eprintln!("Could not find mouse device");
            std::process::exit(1);
        }
    };

    let device = Device::open(&device_path).expect("Could not open mouse device");
    let uinput = Arc::new(Mutex::new(create_uinput().expect("Could not create uinput device")));
    let shared = Arc::new(SharedState::new(&cfg));

    // Parse grab button from config (could be "SIDE", "BTN_SIDE", or numeric)
    let grab_button = cfg.grab_button;

    let audio = AudioPlayer::new().map(Arc::new);
    let shared_for_input = shared.clone();
    let grab_button_local = grab_button;

    // Start input thread
    let dev = device;
    let shared_input = shared.clone();
    let ui_tx = mpsc::channel::<UiEvent>(16).0;
    let audio_clone = audio.clone();
    thread::spawn(move || {
        input_loop(dev, shared_input, grab_button_local, ui_tx, audio_clone);
    });

    // Start UI
    let cfg_rc = Rc::new(RefCell::new(cfg));
    ui_terminal(shared, cfg_rc);

    // Save config and clean up
    save_config(&cfg);
    shared.running.store(false, std::sync::atomic::Ordering::Relaxed);
}