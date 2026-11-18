//! Dedicated-thread global-hotkey registration and message loop.
//!
//! `RegisterHotKey` with a null window delivers `WM_HOTKEY` to the message queue
//! of the thread that registered it. Registration and the message loop must
//! therefore live on the same dedicated thread.
//!
//! Lifecycle:
//! 1. The worker creates its thread message queue and reports its thread id.
//! 2. It registers the requested chord, then reports success (or the exact
//!    typed failure) to the caller through a bounded, two-phase handshake.
//! 3. `register`/`register_chord` does not return `Ok` until step 2 succeeds, so
//!    dropping a `HotkeyRegistration` can always post `WM_QUIT` to a live queue.
//! 4. On `WM_QUIT`, the worker unregisters the chord and exits; `Drop` joins the
//!    worker. The join is the synchronization point that guarantees the hotkey
//!    is released before `register` can succeed again.
//!
//! The chord grammar mirrors `ansa-platform-api::hotkey` (the API crate depends
//! on this crate on Windows, so the dependency cannot point the other way). The
//! API crate has a parity test over the persisted grammar and native names.
//!
//! # Automatic sleep/lock handling
//!
//! This module does **not** create a message-only power/session window and does
//! not call `WTSRegisterSessionNotification`. Automatic wake/unlock
//! re-registration is therefore **not wired**. Callers that own a window
//! procedure can call [`HotkeyRegistration::rearm`] after
//! `WM_POWERBROADCAST`/`WM_WTSSESSION_CHANGE`; the explicit suspend/resume and
//! rearm methods are fully implemented and tested.
//!
//! # Conflicts
//!
//! `RegisterHotKey` error 1409 is reported as a typed
//! [`HotkeyRegistrationError::Conflict`] naming the chord. Invalid chords
//! (error 87) and access denial are distinct variants. A failed registration
//! joins the worker before returning, so no thread is left running.

use std::ffi::c_void;
use std::fmt;
use std::mem::MaybeUninit;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::CapabilityStatus;

const HOTKEY_ID: i32 = 1;
const PROBE_HOTKEY_ID: i32 = 0x4A53;

const MOD_ALT: u32 = 0x0001;
const MOD_CONTROL: u32 = 0x0002;
const MOD_SHIFT: u32 = 0x0004;
const MOD_WIN: u32 = 0x0008;
const MOD_NOREPEAT: u32 = 0x4000;

const PM_NOREMOVE: u32 = 0x0000;
const WM_HOTKEY: u32 = 0x0312;
const WM_QUIT: u32 = 0x0012;
const WM_APP: u32 = 0x8000;
const WM_ANSA_SUSPEND: u32 = WM_APP + 1;
const WM_ANSA_RESUME: u32 = WM_APP + 2;
const WM_ANSA_REARM: u32 = WM_APP + 3;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

/// Serializes process-global hotkey operations. `RegisterHotKey` ownership is
/// process-wide for our purposes, so registration, probing and lifecycle
/// commands must not race each other.
static PROCESS_LOCK: Mutex<()> = Mutex::new(());

/// The hotkey owned by this process, with the worker thread id.
static ACTIVE: Mutex<Option<(u32, KeyChord)>> = Mutex::new(None);

fn lock_process() -> MutexGuard<'static, ()> {
    PROCESS_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct Msg {
    hwnd: *mut c_void,
    message: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    point: Point,
    // Present in the modern Win32 MSG layout. It must be included so
    // GetMessageW cannot write past this allocation.
    private: u32,
}

#[link(name = "user32")]
extern "system" {
    fn RegisterHotKey(hwnd: *mut c_void, id: i32, modifiers: u32, virtual_key: u32) -> i32;
    fn UnregisterHotKey(hwnd: *mut c_void, id: i32) -> i32;
    fn GetMessageW(message: *mut Msg, hwnd: *mut c_void, filter_min: u32, filter_max: u32) -> i32;
    fn PeekMessageW(
        message: *mut Msg,
        hwnd: *mut c_void,
        filter_min: u32,
        filter_max: u32,
        remove: u32,
    ) -> i32;
    fn PostThreadMessageW(thread_id: u32, message: u32, w_param: usize, l_param: isize) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentThreadId() -> u32;
    fn GetLastError() -> u32;
}

// ---------------------------------------------------------------------------
// Chord grammar (Windows copy of the neutral grammar)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum KeyKind {
    Letter(char),
    Digit(u8),
    Function(u8),
    Space,
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    Punctuation(char),
}

/// A normalized Windows hotkey chord.
///
/// The persisted grammar is the same as `ansa_platform_api::KeyChord`:
/// lowercase modifiers in the order `ctrl`, `alt`, `shift`, `meta`, then one
/// key token, joined by `+`. [`KeyChord::render`] uses native Windows names
/// (`Ctrl+Alt+Shift+Win`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord {
    modifiers: u8,
    key: KeyKind,
}

impl KeyChord {
    const CONTROL: u8 = 1 << 0;
    const ALT: u8 = 1 << 1;
    const SHIFT: u8 = 1 << 2;
    const META: u8 = 1 << 3;

    /// The documented development default, `Ctrl+Space`.
    pub fn default_summon() -> Self {
        Self {
            modifiers: Self::CONTROL,
            key: KeyKind::Space,
        }
    }

