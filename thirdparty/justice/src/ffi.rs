//! The C ABI crengine links against.
//!
//! Everything textual stays in Rust: splitting, character classes, grapheme
//! counts, and the whole paragraph-wide solve. The host answers exactly one
//! question — *how wide is byte range `[start, end)` of this paragraph?* — which
//! is the only thing a cumulative advance table can answer, because the same
//! string in a regular and a bold span has two widths.

use std::cell::RefCell;
use std::ffi::CString;
use std::os::raw::{c_char, c_double, c_int, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::engine::{
    prepare_ranged, solve_with, split_ranges, Ending, Error, Measure, Options, Prepared,
};

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

fn fail(message: &str) -> c_int {
    if let Ok(text) = CString::new(message) {
        LAST_ERROR.with(|slot| *slot.borrow_mut() = Some(text));
    }
    -1
}

fn clear_error() {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = None);
}

/// The host's width oracle. `hyphen` asks for the range with a `-` drawn after
/// it, which is not part of the source and so cannot be given as a range.
pub type MeasureFn =
    Option<unsafe extern "C" fn(ctx: *mut c_void, start: u32, end: u32, hyphen: c_int) -> c_double>;

/// Fitting policy, laid out for C. Mirrors [`Options`]; the published defaults
/// are what `justice_options_default` fills in.
#[repr(C)]
pub struct JusticeOptions {
    pub stretch: c_double,
    pub shrink: c_double,
    /// 0 = balanced, 1 = strict.
    pub mode: c_int,
    /// Absolute pixels per character.
    pub tracking: c_double,
    pub compression_penalty: c_double,
    pub emergency_stretch: c_double,
    /// 0 leaves optical margins entirely to the host.
    pub hanging: c_double,
    pub opening: c_double,
    pub protrusion: c_double,
    pub adjacent_penalty: c_double,
    pub last_line: c_double,
    /// 0 = soft, 1 = fit.
    pub ending: c_int,
    pub widow_penalty: c_double,
    pub hyphen_penalty: c_double,
    pub consecutive_hyphen_penalty: c_double,
    pub final_hyphen_penalty: c_double,
    pub explicit_hyphen_penalty: c_double,
}

/// One solved line.
#[repr(C)]
pub struct JusticeLine {
    /// Word indices, `end` exclusive.
    pub word_start: u32,
    pub word_end: u32,
    /// Byte range of the line's text within the prepared paragraph.
    pub byte_start: u32,
    pub byte_end: u32,
    /// Per-gap adjustment from the measured space width.
    pub word_spacing: c_double,
    /// Per-character adjustment.
    pub tracking: c_double,
    pub last: c_int,
}

impl From<&Options> for JusticeOptions {
    fn from(o: &Options) -> Self {
        JusticeOptions {
            stretch: o.stretch,
            shrink: o.shrink,
            mode: match o.mode {
                crate::engine::Mode::Balanced => 0,
                crate::engine::Mode::Strict => 1,
            },
            tracking: o.tracking,
            compression_penalty: o.compression_penalty,
            emergency_stretch: o.emergency_stretch,
            hanging: o.hanging,
            opening: o.opening,
            protrusion: o.protrusion,
            adjacent_penalty: o.adjacent_penalty,
            last_line: o.last_line,
            ending: match o.ending {
                Ending::Soft => 0,
                Ending::Fit => 1,
            },
            widow_penalty: o.widow_penalty,
            hyphen_penalty: o.hyphen_penalty,
            consecutive_hyphen_penalty: o.consecutive_hyphen_penalty,
            final_hyphen_penalty: o.final_hyphen_penalty,
            explicit_hyphen_penalty: o.explicit_hyphen_penalty,
        }
    }
}

impl TryFrom<&JusticeOptions> for Options {
    type Error = Error;

