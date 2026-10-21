use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0} not found")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error("terminal error: {0}")]
    Pty(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

impl AppError {
    pub fn workspace(id: &str) -> Self {
        Self::NotFound(format!("workspace {id}"))
    }

    pub fn terminal(id: &str) -> Self {
        Self::NotFound(format!("terminal {id}"))
    }
}

/// Commands reject with the error message as a plain string.
impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
