//! Bounded, checked icon extraction into the ICO cache format.
//!
//! The adapter extracts one icon image at a time with documented Win32 APIs:
//!
//! 1. `ExtractIconExW` loads the large icon handle for the source path/index.
//! 2. `GetIconInfo` exposes the color and mask bitmaps.
//! 3. `GetDIBits` copies the pixels into bounded, caller-owned buffers.
//! 4. The adapter writes a standard single-image ICO file (BITMAPINFOHEADER +
//!    XOR pixels + AND mask) into a deterministic cache file name.
//!
//! Safety properties:
//! * the source must be an absolute, existing regular file and must not be a
//!   reparse point (symlink/junction), so extraction cannot follow a link;
//! * the icon index is a small bounded integer parsed from the metadata
//!   string; no metadata is interpolated into a shell command;
//! * icon dimensions are capped at [`MAX_ICON_DIMENSION`], the in-memory pixel
//!   buffers are sized from validated `GetObjectW` dimensions, and the output
//!   is capped at [`MAX_ICO_BYTES`];
//! * a cache write goes to a unique temporary file in the cache directory and
//!   is renamed into place only after the ICO bytes validate; every failure
//!   path removes the temporary file and destroys every loaded icon, bitmap and
//!   device context exactly once.
//!
//! Icon extraction is best-effort by nature: an executable can have no icon,
//! can carry a malformed icon resource, or can be inaccessible. Those cases
//! return an explicit error; callers should fall back to a generic icon and
//! must not treat extraction as guaranteed.

use crate::start_menu::{normalize_path_key, stable_id_for_key};
use crate::types::IconCacheEntry;
use std::ffi::c_void;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Largest edge accepted for a single extracted icon image.
pub const MAX_ICON_DIMENSION: u32 = 256;
/// Hard cap for the written ICO file.
pub const MAX_ICO_BYTES: usize = 512 * 1024;
/// Largest icon index accepted from icon-location metadata.
pub const MAX_ICON_INDEX: u32 = 999;

const ICO_HEADER_BYTES: usize = 6;
const ICO_ENTRY_BYTES: usize = 16;
const ICO_IMAGE_OFFSET: u32 = (ICO_HEADER_BYTES + ICO_ENTRY_BYTES) as u32;
const BITMAPINFOHEADER_BYTES: usize = 40;
const BI_RGB: u32 = 0;
const DIB_RGB_COLORS: u32 = 0;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Extracts one icon into a deterministic cache file.
///
/// `icon_source` is the metadata string produced by discovery, such as
/// `C:\App\app.exe,0`, or an absolute executable path. `cache_dir` is created if
/// it does not exist. The returned file name is derived only from a normalized
/// source key, so the same source always maps to the same file.
pub fn extract_icon_to_ico(icon_source: &str, cache_dir: &Path) -> Result<IconCacheEntry, String> {
    let (path, index) = parse_icon_source(icon_source)?;
    validate_icon_source(&path)?;
    let canonical_source = format!("{},{}", path.display(), index);

    // Extract and validate the pixels before touching the cache directory, so
    // a failed extraction cannot leave a partially created cache tree.
    let (width, height, ico) = build_ico(&path, index)?;
    if ico.len() > MAX_ICO_BYTES {
        return Err(format!(
            "extracted icon is {} bytes, above the {MAX_ICO_BYTES}-byte cache limit",
            ico.len()
        ));
    }
    validate_ico(&ico)?;

    fs::create_dir_all(cache_dir).map_err(|error| {
        format!(
            "cannot create the icon cache directory '{}': {error}",
            cache_dir.display()
        )
    })?;
    let metadata = fs::metadata(cache_dir).map_err(|error| {
        format!(
            "cannot inspect the icon cache directory '{}': {error}",
            cache_dir.display()
        )
    })?;
    if !metadata.is_dir() {
        return Err(format!(
            "icon cache path '{}' is not a directory",
            cache_dir.display()
        ));
    }

    let key = format!("icon:{}#{}", normalize_path_key(&path), index);
    let file_name = format!("{}.ico", stable_id_for_key(&key).replace(':', "-"));
    let cache_path = cache_dir.join(file_name);

    write_cache_file(cache_dir, &cache_path, &ico)?;

    let bytes = fs::metadata(&cache_path)
        .map_err(|error| format!("cannot read back the icon cache file: {error}"))?
        .len();

    Ok(IconCacheEntry {
        cache_path,
        source: canonical_source,
        width,
        height,
        bytes,
    })
}

