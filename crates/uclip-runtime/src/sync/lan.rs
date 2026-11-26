use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use socket2::{Domain, Protocol, Socket, Type};
use uclip_core::{
    AppError, CoreService, ErrorCode, FileTicket, ReplicaEntry, ReplicaStamp, SyncRole,
};
use uuid::Uuid;

use super::{
    files::{self, FileEntry, Received, Source},
    transport::{
        block, invalid, reconcile, unavailable, ReplicaTransport, MAX_INVENTORY, MAX_RECORD,
        MAX_STAMPS,
    },
    wire::Wire,
};

pub(super) const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 79, 67);
pub(super) const PORT: u16 = 41764;
const DISCOVERY: Duration = Duration::from_millis(2200);
const ANNOUNCE: Duration = Duration::from_millis(500);
const LEASE: Duration = Duration::from_secs(4);
const APPEARANCE: &[u8] = b"UClip LAN v4";
const SIGNED: usize = APPEARANCE.len() + 16 + 16 + 2 + 1 + 8;
const PACKET_BYTES: usize = SIGNED + 32;
const MAX_PEERS: usize = 32;
const MAX_CONNECTIONS: usize = 4;

#[derive(Clone)]
struct Peer {
    device: Uuid,
    session: Uuid,
    host: bool,
    address: SocketAddr,
    seen: Instant,
    uptime: Duration,
}

pub(super) struct LanStatus {
    pub role: SyncRole,
    pub peers: u32,
    pub last_sync: Option<u64>,
    pub error: Option<AppError>,
}

struct State {
    peers: HashMap<Uuid, Peer>,
    role: SyncRole,
    last_sync: Option<u64>,
    error: Option<AppError>,
}

pub(super) struct Lan {
    key: [u8; 32],
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
}

#[derive(Serialize, Deserialize)]
enum Request {
    Inventory,
    Fetch(ReplicaStamp),
    Apply(Box<ReplicaEntry>),
    Finish,
    File(String),
}

#[derive(Serialize, Deserialize)]
enum Response {
    Identity(String),
    Inventory(Vec<ReplicaStamp>),
    Entry(Option<Box<ReplicaEntry>>),
    Done,
    Error(ErrorCode),
    Files(Vec<FileEntry>),
}

pub(super) fn derive(key: &[u8; 32], label: &[u8]) -> Result<[u8; 32], AppError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| invalid())?;
    mac.update(label);
    Ok(mac.finalize().into_bytes().into())
}

pub(super) fn discovery_socket(port: u16) -> std::io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).into())?;
    socket.join_multicast_v4(&GROUP, &Ipv4Addr::UNSPECIFIED)?;
    socket.set_multicast_ttl_v4(1)?;
    socket.set_multicast_loop_v4(true)?;
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}

fn announce(
    device: Uuid,
    session: Uuid,
    port: u16,
    host: bool,
    uptime: Duration,
    key: &[u8; 32],
) -> Result<[u8; PACKET_BYTES], AppError> {
    let mut packet = [0; PACKET_BYTES];
    packet[..APPEARANCE.len()].copy_from_slice(APPEARANCE);
    let offset = APPEARANCE.len();
    packet[offset..offset + 16].copy_from_slice(device.as_bytes());
    packet[offset + 16..offset + 32].copy_from_slice(session.as_bytes());
    packet[offset + 32..offset + 34].copy_from_slice(&port.to_be_bytes());
    packet[offset + 34] = u8::from(host);
    let millis = uptime.as_millis().min(u64::MAX.into()) as u64;
    packet[offset + 35..SIGNED].copy_from_slice(&millis.to_be_bytes());
    let tag = derive(key, &packet[..SIGNED])?;
    packet[SIGNED..].copy_from_slice(&tag);
    Ok(packet)
}

fn discover(data: &[u8], source: SocketAddr, key: &[u8; 32]) -> Option<Peer> {
    if data.len() != PACKET_BYTES || !data.starts_with(APPEARANCE) || !source.is_ipv4() {
        return None;
    }
    let (body, tag) = data.split_at(SIGNED);
    let mut mac = Hmac::<Sha256>::new_from_slice(key).ok()?;
    mac.update(body);
    mac.verify_slice(tag).ok()?;
    let offset = APPEARANCE.len();
    let device = Uuid::from_slice(&body[offset..offset + 16]).ok()?;
    let session = Uuid::from_slice(&body[offset + 16..offset + 32]).ok()?;
    let port = u16::from_be_bytes([body[offset + 32], body[offset + 33]]);
    if port == 0 || !matches!(body[offset + 34], 0 | 1) {
        return None;
    }
    Some(Peer {
        device,
        session,
        host: body[offset + 34] == 1,
        address: SocketAddr::new(source.ip(), port),
        seen: Instant::now(),
        uptime: Duration::from_millis(u64::from_be_bytes(body[offset + 35..].try_into().ok()?)),
    })
}

