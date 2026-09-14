/// Admission limits applied independently to each static or Entry data source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataLimits {
    /// Maximum raw source bytes read before parsing.
    pub file_size: usize,
    /// Maximum logical Value occurrences after alias and merge expansion.
    pub nodes: usize,
    /// Maximum logical graph depth, with the root at depth one.
    pub depth: usize,
    /// Maximum element or field count of any one Array or Object.
    pub container_size: usize,
    /// Maximum decoded byte length of any one Bytes value.
    pub bytes_len: usize,
    /// Maximum decoded UTF-8 byte length of any String, object key, or temporal value.
    pub string_len: usize,
    /// Maximum total decoded bytes in Strings, object keys, temporal values, and Bytes.
    pub payloads_bytes: usize,
}

impl Default for DataLimits {
    fn default() -> Self {
        Self {
            file_size: 256 * 1024 * 1024,
            nodes: 1_000_000,
            depth: 256,
            container_size: 1_000_000,
            bytes_len: 64 * 1024 * 1024,
            string_len: 64 * 1024 * 1024,
            payloads_bytes: 256 * 1024 * 1024,
        }
    }
}
