/* C ABI for the justice paragraph justifier.
 *
 * Rust keeps everything textual: splitting the paragraph into words, character
 * classes, grapheme counts, and the paragraph-wide solve. This side answers one
 * question and one question only:
 *
 *     how wide is byte range [start, end) of this paragraph?
 *
 * That phrasing is not a convenience. A cumulative advance table answers it in
 * O(1), while the same string in a regular and a bold span has two widths, so a
 * callback that took the substring instead of its position could not be answered
 * correctly at all.
 *
 * All offsets are byte offsets into the UTF-8 paragraph handed to
 * justice_prepare() and land on code point boundaries.
 *
 * Text policy stays in Rust; byte offsets are the seam.
 */

#ifndef JUSTICE_H
#define JUSTICE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/** An opaque prepared paragraph. */
typedef struct JusticeJob JusticeJob;

/**
 * The host's width oracle.
 *
 * Return the width in pixels of text[start..end]. When `hyphen` is non-zero,
 * draw a '-' after that range: it is not part of the source, so it cannot be
 * given as a range and the host must measure the concatenation itself.
 *
 * Must return a finite, non-negative value; anything else fails the prepare.
 * Must not longjmp or throw across this boundary.
 */
typedef double (*justice_measure_fn)(void *ctx, uint32_t start, uint32_t end,
                                     int hyphen);

/**
 * Fitting policy. Fill with justice_options_default() and adjust, or pass NULL
 * to justice_solve() for the published defaults.
 *
 * mode:   0 = balanced (relax word spacing beyond the preferred limits to
 *             complete a line), 1 = strict (stretch and shrink are hard).
 * ending: 0 = price a short final line by missing width alone, 1 = by its
 *             measured flexibility.
 *
 * Set `hanging` and `opening` to 0 to leave optical margins entirely to the
 * host, which is what a host with its own hanging-punctuation pass wants: the
 * line then targets exactly its usable width.
 */
typedef struct {
    double stretch;                  /**< Relative to measured space. */
    double shrink;                   /**< At most 1.0. */
    int mode;
    double tracking;                 /**< Absolute pixels per character. */
    double compression_penalty;
    double emergency_stretch;
    double hanging;                  /**< At most 1.0; 0 disables. */
    double opening;                  /**< At most 1.0; 0 disables. */
    double protrusion;               /**< At most 1.0; 0 disables. */
    double adjacent_penalty;
    double last_line;                /**< At most 1.0. */
    int ending;
    double widow_penalty;
    double hyphen_penalty;
    double consecutive_hyphen_penalty;
    double final_hyphen_penalty;
    double explicit_hyphen_penalty;
} JusticeOptions;

/** One solved line. */
typedef struct {
    /** Word indices; `word_end` is exclusive. */
    uint32_t word_start;
    uint32_t word_end;
    /** Byte range of this line's text within the prepared paragraph. */
    uint32_t byte_start;
    uint32_t byte_end;
    /** Per-gap adjustment to the measured space width, in pixels. */
    double word_spacing;
    /** Per-character adjustment, in pixels. */
    double tracking;
    /** Non-zero on the paragraph's final line. */
    int last;
} JusticeLine;

/** Fill `out` with the published defaults. Tolerates NULL. */
void justice_options_default(JusticeOptions *out);

/**
 * A description of the most recent failure on this thread, or NULL if the last
 * call succeeded. The string stays valid until the next justice_* call on the
 * same thread.
 */
const char *justice_error(void);

/**
 * Split `text` into words and measure each through `measure`.
 *
 * `space` is the width of one inter-word space in pixels. A paragraph may have
 * no space in it at all and still needs a positive value here.
 *
 * Returns NULL on failure: non-UTF-8 text, a missing measure, an empty text, or
 * a width the host reported that is not finite and non-negative. See
 * justice_error().
 *
 * `text` must hold `len` readable bytes. The host must keep nothing that the
 * callback closes over alive beyond this call.
 */
JusticeJob *justice_prepare(const char *text, size_t len, double space,
                            justice_measure_fn measure, void *ctx);

/** Release a job. Tolerates NULL so callers can free unconditionally. */
void justice_free(JusticeJob *job);

/**
 * Choose line breaks and spacing for the whole paragraph at once.
 *
 * `widths` holds one usable width per line in pixels and the final entry
 * repeats: pass a single width for a uniform column, or `[w - indent, w]` for a
 * first-line indent. `count` is the number of entries. `options` may be NULL
 * for the published defaults.
 *
 * Returns 0 on success, -1 on failure. Calling this again replaces the previous
 * solution. justice_line_count() is 0 until this succeeds.
 */
int justice_solve(JusticeJob *job, const double *widths, size_t count,
                  const JusticeOptions *options);

/** Words in the prepared paragraph. Returns 0 for a NULL job. */
uint32_t justice_word_count(const JusticeJob *job);

/**
 * Write 2 * justice_word_count(job) byte offsets: start, end for each word, in
 * order. The end of one word and the start of the next are separated by at
 * least one whitespace character, which is how a break is located.
 *
 * Returns 0 on success, -1 if an argument is NULL.
 */
int justice_word_byte_ranges(const JusticeJob *job, uint32_t *out);

/** Lines from the last successful justice_solve(). 0 before then. */
uint32_t justice_line_count(const JusticeJob *job);

/**
 * Fill `out` with line `index`.
 *
 * For a line that is not the last, byte_end is the first byte after its final
 * word: the character it falls on is the whitespace the line breaks at, and
 * byte_end is where the next line begins. For the last line there is no
 * separator, and byte_end is the end of the text.
 *
 * Returns 0 on success, -1 if `index` is out of range or `out` is NULL.
 */
int justice_line(const JusticeJob *job, uint32_t index, JusticeLine *out);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* JUSTICE_H */
