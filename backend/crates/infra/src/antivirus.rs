//! The antivirus of attachments: ClamAV's daemon, `clamd`, over TCP
//! (specification §12, L05-US05).
//!
//! A file is sent as a stream (`INSTREAM`): chunks prefixed with their
//! length, then an empty chunk; clamd answers `stream: OK` or
//! `stream: <signature> FOUND`. Nothing is written on its disk.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// How long clamd may take to answer once the whole file is sent.
const VERDICT_TIMEOUT: Duration = Duration::from_secs(120);
/// How long connecting and sending one chunk may take.
const IO_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest answer read.
const MAX_ANSWER: usize = 4096;

#[derive(Debug, thiserror::Error)]
pub enum AntivirusError {
    #[error("cannot reach clamd: {0}")]
    Io(String),
    #[error("clamd did not answer in time")]
    Timeout,
    #[error("clamd refused the file: {0}")]
    Refused(String),
}

/// What clamd found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Clean,
    /// The name of the signature found.
    Infected(String),
}

/// Reads clamd's answer to `INSTREAM`.
pub fn parse_answer(answer: &str) -> Result<Verdict, AntivirusError> {
    let answer = answer.trim_end_matches(['\0', '\n']).trim();
    let reply = answer.strip_prefix("stream:").map_or(answer, str::trim);
    if reply == "OK" {
        Ok(Verdict::Clean)
    } else if let Some(signature) = reply.strip_suffix(" FOUND") {
        Ok(Verdict::Infected(signature.trim().to_owned()))
    } else {
        // `INSTREAM size limit exceeded. ERROR`, or any other error.
        Err(AntivirusError::Refused(reply.to_owned()))
    }
}

/// A connection streaming one file to clamd.
#[derive(Debug)]
pub struct Scan {
    stream: TcpStream,
}

impl Scan {
    /// Starts scanning a file with the clamd of `address` (`host:port`).
    pub async fn start(address: &str) -> Result<Self, AntivirusError> {
        let mut stream = tokio::time::timeout(IO_TIMEOUT, TcpStream::connect(address))
            .await
            .map_err(|_| AntivirusError::Timeout)?
            .map_err(|e| AntivirusError::Io(e.to_string()))?;
        Self::io(stream.write_all(b"zINSTREAM\0")).await?;
        Ok(Self { stream })
    }

    async fn io<T>(future: impl Future<Output = std::io::Result<T>>) -> Result<T, AntivirusError> {
        tokio::time::timeout(IO_TIMEOUT, future)
            .await
            .map_err(|_| AntivirusError::Timeout)?
            .map_err(|e| AntivirusError::Io(e.to_string()))
    }

    /// Sends the next part of the file.
    pub async fn send(&mut self, chunk: &[u8]) -> Result<(), AntivirusError> {
        // clamd reads chunks of at most 4 GB; ours are far smaller.
        for part in chunk.chunks(1 << 20) {
            let length = u32::try_from(part.len()).unwrap_or(u32::MAX);
            Self::io(self.stream.write_all(&length.to_be_bytes())).await?;
            Self::io(self.stream.write_all(part)).await?;
        }
        Ok(())
    }

    /// Ends the file; clamd's verdict.
    pub async fn finish(mut self) -> Result<Verdict, AntivirusError> {
        Self::io(self.stream.write_all(&0u32.to_be_bytes())).await?;
        let mut answer = Vec::new();
        let read = async {
            let mut buffer = [0u8; 512];
            loop {
                let n = self.stream.read(&mut buffer).await?;
                if n == 0 || answer.len() > MAX_ANSWER {
                    break;
                }
                answer.extend_from_slice(&buffer[..n]);
                if answer.contains(&0) {
                    break;
                }
            }
            std::io::Result::Ok(())
        };
        tokio::time::timeout(VERDICT_TIMEOUT, read)
            .await
            .map_err(|_| AntivirusError::Timeout)?
            .map_err(|e| AntivirusError::Io(e.to_string()))?;
        parse_answer(&String::from_utf8_lossy(&answer))
    }
}

/// Scans `content` with the clamd of `address`.
pub async fn scan(address: &str, content: &[u8]) -> Result<Verdict, AntivirusError> {
    let mut scan = Scan::start(address).await?;
    scan.send(content).await?;
    scan.finish().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_read_as_verdicts() {
        assert_eq!(parse_answer("stream: OK\0").unwrap(), Verdict::Clean);
        assert_eq!(
            parse_answer("stream: Eicar-Test-Signature FOUND\0").unwrap(),
            Verdict::Infected("Eicar-Test-Signature".to_owned())
        );
        assert!(matches!(
            parse_answer("INSTREAM size limit exceeded. ERROR\0"),
            Err(AntivirusError::Refused(_))
        ));
    }
}