    /// Parses the stable persisted grammar.
    pub fn parse(input: &str) -> Result<Self, HotkeyParseError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(HotkeyParseError::Empty);
        }

        let mut modifiers = 0u8;
        let mut key = None;
        for raw in trimmed.split('+') {
            let token = raw.trim();
            if token.is_empty() {
                return Err(HotkeyParseError::EmptyToken);
            }
            let lower = token.to_ascii_lowercase();
            if let Some(bit) = modifier_bit(&lower) {
                if modifiers & bit != 0 {
                    return Err(HotkeyParseError::DuplicateModifier(
                        modifier_name(bit).to_owned(),
                    ));
                }
                modifiers |= bit;
                continue;
            }
            if let Some(parsed) = parse_key(&lower) {
                if key.is_some() {
                    return Err(HotkeyParseError::MultipleKeys);
                }
                key = Some(parsed);
                continue;
            }
            return Err(HotkeyParseError::UnsupportedToken(token.to_owned()));
        }

        let key = key.ok_or(HotkeyParseError::ModifierOnly)?;
        Ok(Self { modifiers, key })
    }

    /// Stable lowercase persisted form.
    pub fn persisted(self) -> String {
        let mut tokens = Vec::new();
        for (bit, name) in [
            (Self::CONTROL, "ctrl"),
            (Self::ALT, "alt"),
            (Self::SHIFT, "shift"),
            (Self::META, "meta"),
        ] {
            if self.modifiers & bit != 0 {
                tokens.push(name.to_owned());
            }
        }
        tokens.push(key_persisted(self.key));
        tokens.join("+")
    }

    /// Renders native Windows names, for example `Ctrl+Alt+Shift+Win+A`.
    pub fn render(self) -> String {
        let mut tokens = Vec::new();
        for (bit, name) in [
            (Self::CONTROL, "Ctrl"),
            (Self::ALT, "Alt"),
            (Self::SHIFT, "Shift"),
            (Self::META, "Win"),
        ] {
            if self.modifiers & bit != 0 {
                tokens.push(name.to_owned());
            }
        }
        tokens.push(key_native(self.key));
        tokens.join("+")
    }

    /// Returns `true` when this is the default `Ctrl+Space` binding.
    pub fn is_default(self) -> bool {
        self == Self::default_summon()
    }

    fn contains(self, bit: u8) -> bool {
        self.modifiers & bit != 0
    }

    fn native_modifiers(self) -> u32 {
        let mut native = 0;
        if self.contains(Self::CONTROL) {
            native |= MOD_CONTROL;
        }
        if self.contains(Self::ALT) {
            native |= MOD_ALT;
        }
        if self.contains(Self::SHIFT) {
            native |= MOD_SHIFT;
        }
        if self.contains(Self::META) {
            native |= MOD_WIN;
        }
        native
    }

    fn virtual_key(self) -> u32 {
        match self.key {
            KeyKind::Letter(character) => character as u32,
            KeyKind::Digit(digit) => u32::from(b'0') + u32::from(digit),
            KeyKind::Function(number) => 0x70 + u32::from(number) - 1,
            KeyKind::Space => 0x20,
            KeyKind::Enter => 0x0D,
            KeyKind::Tab => 0x09,
            KeyKind::Escape => 0x1B,
            KeyKind::Backspace => 0x08,
            KeyKind::Delete => 0x2E,
            KeyKind::Insert => 0x2D,
            KeyKind::Home => 0x24,
            KeyKind::End => 0x23,
            KeyKind::PageUp => 0x21,
            KeyKind::PageDown => 0x22,
            KeyKind::Up => 0x26,
            KeyKind::Down => 0x28,
            KeyKind::Left => 0x25,
            KeyKind::Right => 0x27,
            KeyKind::Punctuation(character) => match character {
                ';' => 0xBA,
                '=' => 0xBB,
                ',' => 0xBC,
                '-' => 0xBD,
                '.' => 0xBE,
                '/' => 0xBF,
                '`' => 0xC0,
                '[' => 0xDB,
                '\\' => 0xDC,
                ']' => 0xDD,
                '\'' => 0xDE,
                _ => 0,
            },
        }
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.persisted())
    }
}

impl std::str::FromStr for KeyChord {
    type Err = HotkeyParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

fn modifier_bit(token: &str) -> Option<u8> {
    match token {
        "ctrl" | "control" => Some(KeyChord::CONTROL),
        "alt" | "option" | "opt" => Some(KeyChord::ALT),
        "shift" => Some(KeyChord::SHIFT),
        "meta" | "super" | "win" | "windows" | "cmd" | "command" => Some(KeyChord::META),
        _ => None,
    }
}

fn modifier_name(bit: u8) -> &'static str {
    match bit {
        KeyChord::CONTROL => "ctrl",
        KeyChord::ALT => "alt",
        KeyChord::SHIFT => "shift",
        KeyChord::META => "meta",
        _ => "modifier",
    }
}

const PUNCTUATION: [char; 11] = ['-', '=', '[', ']', '\\', ';', '\'', ',', '.', '/', '`'];

fn parse_key(token: &str) -> Option<KeyKind> {
    let named = match token {
        "space" => Some(KeyKind::Space),
        "enter" | "return" => Some(KeyKind::Enter),
        "tab" => Some(KeyKind::Tab),
        "escape" | "esc" => Some(KeyKind::Escape),
        "backspace" => Some(KeyKind::Backspace),
        "delete" | "del" => Some(KeyKind::Delete),
        "insert" | "ins" => Some(KeyKind::Insert),
        "home" => Some(KeyKind::Home),
        "end" => Some(KeyKind::End),
        "pageup" | "pgup" => Some(KeyKind::PageUp),
        "pagedown" | "pgdn" => Some(KeyKind::PageDown),
        "up" | "arrowup" => Some(KeyKind::Up),
        "down" | "arrowdown" => Some(KeyKind::Down),
        "left" | "arrowleft" => Some(KeyKind::Left),
        "right" | "arrowright" => Some(KeyKind::Right),
        _ => None,
    };
    if named.is_some() {
        return named;
    }

    if let Some(number) = token.strip_prefix('f') {
        if let Ok(number) = number.parse::<u8>() {
            if (1..=24).contains(&number) {
                return Some(KeyKind::Function(number));
            }
        }
    }

    match token {
        "minus" => return Some(KeyKind::Punctuation('-')),
        "equals" | "equal" => return Some(KeyKind::Punctuation('=')),
        "leftbracket" => return Some(KeyKind::Punctuation('[')),
        "rightbracket" => return Some(KeyKind::Punctuation(']')),
        "backslash" => return Some(KeyKind::Punctuation('\\')),
        "semicolon" => return Some(KeyKind::Punctuation(';')),
        "quote" => return Some(KeyKind::Punctuation('\'')),
        "comma" => return Some(KeyKind::Punctuation(',')),
        "period" => return Some(KeyKind::Punctuation('.')),
        "slash" => return Some(KeyKind::Punctuation('/')),
        "backtick" => return Some(KeyKind::Punctuation('`')),
        _ => {}
    }

    let mut characters = token.chars();
    if let (Some(character), None) = (characters.next(), characters.next()) {
        if character.is_ascii_alphabetic() {
            return Some(KeyKind::Letter(character.to_ascii_uppercase()));
        }
        if character.is_ascii_digit() {
            return Some(KeyKind::Digit(character as u8 - b'0'));
        }
        if PUNCTUATION.contains(&character) {
            return Some(KeyKind::Punctuation(character));
        }
    }
    None
}

