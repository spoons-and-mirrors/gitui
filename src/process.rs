use std::{
    io::{self, Read, Write},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) stdout_bytes: usize,
    pub(crate) stderr_bytes: usize,
    pub(crate) timeout: Duration,
}

impl Limits {
    pub(crate) const fn new(stdout_bytes: usize, stderr_bytes: usize, timeout: Duration) -> Self {
        Self {
            stdout_bytes,
            stderr_bytes,
            timeout,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Output {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr_truncated: bool,
    pub(crate) timed_out: bool,
}

pub(crate) fn run(command: &mut Command, limits: Limits) -> io::Result<Output> {
    run_inner(command, None, limits)
}

pub(crate) fn run_with_input(
    command: &mut Command,
    input: Vec<u8>,
    limits: Limits,
) -> io::Result<Output> {
    run_inner(command, Some(input), limits)
}

fn run_inner(command: &mut Command, input: Option<Vec<u8>>, limits: Limits) -> io::Result<Output> {
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("child stdout was unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("child stderr was unavailable"))?;
    let stdout_limit = limits.stdout_bytes;
    let stderr_limit = limits.stderr_bytes;
    let stdout_reader = thread::spawn(move || read_bounded(stdout, stdout_limit));
    let stderr_reader = thread::spawn(move || read_bounded(stderr, stderr_limit));
    let input_writer = input.map(|input| {
        let mut stdin = child.stdin.take().expect("piped stdin was requested");
        thread::spawn(move || stdin.write_all(&input))
    });

    let started = Instant::now();
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait()? {
            break (status, false);
        }
        if started.elapsed() >= limits.timeout {
            let _ = child.kill();
            break (child.wait()?, true);
        }
        thread::sleep(Duration::from_millis(10));
    };

    if let Some(writer) = input_writer {
        let _ = writer
            .join()
            .map_err(|_| io::Error::other("child stdin writer panicked"))?;
    }
    let (stdout, stdout_truncated) = stdout_reader
        .join()
        .map_err(|_| io::Error::other("child stdout reader panicked"))??;
    let (stderr, stderr_truncated) = stderr_reader
        .join()
        .map_err(|_| io::Error::other("child stderr reader panicked"))??;

    Ok(Output {
        status,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
        timed_out,
    })
}

fn read_bounded(mut reader: impl Read, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let mut retained = Vec::with_capacity(limit.min(64 * 1024));
    let mut buffer = [0_u8; 16 * 1024];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let available = limit.saturating_sub(retained.len());
        let keep = available.min(read);
        retained.extend_from_slice(&buffer[..keep]);
        truncated |= keep < read;
    }
    Ok((retained, truncated))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::read_bounded;

    #[test]
    fn drains_input_while_retaining_only_the_limit() {
        let (retained, truncated) = read_bounded(Cursor::new(b"abcdef"), 4).unwrap();

        assert_eq!(retained, b"abcd");
        assert!(truncated);
    }
}
