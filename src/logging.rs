use std::{
    backtrace::Backtrace,
    borrow::Borrow,
    fmt::{Display, Formatter, Write},
    sync::LazyLock,
};

use regex::Regex;
use snafu::ErrorCompat;
use tracing_subscriber::{
    fmt,
    fmt::{format, time::SystemTime, Layer},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter,
};
use valuable::{Valuable, Value, Visit};

use crate::whatever::{SnafuExt, WhateverSync};

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

#[derive(Clone, Copy)]
enum EventLevel {
    Error,
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

/// Logs the provided error, its backtrace and causes using the ERROR level.
pub fn log_error_with_backtrace<E: std::error::Error + ErrorCompat + 'static>(
    msg: impl Borrow<str>,
    err: &E,
) {
    log_event_with_backtrace(EventLevel::Error, msg, err);
}

pub fn init_logging() -> Result<(), WhateverSync> {
    let registry = tracing_subscriber::registry().with(EnvFilter::from_default_env());
    let format = custom_logging_format();
    registry
        .with(format.json().flatten_event(true))
        .try_init()
        .whatever()?;
    Ok(())
}

/// Configure a custom event formatter
#[must_use]
fn custom_logging_format<S>(
) -> Layer<S, format::DefaultFields, format::Format<format::Compact, SystemTime>>
where {
    fmt::layer()
        .with_level(true)
        .with_target(true)
        .with_file(true)
        .with_line_number(true)
        .with_ansi(true)
        .compact()
}

/// # Panics
/// Never. See SAFETY comments.
fn log_event_with_backtrace<E: std::error::Error + ErrorCompat + 'static>(
    level: EventLevel,
    msg: impl Borrow<str>,
    err: &E,
) {
    let causes = err.iter_chain().skip(1);
    let mut bt_out = None::<String>;
    let bt_parsed = std::error::request_ref::<Backtrace>(err)
        .map(|bt| TextBacktrace::parse(bt_out.insert(bt.to_string())));

    let cause_msgs = causes.map(ToString::to_string).collect::<Vec<_>>();
    if let Some(bt) = bt_parsed {
        match level {
            EventLevel::Error => {
                tracing::error!(
                    backtrace = bt.as_value(),
                    causes = cause_msgs.as_value(),
                    "{}: {err}",
                    msg.borrow()
                );
            }
        };
    } else {
        match level {
            EventLevel::Error => {
                tracing::error!(causes = cause_msgs.as_value(), "{}: {err}", msg.borrow());
            }
        };
    }
}
