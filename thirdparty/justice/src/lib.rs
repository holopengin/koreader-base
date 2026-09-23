//! Justice's DOM-free paragraph engine, ported from the TypeScript reference.
//!
//! The solver is a total-fit line breaker in the tradition of Knuth and Plass:
//! every break in a paragraph is chosen together to minimize a paragraph-wide
//! cost. You supply word widths for the exact font you render with; the engine
//! returns line boundaries and CSS spacing values.

pub mod engine;
pub mod ffi;
pub mod statics;

pub use engine::{
    line_text, prepare, prepare_ranged, solve, solve_with, split_ranges, with_hyphenation,
    with_optical_margins, Ending, Error, Layout, Line, Margins, Measure, Mode, Options, Prepared,
    Result, WordFragments,
};
pub use statics::{
    compile_static, compile_static_with, static_line, static_line_with, static_spacing,
    static_spacing_with, StaticBand, StaticLine,
};
