use std::{
    collections::HashSet,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use curve25519_dalek::{edwards::CompressedEdwardsY, traits::IsIdentity};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use uclip_core::{AppError, ErrorCode};

use super::{
    lan::{derive, discovery_socket, GROUP, PORT},
    transport::{invalid, unavailable},
    wire::Wire,
    Profile,
};

const OFFER: &[u8] = b"UClip Pair v1";
const OFFER_BYTES: usize = OFFER.len() + 2;
const MAX_ATTEMPTS: usize = 8;
const MAX_OFFERS: usize = 6;
const MAX_PROFILE: usize = 16_384;
const OFFER_LIFETIME: Duration = Duration::from_secs(120);
const DISCOVERY_TIME: Duration = Duration::from_millis(900);
const TICK: Duration = Duration::from_millis(50);

pub(super) struct Offer {
    stop: Arc<AtomicBool>,
    pin: String,
    expires: Instant,
}

impl Offer {
    pub fn matches(&self, text: &str) -> bool {
        self.expires > Instant::now() && self.pin == text
    }
}

impl Drop for Offer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn authentication() -> AppError {
    AppError::new(ErrorCode::SyncAuthentication)
}

fn pin() -> Result<String, AppError> {
    const RANGE: u32 = 100_000_000;
    loop {
        let mut bytes = [0; 4];
        getrandom::fill(&mut bytes).map_err(|_| unavailable())?;
        let value = u32::from_be_bytes(bytes);
        if value < u32::MAX - (u32::MAX % RANGE) {
            return Ok(format!("{:08}", value % RANGE));
        }
    }
}

fn channel(mut stream: TcpStream, pin: &str, initiator: bool) -> Result<Wire, AppError> {
    let timeout = Some(Duration::from_secs(2));
    stream
        .set_read_timeout(timeout)
        .and_then(|()| stream.set_write_timeout(timeout))
        .and_then(|()| stream.set_nodelay(true))
        .map_err(|_| unavailable())?;
    let password = Password::new(pin.as_bytes());
    let client = Identity::new(b"UClip pairing client v1");
    let host = Identity::new(b"UClip pairing host v1");
    let (state, outbound) = if initiator {
        Spake2::<Ed25519Group>::start_a(&password, &client, &host)
    } else {
        Spake2::<Ed25519Group>::start_b(&password, &client, &host)
    };
    let mut inbound = [0; 33];
    if initiator {
        stream.write_all(&outbound).map_err(|_| unavailable())?;
    }
    stream.read_exact(&mut inbound).map_err(|_| unavailable())?;
    let peer = CompressedEdwardsY::from_slice(&inbound[1..]).map_err(|_| authentication())?;
    // The exchange uses the prime-order subgroup and canonical wire encodings.
    if !peer.decompress().is_some_and(|point| {
        point.compress() == peer && point.is_torsion_free() && !point.is_identity()
    }) {
        return Err(authentication());
    }
    if !initiator {
        stream.write_all(&outbound).map_err(|_| unavailable())?;
    }
    let secret = state.finish(&inbound).map_err(|_| authentication())?;
    let secret = <[u8; 32]>::try_from(secret.as_slice()).map_err(|_| authentication())?;
    // The Noise handshake confirms both SPAKE2 shares before any profile is sent.
    Wire::pairing(
        stream,
        &derive(&secret, b"uclip/pairing/transport/v1")?,
        initiator,
    )
}

pub(super) fn start(profile: Profile) -> Result<(Offer, String), AppError> {
    let pin = pin()?;
    let secret = pin.clone();
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(|_| unavailable())?;
    listener.set_nonblocking(true).map_err(|_| unavailable())?;
    let port = listener.local_addr().map_err(|_| unavailable())?.port();
    let udp = discovery_socket(PORT).map_err(|_| unavailable())?;
    let mut packet = [0; OFFER_BYTES];
    packet[..OFFER.len()].copy_from_slice(OFFER);
    packet[OFFER.len()..].copy_from_slice(&port.to_be_bytes());
    let cancelled = Arc::new(AtomicBool::new(false));
    let stop = cancelled.clone();
    let profile = Arc::new(profile);
    let expires = Instant::now() + OFFER_LIFETIME;
    thread::Builder::new()
        .name("uclip-pairing-offer".into())
        .spawn(move || {
            let group = SocketAddrV4::new(GROUP, PORT);
            let mut announce = Instant::now();
            let mut attempts = 0;
            while !stop.load(Ordering::Acquire) && Instant::now() < expires {
                if attempts < MAX_ATTEMPTS {
                    if Instant::now() >= announce {
                        let _ = udp.send_to(&packet, group);
                        announce = Instant::now() + Duration::from_millis(200);
                    }
                    if let Ok((stream, _)) = listener.accept() {
                        attempts += 1;
                        let stop = stop.clone();
                        let profile = profile.clone();
                        let secret = secret.clone();
                        let _ = thread::Builder::new()
                            .name("uclip-pairing-session".into())
                            .spawn(move || {
                                if let Ok(mut wire) = channel(stream, &secret, false) {
                                    if Instant::now() < expires
                                        && !stop.swap(true, Ordering::AcqRel)
                                    {
                                        let _ = wire.send(&*profile, MAX_PROFILE);
                                    }
                                }
                            });
                    }
                }
                thread::sleep(TICK);
            }
        })
        .map_err(|_| unavailable())?;
    Ok((
        Offer {
            stop: cancelled,
            pin: pin.clone(),
            expires,
        },
        pin,
    ))
}

pub(super) fn receive(pin: &str) -> Result<Profile, AppError> {
    if pin.len() != 8 || !pin.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    let udp = discovery_socket(PORT).map_err(|_| unavailable())?;
    let mut endpoints = HashSet::new();
    let expires = Instant::now() + DISCOVERY_TIME;
    while Instant::now() < expires && endpoints.len() < MAX_OFFERS {
        let mut packet = [0; OFFER_BYTES + 1];
        match udp.recv_from(&mut packet) {
            Ok((OFFER_BYTES, source)) if source.is_ipv4() && packet.starts_with(OFFER) => {
                let port = u16::from_be_bytes([packet[OFFER.len()], packet[OFFER.len() + 1]]);
                if port != 0 {
                    endpoints.insert(SocketAddr::new(source.ip(), port));
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(TICK),
            Err(_) => return Err(unavailable()),
        }
    }
    if endpoints.is_empty() {
        return Err(unavailable());
    }
    for address in endpoints {
        if let Ok(stream) = TcpStream::connect_timeout(&address, Duration::from_secs(1)) {
            if let Ok(mut wire) = channel(stream, pin, true) {
                if let Ok(profile) = wire.receive(MAX_PROFILE) {
                    return Ok(profile);
                }
            }
        }
    }
    Err(authentication())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_encrypted_channel_requires_the_same_pin() {
        for (client_pin, accepted) in [("01234567", true), ("01234568", false)] {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                channel(stream, "01234567", false).is_ok()
            });
            let stream = TcpStream::connect(address).unwrap();
            assert_eq!(channel(stream, client_pin, true).is_ok(), accepted);
            assert_eq!(server.join().unwrap(), accepted);
        }
    }
}
