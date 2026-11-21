//! Bounded packaged/MSIX application enumeration.
//!
//! Windows does not expose a small dependency-free WinRT binding in this
//! adapter, so enumeration uses a **fixed** PowerShell command:
//!
//! ```text
//! [Console]::OutputEncoding=[Text.Encoding]::UTF8;
//! Get-StartApps | Select-Object Name,AppID | ConvertTo-Json -Compress
//! ```
//!
//! The command is a compile-time constant. No application name, AUMID, path or
//! other metadata is interpolated into it, and `cmd.exe` is never spawned.
//! Output is size-capped, decoded as strict UTF-8 and parsed by the strict JSON
//! parser below. Rows whose `AppID` is not a valid AUMID (for example the
//! desktop paths that `Get-StartApps` also returns) are ignored.
//!
//! AUMIDs are validated against an allowlist and launched only through
//! `shell:AppsFolder\<AUMID>`.
//!
//! The child process is bounded by a timeout. Reader threads are joined before
//! returning in the normal and successfully-terminated timeout paths. Residual:
//! if Windows cannot terminate a hung child within the 1-second kill grace
//! period, the call returns an explicit error instead of blocking, and the two
//! pipe-reader threads remain alive until that child eventually exits. They do
//! no work while waiting and are never detached in the normal path.

use crate::start_menu::stable_id_for_key;
use crate::types::{AppEntry, CapabilityStatus};
use std::io::{ErrorKind, Read};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_PACKAGED_APPS: usize = 5_000;
const MAX_AUMID_BYTES: usize = 512;
const MAX_JSON_DEPTH: usize = 24;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const LIST_COMMAND: &str = "[Console]::OutputEncoding=[Text.Encoding]::UTF8; Get-StartApps | Select-Object Name,AppID | ConvertTo-Json -Compress";
const PROBE_TTL: Duration = Duration::from_secs(30);

static PROBE_CACHE: std::sync::Mutex<Option<(Instant, CapabilityStatus)>> =
    std::sync::Mutex::new(None);

/// A validated packaged application row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PackagedApp {
    pub(crate) name: String,
    pub(crate) aumid: String,
}

/// Enumerates packaged applications as catalog entries.
pub(crate) fn enumerate() -> Result<Vec<AppEntry>, String> {
    let output = run_powershell(LIST_COMMAND, DEFAULT_TIMEOUT)?;
    if !output.success {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        return Err(if detail.is_empty() {
            format!(
                "Get-StartApps exited with status {}",
                output
                    .code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "unknown".to_owned())
            )
        } else {
            format!("Get-StartApps failed: {detail}")
        });
    }

    let text = decode_powershell_output(&output.stdout)?;
    let apps = parse_packaged_json(&text)?;

    Ok(apps
        .into_iter()
        .map(|app| AppEntry {
            id: stable_id_for_key(&format!("aumid:{}", app.aumid.to_ascii_lowercase())),
            title: app.name,
            path: packaged_launch_path(&app.aumid),
            target: None,
            arguments: None,
            working_directory: None,
            icon_source: None,
            packaged: true,
            broken: None,
        })
        .collect())
}

/// Probes whether packaged enumeration works on this machine right now.
///
/// `Available` is returned only after a real `Get-StartApps` round trip parsed
/// successfully. Failures are reported as `Degraded` with an alternative rather
/// than as a silent absence or a false success.
pub(crate) fn probe_status() -> CapabilityStatus {
    // A full Get-StartApps round trip takes roughly a second. Cache the observed
    // result briefly so capability probes on a UI path cannot block every time,
    // while still noticing a transient failure within the TTL.
    let mut cache = match PROBE_CACHE.lock() {
        Ok(cache) => cache,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some((checked_at, status)) = cache.as_ref() {
        if checked_at.elapsed() < PROBE_TTL {
            return status.clone();
        }
    }

    let status = match enumerate() {
        Ok(_) => CapabilityStatus::Available,
        Err(reason) => CapabilityStatus::Degraded {
            reason: format!("packaged application enumeration could not be validated: {reason}"),
            alternative:
                "classic Start Menu shortcuts are still discovered; use the packaged application's own shortcut if one is installed"
                    .to_owned(),
        },
    };
    *cache = Some((Instant::now(), status.clone()));
    status
}

/// Returns the only launch form this adapter accepts for a packaged AUMID.
pub(crate) fn packaged_launch_path(aumid: &str) -> String {
    format!("shell:AppsFolder\\{aumid}")
}

/// Validates an Application User Model ID.
///
/// Allowed components are ASCII alphanumerics plus `.`, `_`, `-` and `~`, in a
/// single `package!application` pair. This excludes spaces, quotes, path
/// separators, drive letters, percent signs and brace-wrapped shell GUIDs.
pub(crate) fn is_valid_aumid(aumid: &str) -> bool {
    if aumid.is_empty() || aumid.len() > MAX_AUMID_BYTES {
        return false;
    }
    let Some((package, application)) = aumid.split_once('!') else {
        return false;
    };
    if package.is_empty() || application.is_empty() || application.contains('!') {
        return false;
    }
    valid_aumid_component(package) && valid_aumid_component(application)
}

fn valid_aumid_component(component: &str) -> bool {
    component.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '~')
    })
}