fn elect<'a>(
    peers: impl Iterator<Item = &'a Peer> + Clone,
    mine: Uuid,
    host: bool,
    uptime: Duration,
    now: Instant,
) -> Uuid {
    let established = peers
        .clone()
        .filter(|peer| peer.host)
        .map(|peer| peer.session)
        .chain(host.then_some(mine))
        .min();
    established.unwrap_or_else(|| {
        // Compare observed uptime so host selection never depends on synchronized wall clocks.
        peers
            .map(|peer| {
                (
                    peer.uptime
                        .saturating_add(now.saturating_duration_since(peer.seen)),
                    peer.session,
                )
            })
            .chain([(uptime, mine)])
            .max()
            .map_or(mine, |(_, session)| session)
    })
}

struct AcceptSlot(Arc<AtomicUsize>);

impl Drop for AcceptSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

struct WorkSlot(Arc<AtomicBool>);

impl Drop for WorkSlot {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Lan {
    pub fn start(core: CoreService, device: Uuid, key: [u8; 32]) -> Result<Self, AppError> {
        Self::start_on_port(core, device, key, PORT)
    }

    fn start_on_port(
        core: CoreService,
        device: Uuid,
        key: [u8; 32],
        discovery_port: u16,
    ) -> Result<Self, AppError> {
        let udp = discovery_socket(discovery_port).map_err(|_| unavailable())?;
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(|_| unavailable())?;
        listener.set_nonblocking(true).map_err(|_| unavailable())?;
        let port = listener.local_addr().map_err(|_| unavailable())?.port();
        let signing = derive(&key, b"uclip/discovery/v1")?;
        let key = derive(&key, b"uclip/noise/v1")?;
        let session = Uuid::new_v4();
        let state = Arc::new(Mutex::new(State {
            peers: HashMap::new(),
            role: SyncRole::Discovering,
            last_sync: None,
            error: None,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let service = Self {
            state: state.clone(),
            stop: stop.clone(),
            key,
        };
        thread::Builder::new()
            .name("uclip-lan".into())
            .spawn(move || {
                let started = Instant::now();
                let group = SocketAddrV4::new(GROUP, discovery_port);
                let mut next_announce = started;
                let mut next_exchange = started;
                let serving = Arc::new(AtomicUsize::new(0));
                let working = Arc::new(AtomicBool::new(false));
                let mut host = false;
                while !stop.load(Ordering::Acquire) {
                    let now = Instant::now();
                    let mut buf = [0; PACKET_BYTES + 1];
                    for _ in 0..64 {
                        let Ok((length, address)) = udp.recv_from(&mut buf) else {
                            break;
                        };
                        let Some(peer) = discover(&buf[..length], address, &signing) else {
                            continue;
                        };
                        if peer.device == device {
                            continue;
                        }
                        if let Ok(mut state) = state.lock() {
                            let accept = state
                                .peers
                                .get(&peer.device)
                                .map_or(state.peers.len() < MAX_PEERS, |old| {
                                    old.session != peer.session || old.uptime < peer.uptime
                                });
                            if accept {
                                state.peers.insert(peer.device, peer);
                            }
                        }
                    }
                    let mut target = None;
                    if let Ok(mut current) = state.lock() {
                        current
                            .peers
                            .retain(|_, peer| now.saturating_duration_since(peer.seen) < LEASE);
                        let uptime = now.saturating_duration_since(started);
                        if uptime >= DISCOVERY {
                            let leader = elect(current.peers.values(), session, host, uptime, now);
                            host = leader == session;
                            current.role = if host {
                                SyncRole::Host
                            } else {
                                SyncRole::Client
                            };
                            target = current
                                .peers
                                .values()
                                .find(|peer| peer.session == leader)
                                .cloned();
                        }
                    }
                    if now >= next_announce {
                        if let Ok(frame) = announce(
                            device,
                            session,
                            port,
                            host,
                            now.saturating_duration_since(started),
                            &signing,
                        ) {
                            let _ = udp.send_to(&frame, group);
                        }
                        next_announce = now + ANNOUNCE;
                    }

                    while let Ok((stream, _)) = listener.accept() {
                        if serving.load(Ordering::Acquire) >= MAX_CONNECTIONS {
                            break;
                        }
                        serving.fetch_add(1, Ordering::AcqRel);
                        let permit = AcceptSlot(serving.clone());
                        let core = core.clone();
                        let state = state.clone();
                        let stop = stop.clone();
                        let device = device.to_string();
                        let _ = thread::Builder::new()
                            .name("uclip-lan-session".into())
                            .spawn(move || {
                                let _permit = permit;
                                if serve(stream, &key, &device, &core, &stop)
                                    .is_ok_and(|exchanged| exchanged)
                                {
                                    report(&state, Ok(()));
                                }
                            });
                    }

                    if let Some(peer) = target.filter(|_| now >= next_exchange) {
                        if !working.swap(true, Ordering::AcqRel) {
                            next_exchange = now + Duration::from_secs(1);
                            let permit = WorkSlot(working.clone());
                            let core = core.clone();
                            let state = state.clone();
                            let stop = stop.clone();
                            let _ = thread::Builder::new().name("uclip-lan-merge".into()).spawn(
                                move || {
                                    let _permit = permit;
                                    let result = (|| {
                                        let wire = connect(&peer, &key)?;
                                        let mut remote = LanTransport { wire, stop };
                                        reconcile(&core, &mut remote)
                                    })();
                                    match result {
                                        Ok(false) => {}
                                        result => report(&state, result.map(|_| ())),
                                    }
                                },
                            );
                        }
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            })
            .map_err(|_| unavailable())?;
        Ok(service)
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    pub fn status(&self) -> Result<LanStatus, AppError> {
        let state = self.state.lock().map_err(|_| unavailable())?;
        Ok(LanStatus {
            role: state.role,
            peers: state.peers.len() as u32,
            last_sync: state.last_sync,
            error: state.error.clone(),
        })
    }

    pub fn file(
        &self,
        ticket: &FileTicket,
        destination: &Path,
        budget: u64,
    ) -> Result<Received, AppError> {
        let device: Uuid = ticket.device.parse().map_err(|_| invalid())?;
        let peer = {
            let state = self.state.lock().map_err(|_| unavailable())?;
            state
                .peers
                .get(&device)
                .filter(|peer| peer.seen.elapsed() < LEASE)
                .cloned()
                .ok_or_else(offline)?
        };
        let mut wire = connect(&peer, &self.key).map_err(|error| {
            if error.code == ErrorCode::SyncConnection {
                offline()
            } else {
                error
            }
        })?;
        wire.send(&Request::File(ticket.token.clone()), MAX_RECORD)?;
        match response(&mut wire, MAX_INVENTORY)? {
            Response::Files(entries) => {
                let received = files::receive(destination, entries, &mut wire, budget)?;
                if self.stop.load(Ordering::Acquire) {
                    return Err(offline());
                }
                if !matches!(response(&mut wire, MAX_INVENTORY)?, Response::Done) {
                    return Err(invalid());
                }
                Ok(received)
            }
            _ => Err(invalid()),
        }
    }
}

impl Drop for Lan {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

pub(super) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn offline() -> AppError {
    AppError::new(ErrorCode::FilePeerOffline)
}

fn report(state: &Mutex<State>, result: Result<(), AppError>) {
    if let Ok(mut state) = state.lock() {
        if result.is_ok() {
            state.last_sync = Some(now());
        }
        state.error = result.err();
    }
}

fn connect(peer: &Peer, key: &[u8; 32]) -> Result<Wire, AppError> {
    let mut wire = Wire::connect(peer.address, key)?;
    if !matches!(response(&mut wire, MAX_INVENTORY)?, Response::Identity(id) if id == peer.device.to_string())
    {
        return Err(AppError::new(ErrorCode::SyncAuthentication));
    }
    Ok(wire)
}

fn response(wire: &mut Wire, bound: usize) -> Result<Response, AppError> {
    match wire.receive::<Response>(bound)? {
        Response::Error(code) => Err(AppError::new(code)),
        response => Ok(response),
    }
}

fn serve(
    stream: TcpStream,
    key: &[u8; 32],
    device: &str,
    core: &CoreService,
    stop: &AtomicBool,
) -> Result<bool, AppError> {
    let mut wire = Wire::accept(stream, key)?;
    wire.send(&Response::Identity(device.into()), MAX_INVENTORY)?;
    for _ in 0..(MAX_STAMPS * 2 + 4) {
        if stop.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        let request: Request = wire.receive(MAX_RECORD)?;
        if stop.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        let result = match request {
            Request::Inventory => block(core.replica_inventory()).map(Response::Inventory),
            Request::Fetch(stamp) => {
                stamp.validate()?;
                block(core.replica_entry(stamp)).map(|entry| Response::Entry(entry.map(Box::new)))
            }
            Request::Apply(entry) => {
                block(core.apply_replica_entry(*entry)).map(|_| Response::Done)
            }
            Request::Finish => {
                wire.send(&Response::Done, MAX_INVENTORY)?;
                return Ok(true);
            }
            Request::File(token) => {
                let source = block(core.replica_grant(token)).and_then(Source::open);
                match source {
                    Ok(source) => {
                        // Send no absolute paths or bytes until the full source has been validated.
                        wire.send(&Response::Files(source.entries().to_vec()), MAX_INVENTORY)?;
                        source.send(&mut wire, stop)?;
                        wire.send(&Response::Done, MAX_INVENTORY)?;
                    }
                    Err(error) => wire.send(&Response::Error(error.code), MAX_INVENTORY)?,
                }
                return Ok(false);
            }
        };
        wire.send(
            &result.unwrap_or_else(|error| Response::Error(error.code)),
            MAX_RECORD,
        )?;
    }
    Err(invalid())
}

struct LanTransport {
    wire: Wire,
    stop: Arc<AtomicBool>,
}

impl LanTransport {
    fn request(&mut self, request: &Request, bound: usize) -> Result<Response, AppError> {
        if self.stop.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        self.wire.send(request, MAX_RECORD)?;
        let reply = response(&mut self.wire, bound)?;
        if self.stop.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        Ok(reply)
    }
}

impl ReplicaTransport for LanTransport {
    fn inventory(&mut self) -> Result<Vec<ReplicaStamp>, AppError> {
        match self.request(&Request::Inventory, MAX_INVENTORY)? {
            Response::Inventory(stamps) if stamps.len() <= MAX_STAMPS => Ok(stamps),
            _ => Err(invalid()),
        }
    }

    fn fetch(&mut self, stamp: &ReplicaStamp) -> Result<Option<ReplicaEntry>, AppError> {
        match self.request(&Request::Fetch(stamp.clone()), MAX_RECORD)? {
            Response::Entry(entry) => Ok(entry.map(|entry| *entry)),
            _ => Err(invalid()),
        }
    }

    fn publish(&mut self, entry: ReplicaEntry) -> Result<(), AppError> {
        match self.request(&Request::Apply(Box::new(entry)), MAX_INVENTORY)? {
            Response::Done => Ok(()),
            _ => Err(invalid()),
        }
    }

    fn commit(&mut self, _available: Vec<ReplicaStamp>, complete: bool) -> Result<(), AppError> {
        if !complete {
            return Ok(());
        }
        match self.request(&Request::Finish, MAX_INVENTORY)? {
            Response::Done => Ok(()),
            _ => Err(invalid()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uclip_core::ClipPayload;

    fn wait_for(test: impl Fn() -> bool) {
        let expiry = Instant::now() + Duration::from_secs(20);
        while !test() {
            assert!(
                Instant::now() < expiry,
                "LAN peers did not converge before the deadline"
            );
            thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    #[ignore = "requires local IPv4 multicast"]
    fn first_device_hosts_merge_files_and_failover() {
        let directory = tempfile::tempdir().unwrap();
        let a = block(CoreService::open(directory.path().join("a.sqlite3"))).unwrap();
        let b = block(CoreService::open(directory.path().join("b.sqlite3"))).unwrap();
        block(a.enroll_replica()).unwrap();
        block(b.enroll_replica()).unwrap();
        let a_id = block(a.create_text("from A".into())).unwrap();
        let b_id = block(b.create_text("from B".into())).unwrap();
        let source = directory.path().join("Shared");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("example.txt"), b"from the online device").unwrap();
        let file = block(a.capture(ClipPayload::Files(vec![source.to_str().unwrap().into()])))
            .unwrap()
            .unwrap();
        let port = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let key = [17; 32];
        let first = Lan::start_on_port(
            a.clone(),
            block(a.replica_device()).unwrap().parse().unwrap(),
            key,
            port,
        )
        .unwrap();
        // Start B during A's discovery window; A must still coordinate.
        thread::sleep(Duration::from_millis(550));
        let second = Lan::start_on_port(
            b.clone(),
            block(b.replica_device()).unwrap().parse().unwrap(),
            key,
            port,
        )
        .unwrap();
        wait_for(|| {
            first.status().unwrap().role == SyncRole::Host
                && second.status().unwrap().role == SyncRole::Client
        });
        wait_for(|| {
            block(a.read_clip(b_id.clone())).is_ok() && block(b.read_clip(a_id.clone())).is_ok()
        });
        let ticket = block(b.remote_file_tickets(vec![file]))
            .unwrap()
            .pop()
            .unwrap()
            .1;
        let staging = tempfile::tempdir().unwrap();
        let received = second.file(&ticket, staging.path(), 1024).unwrap();
        assert_eq!(
            std::fs::read(Path::new(&received.paths[0]).join("example.txt")).unwrap(),
            b"from the online device"
        );
        first.stop();
        wait_for(|| second.status().unwrap().role == SyncRole::Host);
        assert!(matches!(second.file(&ticket, staging.path(), 1024),
            Err(error) if error.code == ErrorCode::FilePeerOffline));
        second.stop();
        block(a.shutdown()).unwrap();
        block(b.shutdown()).unwrap();
    }
}