/// Validates the structural invariants of an ICO file.
///
/// This is a bounded parser, not a full renderer. It rejects truncation,
/// impossible counts and offsets/sizes that escape the buffer.
pub fn validate_ico(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < ICO_HEADER_BYTES + ICO_ENTRY_BYTES {
        return Err("ICO file is too short for a header and one directory entry".to_owned());
    }
    let reserved = u16::from_le_bytes([bytes[0], bytes[1]]);
    let kind = u16::from_le_bytes([bytes[2], bytes[3]]);
    let count = u16::from_le_bytes([bytes[4], bytes[5]]);
    if reserved != 0 || kind != 1 {
        return Err("ICO header is not a valid icon directory".to_owned());
    }
    if count == 0 {
        return Err("ICO file contains no images".to_owned());
    }
    if count != 1 {
        return Err(format!(
            "ICO file contains {count} images; only one is supported"
        ));
    }

    let entry = &bytes[ICO_HEADER_BYTES..ICO_HEADER_BYTES + ICO_ENTRY_BYTES];
    let width = u32::from(entry[0]);
    let height = u32::from(entry[1]);
    let width = if width == 0 { 256 } else { width };
    let height = if height == 0 { 256 } else { height };
    if width > MAX_ICON_DIMENSION || height > MAX_ICON_DIMENSION {
        return Err(format!(
            "ICO image {width}x{height} exceeds the {MAX_ICON_DIMENSION}-pixel cap"
        ));
    }

    let planes = u16::from_le_bytes([entry[4], entry[5]]);
    let bit_count = u16::from_le_bytes([entry[6], entry[7]]);
    let image_bytes = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]);
    let offset = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]);
    if planes != 1 || bit_count == 0 {
        return Err("ICO directory entry has invalid planes or bit depth".to_owned());
    }

    let offset = offset as usize;
    let image_bytes = image_bytes as usize;
    let end = offset
        .checked_add(image_bytes)
        .ok_or_else(|| "ICO image offset overflows".to_owned())?;
    if offset < ICO_HEADER_BYTES + ICO_ENTRY_BYTES || end > bytes.len() {
        return Err("ICO image data escapes the file buffer".to_owned());
    }
    if image_bytes < BITMAPINFOHEADER_BYTES {
        return Err("ICO image is shorter than a BITMAPINFOHEADER".to_owned());
    }

    let image = &bytes[offset..end];
    let header_bytes = u32::from_le_bytes([image[0], image[1], image[2], image[3]]) as usize;
    if header_bytes != BITMAPINFOHEADER_BYTES {
        return Err("ICO image does not use a BITMAPINFOHEADER".to_owned());
    }
    let dib_width = i32::from_le_bytes([image[4], image[5], image[6], image[7]]);
    let dib_height = i32::from_le_bytes([image[8], image[9], image[10], image[11]]);
    if dib_width <= 0 || dib_height != (height as i32) * 2 {
        return Err("ICO DIB dimensions are inconsistent with the directory entry".to_owned());
    }
    if dib_width as u32 != width {
        return Err("ICO DIB width is inconsistent with the directory entry".to_owned());
    }
    Ok(())
}