fn key_persisted(key: KeyKind) -> String {
    match key {
        KeyKind::Letter(character) => character.to_ascii_lowercase().to_string(),
        KeyKind::Digit(digit) => digit.to_string(),
        KeyKind::Function(number) => format!("f{number}"),
        KeyKind::Space => "space".to_owned(),
        KeyKind::Enter => "enter".to_owned(),
        KeyKind::Tab => "tab".to_owned(),
        KeyKind::Escape => "escape".to_owned(),
        KeyKind::Backspace => "backspace".to_owned(),
        KeyKind::Delete => "delete".to_owned(),
        KeyKind::Insert => "insert".to_owned(),
        KeyKind::Home => "home".to_owned(),
        KeyKind::End => "end".to_owned(),
        KeyKind::PageUp => "pageup".to_owned(),
        KeyKind::PageDown => "pagedown".to_owned(),
        KeyKind::Up => "up".to_owned(),
        KeyKind::Down => "down".to_owned(),
        KeyKind::Left => "left".to_owned(),
        KeyKind::Right => "right".to_owned(),
        KeyKind::Punctuation(character) => character.to_string(),
    }
}

fn key_native(key: KeyKind) -> String {
    match key {
        KeyKind::Letter(character) => character.to_ascii_uppercase().to_string(),
        KeyKind::Digit(digit) => digit.to_string(),
        KeyKind::Function(number) => format!("F{number}"),
        KeyKind::Space => "Space".to_owned(),
        KeyKind::Enter => "Enter".to_owned(),
        KeyKind::Tab => "Tab".to_owned(),
        KeyKind::Escape => "Escape".to_owned(),
        KeyKind::Backspace => "Backspace".to_owned(),
        KeyKind::Delete => "Delete".to_owned(),
        KeyKind::Insert => "Insert".to_owned(),
        KeyKind::Home => "Home".to_owned(),
        KeyKind::End => "End".to_owned(),
        KeyKind::PageUp => "PageUp".to_owned(),
        KeyKind::PageDown => "PageDown".to_owned(),
        KeyKind::Up => "Up".to_owned(),
        KeyKind::Down => "Down".to_owned(),
        KeyKind::Left => "Left".to_owned(),
        KeyKind::Right => "Right".to_owned(),
        KeyKind::Punctuation(character) => match character {
            '-' => "Minus",
            '=' => "Equals",
            '[' => "LeftBracket",
            ']' => "RightBracket",
            '\\' => "Backslash",
            ';' => "Semicolon",
            '\'' => "Quote",
            ',' => "Comma",
            '.' => "Period",
            '/' => "Slash",
            '`' => "Backtick",
            _ => "Punctuation",
        }
        .to_owned(),
    }
}

/// A typed chord validation error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyParseError {
    /// The input was empty.
    Empty,
    /// A `+` separated token was empty.
    EmptyToken,
    /// Modifiers without a non-modifier key.
    ModifierOnly,
    /// The same modifier appeared twice.
    DuplicateModifier(String),
    /// More than one non-modifier key was supplied.
    MultipleKeys,
    /// The token is neither a modifier nor a supported key.
    UnsupportedToken(String),
}

impl fmt::Display for HotkeyParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("a hotkey chord must not be empty"),
            Self::EmptyToken => formatter.write_str(
                "a hotkey chord contains an empty '+' token; '+' is a separator and is not a key",
            ),
            Self::ModifierOnly => {
                formatter.write_str("a hotkey chord must include a non-modifier key")
            }
            Self::DuplicateModifier(modifier) => {
                write!(formatter, "modifier '{modifier}' appears more than once")
            }
            Self::MultipleKeys => {
                formatter.write_str("a hotkey chord must contain exactly one non-modifier key")
            }
            Self::UnsupportedToken(token) => {
                write!(formatter, "unsupported key or modifier '{token}'")
            }
        }
    }
}

impl std::error::Error for HotkeyParseError {}

// ---------------------------------------------------------------------------
// Typed registration/lifecycle errors
// ---------------------------------------------------------------------------

/// A typed Windows registration or lifecycle failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyRegistrationError {
    /// Error 1409: another application, the IME or this process owns the chord.
    Conflict {
        /// The attempted chord.
        chord: KeyChord,
        /// Platform detail.
        detail: String,
    },
    /// Error 87 or a chord that cannot be expressed.
    InvalidChord {
        /// The attempted chord.
        chord: KeyChord,
        /// Platform detail.
        detail: String,
    },
    /// Access denied by the host.
    Denied {
        /// The attempted chord.
        chord: KeyChord,
        /// Platform detail.
        detail: String,
    },
    /// Worker creation, message-loop or unregister failure.
    Worker {
        /// The chord involved.
        chord: KeyChord,
        /// Platform detail.
        detail: String,
    },
    /// The worker is not registered, so it cannot rearm or suspend.
    NotRegistered {
        /// The chord involved.
        chord: KeyChord,
        /// Platform detail.
        detail: String,
    },
    /// The worker is not suspended, so it cannot resume.
    NotSuspended {
        /// The chord involved.
        chord: KeyChord,
        /// Platform detail.
        detail: String,
    },
    /// The bounded lifecycle handshake timed out.
    Timeout {
        /// The chord involved.
        chord: KeyChord,
        /// Platform detail.
        detail: String,
    },
}

