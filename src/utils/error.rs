use std::fmt::{Display, Formatter};

/// Represents a standardized error that can happen during an HTTP API call.
#[derive(Debug)]
pub enum Error {
    UnexpectedStatus(u16),
    ReqwestError(reqwest::Error),
    InvalidContents(String),
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::UnexpectedStatus(code) => {
                f.write_fmt(format_args!("unexpected response status: {}", code))
            }
            Error::ReqwestError(err) => Display::fmt(err, f),
            Error::InvalidContents(msg) => f.write_str(msg),
        }
    }
}

impl Error {
    /// Returns whether the error is indicated by a server error status code.
    pub fn is_server(&self) -> bool {
        if let Error::UnexpectedStatus(code) = self {
            *code >= 500 && *code < 600
        } else {
            false
        }
    }

    /// Returns whether the call failed at the transport level.
    pub fn is_transport(&self) -> bool {
        if let Error::ReqwestError(err) = self {
            err.is_connect()
                || err.is_timeout()
                || err.is_request()
                || (err.is_body() && !err.is_decode())
        } else {
            false
        }
    }

    /// Returns an error if the status code in the provided response differs from the provided expected code.
    pub fn expect_status_code(
        expected_code: u16,
        response: &reqwest::Response,
    ) -> Result<(), Self> {
        let code = response.status().as_u16();
        if code != expected_code {
            Err(Error::UnexpectedStatus(code))
        } else {
            Ok(())
        }
    }
}

impl std::error::Error for Error {}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Self {
        Error::ReqwestError(err)
    }
}