fn parse_icon_source(icon_source: &str) -> Result<(PathBuf, u32), String> {
    let trimmed = icon_source.trim();
    if trimmed.is_empty() {
        return Err("icon source must not be empty".to_owned());
    }
    if trimmed.len() > 32_768 {
        return Err("icon source is longer than the 32768-byte limit".to_owned());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("icon source must not contain control characters".to_owned());
    }

    let (path_text, index) = match trimmed.rsplit_once(',') {
        Some((path, suffix)) => {
            let suffix = suffix.trim();
            if suffix.starts_with('-') {
                return Err(
                    "negative icon resource identifiers are not supported; use a non-negative index"
                        .to_owned(),
                );
            }
            if !suffix.is_empty() && suffix.chars().all(|character| character.is_ascii_digit()) {
                let index = suffix
                    .parse::<u32>()
                    .map_err(|_| "icon index is not a valid non-negative integer".to_owned())?;
                if index > MAX_ICON_INDEX {
                    return Err(format!(
                        "icon index {index} is above the supported maximum {MAX_ICON_INDEX}"
                    ));
                }
                (path, index)
            } else {
                (trimmed, 0)
            }
        }
        None => (trimmed, 0),
    };

    let path = PathBuf::from(path_text);
    if !path.is_absolute() {
        return Err(format!(
            "icon source '{}' is not an absolute path; metadata is never resolved through the shell",
            path.display()
        ));
    }
    Ok((path, index))
}

fn validate_icon_source(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "icon source '{}' is not accessible: {error}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err(format!(
            "icon source '{}' is a reparse point; refusing to follow it",
            path.display()
        ));
    }
    if !metadata.is_file() {
        return Err(format!(
            "icon source '{}' is not a regular file",
            path.display()
        ));
    }
    Ok(())
}

fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    (metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
}

fn build_ico(path: &Path, index: u32) -> Result<(u32, u32, Vec<u8>), String> {
    let icon = load_icon(path, index)?;
    let mut info = IconInfo {
        f_icon: 0,
        x_hotspot: 0,
        y_hotspot: 0,
        hbm_mask: std::ptr::null_mut(),
        hbm_color: std::ptr::null_mut(),
    };
    // SAFETY: `icon.0` is a live HICON and `info` is a valid output buffer.
    if unsafe { GetIconInfo(icon.0, &mut info) } == 0 {
        return Err(last_error_message("GetIconInfo"));
    }
    let mask = OwnedBitmap(info.hbm_mask);
    let color = OwnedBitmap(info.hbm_color);

    let bitmap =
        read_bitmap(color.0).ok_or_else(|| "the icon has no usable color bitmap".to_owned())?;
    let (width, height) = (bitmap.width, bitmap.height);

    let mut device_context = OwnedDeviceContext::new()
        .ok_or_else(|| "cannot acquire a device context for icon pixel extraction".to_owned())?;

    let xor = color.xor_pixels(device_context.0, width, height)?;
    let mask_bits = if mask.0.is_null() {
        synthesize_mask(&xor, width, height)
    } else {
        match mask.and_mask(device_context.0, width, height) {
            Ok(bits) => bits,
            Err(_) => synthesize_mask(&xor, width, height),
        }
    };
    // Keep the device context alive until both GetDIBits calls complete.
    device_context.release_now();

    let xor_bytes = xor.len();
    let mask_stride = mask_stride(width);
    let mask_bytes = mask_stride
        .checked_mul(height as usize)
        .ok_or_else(|| "icon mask size overflows".to_owned())?;
    if mask_bits.len() != mask_bytes {
        return Err("icon mask buffer has an unexpected size".to_owned());
    }

    let image_bytes = BITMAPINFOHEADER_BYTES
        .checked_add(xor_bytes)
        .and_then(|value| value.checked_add(mask_bytes))
        .ok_or_else(|| "icon image size overflows".to_owned())?;
    let total_bytes = ICO_HEADER_BYTES
        .checked_add(ICO_ENTRY_BYTES)
        .and_then(|value| value.checked_add(image_bytes))
        .ok_or_else(|| "icon file size overflows".to_owned())?;
    if total_bytes > MAX_ICO_BYTES {
        return Err(format!(
            "extracted icon would be {total_bytes} bytes, above the {MAX_ICO_BYTES}-byte cache limit"
        ));
    }

    let mut output = Vec::with_capacity(total_bytes);
    output.extend_from_slice(&0_u16.to_le_bytes()); // reserved
    output.extend_from_slice(&1_u16.to_le_bytes()); // type: icon
    output.extend_from_slice(&1_u16.to_le_bytes()); // image count
    output.push(encoded_dimension(width));
    output.push(encoded_dimension(height));
    output.push(0); // palette entry count
    output.push(0); // reserved
    output.extend_from_slice(&1_u16.to_le_bytes()); // planes
    output.extend_from_slice(&32_u16.to_le_bytes()); // 32-bit BGRA
    output.extend_from_slice(&(image_bytes as u32).to_le_bytes());
    output.extend_from_slice(&ICO_IMAGE_OFFSET.to_le_bytes());

    output.extend_from_slice(&(BITMAPINFOHEADER_BYTES as u32).to_le_bytes());
    output.extend_from_slice(&(width as i32).to_le_bytes());
    output.extend_from_slice(&((height as i32) * 2).to_le_bytes());
    output.extend_from_slice(&1_u16.to_le_bytes());
    output.extend_from_slice(&32_u16.to_le_bytes());
    output.extend_from_slice(&BI_RGB.to_le_bytes());
    output.extend_from_slice(&((xor_bytes + mask_bytes) as u32).to_le_bytes());
    output.extend_from_slice(&0_i32.to_le_bytes());
    output.extend_from_slice(&0_i32.to_le_bytes());
    output.extend_from_slice(&0_u32.to_le_bytes());
    output.extend_from_slice(&0_u32.to_le_bytes());

    output.extend_from_slice(&xor);
    output.extend_from_slice(&mask_bits);
    Ok((width, height, output))
}

