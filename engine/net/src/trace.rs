//! Opt-in local trace files with a hard lifetime byte budget.
//!
//! Callers supply privacy-reviewed records. This facility never uploads data and
//! never appends to an existing file, including a symlink to another file.
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

pub struct TraceFile {
    file: File,
    limit: usize,
    written: usize,
    stopped: bool,
}

impl TraceFile {
    pub fn create(path: impl AsRef<Path>, limit: usize) -> io::Result<Self> {
        if !(1..=64 * 1024 * 1024).contains(&limit) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "trace budget must be 1 byte to 64 MiB",
            ));
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        Ok(Self {
            file: options.open(path)?,
            limit,
            written: 0,
            stopped: false,
        })
    }

    /// A record either fits in the remaining budget or writes no bytes. An I/O
    /// failure permanently stops this file so retries cannot exceed the budget.
    pub fn write_record(&mut self, encoded: &[u8]) -> io::Result<()> {
        if self.stopped || encoded.len() > self.limit - self.written {
            self.stopped = true;
            return Err(io::Error::other(
                "trace byte budget exhausted or writer stopped",
            ));
        }
        if let Err(error) = self.file.write_all(encoded) {
            self.stopped = true;
            return Err(error);
        }
        self.written += encoded.len();
        Ok(())
    }

    pub fn bytes_written(&self) -> usize {
        self.written
    }
}

/// A finite append-only sequence of trace files. The first file is `path`,
/// subsequent files append `.0001`, `.0002`, and so on to the complete filename.
/// Every file uses TraceFile's private, exclusive creation and byte budget.
pub struct SegmentedTraceFile {
    path: PathBuf,
    file: TraceFile,
    segment_limit: usize,
    segments: u16,
    position: TracePosition,
    written: usize,
    stopped: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TracePosition {
    pub sequence: u64,
    pub segment: u16,
}
impl SegmentedTraceFile {
    pub fn create(path: impl AsRef<Path>, segment_limit: usize, segments: u16) -> io::Result<Self> {
        if !(1..=128).contains(&segments) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "trace segment count must be 1 to 128",
            ));
        }
        segment_limit
            .checked_mul(usize::from(segments))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "total trace budget overflows platform capacity",
                )
            })?;
        Ok(Self {
            path: path.as_ref().into(),
            file: TraceFile::create(path, segment_limit)?,
            segment_limit,
            segments,
            position: TracePosition {
                sequence: 1,
                segment: 0,
            },
            written: 0,
            stopped: false,
        })
    }
    pub fn next_position(&self) -> TracePosition {
        self.position
    }
    pub fn bytes_written(&self) -> usize {
        self.written
    }
    /// Encode a record with its actual output position. A rotation may invoke
    /// the encoder twice with the same sequence and the next segment number.
    /// Encoders must have no side effects. Any failure permanently stops output;
    /// callers must report failure rather than dropping a sample or retrying.
    pub fn write_record(
        &mut self,
        mut encode: impl FnMut(TracePosition) -> io::Result<Vec<u8>>,
    ) -> io::Result<()> {
        if self.stopped {
            return Err(io::Error::other("segmented trace writer stopped"));
        }
        let result = self.write_next(&mut encode);
        if result.is_err() {
            self.stopped = true;
        }
        result
    }
    fn write_next(
        &mut self,
        encode: &mut impl FnMut(TracePosition) -> io::Result<Vec<u8>>,
    ) -> io::Result<()> {
        let next = self
            .position
            .sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::other("trace sample sequence exhausted"))?;
        let mut bytes = encode(self.position)?;
        if bytes.is_empty() || bytes.len() > self.segment_limit {
            return Err(io::Error::other(
                "trace record exceeds segment byte budget or is empty",
            ));
        }
        if bytes.len() > self.segment_limit - self.file.bytes_written() {
            let segment = self.position.segment + 1;
            if segment >= self.segments {
                return Err(io::Error::other("trace segment budget exhausted"));
            }
            let position = TracePosition {
                sequence: self.position.sequence,
                segment,
            };
            bytes = encode(position)?;
            if bytes.is_empty() || bytes.len() > self.segment_limit {
                return Err(io::Error::other(
                    "trace record exceeds segment byte budget or is empty",
                ));
            }
            let mut path = self.path.as_os_str().to_os_string();
            path.push(format!(".{segment:04}"));
            // Never truncate, overwrite, delete, or wrap an older segment.
            self.file = TraceFile::create(PathBuf::from(path), self.segment_limit)?;
            self.position = position;
        }
        self.file.write_record(&bytes)?;
        self.written += bytes.len();
        self.position.sequence = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trace_is_private_bounded_and_never_overwrites_or_appends() {
        let path = std::env::temp_dir().join(format!(
            "network-trace-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut trace = TraceFile::create(&path, 4).unwrap();
        trace.write_record(b"{}\n").unwrap();
        assert_eq!(trace.bytes_written(), 3);
        assert!(trace.write_record(b"{}\n").is_err());
        assert!(trace.write_record(b"x").is_err());
        assert!(TraceFile::create(&path, 1024).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{}\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_file(path).unwrap();
    }
    fn segment_path() -> PathBuf {
        static NEXT_PATH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "trace-segments-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_PATH.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }
    #[test]
    fn segments_rotate_at_record_boundary_keep_sequence_and_stop_at_total_cap() {
        let path = segment_path();
        let mut trace = SegmentedTraceFile::create(&path, 8, 2).unwrap();
        let encode = |p: TracePosition| Ok(format!("{}:{}\n", p.sequence, p.segment).into_bytes());
        for _ in 0..4 {
            trace.write_record(encode).unwrap();
        }
        assert_eq!(std::fs::read(&path).unwrap(), b"1:0\n2:0\n");
        assert_eq!(
            std::fs::read(format!("{}.0001", path.display())).unwrap(),
            b"3:1\n4:1\n"
        );
        assert_eq!(trace.bytes_written(), 16);
        assert_eq!(
            trace.next_position(),
            TracePosition {
                sequence: 5,
                segment: 1
            }
        );
        assert!(trace.write_record(encode).is_err());
        assert!(trace.write_record(|_| Ok(b"x".to_vec())).is_err());
        assert!(!PathBuf::from(format!("{}.0002", path.display())).exists());
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(format!("{}.0001", path.display())).unwrap();
    }
    #[test]
    fn segment_collision_and_encoder_errors_never_overwrite_or_skip() {
        let path = segment_path();
        let next = format!("{}.0001", path.display());
        std::fs::write(&next, b"existing").unwrap();
        let mut trace = SegmentedTraceFile::create(&path, 3, 2).unwrap();
        trace.write_record(|_| Ok(b"one".to_vec())).unwrap();
        assert!(trace.write_record(|_| Ok(b"two".to_vec())).is_err());
        assert_eq!(trace.next_position().sequence, 2);
        assert_eq!(std::fs::read(&next).unwrap(), b"existing");
        assert_eq!(std::fs::read(&path).unwrap(), b"one");
        assert!(trace.write_record(|_| Ok(b"x".to_vec())).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(next).unwrap();
        let mut trace = SegmentedTraceFile::create(&path, 8, 2).unwrap();
        assert!(
            trace
                .write_record(|_| Err(io::Error::other("encode failed")))
                .is_err()
        );
        assert!(trace.write_record(|_| Ok(b"valid".to_vec())).is_err());
        assert_eq!(trace.next_position().sequence, 1);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        std::fs::remove_file(path).unwrap();
    }
}
