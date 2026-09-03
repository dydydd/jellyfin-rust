//! Culture-invariant formatting writer helpers.

use std::fmt;
use std::io::{self, Write};

/// A small writer wrapper for formatting-sensitive text files.
///
/// .NET's `FormattingStreamWriter` overrides `FormatProvider` so numeric
/// formatting uses `InvariantCulture`. Rust's formatting is already
/// locale-independent, so this wrapper makes that contract explicit at the
/// call site and keeps the official helper represented in this crate.
#[derive(Debug)]
pub struct FormattingStreamWriter<W> {
    inner: W,
}

impl<W> FormattingStreamWriter<W> {
    /// Wraps an existing writer.
    #[must_use]
    pub fn new(inner: W) -> Self {
        Self { inner }
    }

    /// Returns the wrapped writer.
    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W> FormattingStreamWriter<W>
where
    W: Write,
{
    /// Writes preformatted arguments without appending a line ending.
    pub fn write_format(&mut self, arguments: fmt::Arguments<'_>) -> io::Result<()> {
        self.inner.write_fmt(arguments)
    }

    /// Writes preformatted arguments followed by `\n`.
    pub fn write_format_line(&mut self, arguments: fmt::Arguments<'_>) -> io::Result<()> {
        self.inner.write_fmt(arguments)?;
        self.inner.write_all(b"\n")
    }

    /// Flushes the wrapped writer.
    pub fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W> Write for FormattingStreamWriter<W>
where
    W: Write,
{
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.inner.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Escapes a file path for an FFmpeg concat demuxer single-quoted stanza.
#[must_use]
pub fn escape_concat_file_path(path: &str) -> String {
    path.replace('\'', r#"'\''"#)
}
