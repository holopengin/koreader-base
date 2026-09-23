//! Build-time break selection: precomputed band plans for static rendering.

use crate::engine::{line_text, solve_with, Error, Layout, Line, Mode, Options, Prepared, Result};
use std::collections::BTreeMap;
use unicode_segmentation::UnicodeSegmentation;

/// Compact per-line inputs for staticSpacing(), reusable across width bands.
#[derive(Debug, Clone, PartialEq)]
pub struct StaticLine {
    pub line: Line,
    /// Custom-property declarations for this line's spacing rules.
    pub properties: BTreeMap<String, String>,
}

/// One sampled measure band. `lines` is null when the measure is unfit and
/// native wrapping should be retained.
#[derive(Debug, Clone, PartialEq)]
pub struct StaticBand {
    pub min: f64,
    pub max: f64,
    pub lines: Option<Vec<StaticLine>>,
}

/// ECMAScript `Number(n.toFixed(10))`, rendered as a plain decimal string.
fn number(n: f64) -> String {
    let rounded: f64 = format!("{:.10}", n).parse().unwrap_or(n);
    js_number_string(rounded)
}

/// ECMAScript `Number(n.toFixed(7))`, rendered as a plain decimal string.
fn coefficient(n: f64) -> String {
    let rounded: f64 = format!("{:.7}", n).parse().unwrap_or(n);
    js_number_string(rounded)
}

/// `String(n)` in ECMAScript: shortest round-trip digits, exponent form only
/// at magnitude 1e21 or below 1e-6, with a signed exponent.
fn js_number_string(n: f64) -> String {
    if n == 0.0 {
        return "0".to_string();
    }
    if !n.is_finite() {
        return if n.is_nan() {
            "NaN".to_string()
        } else if n > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        };
    }
    let scientific = format!("{n:e}");
    let (mantissa, exponent) = scientific.split_once('e').expect("scientific notation");
    let exponent: i32 = exponent.parse().expect("exponent");
    if (-6..=20).contains(&exponent) {
        let negative = mantissa.starts_with('-');
        let digits: String = mantissa.chars().filter(|c| c.is_ascii_digit()).collect();
        let point = exponent + 1;
        let mut whole = String::new();
        let mut fraction = String::new();
        if point <= 0 {
            whole.push('0');
            fraction.push_str(&"0".repeat(-point as usize));
            fraction.push_str(&digits);
        } else if (point as usize) >= digits.len() {
            whole.push_str(&digits);
            whole.push_str(&"0".repeat(point as usize - digits.len()));
        } else {
            whole.push_str(&digits[..point as usize]);
            fraction.push_str(&digits[point as usize..]);
        }
        fraction = fraction.trim_end_matches('0').to_string();
        let mut out = String::new();
        if negative {
            out.push('-');
        }
        out.push_str(&whole);
        if !fraction.is_empty() {
            out.push('.');
            out.push_str(&fraction);
        }
        out
    } else {
        format!(
            "{mantissa}e{}{}",
            if exponent < 0 { "-" } else { "+" },
            exponent.abs()
        )
    }
}

/// Declare these on each rendered word/fragment and gap. Keeping the expressions
/// shared avoids repeating the fitting algebra in every responsive line rule.
pub fn static_spacing(p: &Prepared) -> BTreeMap<String, String> {
    static_spacing_with(p, Options::default())
}

/// `static_spacing` with an explicit policy.
pub fn static_spacing_with(p: &Prepared, policy: Options) -> BTreeMap<String, String> {
    let o = policy;
    let mut properties = BTreeMap::new();
    properties.insert("--j-cap".to_string(), "1000000px".to_string());
    properties.insert("--j-left".to_string(), "0px".to_string());
    properties.insert(
        "--j-d".to_string(),
        "min(var(--j-cap), calc(100cqw - var(--j-n)))".to_string(),
    );
    properties.insert(
        "--j-t".to_string(),
        format!(
            "clamp(-{}px, calc(min(0px, var(--j-d)) * var(--j-m) + max(0px, var(--j-d)) * var(--j-p)), {}px)",
            number(o.tracking),
            number(o.tracking)
        ),
    );
    properties.insert(
        "--j-s".to_string(),
        format!(
            "max(-{}px, calc((var(--j-d) - var(--j-t) * var(--j-c)) / var(--j-g)))",
            number(p.space * o.shrink)
        ),
    );
    properties.insert(
        "--j-gap".to_string(),
        format!("calc({}px + var(--j-t) + var(--j-s))", number(p.space)),
    );
    properties
}

/// The same fit budgets as the numerical engine, expressed in container units.
/// Each participating piece/gap receives --j-t = tracking and --j-gap = gap.
/// The containing paragraph supplies container-type:inline-size.
pub fn static_line(p: &Prepared, line: &Line) -> StaticLine {
    static_line_with(p, line, Options::default())
}

