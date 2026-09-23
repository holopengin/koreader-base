//! Justice's DOM-free paragraph engine, with optional measured hyphenation.

use std::collections::{HashMap, HashSet};
use std::fmt;
use unicode_segmentation::UnicodeSegmentation;

/// How the solver prices strain against the preferred spacing limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Relax word spacing beyond the preferred limits to complete a line.
    Balanced,
    /// Keep stretch and shrink limits hard.
    Strict,
}

/// How a short final line is priced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// Price a short ending by missing width alone.
    Soft,
    /// Price a short ending by its measured flexibility.
    Fit,
}

/// Fitting policy. Mirrors the reference `Options` record; `Options::default()`
/// is the published default policy.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// Relative to measured space. Stretch may relax; shrink always stays bounded.
    pub stretch: f64,
    pub shrink: f64,
    pub mode: Mode,
    /// Absolute pixels per character.
    pub tracking: f64,
    /// Cost multiplier for tightening a line rather than opening its spacing.
    pub compression_penalty: f64,
    /// Extra scoring flexibility, as a fraction of width, when ordinary fitting fails.
    pub emergency_stretch: f64,
    /// Fraction of trailing punctuation advance allowed beyond the right margin.
    pub hanging: f64,
    /// Optical allowance for a leading quote, as a fraction of its advance.
    pub opening: f64,
    /// Strength of supplied font-aware optical margins (0 disables them).
    pub protrusion: f64,
    /// Cost when neighbouring line fitness classes differ by more than one.
    pub adjacent_penalty: f64,
    /// Preferred fraction of the measure occupied by the last line.
    pub last_line: f64,
    /// Price a short ending by missing width alone, or by its measured flexibility.
    pub ending: Ending,
    pub widow_penalty: f64,
    /// Costs for discretionary, consecutive, and penultimate-line hyphens.
    pub hyphen_penalty: f64,
    pub consecutive_hyphen_penalty: f64,
    pub final_hyphen_penalty: f64,
    /// Cost of breaking after a hyphen already present in the source ("well-known").
    pub explicit_hyphen_penalty: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            stretch: 0.6,
            shrink: 0.25,
            mode: Mode::Balanced,
            tracking: 0.3,
            compression_penalty: 2.0,
            emergency_stretch: 0.15,
            hanging: 1.0,
            opening: 0.3,
            protrusion: 1.0,
            adjacent_penalty: 100.0,
            last_line: 0.33,
            ending: Ending::Fit,
            widow_penalty: 300.0,
            hyphen_penalty: 50.0,
            consecutive_hyphen_penalty: 200.0,
            final_hyphen_penalty: 200.0,
            explicit_hyphen_penalty: 20.0,
        }
    }
}

/// A single column width, or one width per line with the final entry repeating.
/// Use a vector for a first-line indent (`[w - indent, w]`) or a drop cap
/// (`[w - cap, w - cap, w - cap, w]`).
#[derive(Debug, Clone, PartialEq)]
pub struct Measure(Vec<f64>);

impl Measure {
    /// The per-line widths without validation, e.g. for independent oracles.
    pub fn into_widths(self) -> Vec<f64> {
        let Measure(widths) = self;
        widths
    }

    /// Validate and consume, returning the per-line widths.
    fn widths(self) -> std::result::Result<Vec<f64>, Error> {
        let Measure(widths) = self;
        if widths.is_empty() || widths.iter().any(|w| !(w.is_finite() && *w > 0.0)) {
            return Err(Error::Range("Measure must be positive".into()));
        }
        Ok(widths)
    }
}

impl From<f64> for Measure {
    fn from(width: f64) -> Self {
        Measure(vec![width])
    }
}

impl From<i32> for Measure {
    fn from(width: i32) -> Self {
        Measure(vec![width as f64])
    }
}

impl From<Vec<f64>> for Measure {
    fn from(widths: Vec<f64>) -> Self {
        Measure(widths)
    }
}

impl<const N: usize> From<[f64; N]> for Measure {
    fn from(widths: [f64; N]) -> Self {
        Measure(widths.to_vec())
    }
}

impl From<&[f64]> for Measure {
    fn from(widths: &[f64]) -> Self {
        Measure(widths.to_vec())
    }
}

/// The error type raised for invalid measurements, partitions, or policies.
/// Mirrors the reference implementation's `RangeError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Range(String),
}

impl Error {
    pub(crate) fn range(message: impl Into<String>) -> Self {
        Error::Range(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Range(message) => write!(f, "RangeError: {message}"),
        }
    }
}

impl std::error::Error for Error {}

/// Convenience alias matching the reference's throwing signature.
pub type Result<T> = std::result::Result<T, Error>;

/// A measured paragraph, ready to solve across widths and policy changes.
/// Copies share the underlying measurements.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub words: Vec<String>,
    /// Prefix-summed word widths; shared by every copy of this paragraph.
    pub widths: std::sync::Arc<Vec<f64>>,
    pub characters: Vec<f64>,
    /// Per-word advance of a trailing punctuation run, not a prefix sum.
    pub end_hangs: Vec<f64>,
    pub start_hangs: Vec<f64>,
    /// Optional absolute-pixel optical credits. Start replaces quote-only credit;
    /// end combines with punctuation hanging by taking the larger allowance.
    pub start_protrusions: Option<Vec<f64>>,
    pub end_protrusions: Option<Vec<f64>>,
    pub space: f64,
    /// Optional measured discretionary breaks; ordinary words retain the fast path.
    pub hyphenation: Option<Vec<Option<WordFragments>>>,
}