    fn try_from(c: &JusticeOptions) -> Result<Self, Self::Error> {
        let mode = match c.mode {
            0 => crate::engine::Mode::Balanced,
            1 => crate::engine::Mode::Strict,
            _ => {
                return Err(Error::range(
                    "Policy mode must be 0 (balanced) or 1 (strict)",
                ))
            }
        };
        let ending = match c.ending {
            0 => Ending::Soft,
            1 => Ending::Fit,
            _ => return Err(Error::range("Policy ending must be 0 (soft) or 1 (fit)")),
        };
        Ok(Options {
            stretch: c.stretch,
            shrink: c.shrink,
            mode,
            tracking: c.tracking,
            compression_penalty: c.compression_penalty,
            emergency_stretch: c.emergency_stretch,
            hanging: c.hanging,
            opening: c.opening,
            protrusion: c.protrusion,
            adjacent_penalty: c.adjacent_penalty,
            last_line: c.last_line,
            ending,
            widow_penalty: c.widow_penalty,
            hyphen_penalty: c.hyphen_penalty,
            consecutive_hyphen_penalty: c.consecutive_hyphen_penalty,
            final_hyphen_penalty: c.final_hyphen_penalty,
            explicit_hyphen_penalty: c.explicit_hyphen_penalty,
        })
    }
}

/// A prepared paragraph awaiting its measure and its policy.
pub struct JusticeJob {
    ranges: Vec<(usize, usize)>,
    prepared: Prepared,
    layout: Option<crate::engine::Layout>,
}

impl JusticeJob {
    fn range_for(&self, word: usize) -> Option<(usize, usize)> {
        self.ranges.get(word).copied()
    }
}

/// Fill `out` with the published defaults.
///
/// # Safety
/// `out` must point at a writable [`JusticeOptions`].
#[no_mangle]
pub unsafe extern "C" fn justice_options_default(out: *mut JusticeOptions) {
    if out.is_null() {
        return;
    }
    unsafe { out.write(JusticeOptions::from(&Options::default())) };
}

/// A description of the most recent failure on this thread, or null.
///
/// The string stays valid until the next FFI call on the same thread.
#[no_mangle]
pub extern "C" fn justice_error() -> *const c_char {
    LAST_ERROR.with(|slot| match slot.borrow().as_ref() {
        Some(text) => text.as_ptr(),
        None => std::ptr::null(),
    })
}