/// Decodes Windows PowerShell output.
///
/// PowerShell writes UTF-16LE to redirected stdout on current Windows builds,
/// with or without a byte-order mark, while newer hosts emit UTF-8. Only whole,
/// valid strings are accepted; invalid byte sequences are an explicit error.
fn decode_powershell_output(bytes: &[u8]) -> Result<String, String> {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(rest, false);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return std::str::from_utf8(rest)
            .map(str::to_owned)
            .map_err(|_| "Get-StartApps returned invalid UTF-8 after a BOM".to_owned());
    }
    if looks_like_utf16(bytes, true) {
        return decode_utf16(bytes, true);
    }
    if looks_like_utf16(bytes, false) {
        return decode_utf16(bytes, false);
    }
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| "Get-StartApps returned output that is not valid UTF-8 or UTF-16".to_owned())
}

fn looks_like_utf16(bytes: &[u8], little_endian: bool) -> bool {
    if bytes.len() < 8 || bytes.len() % 2 != 0 {
        return false;
    }
    let pairs = bytes.len() / 2;
    let sample = pairs.min(32);
    let mut ascii = 0;
    for index in 0..sample {
        let (zero, text) = if little_endian {
            (bytes[index * 2 + 1], bytes[index * 2])
        } else {
            (bytes[index * 2], bytes[index * 2 + 1])
        };
        if zero == 0 && text != 0 {
            ascii += 1;
        }
    }
    ascii * 2 >= sample
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> Result<String, String> {
    if bytes.len() % 2 != 0 {
        return Err("Get-StartApps returned truncated UTF-16 output".to_owned());
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| {
            if little_endian {
                u16::from_le_bytes([pair[0], pair[1]])
            } else {
                u16::from_be_bytes([pair[0], pair[1]])
            }
        })
        .collect();
    String::from_utf16(&units)
        .map_err(|_| "Get-StartApps returned invalid UTF-16 output".to_owned())
}

/// Parses `Get-StartApps` JSON strictly and returns validated, deduplicated
/// packaged rows.
pub(crate) fn parse_packaged_json(input: &str) -> Result<Vec<PackagedApp>, String> {
    if input.len() > MAX_OUTPUT_BYTES {
        return Err(format!(
            "Get-StartApps output exceeds the {MAX_OUTPUT_BYTES}-byte limit"
        ));
    }
    if input.trim().is_empty() {
        return Ok(Vec::new());
    }

    let mut parser = JsonParser::new(input);
    let value = parser.parse()?;

    let mut rows = Vec::new();
    match value {
        JsonValue::Array(values) => {
            for value in values {
                if let Some(row) = packaged_row(value)? {
                    rows.push(row);
                }
                if rows.len() > MAX_PACKAGED_APPS {
                    return Err(format!(
                        "Get-StartApps returned more than {MAX_PACKAGED_APPS} packaged applications"
                    ));
                }
            }
        }
        JsonValue::Null => {}
        other => {
            if let Some(row) = packaged_row(other)? {
                rows.push(row);
            }
        }
    }

    let mut seen = std::collections::HashSet::new();
    rows.retain(|row| seen.insert(row.aumid.to_ascii_lowercase()));
    Ok(rows)
}

fn packaged_row(value: JsonValue) -> Result<Option<PackagedApp>, String> {
    let JsonValue::Object(fields) = value else {
        return Err("Get-StartApps JSON row is not an object".to_owned());
    };

    let mut name = None;
    let mut app_id = None;
    for (key, value) in fields {
        match key.as_str() {
            "Name" => {
                name = Some(json_string(&key, value)?);
            }
            "AppID" => {
                app_id = Some(json_string(&key, value)?);
            }
            _ => {}
        }
    }

    let app_id = app_id.ok_or_else(|| "Get-StartApps JSON row is missing AppID".to_owned())?;
    let name = name.ok_or_else(|| "Get-StartApps JSON row is missing Name".to_owned())?;

    // `Get-StartApps` also lists classic desktop applications whose AppID is a
    // filesystem path or a shell GUID. They are not packaged AUMIDs.
    if !is_valid_aumid(&app_id) {
        return Ok(None);
    }

    let name = name.trim();
    let name = if name.is_empty() {
        app_id.clone()
    } else {
        name.to_owned()
    };

    Ok(Some(PackagedApp {
        name,
        aumid: app_id,
    }))
}

fn json_string(field: &str, value: JsonValue) -> Result<String, String> {
    match value {
        JsonValue::String(value) => Ok(value),
        _ => Err(format!("Get-StartApps field {field} is not a string")),
    }
}

struct CommandOutput {
    success: bool,
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Resolves Windows PowerShell by absolute path instead of a PATH/CWD lookup.
///
/// The primary location is `%SystemRoot%\System32\WindowsPowerShell\v1.0\`.
/// A 32-bit process or unusual image sees the same files through `SysWOW64`, so
/// that is the documented fallback. If neither exists the capability is
/// reported degraded rather than searching PATH, where a planted executable
/// could be picked up.
fn powershell_executable() -> Result<PathBuf, String> {
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|root| root.is_absolute())
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));

    let candidates = [
        system_root
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe"),
        system_root
            .join("SysWOW64")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe"),
    ];

    for candidate in &candidates {
        if is_file(candidate) {
            return Ok(candidate.clone());
        }
    }

    Err(format!(
        "Windows PowerShell was not found at {} or {}",
        candidates[0].display(),
        candidates[1].display()
    ))
}