/// Measured fragments of one source word, for discretionary breaks.
#[derive(Debug, Clone, PartialEq)]
pub struct WordFragments {
    /// Byte source offsets, including zero and the word's length.
    pub offsets: Vec<usize>,
    /// Square matrices indexed by start * offsets.len() + end.
    pub widths: Vec<f64>,
    pub hyphen_widths: Vec<f64>,
    pub characters: Vec<f64>,
    /// Leading credit at each source boundary, plus the generated hyphen's end.
    pub start_protrusions: Option<Vec<f64>>,
    pub hyphen_protrusion: Option<f64>,
    /// Per boundary: true when the source already ends in a hyphen there, so a
    /// break renders no additional glyph.
    pub explicit: Option<Vec<bool>>,
}

/// One justified line.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub start: usize,
    pub end: usize,
    /// The measure this line was fitted to.
    pub width: f64,
    pub natural: f64,
    pub word_spacing: f64,
    pub tracking: f64,
    /// Available optical margin credit; a ragged last line need not use it.
    pub hanging: f64,
    pub opening: f64,
    pub residual: f64,
    /// True when finishing used word spacing beyond the preferred limits.
    pub relaxed: bool,
    pub cost: f64,
    pub last: bool,
    /// Byte offsets within the first/last included word; absent for whole words.
    pub start_offset: Option<usize>,
    pub end_offset: Option<usize>,
    /// Render a discretionary '-' after the source text, without adding it to the source.
    pub hyphenated: Option<bool>,
    /// 0 tight, 1 decent, 2 loose, 3 very loose; based on unbounded fitting strain.
    pub fitness: Option<i32>,
}

impl Line {
    fn blank() -> Line {
        Line {
            start: 0,
            end: 0,
            width: 0.0,
            natural: 0.0,
            word_spacing: 0.0,
            tracking: 0.0,
            hanging: 0.0,
            opening: 0.0,
            residual: 0.0,
            relaxed: false,
            cost: 0.0,
            last: false,
            start_offset: None,
            end_offset: None,
            hyphenated: None,
            fitness: None,
        }
    }
}

/// A solved paragraph: the chosen breaks and their total cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub lines: Vec<Line>,
    pub cost: f64,
    pub candidates: usize,
}

/// Font-aware edge measurements for one string: how far its first and last
/// glyph may protrude past a margin, in absolute pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Margins {
    pub start: f64,
    pub end: f64,
}

// ---------------------------------------------------------------------------
// JavaScript-compatible numeric helpers. The reference implementation runs on
// IEEE-754 doubles with ECMAScript semantics; reproducing its zero, NaN, and
// rounding behaviour keeps candidate costs bit-for-bit comparable.
// ---------------------------------------------------------------------------

fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a >= b {
        a
    } else {
        b
    }
}

fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a <= b {
        a
    } else {
        b
    }
}

fn js_sign(x: f64) -> f64 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        x
    }
}

/// ECMAScript number truthiness: only 0, -0, and NaN are falsy.
fn truthy(x: f64) -> bool {
    x != 0.0 && !x.is_nan()
}

fn cube(value: f64) -> f64 {
    value * value * value
}

fn pow2(value: f64) -> f64 {
    value * value
}

/// `Math.cbrt(2)`, the emergency-stretch strain threshold in the balanced-mode
/// admissibility prune.
const CBRT2: f64 = 1.2599210498948732;

// ---------------------------------------------------------------------------
// Text scanning. The reference uses regular expressions over UTF-16; the port
// scans bytes with the same character classes. Offsets are byte offsets.
// ---------------------------------------------------------------------------

fn grapheme_count(text: &str) -> usize {
    text.graphemes(true).count()
}

const END_PUNCTUATION: [char; 10] = ['.', ',', ';', ':', '!', '?', '…', '’', '”', '"'];
const APOSTROPHE_PUNCTUATION: char = '\'';
const START_QUOTES: [char; 6] = ['“', '‘', '\'', '"', '«', '‹'];

fn is_end_punctuation(c: char) -> bool {
    END_PUNCTUATION.contains(&c) || c == APOSTROPHE_PUNCTUATION
}

/// The trailing punctuation run matched by `[.,;:!?…’”'"]+$`.
fn trailing_punctuation(word: &str) -> Option<&str> {
    let mut end = word.len();
    for (index, c) in word.char_indices().rev() {
        if is_end_punctuation(c) {
            end = index;
        } else {
            break;
        }
    }
    if end == word.len() {
        None
    } else {
        Some(&word[end..])
    }
}

/// The leading quote matched by `^[“‘"'«‹]`.
fn leading_quote(word: &str) -> Option<&str> {
    let first = word.chars().next()?;
    if START_QUOTES.contains(&first) {
        Some(&word[..first.len_utf8()])
    } else {
        None
    }
}

fn is_letter_or_number(c: char) -> bool {
    c.is_alphabetic() || c.is_numeric()
}