#[derive(Clone, Copy, Debug)]
struct BitmapShape {
    width: u32,
    height: u32,
}

fn read_bitmap(handle: *mut c_void) -> Option<BitmapShape> {
    if handle.is_null() {
        return None;
    }
    let mut bitmap = Bitmap {
        bm_type: 0,
        bm_width: 0,
        bm_height: 0,
        bm_width_bytes: 0,
        bm_planes: 0,
        bm_bits_pixel: 0,
        bm_bits: std::ptr::null_mut(),
    };
    // SAFETY: `handle` is a live bitmap and `bitmap` is a sized output buffer.
    let copied = unsafe {
        GetObjectW(
            handle,
            std::mem::size_of::<Bitmap>() as i32,
            &mut bitmap as *mut Bitmap as *mut c_void,
        )
    };
    if copied == 0 {
        return None;
    }
    let width = bitmap.bm_width.unsigned_abs();
    let height = bitmap.bm_height.unsigned_abs();
    if width == 0 || height == 0 || width > MAX_ICON_DIMENSION || height > MAX_ICON_DIMENSION {
        return None;
    }
    Some(BitmapShape { width, height })
}

impl OwnedBitmap {
    fn xor_pixels(
        &self,
        device_context: *mut c_void,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, String> {
        let length = (width as usize)
            .checked_mul(height as usize)
            .and_then(|value| value.checked_mul(4))
            .ok_or_else(|| "icon pixel buffer size overflows".to_owned())?;
        let mut pixels = vec![0_u8; length];
        let mut info = BitmapInfo {
            header: BitmapInfoHeader {
                bi_size: BITMAPINFOHEADER_BYTES as u32,
                bi_width: width as i32,
                bi_height: height as i32,
                bi_planes: 1,
                bi_bit_count: 32,
                bi_compression: BI_RGB,
                bi_size_image: 0,
                bi_x_pels_per_meter: 0,
                bi_y_pels_per_meter: 0,
                bi_clr_used: 0,
                bi_clr_important: 0,
            },
            colors: [0_u32; 256],
        };
        // SAFETY: the bitmap handle is live, `pixels` is exactly width*height*4
        // bytes and `info` is a valid BITMAPINFO with a color table. GetDIBits
        // writes at most the number of requested scan lines.
        let copied = unsafe {
            GetDIBits(
                device_context,
                self.0,
                0,
                height,
                pixels.as_mut_ptr() as *mut c_void,
                &mut info,
                DIB_RGB_COLORS,
            )
        };
        if copied == 0 {
            return Err(last_error_message("GetDIBits (color)"));
        }
        Ok(pixels)
    }

    fn and_mask(
        &self,
        device_context: *mut c_void,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, String> {
        let stride = mask_stride(width);
        let length = stride
            .checked_mul(height as usize)
            .ok_or_else(|| "icon mask buffer size overflows".to_owned())?;
        let mut mask = vec![0_u8; length];
        let mut info = BitmapInfo {
            header: BitmapInfoHeader {
                bi_size: BITMAPINFOHEADER_BYTES as u32,
                bi_width: width as i32,
                bi_height: height as i32,
                bi_planes: 1,
                bi_bit_count: 1,
                bi_compression: BI_RGB,
                bi_size_image: 0,
                bi_x_pels_per_meter: 0,
                bi_y_pels_per_meter: 0,
                bi_clr_used: 0,
                bi_clr_important: 0,
            },
            colors: [0_u32; 256],
        };
        // SAFETY: the mask bitmap handle is live, `mask` is sized for
        // `stride*height` bytes and `info` is a valid 1-bpp BITMAPINFO.
        let copied = unsafe {
            GetDIBits(
                device_context,
                self.0,
                0,
                height,
                mask.as_mut_ptr() as *mut c_void,
                &mut info,
                DIB_RGB_COLORS,
            )
        };
        if copied == 0 {
            return Err(last_error_message("GetDIBits (mask)"));
        }
        Ok(mask)
    }
}

fn mask_stride(width: u32) -> usize {
    (width as usize).div_ceil(32) * 4
}

/// Builds an AND mask from the alpha channel when the icon has no mask
/// bitmap or the mask read fails. Alpha 0 becomes transparent (bit 1).
fn synthesize_mask(xor: &[u8], width: u32, height: u32) -> Vec<u8> {
    let stride = mask_stride(width);
    let mut mask = vec![0_u8; stride * height as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let alpha = xor[(y * width as usize + x) * 4 + 3];
            if alpha == 0 {
                mask[y * stride + x / 8] |= 1 << (7 - (x % 8));
            }
        }
    }
    mask
}

fn encoded_dimension(value: u32) -> u8 {
    if value >= 256 {
        0
    } else {
        value as u8
    }
}

fn write_cache_file(cache_dir: &Path, cache_path: &Path, bytes: &[u8]) -> Result<(), String> {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp_name = format!(".ansa-icon-{}-{sequence}.tmp", std::process::id());
    let temp_path = cache_dir.join(temp_name);

    let result = (|| -> Result<(), String> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|error| format!("cannot create the temporary icon cache file: {error}"))?;
        file.write_all(bytes)
            .map_err(|error| format!("cannot write the icon cache file: {error}"))?;
        file.flush()
            .map_err(|error| format!("cannot flush the icon cache file: {error}"))?;
        drop(file);

        if cache_path.exists() {
            fs::remove_file(cache_path)
                .map_err(|error| format!("cannot replace the existing icon cache file: {error}"))?;
        }
        fs::rename(&temp_path, cache_path)
            .map_err(|error| format!("cannot publish the icon cache file: {error}"))?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn load_icon(path: &Path, index: u32) -> Result<OwnedIcon, String> {
    use std::os::windows::ffi::OsStrExt;

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err("icon source path must not contain NUL".to_owned());
    }
    wide.push(0);

    let mut large = std::ptr::null_mut();
    let mut small = std::ptr::null_mut();
    // SAFETY: `wide` is NUL-terminated and alive for the call; the two output
    // pointers reference live locals. ExtractIconExW returns owned HICON
    // handles that are wrapped and destroyed exactly once.
    let count = unsafe { ExtractIconExW(wide.as_ptr(), index as i32, &mut large, &mut small, 1) };
    if count == 0 {
        return Err(format!(
            "no icon at index {index} could be extracted from '{}'",
            path.display()
        ));
    }
    if count == u32::MAX {
        return Err(format!("'{}' is not a valid icon source", path.display()));
    }

    let large = OwnedIcon(large);
    let small = OwnedIcon(small);
    if !large.0.is_null() {
        Ok(large)
    } else if !small.0.is_null() {
        Ok(small)
    } else {
        Err("Windows returned no icon handle".to_owned())
    }
}

fn last_error_message(operation: &str) -> String {
    // SAFETY: GetLastError has no preconditions and is read immediately.
    let code = unsafe { GetLastError() };
    format!("{operation} failed with Win32 error {code}")
}

#[derive(Debug)]
struct OwnedIcon(*mut c_void);

impl Drop for OwnedIcon {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the handle came from ExtractIconExW and is destroyed once.
            unsafe {
                DestroyIcon(self.0);
            }
        }
    }
}