fn is_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

fn run_powershell(command: &str, timeout: Duration) -> Result<CommandOutput, String> {
    let executable = powershell_executable()?;
    let mut child = Command::new(&executable)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            command,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|error| match error.kind() {
            ErrorKind::NotFound => {
                "the resolved Windows PowerShell executable was not found".to_owned()
            }
            ErrorKind::PermissionDenied => {
                "permission denied while starting powershell.exe".to_owned()
            }
            _ => format!("cannot start powershell.exe: {error}"),
        })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "cannot capture powershell.exe stdout".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "cannot capture powershell.exe stderr".to_owned())?;

    let stdout_reader = std::thread::spawn(move || read_bounded(stdout, MAX_OUTPUT_BYTES));
    let stderr_reader = std::thread::spawn(move || read_bounded(stderr, 64 * 1024));

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                timed_out = true;
                break None;
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("cannot wait for powershell.exe: {error}"));
            }
        }
    };

    let mut reaped = !timed_out;
    if timed_out {
        let _ = child.kill();
        let kill_deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    reaped = true;
                    break;
                }
                Ok(None) if Instant::now() < kill_deadline => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Ok(None) | Err(_) => break,
            }
        }
    }

    if !reaped {
        // Do not block forever waiting for an un-killable child or its pipe
        // readers. The reader threads end when the process finally exits.
        return Err(format!(
            "Get-StartApps did not finish within {} ms and could not be terminated",
            timeout.as_millis()
        ));
    }

    let stdout = stdout_reader
        .join()
        .map_err(|_| "powershell.exe stdout reader panicked".to_owned())??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "powershell.exe stderr reader panicked".to_owned())??;

    if timed_out {
        return Err(format!(
            "Get-StartApps did not finish within {} ms and was terminated",
            timeout.as_millis()
        ));
    }

    let status = status.ok_or_else(|| "powershell.exe exited without a status".to_owned())?;
    Ok(CommandOutput {
        success: status.success(),
        code: status.code(),
        stdout,
        stderr,
    })
}