/// Offsets just after a hyphen joining two letters or digits; browsers and TeX
/// both allow a break there without adding a glyph.
fn explicit_hyphens(word: &str) -> Vec<usize> {
    if word.contains('\u{a0}') {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = word.char_indices().collect();
    let mut offsets = Vec::new();
    for (index, &(position, c)) in chars.iter().enumerate() {
        if c != '-' && c != '\u{2010}' {
            continue;
        }
        let before = if index > 0 {
            Some(chars[index - 1].1)
        } else {
            None
        };
        let after = chars.get(index + 1).map(|&(_, c)| c);
        if before.map(is_letter_or_number).unwrap_or(false)
            && after.map(is_letter_or_number).unwrap_or(false)
        {
            offsets.push(position + c.len_utf8());
        }
    }
    offsets
}

// ---------------------------------------------------------------------------
// Preparation
// ---------------------------------------------------------------------------

/// Byte ranges of each whitespace-delimited word in `text`, split exactly as
/// [`prepare`] splits them. Adapters that must map a line's words back to
/// source positions — byte offset to glyph cluster index, say — read this
/// instead of re-deriving the split rule.
pub fn split_ranges(text: &str) -> Vec<(usize, usize)> {
    let trimmed = text.trim();
    let base = text.len() - text.trim_start().len();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut start: Option<usize> = None;
    for (offset, c) in trimmed.char_indices() {
        let at = base + offset;
        if c == ' ' || c == '\t' || c == '\r' || c == '\n' || c == '\x0c' {
            if let Some(begin) = start.take() {
                ranges.push((begin, at));
            }
        } else if start.is_none() {
            start = Some(at);
        }
    }
    if let Some(begin) = start {
        ranges.push((begin, base + trimmed.len()));
    }
    ranges
}

/// Measure once; reuse the prepared paragraph across widths and policy changes.
/// ASCII whitespace collapses; NBSP remains inside an indivisible word.
/// Pass each hard-break-delimited segment separately.
pub fn prepare<F>(text: &str, mut measure: F) -> Result<Prepared>
where
    F: FnMut(&str) -> f64,
{
    let space = measure(" ");
    let mut cache: HashMap<String, f64> = HashMap::new();
    prepare_impl(text, space, &mut |from, to, hyphen| {
        let slice = &text[from..to];
        let owned;
        let key: &str = if hyphen {
            owned = format!("{slice}-");
            &owned
        } else {
            slice
        };
        if let Some(&cached) = cache.get(key) {
            return Ok(cached);
        }
        let width = measure(key);
        if !width.is_finite() || width < 0.0 {
            return Err(Error::range("Invalid measured word width"));
        }
        cache.insert(key.to_string(), width);
        Ok(width)
    })
}

/// [`prepare`] for adapters whose width depends on *where* the text sits rather
/// than what it says: a cumulative advance table, a bold span mid-sentence, or
/// kerning across a node boundary all give one string two widths, so the
/// callback takes byte ranges into `text` instead of the substring itself.
/// The `hyphen` flag asks for that range with a `-` drawn after it, which is
/// not part of the source. `space` is the width of one inter-word space, since
/// a paragraph need not contain one to be laid out.
///
/// [`prepare`]: prepare
pub fn prepare_ranged<F>(text: &str, space: f64, mut measure: F) -> Result<Prepared>
where
    F: FnMut(usize, usize, bool) -> f64,
{
    prepare_impl(text, space, &mut |from, to, hyphen| {
        let width = measure(from, to, hyphen);
        if !width.is_finite() || width < 0.0 {
            return Err(Error::range("Invalid measured word width"));
        }
        Ok(width)
    })
}

fn prepare_impl<F>(text: &str, space: f64, width_of: &mut F) -> Result<Prepared>
where
    F: FnMut(usize, usize, bool) -> Result<f64>,
{
    let ranges = split_ranges(text);
    let words: Vec<String> = ranges
        .iter()
        .map(|&(start, end)| text[start..end].to_string())
        .collect();
    let mut widths = vec![0.0; words.len() + 1];
    let mut characters = vec![0.0; words.len() + 1];
    let mut end_hangs = vec![0.0; words.len()];
    let mut start_hangs = vec![0.0; words.len()];

    if !space.is_finite() || space < 0.0 {
        return Err(Error::range("Invalid measured word width"));
    }
    if space <= 0.0 {
        return Err(Error::range("Space width must be positive"));
    }

    let mut hyphenation: Vec<Option<WordFragments>> = vec![None; words.len()];
    let mut any_fragments = false;
    for (i, &(start, end)) in ranges.iter().enumerate() {
        let word = &words[i];
        let w = width_of(start, end, false)?;
        if let Some(punctuation) = trailing_punctuation(word) {
            let offset = word.len() - punctuation.len();
            end_hangs[i] = js_min(w, width_of(start + offset, end, false)?);
        }
        if let Some(quote) = leading_quote(word) {
            start_hangs[i] = js_min(w, width_of(start, start + quote.len(), false)?);
        }
        widths[i + 1] = widths[i] + w;
        characters[i + 1] = characters[i] + grapheme_count(word) as f64;
        let explicit = explicit_hyphens(word);
        if !explicit.is_empty() {
            any_fragments = true;
            let mut offsets = Vec::with_capacity(explicit.len() + 2);
            offsets.push(0usize);
            offsets.extend(explicit.iter().copied());
            offsets.push(word.len());
            let explicit_at: HashSet<usize> = explicit.into_iter().collect();
            let mut within_word =
                |from: usize, to: usize, hyphen: bool| width_of(start + from, start + to, hyphen);
            hyphenation[i] = Some(fragments(
                word,
                w,
                &offsets,
                &explicit_at,
                &mut within_word,
            )?);
        }
    }

    Ok(Prepared {
        words,
        widths: std::sync::Arc::new(widths),
        characters,
        end_hangs,
        start_hangs,
        start_protrusions: None,
        end_protrusions: None,
        space,
        hyphenation: any_fragments.then_some(hyphenation),
    })
}

/// Build the fragment matrices for one word.
fn fragments<F>(
    word: &str,
    word_width: f64,
    offsets: &[usize],
    explicit_at: &HashSet<usize>,
    width_of: &mut F,
) -> Result<WordFragments>
where
    F: FnMut(usize, usize, bool) -> Result<f64>,
{
    let n = offsets.len();
    let mut widths = vec![0.0; n * n];
    let mut hyphen_widths = vec![0.0; n * n];
    let mut characters = vec![0.0; n * n];
    let explicit: Vec<bool> = offsets
        .iter()
        .map(|offset| explicit_at.contains(offset))
        .collect();
    for from in 0..n.saturating_sub(1) {
        for to in from + 1..n {
            let text = &word[offsets[from]..offsets[to]];
            let cell = from * n + to;
            widths[cell] = if from == 0 && to == n - 1 {
                word_width
            } else {
                width_of(offsets[from], offsets[to], false)?
            };
            if to < n - 1 {
                hyphen_widths[cell] = if explicit[to] {
                    widths[cell]
                } else {
                    width_of(offsets[from], offsets[to], true)?
                };
            }
            characters[cell] = grapheme_count(text) as f64;
        }
    }
    Ok(WordFragments {
        offsets: offsets.to_vec(),
        widths,
        hyphen_widths,
        characters,
        start_protrusions: None,
        hyphen_protrusion: None,
        explicit: explicit.iter().any(|&flag| flag).then_some(explicit),
    })
}

/// Add optional dictionary-supplied breaks without changing source words.
/// Measure every legal fragment as a shaped unit, including its visible hyphen.
/// The word index lets adapters preserve the source word's font/style.
/// Breaks after source hyphens found by `prepare` are retained alongside.
pub fn with_hyphenation<F, M>(p: &Prepared, mut hyphenate: F, mut measure: M) -> Result<Prepared>
where
    F: FnMut(&str, usize) -> Vec<String>,
    M: FnMut(&str, usize) -> f64,
{
    let mut hyphenation: Vec<Option<WordFragments>> = Vec::with_capacity(p.words.len());
    for (index, word) in p.words.iter().enumerate() {
        let existing = p
            .hyphenation
            .as_ref()
            .and_then(|entries| entries.get(index))
            .and_then(|entry| entry.as_ref());
        if word.contains('\u{a0}') {
            hyphenation.push(existing.cloned());
            continue;
        }
        let parts = hyphenate(word, index);
        if parts.is_empty() || parts.iter().any(|part| part.is_empty()) || parts.concat() != *word {
            return Err(Error::range("Hyphenation must partition the source word"));
        }
        if parts.len() == 1 {
            hyphenation.push(existing.cloned());
            continue;
        }
        let boundaries: HashSet<usize> = word.grapheme_indices(true).map(|(i, _)| i).collect();
        let mut offsets = vec![0usize];
        for part in &parts {
            offsets.push(offsets.last().copied().unwrap() + part.len());
        }
        if offsets[1..offsets.len() - 1]
            .iter()
            .any(|offset| !boundaries.contains(offset))
        {
            return Err(Error::range("Hyphenation must not split a grapheme"));
        }
        let explicit_at: HashSet<usize> = match existing {
            Some(fragments) => match &fragments.explicit {
                Some(explicit) => fragments
                    .offsets
                    .iter()
                    .zip(explicit)
                    .filter(|(_, &flag)| flag)
                    .map(|(&offset, _)| offset)
                    .collect(),
                None => HashSet::new(),
            },
            None => HashSet::new(),
        };
        let mut merged: Vec<usize> = offsets
            .iter()
            .copied()
            .chain(explicit_at.iter().copied())
            .collect();
        merged.sort_unstable();
        merged.dedup();
        let mut cache: HashMap<String, f64> = HashMap::new();
        let word_width = p.widths[index + 1] - p.widths[index];
        let fragments = fragments(
            word,
            word_width,
            &merged,
            &explicit_at,
            &mut |from, to, hyphen| {
                let borrowed = &word[from..to];
                let owned = hyphen.then(|| {
                    let mut with_hyphen = String::with_capacity(borrowed.len() + 1);
                    with_hyphen.push_str(borrowed);
                    with_hyphen.push('-');
                    with_hyphen
                });
                let key = owned.as_deref().unwrap_or(borrowed);
                if let Some(&cached) = cache.get(key) {
                    return Ok(cached);
                }
                let width = measure(key, index);
                if !width.is_finite() || width < 0.0 {
                    return Err(Error::range("Invalid measured fragment width"));
                }
                cache.insert(key.to_string(), width);
                Ok(width)
            },
        )?;
        hyphenation.push(Some(fragments));
    }
    let mut cloned = p.clone();
    cloned.hyphenation = Some(hyphenation);
    Ok(cloned)
}

/// Attach font-aware edge measurements after preparing optional hyphenation.
/// The callback receives full words, continuation suffixes, and the generated '-'.
/// The numerical core never measures or rasterizes a font itself.
pub fn with_optical_margins<F>(p: &Prepared, mut measure: F) -> Result<Prepared>
where
    F: FnMut(&str, usize) -> Margins,
{
    let mut start_protrusions = vec![0.0; p.words.len()];
    let mut end_protrusions = vec![0.0; p.words.len()];
    let edge = |text: &str, index: usize, measure: &mut F| -> Result<Margins> {
        let margins = measure(text, index);
        if ![(margins.start, margins.end)]
            .iter()
            .all(|&(start, end)| start.is_finite() && start >= 0.0 && end.is_finite() && end >= 0.0)
        {
            return Err(Error::range(
                "Optical margins must be finite and nonnegative",
            ));
        }
        Ok(margins)
    };
    let mut hyphenation: Vec<Option<WordFragments>> = Vec::with_capacity(p.words.len());
    for (index, word) in p.words.iter().enumerate() {
        let margins = edge(word, index, &mut measure)?;
        start_protrusions[index] = margins.start;
        end_protrusions[index] = margins.end;
        let Some(fragments) = p
            .hyphenation
            .as_ref()
            .and_then(|entries| entries.get(index))
            .and_then(|entry| entry.as_ref())
        else {
            hyphenation.push(None);
            continue;
        };
        let mut fragments = fragments.clone();
        let mut start = Vec::with_capacity(fragments.offsets.len());
        for &offset in &fragments.offsets {
            if offset == word.len() {
                start.push(0.0);
            } else {
                start.push(edge(&word[offset..], index, &mut measure)?.start);
            }
        }
        fragments.start_protrusions = Some(start);
        fragments.hyphen_protrusion = Some(edge("-", index, &mut measure)?.end);
        hyphenation.push(Some(fragments));
    }
    let mut cloned = p.clone();
    cloned.start_protrusions = Some(start_protrusions);
    cloned.end_protrusions = Some(end_protrusions);
    cloned.hyphenation = Some(hyphenation);
    Ok(cloned)
}

// ---------------------------------------------------------------------------
// Candidate scoring
// ---------------------------------------------------------------------------

fn validate(measure: Measure, o: &Options) -> Result<Vec<f64>> {
    let widths = measure.widths()?;
    let Options {
        mode: _, ending: _, ..
    } = *o;
    let numeric = [
        o.stretch,
        o.shrink,
        o.tracking,
        o.compression_penalty,
        o.emergency_stretch,
        o.hanging,
        o.opening,
        o.protrusion,
        o.adjacent_penalty,
        o.last_line,
        o.widow_penalty,
        o.hyphen_penalty,
        o.consecutive_hyphen_penalty,
        o.final_hyphen_penalty,
        o.explicit_hyphen_penalty,
    ];
    if numeric
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
    {
        return Err(Error::range("Policy values must be finite and nonnegative"));
    }
    if o.shrink > 1.0
        || o.last_line > 1.0
        || o.hanging > 1.0
        || o.opening > 1.0
        || o.protrusion > 1.0
    {
        return Err(Error::range(
            "Shrink, lastLine, hanging, opening, and protrusion must be at most 1",
        ));
    }
    Ok(widths)
}

/// Metrics of a partially-measured line, supplied by the hyphenated solver.
#[derive(Debug, Default, Clone, Copy)]
struct FragmentMetrics {
    natural: Option<f64>,
    chars: Option<f64>,
    last: Option<bool>,
    hanging: Option<f64>,
    opening: Option<f64>,
    continued: Option<bool>,
    start_protrusion: Option<f64>,
    end_protrusion: Option<f64>,
}

fn fit(
    p: &Prepared,
    start: usize,
    end: usize,
    width: f64,
    o: &Options,
    out: &mut Line,
    emergency: bool,
    fragment: Option<&FragmentMetrics>,
) {
    let gaps = end - start - 1;
    let gaps_f = gaps as f64;
    let chars = match fragment.and_then(|f| f.chars) {
        Some(chars) => chars,
        None => p.characters[end] - p.characters[start] + gaps_f,
    };
    let natural = match fragment.and_then(|f| f.natural) {
        Some(natural) => natural,
        None => p.widths[end] - p.widths[start] + gaps_f * p.space,
    };
    let last = match fragment.and_then(|f| f.last) {
        Some(last) => last,
        None => end == p.words.len(),
    };
    let hang_base = fragment
        .and_then(|f| f.hanging)
        .unwrap_or(p.end_hangs[end - 1]);
    let end_protrusion = match fragment {
        Some(fragment) => fragment.end_protrusion.unwrap_or(0.0),
        None => p
            .end_protrusions
            .as_ref()
            .map(|protrusions| protrusions[end - 1])
            .unwrap_or(0.0),
    };
    let hanging = js_max(hang_base * o.hanging, end_protrusion * o.protrusion);
    let optical_start = match fragment {
        Some(fragment) => fragment.start_protrusion,
        None => p
            .start_protrusions
            .as_ref()
            .map(|protrusions| protrusions[start]),
    };
    let opening = match optical_start {
        None => {
            fragment
                .and_then(|f| f.opening)
                .unwrap_or(p.start_hangs[start])
                * o.opening
        }
        Some(credit) => credit * o.protrusion,
    };
    let target = width + hanging + opening;
    let delta = if last && natural <= target {
        0.0
    } else {
        target - natural
    };
    let space_budget = gaps_f * p.space * if delta >= 0.0 { o.stretch } else { o.shrink };
    let track_budget = chars * o.tracking;
    let capacity = space_budget + track_budget;
    let ratio = if truthy(capacity) {
        js_min(1.0, delta.abs() / capacity)
    } else {
        0.0
    };
    let sign = js_sign(delta);
    let word_spacing = if gaps != 0 {
        sign * ratio * space_budget / gaps_f
    } else {
        0.0
    };
    let tracking = sign * ratio * o.tracking;
    let residual = delta - sign * ratio * capacity;
    // Price preferred-limit strain, but distinguish a fillable loose line from
    // a real overflow. Otherwise a narrow column can choose protruding words
    // over a feasible line simply because the latter needs wider spaces.
    let strain = if emergency && delta > 0.0 {
        100.0 * cube(delta / (capacity + width * o.emergency_stretch))
    } else if o.mode == Mode::Balanced {
        100.0 * cube(ratio + residual.abs() / js_max(capacity, js_max(gaps_f * p.space, p.space)))
    } else {
        ratio * ratio * ratio * 100.0
    };
    let missing = js_max(0.0, o.last_line * width - natural);
    // A one-word tail has little flexibility: missing half the target should not
    // cost less than a modestly tightened body line merely because the page is wide.
    let continued = match fragment.and_then(|f| f.continued) {
        Some(continued) => continued,
        None => start > 0,
    };
    let ending = if last && continued {
        match o.ending {
            Ending::Fit => {
                o.widow_penalty / 300.0
                    * js_min(
                        10000.0,
                        100.0
                            * cube(
                                missing
                                    / js_max(p.space, gaps_f * p.space * o.stretch + track_budget),
                            ),
                    )
            }
            Ending::Soft => o.widow_penalty * pow2(missing / width),
        }
    } else {
        0.0
    };
    out.start = start;
    out.end = end;
    out.width = width;
    out.natural = natural;
    out.word_spacing = word_spacing;
    out.tracking = tracking;
    out.hanging = hanging;
    out.opening = opening;
    out.residual = residual;
    out.relaxed = false;
    out.last = last;
    let unbounded = if truthy(capacity) {
        delta / capacity
    } else if delta == 0.0 {
        0.0
    } else {
        sign * f64::INFINITY
    };
    out.fitness = Some(if unbounded < -0.5 {
        0
    } else if unbounded <= 0.5 {
        1
    } else if unbounded <= 1.0 {
        2
    } else {
        3
    });
    finish(out, p, o);
    let remaining = out.residual;
    let failure = if remaining.abs() > 0.01 {
        overflow_cost(
            remaining,
            if remaining < 0.0 {
                o.mode
            } else {
                Mode::Strict
            },
        )
    } else {
        0.0
    };
    out.cost = 1.0
        + strain
            * (if delta < 0.0 {
                o.compression_penalty
            } else {
                1.0
            })
        + failure
        + ending;
    if o.mode == Mode::Balanced
        && o.emergency_stretch > 0.0
        && !emergency
        && (delta > capacity * CBRT2 + 0.01 || remaining.abs() > 0.01)
    {
        out.cost = f64::INFINITY;
    }
}

fn overflow_cost(residual: f64, mode: Mode) -> f64 {
    if mode == Mode::Balanced {
        1e6 + residual * residual * 1e4
    } else {
        1e4 + residual * residual * 100.0
    }
}

/// Finish each candidate before scoring its actual residual.
/// Tracking remains bounded. Spaces can expand to complete a line, or contract
/// only within the configured shrink limit.
/// Single tokens and genuinely unfit material retain their reported residual.
fn finish(line: &mut Line, p: &Prepared, o: &Options) {
    let gaps = line.end - line.start - 1;
    if o.mode == Mode::Strict || gaps == 0 || line.residual == 0.0 {
        return;
    }
    let spacing = js_max(
        -p.space * o.shrink,
        line.word_spacing + line.residual / gaps as f64,
    );
    let adjustment = spacing - line.word_spacing;
    line.relaxed = adjustment.abs() > 1e-9;
    line.residual -= adjustment * gaps as f64;
    if line.residual.abs() < 1e-9 {
        line.residual = 0.0;
    }
    line.word_spacing = spacing;
}

/// Dynamic-programming state per break: which measure the next line takes
/// (line index, clamped once widths repeat) x the finished line's fitness class.
struct Shape {
    fitnesses: usize,
    lines: usize,
    states: usize,
    origin: usize,
    widths: Vec<f64>,
    widest: f64,
}

impl Shape {
    fn new(widths: Vec<f64>, o: &Options) -> Shape {
        let fitnesses = if o.adjacent_penalty != 0.0 { 4 } else { 1 };
        let lines = widths.len();
        let states = fitnesses * lines;
        let origin = if fitnesses == 4 { 1 } else { 0 };
        let widest = widths
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, |a, b| js_max(a, b));
        Shape {
            fitnesses,
            lines,
            states,
            origin,
            widths,
            widest,
        }
    }

    fn width(&self, l: usize) -> f64 {
        self.widths[l]
    }

    fn next(&self, l: usize) -> usize {
        (l + 1).min(self.lines - 1)
    }
}

