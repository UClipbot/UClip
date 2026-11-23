use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use serde::{de::DeserializeOwned, Serialize};
use snow::{Builder, TransportState};
use uclip_core::{AppError, ErrorCode};

use super::transport::{decode, encode, invalid, limit, unavailable};

const NOISE_SUITE: &str = "Noise_NNpsk0_25519_ChaChaPoly_SHA256";
const CHUNK: usize = 32 * 1024;
const TAG: usize = 16;
const STEP_TIMEOUT: Duration = Duration::from_secs(6);
const SESSION_TIMEOUT: Duration = Duration::from_secs(120);

pub(super) struct Wire {
    stream: TcpStream,
    noise: TransportState,
    expires: Instant,
    scratch: Vec<u8>,
}

fn read_frame(stream: &mut TcpStream, buffer: &mut [u8]) -> Result<usize, AppError> {
    let mut length = [0; 2];
    stream.read_exact(&mut length).map_err(|_| unavailable())?;
    let length = u16::from_be_bytes(length) as usize;
    if length == 0 || length > buffer.len() {
        return Err(invalid());
    }
    stream
        .read_exact(&mut buffer[..length])
        .map_err(|_| unavailable())?;
    Ok(length)
}

fn write_frame(stream: &mut TcpStream, payload: &[u8]) -> Result<(), AppError> {
    let length = u16::try_from(payload.len())
        .map_err(|_| limit())?
        .to_be_bytes();
    stream
        .write_all(&length)
        .and_then(|()| stream.write_all(payload))
        .map_err(|_| unavailable())
}

fn authentication() -> AppError {
    AppError::new(ErrorCode::SyncAuthentication)
}

impl Wire {
    pub fn connect(address: SocketAddr, key: &[u8; 32]) -> Result<Self, AppError> {
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
            .map_err(|_| unavailable())?;
        Self::handshake(stream, key, true)
    }

    pub fn accept(stream: TcpStream, key: &[u8; 32]) -> Result<Self, AppError> {
        Self::handshake(stream, key, false)
    }

    fn handshake(mut stream: TcpStream, key: &[u8; 32], initiator: bool) -> Result<Self, AppError> {
        stream
            .set_read_timeout(Some(STEP_TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(STEP_TIMEOUT)))
            .and_then(|()| stream.set_nodelay(true))
            .map_err(|_| unavailable())?;
        let builder = Builder::new(NOISE_SUITE.parse().map_err(|_| invalid())?)
            .prologue(b"uclip-lan/v4")
            .map_err(|_| invalid())?
            .psk(0, key)
            .map_err(|_| authentication())?;
        let mut handshake = if initiator {
            builder.build_initiator()
        } else {
            builder.build_responder()
        }
        .map_err(|_| authentication())?;
        let mut frame = [0; 128];
        for sending in [initiator, !initiator] {
            if sending {
                let count = handshake
                    .write_message(&[], &mut frame)
                    .map_err(|_| authentication())?;
                write_frame(&mut stream, &frame[..count])?;
            } else {
                let count = read_frame(&mut stream, &mut frame)?;
                let mut payload = [0; 128];
                if handshake
                    .read_message(&frame[..count], &mut payload)
                    .map_err(|_| authentication())?
                    != 0
                {
                    return Err(invalid());
                }
            }
        }
        Ok(Self {
            stream,
            noise: handshake
                .into_transport_mode()
                .map_err(|_| authentication())?,
            expires: Instant::now() + SESSION_TIMEOUT,
            scratch: vec![0; CHUNK + TAG],
        })
    }

    fn step(&self, sending: bool) -> Result<(), AppError> {
        let remaining = self
            .expires
            .saturating_duration_since(Instant::now())
            .min(STEP_TIMEOUT);
        if remaining.is_zero() {
            return Err(unavailable());
        }
        if sending {
            self.stream.set_write_timeout(Some(remaining))
        } else {
            self.stream.set_read_timeout(Some(remaining))
        }
        .map_err(|_| unavailable())
    }

    fn send_chunk(&mut self, plaintext: &[u8]) -> Result<(), AppError> {
        self.step(true)?;
        let length = self
            .noise
            .write_message(plaintext, &mut self.scratch)
            .map_err(|_| invalid())?;
        write_frame(&mut self.stream, &self.scratch[..length])
    }

    fn receive_chunk(&mut self, buffer: &mut [u8]) -> Result<(), AppError> {
        self.step(false)?;
        let length = read_frame(&mut self.stream, &mut self.scratch)?;
        if length != buffer.len() + TAG {
            return Err(invalid());
        }
        let count = self
            .noise
            .read_message(&self.scratch[..length], buffer)
            .map_err(|_| invalid())?;
        if count != buffer.len() {
            return Err(invalid());
        }
        Ok(())
    }

    pub fn send<T: Serialize>(&mut self, message: &T, bound: usize) -> Result<(), AppError> {
        let data = encode(message, bound)?;
        let length = u32::try_from(data.len())
            .map_err(|_| limit())?
            .to_be_bytes();
        self.send_chunk(&length)?;
        for chunk in data.chunks(CHUNK) {
            self.send_chunk(chunk)?;
        }
        Ok(())
    }

    pub fn receive<T: DeserializeOwned>(&mut self, bound: usize) -> Result<T, AppError> {
        let mut length = [0; 4];
        self.receive_chunk(&mut length)?;
        let length = u32::from_be_bytes(length) as usize;
        if length > bound {
            return Err(limit());
        }
        let mut data = vec![0; length];
        for chunk in data.chunks_mut(CHUNK) {
            self.receive_chunk(chunk)?;
        }
        decode(&data)
    }

    pub fn send_file(
        &mut self,
        input: &mut impl Read,
        mut bytes: u64,
        stop: &AtomicBool,
    ) -> Result<(), AppError> {
        let mut buffer = [0; CHUNK];
        while bytes != 0 {
            if stop.load(Ordering::Acquire) {
                return Err(unavailable());
            }
            let count = bytes.min(CHUNK as u64) as usize;
            input
                .read_exact(&mut buffer[..count])
                .map_err(|_| AppError::new(ErrorCode::MissingFile))?;
            self.send_chunk(&buffer[..count])?;
            bytes -= count as u64;
        }
        Ok(())
    }

    pub fn receive_file(
        &mut self,
        output: &mut impl Write,
        mut bytes: u64,
    ) -> Result<(), AppError> {
        let mut buffer = [0; CHUNK];
        while bytes != 0 {
            let count = bytes.min(CHUNK as u64) as usize;
            self.receive_chunk(&mut buffer[..count])?;
            output
                .write_all(&buffer[..count])
                .map_err(|_| AppError::new(ErrorCode::Storage))?;
            bytes -= count as u64;
        }
        Ok(())
    }
}
