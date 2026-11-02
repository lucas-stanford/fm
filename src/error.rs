//! Failures and their exit status: 2 when the input is wrong (arguments,
//! flowchart or state), 1 when reading, writing or rendering fails.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    Usage,
    Plan,
    State,
    Io,
    Render,
}

impl Code {
    pub const fn exit(self) -> u8 {
        match self {
            Self::Usage | Self::Plan | Self::State => 2,
            Self::Io | Self::Render => 1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Failure {
    pub code: Code,
    pub message: String,
}

impl Failure {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

macro_rules! fail {
    ($code:expr, $($arg:tt)*) => {
        $crate::error::Failure::new($code, format!($($arg)*))
    };
}
pub(crate) use fail;

pub type Result<T> = std::result::Result<T, Failure>;