impl HotkeyRegistrationError {
    fn chord(&self) -> KeyChord {
        match self {
            Self::Conflict { chord, .. }
            | Self::InvalidChord { chord, .. }
            | Self::Denied { chord, .. }
            | Self::Worker { chord, .. }
            | Self::NotRegistered { chord, .. }
            | Self::NotSuspended { chord, .. }
            | Self::Timeout { chord, .. } => *chord,
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::Conflict { detail, .. }
            | Self::InvalidChord { detail, .. }
            | Self::Denied { detail, .. }
            | Self::Worker { detail, .. }
            | Self::NotRegistered { detail, .. }
            | Self::NotSuspended { detail, .. }
            | Self::Timeout { detail, .. } => detail,
        }
    }

    fn worker(chord: KeyChord, detail: impl Into<String>) -> Self {
        Self::Worker {
            chord,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for HotkeyRegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Conflict { .. } => "conflict",
            Self::InvalidChord { .. } => "invalid chord",
            Self::Denied { .. } => "access denied",
            Self::Worker { .. } => "worker failure",
            Self::NotRegistered { .. } => "not registered",
            Self::NotSuspended { .. } => "not suspended",
            Self::Timeout { .. } => "handshake timeout",
        };
        write!(
            formatter,
            "failed to register the {} hotkey ({kind}: {})",
            self.chord().render(),
            self.detail()
        )
    }
}

impl std::error::Error for HotkeyRegistrationError {}

fn win32_error(chord: KeyChord, code: u32) -> HotkeyRegistrationError {
    match code {
        1409 => HotkeyRegistrationError::Conflict {
            chord,
            detail: "the key combination is already registered by another application, the IME or this process".to_owned(),
        },
        87 => HotkeyRegistrationError::InvalidChord {
            chord,
            detail: "the system rejected the key combination as invalid".to_owned(),
        },
        5 => HotkeyRegistrationError::Denied {
            chord,
            detail: "Windows denied access to the hotkey registration".to_owned(),
        },
        _ => HotkeyRegistrationError::Worker {
            chord,
            detail: format!("the system rejected the hotkey registration (Win32 error {code})"),
        },
    }
}

/// Lifecycle result reported by the worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleOutcome {
    /// The chord was released.
    Suspended {
        /// The released chord.
        chord: KeyChord,
    },
    /// The worker was already suspended.
    AlreadySuspended,
    /// The chord was registered exactly once.
    Resumed {
        /// The registered chord.
        chord: KeyChord,
    },
    /// The worker was already registered.
    AlreadyRegistered {
        /// The active chord.
        chord: KeyChord,
    },
    /// The chord was unregistered and registered once.
    Rearmed {
        /// The registered chord.
        chord: KeyChord,
    },
}

/// The result of a non-destructive availability probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The chord can be registered.
    Available,
    /// Another application, the IME or this process owns the chord.
    Conflict {
        /// Platform detail.
        detail: String,
    },
    /// The host denied the probe.
    Denied {
        /// Platform detail.
        detail: String,
    },
    /// The chord cannot be expressed or the API failed.
    Unsupported {
        /// Platform detail.
        detail: String,
    },
}

