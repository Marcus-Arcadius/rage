use radix64::{configs::UrlSafeNoPad, io::EncodeWriter, URL_SAFE_NO_PAD};
use std::cmp;
use std::io::{self, BufRead, BufReader, Read, Write};
use zeroize::Zeroizing;

use crate::util::LINE_ENDING;

const ARMORED_COLUMNS_PER_LINE: usize = 56;
const ARMORED_BYTES_PER_LINE: usize = ARMORED_COLUMNS_PER_LINE / 4 * 3;
const ARMORED_END_MARKER: &str = "--- end of file ---";

pub(crate) struct LineEndingWriter<W: Write> {
    inner: W,
    total_written: usize,
}

impl<W: Write> LineEndingWriter<W> {
    fn new(inner: W) -> Self {
        LineEndingWriter {
            inner,
            total_written: 0,
        }
    }

    fn finish(mut self) -> io::Result<W> {
        // Write the end marker
        self.inner.write_all(LINE_ENDING.as_bytes())?;
        self.inner.write_all(ARMORED_END_MARKER.as_bytes())?;
        self.inner.write_all(LINE_ENDING.as_bytes())?;

        Ok(self.inner)
    }
}

impl<W: Write> Write for LineEndingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        let remaining = ARMORED_COLUMNS_PER_LINE - (self.total_written % ARMORED_COLUMNS_PER_LINE);

        // Write the next newline if we are at the end of the line.
        if remaining == ARMORED_COLUMNS_PER_LINE && self.total_written > 0 {
            // This may involve multiple write calls to the wrapped writer, but consumes
            // no bytes from the input buffer.
            self.inner.write_all(LINE_ENDING.as_bytes())?;
        }

        let to_write = cmp::min(remaining, buf.len());

        // Write at most one line's worth of input. This ensures that we maintain the
        // invariant that if the wrapped writer returns an error, no bytes of the input
        // buffer have been written.
        let written = self.inner.write(&buf[..to_write])?;

        self.total_written += written;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub(crate) enum ArmoredWriter<W: Write> {
    Enabled {
        encoder: EncodeWriter<UrlSafeNoPad, LineEndingWriter<W>>,
    },

    Disabled {
        inner: W,
    },
}

impl<W: Write> ArmoredWriter<W> {
    pub(crate) fn wrap_output(inner: W, enabled: bool) -> Self {
        if enabled {
            ArmoredWriter::Enabled {
                encoder: EncodeWriter::new(URL_SAFE_NO_PAD, LineEndingWriter::new(inner)),
            }
        } else {
            ArmoredWriter::Disabled { inner }
        }
    }

    pub(crate) fn finish(self) -> io::Result<W> {
        match self {
            ArmoredWriter::Enabled { encoder } => encoder
                .finish()
                .map_err(|e| io::Error::from(e.error().kind()))
                .and_then(|line_ending| line_ending.finish()),
            ArmoredWriter::Disabled { inner } => Ok(inner),
        }
    }
}

impl<W: Write> Write for ArmoredWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            ArmoredWriter::Enabled { encoder } => encoder.write(buf),
            ArmoredWriter::Disabled { inner } => inner.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            ArmoredWriter::Enabled { encoder } => encoder.flush(),
            ArmoredWriter::Disabled { inner } => inner.flush(),
        }
    }
}

pub(crate) struct ArmoredReader<R: Read> {
    inner: BufReader<R>,
    enabled: bool,
    line_buf: Zeroizing<String>,
    byte_buf: Zeroizing<[u8; ARMORED_BYTES_PER_LINE]>,
    byte_start: usize,
    byte_end: usize,
    found_end: bool,
}

impl<R: Read> ArmoredReader<R> {
    pub(crate) fn from_reader(inner: R, enabled: bool) -> Self {
        ArmoredReader {
            inner: BufReader::new(inner),
            enabled,
            line_buf: Zeroizing::new(String::with_capacity(ARMORED_COLUMNS_PER_LINE + 2)),
            byte_buf: Zeroizing::new([0; ARMORED_BYTES_PER_LINE]),
            byte_start: ARMORED_BYTES_PER_LINE,
            byte_end: ARMORED_BYTES_PER_LINE,
            found_end: false,
        }
    }
}

impl<R: Read> Read for ArmoredReader<R> {
    fn read(&mut self, mut buf: &mut [u8]) -> io::Result<usize> {
        if !self.enabled {
            return self.inner.read(buf);
        }
        if self.found_end {
            return Ok(0);
        }

        let buf_len = buf.len();

        // Output any remaining bytes from the previous line
        if self.byte_start + buf_len <= self.byte_end {
            buf.copy_from_slice(&self.byte_buf[self.byte_start..self.byte_start + buf_len]);
            self.byte_start += buf_len;
            return Ok(buf_len);
        } else {
            let to_read = self.byte_end - self.byte_start;
            buf[..to_read].copy_from_slice(&self.byte_buf[self.byte_start..self.byte_end]);
            buf = &mut buf[to_read..];
        }

        loop {
            // Read the next line
            self.line_buf.clear();
            self.inner.read_line(&mut self.line_buf)?;

            // Handle line endings
            let line = if self.line_buf.ends_with("\r\n") {
                // trim_end_matches will trim the pattern repeatedly, but because
                // BufRead::read_line splits on line endings, this will never occur.
                self.line_buf.trim_end_matches("\r\n")
            } else if self.line_buf.ends_with('\n') {
                self.line_buf.trim_end_matches('\n')
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "missing line ending",
                ));
            };
            if line.contains('\r') {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "line contains CR",
                ));
            }

            // If this line is the EOF marker, we are done!
            if line == ARMORED_END_MARKER {
                self.found_end = true;
                break;
            }

            // Decode the line
            self.byte_end = base64::decode_config_slice(
                line.as_bytes(),
                base64::URL_SAFE_NO_PAD,
                self.byte_buf.as_mut(),
            )
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

            // Output as much as we can of this line
            if buf.len() <= self.byte_end {
                buf.copy_from_slice(&self.byte_buf[..buf.len()]);
                self.byte_start = buf.len();
                return Ok(buf_len);
            } else {
                buf[..self.byte_end].copy_from_slice(&self.byte_buf[..self.byte_end]);
                buf = &mut buf[self.byte_end..];
            }
        }

        Ok(buf_len - buf.len())
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use super::{ArmoredReader, ArmoredWriter, ARMORED_BYTES_PER_LINE};

    #[test]
    fn armored_round_trip() {
        const MAX_LEN: usize = ARMORED_BYTES_PER_LINE * 50;

        let mut data = Vec::with_capacity(MAX_LEN);

        for i in 0..MAX_LEN {
            data.push(i as u8);

            let mut encoded = vec![];
            {
                let mut out = ArmoredWriter::wrap_output(&mut encoded, true);
                out.write_all(&data).unwrap();
                out.finish().unwrap();
            }

            let mut buf = vec![];
            {
                let mut input = ArmoredReader::from_reader(&encoded[..], true);
                input.read_to_end(&mut buf).unwrap();
            }

            assert_eq!(buf, data);
        }
    }
}