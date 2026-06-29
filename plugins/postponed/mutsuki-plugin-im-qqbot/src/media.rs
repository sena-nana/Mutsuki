use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaChunk {
    pub index: u64,
    pub bytes: Vec<u8>,
    pub md5: String,
}

pub trait QqMediaProvider: Send {
    fn read_chunks(
        &mut self,
        resource_ref: &str,
        block_size: u64,
    ) -> Result<Vec<MediaChunk>, QqMediaError>;
}

#[derive(Debug, Error)]
pub enum QqMediaError {
    #[error("media resource not found: {0}")]
    NotFound(String),
    #[error("media resource is not readable: {0}")]
    NotReadable(String),
    #[error("media resource failed: {0}")]
    Failed(String),
}
