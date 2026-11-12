//! Finds the slowest stretch of input in one run of the engine.
//!
//! ```sh
//! cargo run --release --example row_latency -- input.csv
//! ```
//!
//! An insert that grows the transaction table rehashes every stored record,
//! so the cost of a row is constant only on average (ADR-0011, Hostile
//! input). This example shows how long one growth step takes.
//!
//! It calls the library's public `run` on the file, with stdout and stderr
//! discarded, through a `Read` wrapper that times the gap between two reads.
//! The engine's reader asks for one fixed-size chunk per read (8 KiB, a few
//! hundred rows), and processes the whole chunk before it asks again, so the
//! gap before a read is the time spent on the rows of the previous chunk.
//! The result is per chunk, not per row: an upper bound on the slowest row
//! that contains it. The engine itself is not changed or exposed.
//!
//! The report goes to stdout: total time, the number of chunks, the median
//! chunk, and the slowest chunks with the byte offset where each starts.

use std::cmp::Reverse;
use std::env;
use std::fs::File;
use std::io::{self, Read, Write};
use std::process::ExitCode;
use std::time::{Duration, Instant};

/// How many of the slowest chunks to report.
const SLOWEST: usize = 5;

/// One chunk of input: where it starts and how long its rows took.
#[derive(Debug, Clone, Copy)]
struct Chunk {
    offset: u64,
    took: Duration,
}

/// A `Read` that records, for every chunk, the time until the next read.
struct Timed<R> {
    inner: R,
    /// Bytes read so far.
    offset: u64,
    /// The chunk last returned and the time it was returned.
    pending: Option<(u64, Instant)>,
    /// Every chunk's duration; one entry per read, so a few per MiB.
    chunks: Vec<Chunk>,
}

impl<R: Read> Timed<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            offset: 0,
            pending: None,
            chunks: Vec::new(),
        }
    }

    /// Closes the chunk last returned, if any.
    fn close_pending(&mut self, now: Instant) {
        if let Some((offset, returned)) = self.pending.take() {
            let took = now.saturating_duration_since(returned);
            self.chunks.push(Chunk { offset, took });
        }
    }
}

impl<R: Read> Read for Timed<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.close_pending(Instant::now());
        let read = self.inner.read(buf)?;
        let start = self.offset;
        self.offset = self
            .offset
            .saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if read > 0 {
            self.pending = Some((start, Instant::now()));
        }
        Ok(read)
    }
}

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    let (Some(path), None) = (args.next(), args.next()) else {
        let _ = writeln!(io::stderr(), "usage: row_latency <input.csv>");
        return ExitCode::from(64);
    };
    match measure(File::open(path)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr(), "row_latency: {error}");
            ExitCode::from(74)
        }
    }
}

fn measure(file: io::Result<File>) -> Result<(), Box<dyn std::error::Error>> {
    let mut timed = Timed::new(file?);
    let started = Instant::now();
    rust_transactions_practice::run(&mut timed, io::sink(), io::sink())?;
    let total = started.elapsed();
    // The last chunk ends when `run` stops reading.
    timed.close_pending(Instant::now());

    let mut chunks = timed.chunks;
    chunks.sort_by_key(|chunk| Reverse(chunk.took));
    let median = chunks
        .get(chunks.len() / 2)
        .map_or(Duration::ZERO, |c| c.took);

    let mut out = io::stdout().lock();
    writeln!(out, "total: {total:?}")?;
    writeln!(out, "chunks: {}", chunks.len())?;
    writeln!(out, "median chunk: {median:?}")?;
    for chunk in chunks.iter().take(SLOWEST) {
        writeln!(out, "slow chunk: {:?} at byte {}", chunk.took, chunk.offset)?;
    }
    Ok(())
}