fn read_bounded(mut reader: impl Read, limit: usize) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                if output.len() + read <= limit {
                    output.extend_from_slice(&buffer[..read]);
                } else if output.len() < limit {
                    let remaining = limit - output.len();
                    output.extend_from_slice(&buffer[..remaining]);
                }
                // Keep draining after the cap so the child cannot block on a
                // full pipe; the excess is discarded.
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(format!("cannot read powershell.exe output: {error}")),
        }
    }
    Ok(output)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum JsonValue {
    Null,
    Bool,
    Number,
    String(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            bytes: input.as_bytes(),
            position: 0,
        }
    }

    fn parse(&mut self) -> Result<JsonValue, String> {
        let value = self.parse_value(0)?;
        self.skip_whitespace();
        if self.position != self.bytes.len() {
            return Err(format!(
                "unexpected trailing data at byte {} of Get-StartApps JSON",
                self.position
            ));
        }
        Ok(value)
    }

    fn parse_value(&mut self, depth: usize) -> Result<JsonValue, String> {
        if depth > MAX_JSON_DEPTH {
            return Err("Get-StartApps JSON nesting is too deep".to_owned());
        }
        self.skip_whitespace();
        match self.peek() {
            Some(b'"') => Ok(JsonValue::String(self.parse_string()?)),
            Some(b'{') => self.parse_object(depth),
            Some(b'[') => self.parse_array(depth),
            Some(b't') => {
                self.expect_literal("true")?;
                Ok(JsonValue::Bool)
            }
            Some(b'f') => {
                self.expect_literal("false")?;
                Ok(JsonValue::Bool)
            }
            Some(b'n') => {
                self.expect_literal("null")?;
                Ok(JsonValue::Null)
            }
            Some(b'-' | b'0'..=b'9') => {
                self.parse_number()?;
                Ok(JsonValue::Number)
            }
            Some(other) => Err(format!(
                "unexpected byte {other:#04x} at byte {} of Get-StartApps JSON",
                self.position
            )),
            None => Err("unexpected end of Get-StartApps JSON".to_owned()),
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<JsonValue, String> {
        self.expect(b'{')?;
        self.skip_whitespace();
        let mut fields = Vec::new();
        if self.peek() == Some(b'}') {
            self.position += 1;
            return Ok(JsonValue::Object(fields));
        }

        loop {
            self.skip_whitespace();
            let key = self.parse_string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            let value = self.parse_value(depth + 1)?;
            fields.push((key, value));
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.position += 1;
                }
                Some(b'}') => {
                    self.position += 1;
                    return Ok(JsonValue::Object(fields));
                }
                Some(other) => {
                    return Err(format!(
                        "unexpected byte {other:#04x} in Get-StartApps JSON object"
                    ))
                }
                None => return Err("unterminated Get-StartApps JSON object".to_owned()),
            }
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<JsonValue, String> {
        self.expect(b'[')?;
        self.skip_whitespace();
        let mut values = Vec::new();
        if self.peek() == Some(b']') {
            self.position += 1;
            return Ok(JsonValue::Array(values));
        }

        loop {
            values.push(self.parse_value(depth + 1)?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.position += 1;
                }
                Some(b']') => {
                    self.position += 1;
                    return Ok(JsonValue::Array(values));
                }
                Some(other) => {
                    return Err(format!(
                        "unexpected byte {other:#04x} in Get-StartApps JSON array"
                    ))
                }
                None => return Err("unterminated Get-StartApps JSON array".to_owned()),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut output = String::new();
        loop {
            let byte = self
                .next()
                .ok_or_else(|| "unterminated string in Get-StartApps JSON".to_owned())?;
            match byte {
                b'"' => return Ok(output),
                b'\\' => {
                    let escape = self
                        .next()
                        .ok_or_else(|| "truncated escape in Get-StartApps JSON".to_owned())?;
                    match escape {
                        b'"' => output.push('"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{8}'),
                        b'f' => output.push('\u{c}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => output.push(self.parse_unicode_escape()?),
                        other => {
                            return Err(format!(
                                "invalid escape \\{} in Get-StartApps JSON",
                                char::from(other)
                            ))
                        }
                    }
                }
                0x00..=0x1F => {
                    return Err("raw control character in Get-StartApps JSON string".to_owned())
                }
                other => {
                    // The input is already valid UTF-8, so copy one complete
                    // character and advance past its remaining bytes.
                    let start = self.position - 1;
                    let width = utf8_width(other);
                    let end = start + width;
                    if end > self.bytes.len() {
                        return Err("truncated UTF-8 in Get-StartApps JSON".to_owned());
                    }
                    let text = std::str::from_utf8(&self.bytes[start..end])
                        .map_err(|_| "invalid UTF-8 in Get-StartApps JSON".to_owned())?;
                    output.push_str(text);
                    self.position = end;
                }
            }
        }
    }

    fn parse_unicode_escape(&mut self) -> Result<char, String> {
        let first = self.parse_hex_quad()?;
        if (0xD800..=0xDBFF).contains(&first) {
            if self.next() != Some(b'\\') || self.next() != Some(b'u') {
                return Err("lone high surrogate in Get-StartApps JSON".to_owned());
            }
            let second = self.parse_hex_quad()?;
            if !(0xDC00..=0xDFFF).contains(&second) {
                return Err("invalid low surrogate in Get-StartApps JSON".to_owned());
            }
            let code =
                0x1_0000 + ((u32::from(first) - 0xD800) << 10) + (u32::from(second) - 0xDC00);
            char::from_u32(code).ok_or_else(|| "invalid surrogate pair".to_owned())
        } else if (0xDC00..=0xDFFF).contains(&first) {
            Err("lone low surrogate in Get-StartApps JSON".to_owned())
        } else {
            char::from_u32(u32::from(first))
                .ok_or_else(|| "invalid Unicode escape in Get-StartApps JSON".to_owned())
        }
    }

    fn parse_hex_quad(&mut self) -> Result<u16, String> {
        let mut value = 0_u16;
        for _ in 0..4 {
            let byte = self
                .next()
                .ok_or_else(|| "truncated Unicode escape in Get-StartApps JSON".to_owned())?;
            let digit = match byte {
                b'0'..=b'9' => u16::from(byte - b'0'),
                b'a'..=b'f' => u16::from(byte - b'a') + 10,
                b'A'..=b'F' => u16::from(byte - b'A') + 10,
                _ => return Err("invalid Unicode escape in Get-StartApps JSON".to_owned()),
            };
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn parse_number(&mut self) -> Result<(), String> {
        let start = self.position;
        if self.peek() == Some(b'-') {
            self.position += 1;
        }
        let digits = if self.peek() == Some(b'0') {
            self.position += 1;
            1
        } else {
            let mut digits = 0;
            while matches!(self.peek(), Some(b'1'..=b'9')) {
                self.position += 1;
                digits += 1;
            }
            digits
        };
        if digits == 0 || matches!(self.peek(), Some(b'0'..=b'9')) {
            return Err("invalid number in Get-StartApps JSON".to_owned());
        }
        if self.peek() == Some(b'.') {
            self.position += 1;
            let mut fraction = 0;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.position += 1;
                fraction += 1;
            }
            if fraction == 0 {
                return Err("invalid number in Get-StartApps JSON".to_owned());
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.position += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.position += 1;
            }
            let mut exponent = 0;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.position += 1;
                exponent += 1;
            }
            if exponent == 0 {
                return Err("invalid number in Get-StartApps JSON".to_owned());
            }
        }
        if self.position == start {
            return Err("invalid number in Get-StartApps JSON".to_owned());
        }
        Ok(())
    }

    fn expect_literal(&mut self, literal: &str) -> Result<(), String> {
        let end = self.position + literal.len();
        if self.bytes.get(self.position..end) != Some(literal.as_bytes()) {
            return Err(format!(
                "invalid literal in Get-StartApps JSON at byte {}",
                self.position
            ));
        }
        self.position = end;
        Ok(())
    }

    fn expect(&mut self, expected: u8) -> Result<(), String> {
        match self.next() {
            Some(actual) if actual == expected => Ok(()),
            Some(actual) => Err(format!(
                "expected '{}' but found {actual:#04x} in Get-StartApps JSON",
                char::from(expected)
            )),
            None => Err("unexpected end of Get-StartApps JSON".to_owned()),
        }
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.position += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.position += 1;
        Some(byte)
    }
}

fn utf8_width(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_json_array_and_ignores_desktop_app_ids() {
        let input = r#"[
            {"Name":"Calculator","AppID":"Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"},
            {"Name":"Android Studio","AppID":"{6D809377-6AF0-444B-8957-A3773F02200E}\\Android\\studio64.exe"},
            {"Name":"Terminal","AppID":"Microsoft.WindowsTerminal_8wekyb3d8bbwe!App"}
        ]"#;

        let apps = parse_packaged_json(input).expect("valid JSON");
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].name, "Calculator");
        assert_eq!(
            apps[0].aumid,
            "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"
        );
        assert_eq!(apps[1].name, "Terminal");
    }

    #[test]
    fn accepts_a_single_json_object() {
        let input =
            r#"{"Name":"Calculator","AppID":"Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"}"#;
        let apps = parse_packaged_json(input).expect("valid JSON");
        assert_eq!(apps.len(), 1);
    }

    #[test]
    fn accepts_empty_output() {
        assert_eq!(parse_packaged_json("").unwrap(), Vec::new());
        assert_eq!(parse_packaged_json("[]").unwrap(), Vec::new());
        assert_eq!(parse_packaged_json("null").unwrap(), Vec::new());
    }

    #[test]
    fn deduplicates_aumids_case_insensitively() {
        let input = r#"[
            {"Name":"One","AppID":"Vendor.App_pkg!App"},
            {"Name":"Two","AppID":"vendor.app_pkg!app"}
        ]"#;
        let apps = parse_packaged_json(input).expect("valid JSON");
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "One");
    }

    #[test]
    fn rejects_malformed_json() {
        for input in [
            r#"{"Name":"A","AppID":"X!Y""#,
            r#"{"Name":"A","AppID":"X!Y"} trailing"#,
            r#"[{"Name":"A","AppID":"X!Y"},]"#,
            r#"{"Name":"A" "AppID":"X!Y"}"#,
            r#"{"Name":"A","AppID":42}"#,
            r#"{"Name":"A"}"#,
            r#"{"Name":"A","AppID":"X!Y\q"}"#,
            r#"{"Name":"A","AppID":"X!Y"} {"second":true}"#,
        ] {
            assert!(
                parse_packaged_json(input).is_err(),
                "input should be rejected: {input}"
            );
        }
    }

    #[test]
    fn rejects_non_object_rows() {
        assert!(parse_packaged_json(r#"[1,2,3]"#).is_err());
        assert!(parse_packaged_json(r#""just a string""#).is_err());
    }

    #[test]
    fn rejects_excessive_nesting() {
        let mut input = String::new();
        for _ in 0..MAX_JSON_DEPTH + 2 {
            input.push('[');
        }
        for _ in 0..MAX_JSON_DEPTH + 2 {
            input.push(']');
        }
        assert!(parse_packaged_json(&input).is_err());
    }

    #[test]
    fn decodes_escaped_names() {
        let input = r#"{"Name":"Line\n\"Break\u0041","AppID":"Vendor.App_pkg!App"}"#;
        let apps = parse_packaged_json(input).expect("valid JSON");
        assert_eq!(apps[0].name, "Line\n\"BreakA");
    }

    #[test]
    fn powershell_path_is_absolute_and_never_resolved_through_path() {
        match powershell_executable() {
            Ok(path) => {
                assert!(path.is_absolute(), "{}", path.display());
                assert!(path.is_file(), "{}", path.display());
                assert!(
                    path.to_string_lossy()
                        .to_ascii_lowercase()
                        .contains("windowspowershell"),
                    "{}",
                    path.display()
                );
            }
            Err(reason) => {
                assert!(
                    reason.contains("Windows PowerShell was not found"),
                    "{reason}"
                );
            }
        }
    }

    #[test]
    fn decodes_utf8_and_utf16_powershell_output() {
        let text = r#"[{"Name":"Calculator","AppID":"Vendor.App_pkg!App"}]"#;
        assert_eq!(decode_powershell_output(text.as_bytes()).unwrap(), text);

        let mut with_bom = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            with_bom.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode_powershell_output(&with_bom).unwrap(), text);

        let mut without_bom = Vec::new();
        for unit in text.encode_utf16() {
            without_bom.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode_powershell_output(&without_bom).unwrap(), text);

        assert!(decode_powershell_output(&[0xFF, 0xFE, 0x00]).is_err());
    }

    #[test]
    fn aumid_validation_uses_a_strict_allowlist() {
        for valid in [
            "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
            "Canva.Affinity_8a0j1tnjnt4a4!Canva.Affinity",
            "Vendor-App_1.2~beta!Main-App",
        ] {
            assert!(is_valid_aumid(valid), "{valid} should be valid");
        }

        for invalid in [
            "",
            "NoSeparator",
            "!App",
            "Package!",
            "Package!App!Extra",
            r"{6D809377-6AF0-444B-8957-A3773F02200E}\Android\studio64.exe",
            r"C:\Windows\notepad.exe",
            "Package!App WithSpace",
            "Package!App\"Quote",
            "Package!App;Calc",
            "Package!App%PATH%",
            "Package!App/Child",
        ] {
            assert!(!is_valid_aumid(invalid), "{invalid} should be invalid");
        }
    }

    #[test]
    fn encoded_launch_path_is_the_validated_aumid_only() {
        let aumid = "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App";
        assert_eq!(
            packaged_launch_path(aumid),
            r"shell:AppsFolder\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"
        );
    }
}