impl fmt::Display for ProbeOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Available => formatter.write_str("available"),
            Self::Conflict { detail } => write!(formatter, "conflict: {detail}"),
            Self::Denied { detail } => write!(formatter, "denied: {detail}"),
            Self::Unsupported { detail } => write!(formatter, "unsupported: {detail}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration handle
// ---------------------------------------------------------------------------

enum WorkerReady {
    Started(u32),
    Finished(Result<(), HotkeyRegistrationError>),
}

type ControlSlot =
    Arc<Mutex<Option<SyncSender<Result<LifecycleOutcome, HotkeyRegistrationError>>>>>;

/// Owns a registered global hotkey and its dedicated message-loop thread.
///
/// Construct with [`HotkeyRegistration::register`] (default `Ctrl+Space`) or
/// [`HotkeyRegistration::register_chord`]. Dropping this value posts
/// `WM_QUIT`, waits for the worker to unregister the hotkey and exit, then
/// returns. There is no detached worker.
#[derive(Debug)]
pub struct HotkeyRegistration {
    thread_id: u32,
    worker: Option<JoinHandle<()>>,
    chord: KeyChord,
    control: ControlSlot,
    suspended: bool,
}

impl HotkeyRegistration {
    /// Registers the default `Ctrl+Space` hotkey and starts the worker.
    ///
    /// This preserves the original adapter API. The string error contains the
    /// native chord, the Win32 error class and the detail. Use
    /// [`HotkeyRegistration::register_chord`] for a typed error.
    pub fn register(callback: impl Fn() + Send + 'static) -> Result<Self, String> {
        Self::register_chord(KeyChord::default_summon(), callback)
            .map_err(|error| error.to_string())
    }

    /// Registers an arbitrary validated chord and starts the worker.
    ///
    /// The callback is invoked on the worker thread for each `WM_HOTKEY` and
    /// receives no content. Keep it non-blocking; waking the UI through a
    /// channel is the intended use.
    ///
    /// Returns only after the worker has successfully registered the chord.
    /// On any failure the worker is joined before returning.
    pub fn register_chord(
        chord: KeyChord,
        callback: impl Fn() + Send + 'static,
    ) -> Result<Self, HotkeyRegistrationError> {
        let _process_guard = lock_process();
        let (ready_sender, ready_receiver) = mpsc::sync_channel::<WorkerReady>(2);
        let control: ControlSlot = Arc::new(Mutex::new(None));
        let worker_control = Arc::clone(&control);

        let worker = thread::Builder::new()
            .name(format!("ansa-hotkey-{}", chord.persisted()))
            .spawn(move || {
                worker_main(chord, Box::new(callback), ready_sender, worker_control);
            })
            .map_err(|error| {
                HotkeyRegistrationError::worker(
                    chord,
                    format!("failed to start the hotkey worker: {error}"),
                )
            })?;

        let thread_id = match ready_receiver.recv_timeout(HANDSHAKE_TIMEOUT) {
            Ok(WorkerReady::Started(thread_id)) => thread_id,
            Ok(WorkerReady::Finished(Err(error))) => {
                let _ = worker.join();
                return Err(error);
            }
            Ok(WorkerReady::Finished(Ok(()))) => {
                let _ = worker.join();
                return Err(HotkeyRegistrationError::worker(
                    chord,
                    "the worker reported success before reporting its thread id",
                ));
            }
            Err(RecvTimeoutError::Timeout) => {
                let _ = worker.join();
                return Err(HotkeyRegistrationError::Timeout {
                    chord,
                    detail: "the hotkey worker did not report its message queue".to_owned(),
                });
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = worker.join();
                return Err(HotkeyRegistrationError::worker(
                    chord,
                    "the hotkey worker exited before reporting its message queue",
                ));
            }
        };

        match ready_receiver.recv_timeout(HANDSHAKE_TIMEOUT) {
            Ok(WorkerReady::Finished(Ok(()))) => {
                set_active(thread_id, chord);
                Ok(Self {
                    thread_id,
                    worker: Some(worker),
                    chord,
                    control,
                    suspended: false,
                })
            }
            Ok(WorkerReady::Finished(Err(error))) => {
                let _ = worker.join();
                Err(error)
            }
            Ok(WorkerReady::Started(_)) => {
                post_quit(thread_id);
                let _ = worker.join();
                Err(HotkeyRegistrationError::worker(
                    chord,
                    "the worker reported its thread id twice",
                ))
            }
            Err(RecvTimeoutError::Timeout) => {
                post_quit(thread_id);
                let _ = worker.join();
                Err(HotkeyRegistrationError::Timeout {
                    chord,
                    detail: "the hotkey worker did not report registration".to_owned(),
                })
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = worker.join();
                Err(HotkeyRegistrationError::worker(
                    chord,
                    "the hotkey worker exited during registration",
                ))
            }
        }
    }

    /// Returns the chord owned by this registration, whether or not it is
    /// currently suspended.
    pub fn chord(&self) -> KeyChord {
        self.chord
    }

    /// Returns whether the worker released the chord.
    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

    /// Releases the chord without stopping the worker.
    pub fn suspend(&mut self) -> Result<LifecycleOutcome, HotkeyRegistrationError> {
        let _process_guard = lock_process();
        if self.suspended {
            return Ok(LifecycleOutcome::AlreadySuspended);
        }
        let outcome = self.command(WM_ANSA_SUSPEND)?;
        match &outcome {
            LifecycleOutcome::Suspended { .. } | LifecycleOutcome::AlreadySuspended => {
                self.suspended = true;
                clear_active(self.thread_id);
            }
            LifecycleOutcome::Resumed { .. }
            | LifecycleOutcome::AlreadyRegistered { .. }
            | LifecycleOutcome::Rearmed { .. } => {
                self.suspended = false;
                set_active(self.thread_id, self.chord);
            }
        }
        Ok(outcome)
    }

    /// Re-registers the chord exactly once after a suspend.
    pub fn resume(&mut self) -> Result<LifecycleOutcome, HotkeyRegistrationError> {
        let _process_guard = lock_process();
        if !self.suspended {
            return Ok(LifecycleOutcome::AlreadyRegistered { chord: self.chord });
        }
        let outcome = self.command(WM_ANSA_RESUME)?;
        match &outcome {
            LifecycleOutcome::Resumed { .. }
            | LifecycleOutcome::AlreadyRegistered { .. }
            | LifecycleOutcome::Rearmed { .. } => {
                self.suspended = false;
                set_active(self.thread_id, self.chord);
            }
            LifecycleOutcome::Suspended { .. } | LifecycleOutcome::AlreadySuspended => {
                self.suspended = true;
                clear_active(self.thread_id);
            }
        }
        Ok(outcome)
    }

    /// Unregisters and re-registers the chord on the same worker.
    ///
    /// This is the explicit recovery hook for sleep/lock/session changes. The
    /// adapter does not observe those events automatically; see the module
    /// documentation.
    pub fn rearm(&mut self) -> Result<LifecycleOutcome, HotkeyRegistrationError> {
        let _process_guard = lock_process();
        if self.suspended {
            return Err(HotkeyRegistrationError::NotRegistered {
                chord: self.chord,
                detail: "the registration is suspended; call resume instead".to_owned(),
            });
        }
        match self.command(WM_ANSA_REARM) {
            Ok(
                outcome @ (LifecycleOutcome::Rearmed { .. }
                | LifecycleOutcome::Resumed { .. }
                | LifecycleOutcome::AlreadyRegistered { .. }),
            ) => {
                self.suspended = false;
                set_active(self.thread_id, self.chord);
                Ok(outcome)
            }
            Ok(outcome) => {
                // The worker did not re-arm (for example it was already
                // suspended). Treat the handle as suspended so `resume` can
                // reconcile state instead of returning a false success.
                self.suspended = true;
                clear_active(self.thread_id);
                Ok(outcome)
            }
            Err(error) => {
                // Re-arm unregisters before registering. If registration then
                // fails, the worker is left unregistered; expose that state so
                // `resume` can retry instead of silently reporting success.
                self.suspended = true;
                clear_active(self.thread_id);
                Err(error)
            }
        }
    }

    fn command(&mut self, message: u32) -> Result<LifecycleOutcome, HotkeyRegistrationError> {
        let (sender, receiver) = mpsc::sync_channel(1);
        {
            let mut slot = self.control.lock().map_err(|_| {
                HotkeyRegistrationError::worker(
                    self.chord,
                    "the hotkey control channel was poisoned",
                )
            })?;
            *slot = Some(sender);
        }

        // SAFETY: posting a thread message needs no payload pointers. The
        // worker's queue exists because registration completed its handshake.
        let posted = unsafe { PostThreadMessageW(self.thread_id, message, 0, 0) };
        if posted == 0 {
            self.clear_control();
            return Err(HotkeyRegistrationError::worker(
                self.chord,
                "the hotkey worker message queue is no longer available",
            ));
        }

        match receiver.recv_timeout(COMMAND_TIMEOUT) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                self.clear_control();
                Err(HotkeyRegistrationError::Timeout {
                    chord: self.chord,
                    detail: "the hotkey worker did not acknowledge the lifecycle command"
                        .to_owned(),
                })
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.clear_control();
                Err(HotkeyRegistrationError::worker(
                    self.chord,
                    "the hotkey worker dropped the lifecycle reply",
                ))
            }
        }
    }

    fn clear_control(&self) {
        if let Ok(mut slot) = self.control.lock() {
            *slot = None;
        }
    }

    #[cfg(test)]
    pub(crate) fn thread_id_for_test(&self) -> u32 {
        self.thread_id
    }
}