/// `static_line` with an explicit policy.
pub fn static_line_with(p: &Prepared, line: &Line, policy: Options) -> StaticLine {
    let o = policy;
    let gaps = line.end - line.start - 1;
    let rendered = line_text(p, line);
    let chars = rendered.graphemes(true).count();
    let loose = gaps as f64 * p.space * o.stretch + chars as f64 * o.tracking;
    let tight = gaps as f64 * p.space * o.shrink + chars as f64 * o.tracking;
    let target = format!(
        "calc(100cqw - {}px)",
        number(line.natural - line.hanging - line.opening)
    );
    let delta = if line.last {
        format!("min(0px, {target})")
    } else {
        target
    };
    let mut spacing = "0px".to_string();
    if gaps > 0 && o.mode == Mode::Strict {
        let shrink = if tight != 0.0 {
            format!(
                "clamp(-{}px, calc({} * {}), 0px)",
                number(p.space * o.shrink),
                delta,
                number(p.space * o.shrink / tight)
            )
        } else {
            "0px".to_string()
        };
        let stretch = if loose != 0.0 {
            format!(
                "clamp(0px, calc({} * {}), {}px)",
                delta,
                number(p.space * o.stretch / loose),
                number(p.space * o.stretch)
            )
        } else {
            "0px".to_string()
        };
        spacing = format!("calc({shrink} + {stretch})");
    }
    let mut properties = BTreeMap::new();
    properties.insert(
        "--j-n".to_string(),
        format!("{}px", number(line.natural - line.hanging - line.opening)),
    );
    properties.insert(
        "--j-m".to_string(),
        coefficient(if tight != 0.0 {
            o.tracking / tight
        } else {
            0.0
        }),
    );
    properties.insert(
        "--j-p".to_string(),
        coefficient(if loose != 0.0 {
            o.tracking / loose
        } else {
            0.0
        }),
    );
    properties.insert("--j-c".to_string(), chars.to_string());
    properties.insert("--j-g".to_string(), gaps.max(1).to_string());
    if line.opening != 0.0 {
        properties.insert(
            "--j-left".to_string(),
            format!("{}px", number(-line.opening)),
        );
    }
    if line.last {
        properties.insert("--j-cap".to_string(), "0px".to_string());
    }
    if o.mode == Mode::Strict || gaps == 0 {
        properties.insert("--j-s".to_string(), spacing);
    }
    StaticLine {
        line: line.clone(),
        properties,
    }
}

/// ECMAScript `Math.round` for the finite, nonnegative values used below.
fn js_round(x: f64) -> f64 {
    if x.is_finite() {
        (x + 0.5).floor()
    } else {
        x
    }
}

fn plan_key(lines: &[Line]) -> String {
    let mut key = String::from("[");
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            key.push(',');
        }
        key.push('[');
        key.push_str(&line.start.to_string());
        key.push(',');
        key.push_str(&line.end.to_string());
        key.push(',');
        match line.start_offset {
            Some(offset) => key.push_str(&offset.to_string()),
            None => key.push_str("null"),
        }
        key.push(',');
        match line.end_offset {
            Some(offset) => key.push_str(&offset.to_string()),
            None => key.push_str("null"),
        }
        key.push(']');
    }
    key.push(']');
    key
}

/// Build-time break selection. Equal adjacent plans collapse into one CSS band.
/// Measures are sampled at `step` CSS pixels; spacing is continuous within a band.
/// This deliberately makes the sampling policy explicit rather than claiming an
/// unsampled fractional measure necessarily has the same global optimum.
pub fn compile_static(p: &Prepared, min: f64, max: f64) -> Result<Vec<StaticBand>> {
    compile_static_with(p, min, max, 1.0, Options::default())
}

/// `compile_static` with an explicit sampling step and policy.
pub fn compile_static_with(
    p: &Prepared,
    min: f64,
    max: f64,
    step: f64,
    policy: Options,
) -> Result<Vec<StaticBand>> {
    if !(min > 0.0)
        || !(max >= min)
        || !(step > 0.0)
        || !min.is_finite()
        || !max.is_finite()
        || !step.is_finite()
        || min + step == min
        || max + step == max
    {
        return Err(Error::range("Invalid static measure range"));
    }
    let mut bands: Vec<StaticBand> = Vec::new();
    let mut previous: Option<String> = None;
    let mut width = min;
    while width <= max + 1e-8 {
        let layout: Layout = solve_with(p, width, policy.clone())?;
        let fitted = !layout.lines.iter().any(|line| line.residual.abs() > 0.5);
        let key = if fitted {
            plan_key(&layout.lines)
        } else {
            "native".to_string()
        };
        match (bands.last_mut(), previous.as_ref()) {
            (Some(band), Some(previous_key)) if key == *previous_key => band.max = width + step,
            _ => bands.push(make_band(p, &layout, width, step, policy.clone())?),
        }
        previous = Some(key);
        width = min + js_round((width - min) / step + 1.0) * step;
    }
    Ok(bands)
}

fn make_band(
    p: &Prepared,
    layout: &Layout,
    width: f64,
    step: f64,
    policy: Options,
) -> Result<StaticBand> {
    let fitted = !layout.lines.iter().any(|line| line.residual.abs() > 0.5);
    Ok(StaticBand {
        min: width,
        max: width + step,
        lines: if fitted {
            Some(
                layout
                    .lines
                    .iter()
                    .map(|line| static_line_with(p, line, policy.clone()))
                    .collect(),
            )
        } else {
            None
        },
    })
}