fn any_finite(costs: &[f64], from: usize, count: usize) -> bool {
    (from..from + count).any(|i| costs[i] != f64::INFINITY)
}

fn cheapest(costs: &[f64], from: usize) -> usize {
    let mut best = from;
    for i in from + 1..costs.len() {
        if costs[i] < costs[best] {
            best = i;
        }
    }
    best
}

/// The line's text as rendered: source words joined by spaces, trimmed to the
/// chosen offsets, with a rendered hyphen appended when generated.
pub fn line_text(p: &Prepared, line: &Line) -> String {
    if line.start >= line.end {
        return String::new();
    }
    let mut words: Vec<String> = p.words[line.start..line.end].to_vec();
    if let Some(end_offset) = line.end_offset {
        let last = words.last_mut().unwrap();
        last.truncate(end_offset);
    }
    if let Some(start_offset) = line.start_offset {
        if start_offset != 0 {
            words[0] = words[0][start_offset..].to_string();
        }
    }
    let joined = words.join(" ");
    if line.hyphenated == Some(true) {
        joined + "-"
    } else {
        joined
    }
}

/// Exact optimum within the first viable pass: ordinary fitting, then relaxed
/// stretch scoring if necessary. Both passes use admissible overflow pruning.
pub fn solve(p: &Prepared, measure: impl Into<Measure>) -> Result<Layout> {
    solve_with(p, measure, Options::default())
}