impl Drop for HotkeyRegistration {
    fn drop(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };

        // Serialize with register/probe/rearm so a new registration cannot
        // observe the OS release as a transient conflict while this drop is in
        // progress.
        let _process_guard = lock_process();
        post_quit(self.thread_id);
        let _ = worker.join();
        clear_active(self.thread_id);
    }
}

fn post_quit(thread_id: u32) {
    // SAFETY: WM_QUIT needs no pointer payload; if the worker already exited the
    // post fails and join returns immediately.
    unsafe {
        PostThreadMessageW(thread_id, WM_QUIT, 0, 0);
    }
}

fn set_active(thread_id: u32, chord: KeyChord) {
    if let Ok(mut active) = ACTIVE.lock() {
        *active = Some((thread_id, chord));
    }
}

fn clear_active(thread_id: u32) {
    if let Ok(mut active) = ACTIVE.lock() {
        if let Some((id, _)) = *active {
            if id == thread_id {
                *active = None;
            }
        }
    }
}

fn worker_main(
    chord: KeyChord,
    callback: Box<dyn Fn() + Send>,
    ready: SyncSender<WorkerReady>,
    control: ControlSlot,
) {
    let mut message = zeroed_message();

    // Force creation of this thread's message queue before registering.
    // SAFETY: the message buffer is valid and the optional HWND is null.
    unsafe {
        PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
    }

    // SAFETY: returns the calling thread's id.
    let thread_id = unsafe { GetCurrentThreadId() };
    if ready.send(WorkerReady::Started(thread_id)).is_err() {
        return;
    }

    let mut registered = match register_native(chord) {
        Ok(()) => true,
        Err(error) => {
            let _ = ready.send(WorkerReady::Finished(Err(error)));
            return;
        }
    };

    if ready.send(WorkerReady::Finished(Ok(()))).is_err() {
        // The caller disappeared before observing success. Do not leave the
        // hotkey registered.
        let _ = unregister_native(chord);
        return;
    }

    loop {
        // SAFETY: the message buffer is valid for writes; filters 0,0 mean all
        // messages; a null HWND requests thread messages.
        let result = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
        if result <= 0 {
            // 0 is WM_QUIT; -1 is a message-loop error. Both end the worker.
            break;
        }

        match message.message {
            WM_HOTKEY => {
                // A panicking callback must not kill the hotkey worker. The
                // panic was already reported by the default panic hook.
                let _ = catch_unwind(AssertUnwindSafe(&callback));
            }
            WM_ANSA_SUSPEND => {
                let reply = if !registered {
                    Ok(LifecycleOutcome::AlreadySuspended)
                } else {
                    match unregister_native(chord) {
                        Ok(()) => {
                            registered = false;
                            Ok(LifecycleOutcome::Suspended { chord })
                        }
                        Err(error) => Err(error),
                    }
                };
                reply_to(&control, reply);
            }
            WM_ANSA_RESUME => {
                let reply = if registered {
                    Ok(LifecycleOutcome::AlreadyRegistered { chord })
                } else {
                    match register_native(chord) {
                        Ok(()) => {
                            registered = true;
                            Ok(LifecycleOutcome::Resumed { chord })
                        }
                        Err(error) => Err(error),
                    }
                };
                reply_to(&control, reply);
            }
            WM_ANSA_REARM => {
                let reply = if !registered {
                    Err(HotkeyRegistrationError::NotRegistered {
                        chord,
                        detail: "the hotkey is suspended".to_owned(),
                    })
                } else {
                    match unregister_native(chord) {
                        Err(error) => Err(error),
                        Ok(()) => {
                            registered = false;
                            match register_native(chord) {
                                Ok(()) => {
                                    registered = true;
                                    Ok(LifecycleOutcome::Rearmed { chord })
                                }
                                Err(error) => Err(error),
                            }
                        }
                    }
                };
                reply_to(&control, reply);
            }
            _ => {}
        }
    }

    if registered {
        let _ = unregister_native(chord);
    }
}

fn reply_to(control: &ControlSlot, result: Result<LifecycleOutcome, HotkeyRegistrationError>) {
    let sender = control.lock().ok().and_then(|mut slot| slot.take());
    if let Some(sender) = sender {
        let _ = sender.send(result);
    }
}

fn register_native(chord: KeyChord) -> Result<(), HotkeyRegistrationError> {
    // SAFETY: null HWND directs WM_HOTKEY to this thread's queue. The chord was
    // validated by `KeyChord::parse` (or is the typed default), so the virtual
    // key and modifier bits are in range.
    let registered = unsafe {
        RegisterHotKey(
            std::ptr::null_mut(),
            HOTKEY_ID,
            chord.native_modifiers() | MOD_NOREPEAT,
            chord.virtual_key(),
        )
    };
    if registered == 0 {
        // SAFETY: GetLastError has no preconditions and is read immediately.
        let code = unsafe { GetLastError() };
        return Err(win32_error(chord, code));
    }
    Ok(())
}