/// Split `text` into words and measure each one through `measure`.
///
/// Returns null if `text` is not valid UTF-8, `measure` is missing, or the
/// host reports a width that is not finite and non-negative.
///
/// # Safety
/// `text` must hold `len` readable bytes. `measure` must be callable with a
/// `ctx` it recognises, and must not longjmp.
#[no_mangle]
pub unsafe extern "C" fn justice_prepare(
    text: *const c_char,
    len: usize,
    space: c_double,
    measure: MeasureFn,
    ctx: *mut c_void,
) -> *mut JusticeJob {
    clear_error();
    if text.is_null() || len == 0 || measure.is_none() || len > u32::MAX as usize {
        fail("justice_prepare: null text, empty text, or missing measure");
        return std::ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(text.cast::<u8>(), len) };
    let paragraph = match std::str::from_utf8(bytes) {
        Ok(paragraph) => paragraph,
        Err(_) => {
            fail("justice_prepare: text is not valid UTF-8");
            return std::ptr::null_mut();
        }
    };
    let callback = match measure {
        Some(callback) => callback,
        None => unreachable!("checked above"),
    };
    let prepared = prepare_ranged(paragraph, space, |start, end, hyphen| {
        // The ranges come from our own splitter, so they are in bounds and on
        // code point boundaries.
        unsafe { callback(ctx, start as u32, end as u32, hyphen as c_int) }
    });
    match prepared {
        Ok(prepared) => Box::into_raw(Box::new(JusticeJob {
            ranges: split_ranges(paragraph),
            prepared,
            layout: None,
        })),
        Err(error) => {
            fail(&error.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Release a job. Accepts null so callers can free unconditionally.
///
/// # Safety
/// `job` must come from [`justice_prepare`] and must not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn justice_free(job: *mut JusticeJob) {
    if !job.is_null() {
        unsafe { drop(Box::from_raw(job)) };
    }
}

/// Solve for line breaks and spacing.
///
/// `widths` holds one usable width per line, the final entry repeating — pass a
/// single width for a uniform column, or `[w - indent, w]` for a first-line
/// indent. `options` may be null for the published defaults.
///
/// Returns 0 on success, -1 on failure; see [`justice_error`].
///
/// # Safety
/// `job` must come from [`justice_prepare`]. `options` must be null or a
/// readable [`JusticeOptions`]. `widths` must hold `count` readable doubles.
#[no_mangle]
pub unsafe extern "C" fn justice_solve(
    job: *mut JusticeJob,
    widths: *const c_double,
    count: usize,
    options: *const JusticeOptions,
) -> c_int {
    clear_error();
    if job.is_null() || widths.is_null() || count == 0 {
        return fail("justice_solve: null job or widths, or no widths at all");
    }
    let width_slice = unsafe { std::slice::from_raw_parts(widths, count) };
    if width_slice.iter().any(|w| !(w.is_finite() && *w > 0.0)) {
        return fail("justice_solve: Measure must be positive");
    }
    let policy: Options = if options.is_null() {
        Options::default()
    } else {
        match Options::try_from(unsafe { &*options }) {
            Ok(policy) => policy,
            Err(error) => return fail(&error.to_string()),
        }
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let job = unsafe { &mut *job };
        let solved = solve_with(&job.prepared, Measure::from(width_slice.to_vec()), policy);
        match solved {
            Ok(layout) => {
                job.layout = Some(layout);
                0
            }
            Err(error) => fail(&error.to_string()),
        }
    }));
    match result {
        Ok(code) => code,
        Err(_) => fail("justice_solve: solver failed on this input"),
    }
}

/// Words in the prepared paragraph.
///
/// # Safety
/// `job` must come from [`justice_prepare`].
#[no_mangle]
pub unsafe extern "C" fn justice_word_count(job: *const JusticeJob) -> u32 {
    if job.is_null() {
        return 0;
    }
    unsafe { &*job }.ranges.len() as u32
}

/// Write `2 * word_count` byte offsets — start, end per word.
///
/// # Safety
/// `out` must be writable for `2 * justice_word_count(job)` entries.
#[no_mangle]
pub unsafe extern "C" fn justice_word_byte_ranges(job: *const JusticeJob, out: *mut u32) -> c_int {
    if job.is_null() || out.is_null() {
        return fail("justice_word_byte_ranges: null argument");
    }
    let job = unsafe { &*job };
    let sink = unsafe { std::slice::from_raw_parts_mut(out, job.ranges.len().saturating_mul(2)) };
    for (index, &(start, end)) in job.ranges.iter().enumerate() {
        sink[index * 2] = start as u32;
        sink[index * 2 + 1] = end as u32;
    }
    0
}

/// Lines produced by the last successful [`justice_solve`].
///
/// # Safety
/// `job` must come from [`justice_prepare`].
#[no_mangle]
pub unsafe extern "C" fn justice_line_count(job: *const JusticeJob) -> u32 {
    if job.is_null() {
        return 0;
    }
    unsafe { &*job }
        .layout
        .as_ref()
        .map(|layout| layout.lines.len() as u32)
        .unwrap_or(0)
}

/// Fill `out` with line `index`.
///
/// Returns 0 on success, -1 if the index is out of range or the line falls
/// outside the prepared text.
///
/// # Safety
/// `job` must come from [`justice_prepare`]; `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn justice_line(
    job: *const JusticeJob,
    index: u32,
    out: *mut JusticeLine,
) -> c_int {
    if job.is_null() || out.is_null() {
        return fail("justice_line: null argument");
    }
    let job = unsafe { &*job };
    let layout = match job.layout.as_ref() {
        Some(layout) => layout,
        None => return fail("justice_line: solved before justice_solve"),
    };
    let line = match layout.lines.get(index as usize) {
        Some(line) => line,
        None => return fail("justice_line: index out of range"),
    };
    let (word_start, word_end) = (line.start, line.end);
    if word_start >= word_end {
        return fail("justice_line: empty line");
    }
    let start = match job.range_for(word_start) {
        Some((start, _)) => start,
        None => return fail("justice_line: word range missing"),
    };
    let end = match job.range_for(word_end - 1) {
        Some((_, end)) => end,
        None => return fail("justice_line: word range missing"),
    };
    unsafe {
        out.write(JusticeLine {
            word_start: word_start as u32,
            word_end: word_end as u32,
            byte_start: start as u32,
            byte_end: end as u32,
            word_spacing: line.word_spacing,
            tracking: line.tracking,
            last: line.last as c_int,
        })
    };
    0
}
