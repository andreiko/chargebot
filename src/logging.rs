use std::{
    fmt::{Display, Formatter, Write},
    sync::LazyLock,
};

use regex::Regex;
use valuable::{Valuable, Value, Visit};

static RE_BACKTRACE_FUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\d+: (.*)$").unwrap());
static RE_BACKTRACE_LOC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*at (.*)$").unwrap());

pub struct TextBacktrace<'a> {
    pub frames: Vec<TextFrame<'a>>,
}

#[derive(Valuable)]
pub struct TextFrame<'a> {
    fun: &'a str,
    loc: &'a str,
}

impl<'a> TextBacktrace<'a> {
    /// Scrapes and filters formatted output of a `std::backtrace::Backtrace`.
    pub fn parse(backtrace: &'a str) -> TextBacktrace<'a> {
        let mut frames = Vec::<TextFrame>::new();
        let mut function = None::<&str>;
        for line in backtrace.lines() {
            if let Some(fun_cap) = RE_BACKTRACE_FUN.captures(line) {
                let (_, [fun]) = fun_cap.extract::<1>();
                function = Some(fun);
            } else if let (Some(fun), Some(loc_cap)) = (function, RE_BACKTRACE_LOC.captures(line)) {
                let (_, [loc]) = loc_cap.extract::<1>();
                // filter out library crates
                if loc.starts_with("./crates/") {
                    frames.push(TextFrame { fun, loc });
                }
            }
        }

        TextBacktrace { frames }
    }
}

/// This trait allows the logger to represent the backtrace as a nested structure when
/// using JSON output format.
impl Valuable for TextBacktrace<'_> {
    fn as_value(&self) -> Value<'_> {
        self.frames.as_value()
    }

    fn visit(&self, visit: &mut dyn Visit) {
        self.frames.visit(visit);
    }
}

/// Formats the backtrace for non-JSON log format.
impl Display for TextBacktrace<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for frame in &self.frames {
            f.write_str(frame.fun)?;
            f.write_str("\n    at ")?;
            f.write_str(frame.loc)?;
            f.write_char('\n')?;
        }
        Ok(())
    }
}
