//! Bounded, offset-based file reads shared by Tokio H1/H2/H3 writers.
//! No shared seek cursor and no allocation proportional to the whole file.
use crate::error::ServeError;
use crate::io::FileBody;
use bytes::Bytes;

const CHUNK_BYTES: u64 = 256 * 1024;

pub(crate) struct FileChunks {
    remaining: FileBody,
}

impl FileChunks {
    pub(crate) fn new(file: &FileBody) -> Self {
        Self {
            remaining: file.clone(),
        }
    }

    pub(crate) async fn next(&mut self) -> Result<Option<Bytes>, ServeError> {
        if self.remaining.len == 0 {
            return Ok(None);
        }
        let mut range = self.remaining.clone();
        range.len = range.len.min(CHUNK_BYTES);
        let bytes = tokio::task::spawn_blocking(move || range.read_to_bytes())
            .await
            .map_err(|error| ServeError::Io(std::io::Error::other(error)))??;
        let len = bytes.len() as u64;
        self.remaining.offset = self
            .remaining
            .offset
            .checked_add(len)
            .ok_or_else(|| ServeError::Io(std::io::Error::other("file offset overflow")))?;
        self.remaining.len -= len;
        Ok(Some(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn chunked_file_reads_are_bounded_and_preserve_range_offsets() {
        let mut file = tempfile::tempfile().unwrap();
        let input: Vec<u8> = (0..800_000).map(|i| (i % 251) as u8).collect();
        std::io::Write::write_all(&mut file, &input).unwrap();
        let body = FileBody {
            file: Arc::new(file),
            offset: 123,
            len: 700_000,
        };
        let mut chunks = FileChunks::new(&body);
        let mut result = Vec::new();
        while let Some(bytes) = chunks.next().await.unwrap() {
            assert!(bytes.len() <= CHUNK_BYTES as usize);
            result.extend_from_slice(&bytes);
        }
        assert_eq!(result, input[123..700_123]);
    }
}