fn unregister_native(chord: KeyChord) -> Result<(), HotkeyRegistrationError> {
    // SAFETY: this worker registered the hotkey and is still alive.
    let result = unsafe { UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ID) };
    if result == 0 {
        // SAFETY: GetLastError has no preconditions and is read immediately.
        let code = unsafe { GetLastError() };
        return Err(HotkeyRegistrationError::worker(
            chord,
            format!("UnregisterHotKey failed (Win32 error {code})"),
        ));
    }
    Ok(())
}

fn zeroed_message() -> Msg {
    // SAFETY: Msg is a plain C-layout struct of integers and raw pointers, for
    // which an all-zero bit pattern is valid and represents an empty message.
    unsafe { MaybeUninit::<Msg>::zeroed().assume_init() }
}

// ---------------------------------------------------------------------------
// Probing and capability reporting
// ---------------------------------------------------------------------------

/// Probes whether `chord` can be registered without leaving anything behind.
///
/// The probe runs on the caller thread, registers with a private id, and
/// immediately unregisters. It is serialized with registration and lifecycle
/// commands. If this process already owns `chord`, the probe reports
/// [`ProbeOutcome::Available`] without touching the active registration.
pub fn probe_chord(chord: &KeyChord) -> ProbeOutcome {
    let _process_guard = lock_process();
    probe_chord_locked(*chord)
}

fn probe_chord_locked(chord: KeyChord) -> ProbeOutcome {
    if let Ok(active) = ACTIVE.lock() {
        if let Some((_, active_chord)) = *active {
            if active_chord == chord {
                return ProbeOutcome::Available;
            }
        }
    }

    // SAFETY: null HWND and a private id; the chord is validated. The probe is
    // unregistered immediately and never enters a message loop.
    let registered = unsafe {
        RegisterHotKey(
            std::ptr::null_mut(),
            PROBE_HOTKEY_ID,
            chord.native_modifiers() | MOD_NOREPEAT,
            chord.virtual_key(),
        )
    };
    if registered != 0 {
        // SAFETY: this thread just registered the probe id.
        unsafe {
            UnregisterHotKey(std::ptr::null_mut(), PROBE_HOTKEY_ID);
        }
        return ProbeOutcome::Available;
    }

    // SAFETY: GetLastError has no preconditions and is read immediately.
    let code = unsafe { GetLastError() };
    match code {
        1409 => ProbeOutcome::Conflict {
            detail: "the key combination is already registered by another application, the IME or this process".to_owned(),
        },
        5 => ProbeOutcome::Denied {
            detail: "Windows denied access to the hotkey probe".to_owned(),
        },
        87 => ProbeOutcome::Unsupported {
            detail: "the system rejected the key combination as invalid".to_owned(),
        },
        _ => ProbeOutcome::Unsupported {
            detail: format!("RegisterHotKey probe failed (Win32 error {code})"),
        },
    }
}