/// `solve` with an explicit policy, mirroring `solve(paragraph, width, {...})`.
pub fn solve_with(p: &Prepared, measure: impl Into<Measure>, policy: Options) -> Result<Layout> {
    let o = policy;
    let widths = validate(measure.into(), &o)?;
    if p.hyphenation
        .as_ref()
        .is_some_and(|entries| entries.iter().any(Option::is_some))
    {
        return solve_hyphenated(p, widths, &o);
    }
    let n = p.words.len();
    let s = Shape::new(widths, &o);
    let total = (n + 1) * s.states;
    let mut costs = vec![f64::INFINITY; total];
    let mut previous = vec![0usize; total];
    // Keep reachable prefix boundaries in source order. Scanning backward
    // preserves tie-breaking while skipping prefixes this pass cannot compose.
    let mut reachable = vec![0usize; n + 1];
    costs[s.origin] = 0.0;
    let mut candidates = 0usize;
    let mut scratch = Line::blank();
    let mut emergency = false;
    // A longer candidate must not become narrower at maximum compression.
    // Exotic policies (e.g. tracking larger than glyph advances) use full search.
    let max_opening = match &p.start_protrusions {
        Some(protrusions) => protrusions
            .iter()
            .fold(0.0, |max, &value| js_max(max, value * o.protrusion)),
        None => p
            .start_hangs
            .iter()
            .fold(0.0, |max, &value| js_max(max, value * o.opening)),
    };
    let mut monotone = p.space * (1.0 - o.shrink) >= o.tracking;
    for i in 0..n {
        if !monotone {
            break;
        }
        monotone =
            p.widths[i + 1] - p.widths[i] >= (p.characters[i + 1] - p.characters[i]) * o.tracking;
    }
    for pass in 0..2 {
        emergency = pass == 1;
        costs.fill(f64::INFINITY);
        costs[s.origin] = 0.0;
        let mut count = 1usize;
        reachable[0] = 0;
        for end in 1..=n {
            for index in (0..count).rev() {
                let start = reachable[index];
                let mut fitted: Option<Line> = None;
                for l in 0..s.lines {
                    let base = start * s.states + l * s.fitnesses;
                    if !any_finite(&costs, base, s.fitnesses) {
                        continue;
                    }
                    fit(p, start, end, s.width(l), &o, &mut scratch, emergency, None);
                    candidates += 1;
                    let line = scratch.clone();
                    fitted = Some(line.clone());
                    let target = end * s.states
                        + s.next(l) * s.fitnesses
                        + if s.fitnesses == 4 {
                            line.fitness.unwrap() as usize
                        } else {
                            0
                        };
                    for fitness in 0..s.fitnesses {
                        let source = base + fitness;
                        let adjacent =
                            if start != 0 && (fitness as i32 - line.fitness.unwrap()).abs() > 1 {
                                o.adjacent_penalty
                            } else {
                                0.0
                            };
                        let cost = costs[source] + line.cost + adjacent;
                        if cost < costs[target] {
                            costs[target] = cost;
                            previous[target] = source;
                        }
                    }
                }
                let Some(line) = fitted else { continue };
                // Prefix costs are nonnegative. Allow the largest possible opening
                // credit and the widest measure before ruling out all earlier starts.
                let overflow_bound =
                    line.residual + (s.widest - line.width) + max_opening - line.opening;
                // All earlier overflowing candidates are tight: compare only that state,
                // never the best of other fitness classes that future lines may need.
                let mut best_tight = f64::INFINITY;
                for l in 0..s.lines {
                    best_tight = js_min(best_tight, costs[end * s.states + l * s.fitnesses]);
                }
                if monotone
                    && overflow_bound < -0.01
                    && ((!emergency && o.mode == Mode::Balanced && o.emergency_stretch > 0.0)
                        || 1.0 + overflow_cost(overflow_bound, o.mode) >= best_tight)
                {
                    break;
                }
            }
            if any_finite(&costs, end * s.states, s.states) {
                reachable[count] = end;
                count += 1;
            }
        }
        if any_finite(&costs, n * s.states, s.states) {
            break;
        }
    }
    let mut lines: Vec<Line> = Vec::new();
    let terminal = cheapest(&costs, n * s.states);
    let mut slot = terminal;
    while slot >= s.states {
        let source = previous[slot];
        let start = source / s.states;
        let end = slot / s.states;
        fit(
            p,
            start,
            end,
            s.width((source % s.states) / s.fitnesses),
            &o,
            &mut scratch,
            emergency,
            None,
        );
        let mut line = scratch.clone();
        if start != 0 && ((source % s.fitnesses) as i32 - line.fitness.unwrap()).abs() > 1 {
            line.cost += o.adjacent_penalty;
        }
        lines.push(line);
        slot = source;
    }
    lines.reverse();
    Ok(Layout {
        lines,
        cost: costs[terminal],
        candidates,
    })
}

