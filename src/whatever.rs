use std::{
    backtrace::Backtrace,
    fmt::{Debug, Display, Formatter},
};

use snafu::{ErrorCompat, FromString, Snafu};

use crate::logging::TextBacktrace;

/// Creates an instance of `WhateverSync` from a string.
#[macro_export]
macro_rules! snafu {
    ($s:literal) => {
        <$crate::whatever::WhateverSync as ::snafu::FromString>::without_source(format!($s))
    };
    ($s:literal, $($a:expr),+) => {
        <$crate::whatever::WhateverSync as ::snafu::FromString>::without_source(format!($s, $($a),+))
    };
}

/// General-purpose opaque error suitable when no inspection is necessary.
/// This is an extended version of `snafu::Whatever` with several improvements:
/// 1. Implements Send + Sync
/// 2. Can wrap source error transparently, capturing backtrace without changing representation.
#[derive(Debug)]
pub enum WhateverSync {
    /// Wraps an error delegating Display formatting to it.
    /// Construct using `SnafuExt::whatever()`.
    Transparent {
        backtrace: Backtrace,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// Wraps an error replacing its Display formatting with a message.
    /// Construct using `SnafuExt::whatever_msg()`.
    Context {
        backtrace: Backtrace,
        source: Box<dyn std::error::Error + Send + Sync>,
        message: String,
    },
    /// An error represented by a text error message.
    /// Construct using `snafu!`.
    Stringly {
        backtrace: Backtrace,
        message: String,
    },
}

/// Adapter that can be used as an error type for `main()` functions to ensure that the failure
/// message includes causes and a backtrace.
pub struct WhateverReport {
    error: Box<dyn std::error::Error>,
    backtrace: Backtrace,
}

/// Generic wrapper for errors for which Debug formatter provides
/// useful information that is missing from Display.
/// This is useful when you want to hide the error details from the end-user,
/// but still be able to see them in the logs.
#[derive(Debug, Snafu)]
#[snafu(display("{source:?}"), context(false))]
pub struct WrapDebug<E: std::error::Error + 'static> {
    pub source: E,
}

/// Convenience methods for converting results into `Result<T, WhateverSync>`.
pub trait SnafuExt<T> {
    /// # Errors
    /// Repacks the error from self.
    fn whatever(self) -> Result<T, WhateverSync>;
    /// # Errors
    /// Repacks the error from self.
    fn whatever_msg(self, message: impl Into<String>) -> Result<T, WhateverSync>;
}

/// One of the standard Snafu traits usually derived automatically.
impl FromString for WhateverSync {
    type Source = Box<dyn std::error::Error + Send + Sync>;

    fn without_source(message: String) -> Self {
        Self::Stringly {
            backtrace: Backtrace::capture(),
            message,
        }
    }

    fn with_source(source: Self::Source, message: String) -> Self {
        Self::Context {
            backtrace: Backtrace::capture(),
            source,
            message,
        }
    }
}

/// One of the standard Snafu traits usually derived automatically.
/// This is derived manually to support the transparent mode.
impl Display for WhateverSync {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            WhateverSync::Transparent { source, .. } => Display::fmt(source, f),
            WhateverSync::Context { message, .. } | WhateverSync::Stringly { message, .. } => {
                Display::fmt(message, f)
            }
        }
    }
}

/// One of the standard Snafu traits usually derived automatically.
impl ErrorCompat for WhateverSync {
    fn backtrace(&self) -> Option<&Backtrace> {
        Some(match self {
            WhateverSync::Transparent { backtrace, .. }
            | WhateverSync::Context { backtrace, .. }
            | WhateverSync::Stringly { backtrace, .. } => backtrace,
        })
    }
}

/// One of the standard Snafu traits usually derived automatically.
/// This is derived manually to support the transparent mode.
impl std::error::Error for WhateverSync {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WhateverSync::Transparent { source, .. } | WhateverSync::Context { source, .. } => {
                Some(source.as_ref())
            }
            WhateverSync::Stringly { .. } => None,
        }
    }

    fn cause(&self) -> Option<&dyn std::error::Error> {
        match self {
            WhateverSync::Transparent { source, .. } | WhateverSync::Context { source, .. } => {
                Some(source.as_ref())
            }
            WhateverSync::Stringly { .. } => None,
        }
    }

    fn provide<'a>(&'a self, request: &mut std::error::Request<'a>) {
        match self {
            WhateverSync::Transparent { source, .. } | WhateverSync::Context { source, .. } => {
                std::error::Error::provide(source.as_ref(), request);
            }
            WhateverSync::Stringly { .. } => {}
        }

        match self {
            WhateverSync::Transparent {
                source, backtrace, ..
            } => {
                if request.would_be_satisfied_by_ref_of::<Backtrace>() {
                    request.provide_ref(backtrace);
                }
                if request.would_be_satisfied_by_ref_of::<dyn std::error::Error>() {
                    request.provide_ref(source.as_ref());
                }
            }
            WhateverSync::Context {
                source, backtrace, ..
            } => {
                if request.would_be_satisfied_by_ref_of::<Backtrace>() {
                    request.provide_ref(backtrace);
                }
                if request.would_be_satisfied_by_ref_of::<dyn std::error::Error>() {
                    request.provide_ref(source.as_ref());
                }
            }
            WhateverSync::Stringly { backtrace, .. } => {
                if request.would_be_satisfied_by_ref_of::<Backtrace>() {
                    request.provide_ref(backtrace);
                }
            }
        }
    }
}

/// When `main()` has a return type of `Result<T, E>` and returns an `Err()`,
/// its `Debug` representation is printed just before the process exits.
impl Debug for WhateverReport {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self.error.as_ref(), f)?;

        let mut next_cause = self.error.source();
        while let Some(cause) = next_cause {
            f.write_fmt(format_args!("\n\nCaused by: {cause}"))?;
            next_cause = cause.source();
        }

        let bt =
            std::error::request_ref::<Backtrace>(self.error.as_ref()).unwrap_or(&self.backtrace);
        let buf = bt.to_string();
        f.write_fmt(format_args!(
            "\n\nBacktrace:\n{}",
            TextBacktrace::parse(&buf)
        ))?;

        Ok(())
    }
}

impl<E: std::error::Error + 'static> From<E> for WhateverReport {
    fn from(err: E) -> Self {
        Self {
            error: Box::new(err),
            backtrace: Backtrace::capture(),
        }
    }
}

impl<T, E: std::error::Error + 'static + Sync + Send> SnafuExt<T> for Result<T, E> {
    fn whatever(self) -> Result<T, WhateverSync> {
        match self {
            Ok(value) => Ok(value),
            Err(err) => Err(WhateverSync::Transparent {
                backtrace: Backtrace::capture(),
                source: Box::new(err),
            }),
        }
    }
    fn whatever_msg(self, message: impl Into<String>) -> Result<T, WhateverSync> {
        match self {
            Ok(value) => Ok(value),
            Err(err) => Err(WhateverSync::Context {
                backtrace: Backtrace::capture(),
                source: Box::new(err),
                message: message.into(),
            }),
        }
    }
}