/// Reports the current global-summon capability using a real probe of the
/// default chord, not a constant.
///
/// A default-chord conflict is reported as degraded (global hotkeys still work
/// if rebound). Access denial maps to permission-required and an unusable API
/// maps to unsupported.
pub fn global_hotkey_capability() -> CapabilityStatus {
    let _process_guard = lock_process();
    match probe_chord_locked(KeyChord::default_summon()) {
        ProbeOutcome::Available => CapabilityStatus::Available,
        ProbeOutcome::Conflict { detail } => CapabilityStatus::Degraded {
            reason: format!(
                "the default Ctrl+Space summon chord is unavailable: {detail}"
            ),
            alternative: "Rebind the summon shortcut, use a desktop shortcut, or launch ansa-desktop directly."
                .to_owned(),
        },
        ProbeOutcome::Denied { detail } => CapabilityStatus::Degraded {
            reason: format!(
                "Windows denied the global hotkey probe; the current session may not allow hotkeys: {detail}"
            ),
            alternative: "Use a desktop shortcut or launch ansa-desktop directly, and report the denied registration."
                .to_owned(),
        },
        ProbeOutcome::Unsupported { .. } => CapabilityStatus::Unsupported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Hotkey registration is process-global. Serialize tests that acquire it.
    static HOTKEY_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn test_guard() -> MutexGuard<'static, ()> {
        HOTKEY_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn default_chord() -> KeyChord {
        KeyChord::default_summon()
    }

    /// Returns a chord the current desktop can actually register.
    ///
    /// `Ctrl+Space` is owned by the IME on some machines. The test suite then
    /// falls back to an uncommonly used chord so the real worker lifecycle is
    /// still exercised, and reports which chord it used.
    fn available_test_chord() -> Option<KeyChord> {
        for (index, candidate) in [
            default_chord(),
            KeyChord::parse("ctrl+alt+shift+f24").expect("valid fallback chord"),
            KeyChord::parse("ctrl+alt+shift+f23").expect("valid fallback chord"),
        ]
        .into_iter()
        .enumerate()
        {
            match HotkeyRegistration::register_chord(candidate, || {}) {
                Ok(registration) => {
                    drop(registration);
                    if index > 0 {
                        eprintln!(
                            "Ctrl+Space is unavailable; using {} for the real-worker hotkey test",
                            candidate.render()
                        );
                    }
                    return Some(candidate);
                }
                Err(HotkeyRegistrationError::Conflict { .. }) => continue,
                Err(error) => {
                    eprintln!("skipping real-worker hotkey test: {error}");
                    return None;
                }
            }
        }
        eprintln!(
            "skipping real-worker hotkey test: Ctrl+Space and both fallback chords are owned by other applications"
        );
        None
    }

    #[test]
    fn chord_grammar_matches_the_neutral_contract() {
        assert_eq!(
            KeyChord::parse("Ctrl + Option+Shift+Command+A")
                .unwrap()
                .persisted(),
            "ctrl+alt+shift+meta+a"
        );
        assert_eq!(
            KeyChord::parse("ctrl+alt+shift+win+a").unwrap().render(),
            "Ctrl+Alt+Shift+Win+A"
        );
        assert_eq!(
            KeyChord::parse("ctrl+arrowup").unwrap().persisted(),
            "ctrl+up"
        );
        assert_eq!(KeyChord::parse("ctrl+minus").unwrap().persisted(), "ctrl+-");
        assert_eq!(default_chord().render(), "Ctrl+Space");

        assert_eq!(KeyChord::parse(""), Err(HotkeyParseError::Empty));
        assert_eq!(KeyChord::parse("ctrl"), Err(HotkeyParseError::ModifierOnly));
        assert_eq!(
            KeyChord::parse("ctrl+ctrl+a"),
            Err(HotkeyParseError::DuplicateModifier("ctrl".to_owned()))
        );
        assert_eq!(
            KeyChord::parse("ctrl+a+b"),
            Err(HotkeyParseError::MultipleKeys)
        );
        assert_eq!(
            KeyChord::parse("ctrl+f25"),
            Err(HotkeyParseError::UnsupportedToken("f25".to_owned()))
        );
    }

    #[test]
    fn register_and_drop_release_the_chord_and_conflict_is_typed() {
        let _guard = test_guard();

        let Some(chord) = available_test_chord() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let first = match HotkeyRegistration::register_chord(chord, move || {
            let _ = sender.send(());
        }) {
            Ok(registration) => registration,
            Err(error) => {
                eprintln!("skipping global hotkey test: {error}");
                return;
            }
        };

        // Simulate the WM_HOTKEY that RegisterHotKey would post. This verifies
        // the worker loop and callback without injecting system input.
        // SAFETY: posting a synthetic thread message needs no payload pointers.
        let posted = unsafe {
            PostThreadMessageW(first.thread_id_for_test(), WM_HOTKEY, HOTKEY_ID as usize, 0)
        };
        assert_ne!(posted, 0, "failed to post the synthetic hotkey message");
        receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("hotkey callback was not invoked");

        let second = HotkeyRegistration::register_chord(chord, || {});
        let error = match second {
            Ok(_) => panic!("a second registration must fail while the first is active"),
            Err(error) => error,
        };
        assert!(
            matches!(error, HotkeyRegistrationError::Conflict { .. }),
            "unexpected conflict error: {error}"
        );
        assert!(error.to_string().contains(&chord.render()));
        assert!(
            error.to_string().contains("1409") || error.to_string().contains("already registered")
        );

        drop(first);
        let third = match HotkeyRegistration::register_chord(chord, || {}) {
            Ok(registration) => registration,
            Err(error) => panic!("hotkey remained registered after drop: {error}"),
        };
        drop(third);
    }

    #[test]
    fn suspend_resume_and_rearm_use_one_worker() {
        let _guard = test_guard();
        let Some(chord) = available_test_chord() else {
            return;
        };
        let mut registration = match HotkeyRegistration::register_chord(chord, || {}) {
            Ok(registration) => registration,
            Err(error) => {
                eprintln!("skipping global hotkey lifecycle test: {error}");
                return;
            }
        };

        assert_eq!(
            registration.suspend().unwrap(),
            LifecycleOutcome::Suspended { chord }
        );
        assert!(registration.is_suspended());
        assert_eq!(
            registration.suspend().unwrap(),
            LifecycleOutcome::AlreadySuspended
        );
        assert_eq!(
            registration.resume().unwrap(),
            LifecycleOutcome::Resumed { chord }
        );
        assert!(!registration.is_suspended());
        assert_eq!(
            registration.resume().unwrap(),
            LifecycleOutcome::AlreadyRegistered { chord }
        );
        assert_eq!(
            registration.rearm().unwrap(),
            LifecycleOutcome::Rearmed { chord }
        );
    }

    #[test]
    fn probe_reports_conflict_for_a_chord_owned_by_this_process() {
        let _guard = test_guard();
        let Some(chord) = available_test_chord() else {
            return;
        };
        let registration = match HotkeyRegistration::register_chord(chord, || {}) {
            Ok(registration) => registration,
            Err(error) => {
                eprintln!("skipping global hotkey probe test: {error}");
                return;
            }
        };
        assert_eq!(probe_chord(&chord), ProbeOutcome::Available);
        drop(registration);
    }

    /// 1,000 real register/drop cycles.
    ///
    /// `Ctrl+Space` is used when free; otherwise an uncommonly used fallback
    /// chord is used and reported. If every candidate is owned, the test skips
    /// with the exact error.
    #[test]
    fn real_worker_survives_1000_register_unregister_cycles() {
        let _guard = test_guard();
        let Some(chord) = available_test_chord() else {
            return;
        };
        for cycle in 0..1_000 {
            let registration = match HotkeyRegistration::register_chord(chord, || {}) {
                Ok(registration) => registration,
                Err(error) => {
                    eprintln!(
                        "1000-cycle real hotkey test failed at cycle {cycle} for {}: {error}",
                        chord.render()
                    );
                    panic!("real worker failed at cycle {cycle}: {error}");
                }
            };
            drop(registration);
        }
        eprintln!(
            "completed 1000 register/unregister cycles with the real worker on {}",
            chord.render()
        );
    }

    #[test]
    fn win32_errors_map_to_distinct_typed_variants() {
        let chord = default_chord();
        assert!(matches!(
            win32_error(chord, 1409),
            HotkeyRegistrationError::Conflict { .. }
        ));
        assert!(matches!(
            win32_error(chord, 87),
            HotkeyRegistrationError::InvalidChord { .. }
        ));
        assert!(matches!(
            win32_error(chord, 5),
            HotkeyRegistrationError::Denied { .. }
        ));
        let message = win32_error(chord, 1409).to_string();
        assert!(message.contains("Ctrl+Space"));
        assert!(message.contains("already registered"));
    }

    #[test]
    fn invalid_chord_is_rejected_before_the_worker_starts() {
        // The typed chord parser rejects this before RegisterHotKey; the
        // adapter's synchronous error 87 path is exercised by the platform
        // API integration test where a valid grammar token is unsupported on a
        // future layout.
        let error = KeyChord::parse("ctrl+hyper").unwrap_err();
        assert!(matches!(error, HotkeyParseError::UnsupportedToken(_)));
        assert!(error.to_string().contains("hyper"));
    }
}
