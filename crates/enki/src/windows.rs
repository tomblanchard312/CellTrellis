//! Windows transport for the same bounded protocol used on Unix.
#![allow(clippy::disallowed_methods)] // Native-only polling; never built for wasm.

use super::{Duration, Error, PathBuf, find_daemon};
use interprocess::os::windows::named_pipe::{DuplexPipeStream, pipe_mode};
type DuplexBytePipeStream = DuplexPipeStream<pipe_mode::Bytes>;
use std::io::{self, Read, Write};
use std::os::windows::{fs::OpenOptionsExt, io::OwnedHandle};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use winsafe::{HPROCESS, TokenInfo, co};

fn process_sid(process: &HPROCESS) -> Result<String, Error> {
    let token = process.OpenProcessToken(co::TOKEN::QUERY).map_err(|e| io::Error::other(e.to_string()))?;
    let info = token.GetTokenInformation(co::TOKEN_INFORMATION_CLASS::User).map_err(|e| io::Error::other(e.to_string()))?;
    let TokenInfo::User(user) = info else { return Err(Error::Invalid("Cannot identify the Enki account".into())) };
    let sid = user.User.Sid().ok_or_else(|| Error::Invalid("Missing Enki account SID".into()))?;
    winsafe::ConvertSidToStringSid(sid).map_err(|e| io::Error::other(e.to_string()).into())
}

fn current_sid() -> Result<String, Error> {
    process_sid(&HPROCESS::GetCurrentProcess())
}

fn validate(path: &Path, sid: &str) -> Result<(), Error> {
    let prefix = format!(r"\\.\pipe\SharedAI-{sid}-");
    let suffix = path.to_str().and_then(|p| p.strip_prefix(&prefix));
    if suffix.is_some_and(|s| !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')) {
        Ok(())
    } else {
        Err(Error::Invalid("Enki must use a local pipe belonging to your Windows account".into()))
    }
}

pub fn endpoint(configured: &str) -> Result<PathBuf, Error> {
    let sid = current_sid()?;
    let path = if !configured.trim().is_empty() {
        configured.trim().into()
    } else {
        std::env::var_os("SHAREDAI_PIPE")
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| format!(r"\\.\pipe\SharedAI-{sid}-default").into())
    };
    validate(&path, &sid)?;
    Ok(path)
}

pub struct Stream {
    pipe: Arc<DuplexBytePipeStream>,
    read_timeout: Duration,
    write_timeout: Duration,
}

impl Stream {
    pub fn connect(path: &Path) -> Result<Self, Error> {
        let sid = current_sid()?;
        validate(path, &sid)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let file = loop {
            // OVERLAPPED + identification-only SQOS: the server cannot impersonate this app.
            let result =
                std::fs::OpenOptions::new().access_mode(0xc0000100).share_mode(3).custom_flags(0x40000000 | 0x00100000 | 0x00010000).open(path);
            match result {
                Err(e) if e.raw_os_error() == Some(231) && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                other => break other?,
            }
        };
        let pipe = DuplexBytePipeStream::try_from(OwnedHandle::from(file)).map_err(|e| io::Error::other(e.to_string()))?;
        let process = HPROCESS::OpenProcess(co::PROCESS::QUERY_LIMITED_INFORMATION, false, pipe.server_process_id()?)
            .map_err(|e| io::Error::other(e.to_string()))?;
        if process_sid(&process)? != sid {
            return Err(Error::Invalid("Enki is running under a different Windows account".into()));
        }
        pipe.set_nonblocking(true)?;
        Ok(Self { pipe: Arc::new(pipe), read_timeout: Duration::from_secs(5), write_timeout: Duration::from_secs(30) })
    }

    pub fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        self.read_timeout = timeout.unwrap_or(Duration::from_secs(5));
        Ok(())
    }
    pub fn set_write_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        self.write_timeout = timeout.unwrap_or(Duration::from_secs(30));
        Ok(())
    }
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self { pipe: self.pipe.clone(), read_timeout: self.read_timeout, write_timeout: self.write_timeout })
    }
}

fn pending(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock || error.raw_os_error() == Some(232)
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let deadline = Instant::now() + self.read_timeout;
        loop {
            match (&*self.pipe).read(buf) {
                Err(e) if pending(&e) => {
                    if Instant::now() >= deadline {
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                other => return other,
            }
        }
    }
}
impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let deadline = Instant::now() + self.write_timeout;
        loop {
            match (&*self.pipe).write(buf) {
                Ok(0) => {}
                Err(e) if pending(&e) => {}
                other => return other,
            }
            if Instant::now() >= deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        // Closing a cancelled request must not wait for an unresponsive peer to drain the pipe.
        self.pipe.assume_flushed();
    }
}

pub fn ensure_running(endpoint: &Path, _: Option<&Path>, envs: &[(&str, &std::ffi::OsStr)]) -> Result<(), Error> {
    use std::os::windows::process::CommandExt;
    validate(endpoint, &current_sid()?)?;
    if Stream::connect(endpoint).is_ok() {
        return Ok(());
    }
    let daemon = find_daemon().ok_or(Error::NotInstalled)?;
    let mut command = std::process::Command::new(daemon);
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).creation_flags(0x08000000);
    for (key, value) in envs {
        command.env(key, value);
    }
    command.env("SHAREDAI_PIPE", endpoint);
    let mut child = command.spawn().map_err(|e| Error::StartFailed(e.to_string()))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if Stream::connect(endpoint).is_ok() {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            return Err(Error::StartFailed(format!("exited with {status}")));
        }
        if Instant::now() >= deadline {
            return Err(Error::StartFailed("not listening after 10 s".into()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests;
