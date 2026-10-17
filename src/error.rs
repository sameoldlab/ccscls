//! Crate-wide error type with a context chain, in the spirit of `anyhow`
//! (which is deliberately not a dependency).
//!
//! Wrap failures with `.context("what we were doing")` as they travel up; the
//! display form reads outermost first: `starting up: reading the file: not found`.

use std::fmt;

#[derive(Debug)]
pub struct Error {
    /// Outermost context first, root cause last.
    chain: Vec<String>,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self { chain: vec![message.into()] }
    }

    pub fn context(mut self, message: impl Into<String>) -> Self {
        self.chain.insert(0, message.into());
        self
    }

    pub fn chain(&self) -> impl Iterator<Item = &str> {
        self.chain.iter().map(String::as_str)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (position, message) in self.chain.iter().enumerate() {
            if position > 0 {
                formatter.write_str(": ")?;
            }
            formatter.write_str(message)?;
        }
        Ok(())
    }
}

impl<E: std::error::Error> From<E> for Error {
    fn from(error: E) -> Self {
        Self::new(error.to_string())
    }
}

/// Adds `.context(..)` to `Result` and `Option`.
pub trait Context<T> {
    fn context(self, message: impl Into<String>) -> Result<T>;
    fn with_context(self, message: impl FnOnce() -> String) -> Result<T>;
}

impl<T, E: Into<Error>> Context<T> for std::result::Result<T, E> {
    fn context(self, message: impl Into<String>) -> Result<T> {
        self.map_err(|error| error.into().context(message))
    }

    fn with_context(self, message: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|error| error.into().context(message()))
    }
}

impl<T> Context<T> for Option<T> {
    fn context(self, message: impl Into<String>) -> Result<T> {
        self.ok_or_else(|| Error::new(message))
    }

    fn with_context(self, message: impl FnOnce() -> String) -> Result<T> {
        self.ok_or_else(|| Error::new(message()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_wraps_outermost_first() {
        let failing: std::result::Result<(), std::io::Error> =
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"));
        let error = failing.context("reading the file").context("starting up").unwrap_err();
        assert_eq!(error.to_string(), "starting up: reading the file: no such file");
        assert_eq!(error.chain().count(), 3);
    }

    #[test]
    fn option_none_becomes_an_error() {
        let missing: Option<u32> = None;
        let error = missing.context("no value").unwrap_err();
        assert_eq!(error.to_string(), "no value");
        assert_eq!(Some(3).context("unused").unwrap(), 3);
    }

    #[test]
    fn lazy_context_only_runs_on_failure() {
        let fine: std::result::Result<u32, std::io::Error> = Ok(1);
        let value = fine.with_context(|| unreachable!("not called on success")).unwrap();
        assert_eq!(value, 1);
    }

    #[test]
    fn plain_errors_convert_with_question_mark() {
        fn parse(text: &str) -> Result<u32> {
            Ok(text.parse::<u32>()?)
        }
        assert!(parse("12").is_ok());
        assert!(parse("twelve").is_err());
    }
}