#[derive(Debug)]
struct OwnedBitmap(*mut c_void);

impl Drop for OwnedBitmap {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the handle came from GetIconInfo and is deleted once.
            unsafe {
                DeleteObject(self.0);
            }
        }
    }
}

#[derive(Debug)]
struct OwnedDeviceContext(*mut c_void);

impl OwnedDeviceContext {
    fn new() -> Option<Self> {
        // SAFETY: GetDC with a null window requests the screen device context.
        let handle = unsafe { GetDC(std::ptr::null_mut()) };
        (!handle.is_null()).then_some(Self(handle))
    }

    fn release_now(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the handle came from GetDC and is released once.
            unsafe {
                ReleaseDC(std::ptr::null_mut(), self.0);
            }
            self.0 = std::ptr::null_mut();
        }
    }
}

impl Drop for OwnedDeviceContext {
    fn drop(&mut self) {
        self.release_now();
    }
}

#[repr(C)]
#[allow(dead_code)]
struct IconInfo {
    f_icon: i32,
    x_hotspot: u32,
    y_hotspot: u32,
    hbm_mask: *mut c_void,
    hbm_color: *mut c_void,
}

#[repr(C)]
#[allow(dead_code)]
struct Bitmap {
    bm_type: u32,
    bm_width: i32,
    bm_height: i32,
    bm_width_bytes: i32,
    bm_planes: u16,
    bm_bits_pixel: u16,
    bm_bits: *mut c_void,
}

