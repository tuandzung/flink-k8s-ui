use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub struct UpstreamHttpError {
    pub status_code: u16,
    pub message: String,
}

impl Display for UpstreamHttpError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for UpstreamHttpError {}