// ---------------------------------------------------------------------------
// Hyphenated solving
// ---------------------------------------------------------------------------

/// A break node also records whether the preceding line hyphenated. This makes
/// adjacency penalties part of the optimum rather than a post-layout repair.
#[derive(Debug, Clone, Copy)]
struct Node {
    word: usize,
    part: usize,
    offset: usize,
    explicit: bool,
}

fn solve_hyphenated(p: &Prepared, widths: Vec<f64>, o: &Options) -> Result<Layout> {
    let mut nodes = vec![Node {
        word: 0,
        part: 0,
        offset: 0,
        explicit: false,
    }];
    for word in 0..p.words.len() {
        let prepared = p
            .hyphenation
            .as_ref()
            .and_then(|entries| entries.get(word))
            .and_then(|entry| entry.as_ref());
        if let Some(prepared) = prepared {
            for (index, &offset) in prepared.offsets[1..prepared.offsets.len() - 1]
                .iter()
                .enumerate()
            {
                nodes.push(Node {
                    word,
                    part: index + 1,
                    offset,
                    explicit: prepared
                        .explicit
                        .as_ref()
                        .map_or(false, |flags| flags[index + 1]),
                });
            }
        }
        nodes.push(Node {
            word: word + 1,
            part: 0,
            offset: 0,
            explicit: false,
        });
    }
    let s = Shape::new(widths, o);
    let total = nodes.len() * s.states;
    let mut costs = vec![f64::INFINITY; total];
    let mut previous = vec![0usize; total];
    let mut candidates = 0usize;
    let mut emergency = false;
    let mut scratch = Line::blank();
    let hyphenation = p
        .hyphenation
        .as_ref()
        .expect("hyphenated solve requires fragments");

    let fragment = |word: usize, from: usize, to: Option<usize>, hyphen: bool| -> (f64, f64) {
        let Some(prepared) = hyphenation.get(word).and_then(|entry| entry.as_ref()) else {
            return (
                p.widths[word + 1] - p.widths[word],
                p.characters[word + 1] - p.characters[word],
            );
        };
        let n = prepared.offsets.len();
        let cell = from * n + to.unwrap_or(n - 1);
        let generated = hyphen && {
            let target = to.expect("a hyphen break always targets a boundary");
            !prepared
                .explicit
                .as_ref()
                .map_or(false, |flags| flags[target])
        };
        let width = if hyphen {
            prepared.hyphen_widths[cell]
        } else {
            prepared.widths[cell]
        };
        (width, prepared.characters[cell] + generated as u8 as f64)
    };

    let candidate =
        |start: usize, end: usize, width: f64, emergency: bool, scratch: &mut Line| -> Line {
            let a = nodes[start];
            let b = nodes[end];
            let hyphen = b.offset > 0;
            let generated = hyphen && !b.explicit;
            let last_word = if hyphen { b.word } else { b.word - 1 };
            let (natural, chars);
            if a.word == last_word {
                let part = fragment(
                    a.word,
                    a.part,
                    if hyphen { Some(b.part) } else { None },
                    hyphen,
                );
                natural = part.0;
                chars = part.1;
            } else {
                let first = fragment(a.word, a.part, None, false);
                let last = fragment(
                    last_word,
                    0,
                    if hyphen { Some(b.part) } else { None },
                    hyphen,
                );
                let gaps = last_word - a.word;
                natural = first.0 + p.widths[last_word] - p.widths[a.word + 1]
                    + last.0
                    + gaps as f64 * p.space;
                chars = first.1 + p.characters[last_word] - p.characters[a.word + 1]
                    + last.1
                    + gaps as f64;
            }
            let start_protrusion = if a.offset > 0 {
                hyphenation
                    .get(a.word)
                    .and_then(|entry| entry.as_ref())
                    .and_then(|fragments| fragments.start_protrusions.as_ref())
                    .and_then(|protrusions| protrusions.get(a.part).copied())
            } else {
                p.start_protrusions
                    .as_ref()
                    .map(|protrusions| protrusions[a.word])
            };
            let end_protrusion = if hyphen {
                hyphenation
                    .get(last_word)
                    .and_then(|entry| entry.as_ref())
                    .and_then(|fragments| fragments.hyphen_protrusion)
            } else {
                p.end_protrusions
                    .as_ref()
                    .map(|protrusions| protrusions[last_word])
            };
            let metrics = FragmentMetrics {
                natural: Some(natural),
                chars: Some(chars),
                last: Some(end == nodes.len() - 1),
                continued: Some(start > 0),
                hanging: Some(if hyphen { 0.0 } else { p.end_hangs[last_word] }),
                opening: Some(if a.offset > 0 {
                    0.0
                } else {
                    p.start_hangs[a.word]
                }),
                start_protrusion,
                end_protrusion,
            };
            fit(
                p,
                a.word,
                last_word + 1,
                width,
                o,
                scratch,
                emergency,
                Some(&metrics),
            );
            let mut line = scratch.clone();
            line.start_offset = (a.offset != 0).then_some(a.offset);
            line.end_offset = (b.offset != 0).then_some(b.offset);
            line.hyphenated = Some(generated);
            let after_generated = a.offset > 0 && !a.explicit;
            line.cost += if generated {
                o.hyphen_penalty
                    + if after_generated {
                        o.consecutive_hyphen_penalty
                    } else {
                        0.0
                    }
            } else if hyphen {
                o.explicit_hyphen_penalty
            } else if line.last && after_generated {
                o.final_hyphen_penalty
            } else {
                0.0
            };
            line
        };

    for pass in 0..2 {
        emergency = pass == 1;
        costs.fill(f64::INFINITY);
        costs[s.origin] = 0.0;
        for end in 1..nodes.len() {
            for start in (0..end).rev() {
                for l in 0..s.lines {
                    let base = start * s.states + l * s.fitnesses;
                    if !any_finite(&costs, base, s.fitnesses) {
                        continue;
                    }
                    let line = candidate(start, end, s.width(l), emergency, &mut scratch);
                    candidates += 1;
                    let target = end * s.states
                        + s.next(l) * s.fitnesses
                        + if s.fitnesses == 4 {
                            line.fitness.unwrap() as usize
                        } else {
                            0
                        };
                    for fitness in 0..s.fitnesses {
                        let source = base + fitness;
                        let adjacent =
                            if start != 0 && (fitness as i32 - line.fitness.unwrap()).abs() > 1 {
                                o.adjacent_penalty
                            } else {
                                0.0
                            };
                        let cost = costs[source] + line.cost + adjacent;
                        if cost < costs[target] {
                            costs[target] = cost;
                            previous[target] = source;
                        }
                    }
                }
            }
        }
        if any_finite(&costs, (nodes.len() - 1) * s.states, s.states) {
            break;
        }
    }
    let mut lines: Vec<Line> = Vec::new();
    let terminal = cheapest(&costs, (nodes.len() - 1) * s.states);
    let mut slot = terminal;
    while slot >= s.states {
        let source = previous[slot];
        let start = source / s.states;
        let end = slot / s.states;
        let mut line = candidate(
            start,
            end,
            s.width((source % s.states) / s.fitnesses),
            emergency,
            &mut scratch,
        );
        if start != 0 && ((source % s.fitnesses) as i32 - line.fitness.unwrap()).abs() > 1 {
            line.cost += o.adjacent_penalty;
        }
        lines.push(line);
        slot = source;
    }
    lines.reverse();
    Ok(Layout {
        lines,
        cost: costs[terminal],
        candidates,
    })
}