#[repr(C)]
struct BitmapInfoHeader {
    bi_size: u32,
    bi_width: i32,
    bi_height: i32,
    bi_planes: u16,
    bi_bit_count: u16,
    bi_compression: u32,
    bi_size_image: u32,
    bi_x_pels_per_meter: i32,
    bi_y_pels_per_meter: i32,
    bi_clr_used: u32,
    bi_clr_important: u32,
}

#[repr(C)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [u32; 256],
}

#[link(name = "user32")]
extern "system" {
    fn ExtractIconExW(
        file: *const u16,
        index: i32,
        large_icon: *mut *mut c_void,
        small_icon: *mut *mut c_void,
        icons: u32,
    ) -> u32;
    fn GetIconInfo(icon: *mut c_void, info: *mut IconInfo) -> i32;
    fn DestroyIcon(icon: *mut c_void) -> i32;
    fn GetDC(window: *mut c_void) -> *mut c_void;
    fn ReleaseDC(window: *mut c_void, device_context: *mut c_void) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn GetObjectW(object: *mut c_void, size: i32, out: *mut c_void) -> i32;
    fn GetDIBits(
        device_context: *mut c_void,
        bitmap: *mut c_void,
        start: u32,
        lines: u32,
        bits: *mut c_void,
        info: *mut BitmapInfo,
        usage: u32,
    ) -> i32;
    fn DeleteObject(object: *mut c_void) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetLastError() -> u32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(label: &str) -> Self {
            let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "ansa-platform-icon-{}-{label}-{sequence}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn notepad() -> Option<PathBuf> {
        let root = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let path = root.join("System32").join("notepad.exe");
        path.is_file().then_some(path)
    }

    #[test]
    fn rejects_missing_relative_and_reparse_sources_before_extracting() {
        let temp = TempDir::new("invalid");
        assert!(extract_icon_to_ico("", &temp.path).is_err());
        assert!(extract_icon_to_ico("notepad.exe", &temp.path).is_err());
        assert!(
            extract_icon_to_ico(r"C:\Windows\System32\missing-icon-source.exe", &temp.path)
                .is_err()
        );
        assert!(extract_icon_to_ico(r"C:\Windows\System32\notepad.exe,-1", &temp.path).is_err());
        assert!(extract_icon_to_ico(r"C:\Windows\System32\notepad.exe,1000", &temp.path).is_err());
    }

    #[test]
    fn cache_name_is_deterministic_for_the_same_source() {
        let Some(notepad) = notepad() else {
            eprintln!("skipping icon extraction: notepad.exe is absent");
            return;
        };
        let temp = TempDir::new("deterministic");
        let source = format!("{},0", notepad.display());
        let first = extract_icon_to_ico(&source, &temp.path).expect("first extraction");
        let second = extract_icon_to_ico(&source, &temp.path).expect("second extraction");
        assert_eq!(first.cache_path, second.cache_path);
        assert_eq!(first.source, second.source);
    }

    #[test]
    fn extracts_a_real_notepad_icon_into_a_valid_non_empty_ico() {
        let Some(notepad) = notepad() else {
            eprintln!("skipping icon extraction: notepad.exe is absent");
            return;
        };
        let temp = TempDir::new("notepad");
        let source = format!("{},0", notepad.display());

        let entry = extract_icon_to_ico(&source, &temp.path).expect("notepad icon extraction");
        let bytes = fs::read(&entry.cache_path).expect("read cache file");
        validate_ico(&bytes).expect("valid ICO");
        assert!(!bytes.is_empty());
        assert_eq!(entry.bytes, bytes.len() as u64);
        assert_eq!(entry.width, entry.width.min(MAX_ICON_DIMENSION));
        assert_eq!(entry.height, entry.height.min(MAX_ICON_DIMENSION));
        assert_eq!(&bytes[..4], &[0, 0, 1, 0], "ICO signature");
        assert!(entry.bytes < MAX_ICO_BYTES as u64);
        eprintln!(
            "notepad icon: {}x{}, {} bytes at {}",
            entry.width,
            entry.height,
            entry.bytes,
            entry.cache_path.display()
        );
    }

    #[test]
    fn validate_ico_rejects_truncated_and_inconsistent_input() {
        assert!(validate_ico(&[]).is_err());
        assert!(validate_ico(&[0, 0, 1, 0, 1, 0]).is_err());

        let Some(notepad) = notepad() else {
            return;
        };
        let temp = TempDir::new("truncated");
        let source = format!("{},0", notepad.display());
        let entry = extract_icon_to_ico(&source, &temp.path).expect("notepad icon extraction");
        let bytes = fs::read(&entry.cache_path).expect("read cache file");

        for length in [0_usize, 5, 21, 22, bytes.len() - 1] {
            assert!(
                validate_ico(&bytes[..length]).is_err(),
                "length {length} must be rejected"
            );
        }
        let mut altered = bytes.clone();
        altered[2] = 2;
        assert!(validate_ico(&altered).is_err());
    }
}
