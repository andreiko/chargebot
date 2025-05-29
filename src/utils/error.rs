use std::backtrace::Backtrace;

use snafu::Snafu;

use crate::utils::retry::MaybeRetriable;

/// Represents a standardized error that can happen during an HTTP API call.
#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("unexpected response status: {code}"))]
    UnexpectedStatus { code: u16 },
    #[snafu(transparent, context(false))]
    ReqwestError {
        source: reqwest::Error,
        backtrace: Backtrace,
    },
    #[snafu(display("{msg}"))]
    InvalidContents { msg: String },
}

impl Error {
    /// Returns an error if the status code in the provided response differs from the provided expected code.
    pub fn expect_status_code(
        expected_code: u16,
        response: &reqwest::Response,
    ) -> Result<(), Self> {
        let code = response.status().as_u16();
        if code != expected_code {
            Err(Error::UnexpectedStatus { code })
        } else {
            Ok(())
        }
    }
}

impl MaybeRetriable for Error {
    fn is_retriable(&self) -> bool {
        match self {
            Error::UnexpectedStatus { code } => *code >= 500 && *code < 600,
            Error::ReqwestError { source, .. } => source.is_retriable(),
            Error::InvalidContents { .. } => false,
        }
    }
}

impl MaybeRetriable for reqwest::Error {
    fn is_retriable(&self) -> bool {
        self.is_connect()
            || self.is_timeout()
            || self.is_request()
            || (self.is_body() && !self.is_decode())
            || (self
                .status()
                .is_some_and(|s| s.as_u16() >= 500 && s.as_u16() < 600))
    }
}
