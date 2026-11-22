use std::collections::VecDeque;
use std::convert::Infallible;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::process;
use std::time::Instant;

use audiod::{AudioChunk, NUM_SUB_BUFFS, SUB_BUFF_SIZE};
use common::MemoryType;
use common::dma::Dma;
use drv_i18n::{Catalog, Fail, Msg, as_msg};
use event::{EventFlags, EventQueue, user_data};
use pcid_interface::PciFunctionHandle;
use pcid_interface::irq_helpers::pci_allocate_interrupt_vector;
use redox_scheme::scheme::{SchemeSync, register_sync_scheme};
use redox_scheme::{CallerCtx, OpenResult, Socket};
use rtkhda_core::{Controller, Refill, describe};
use rtkhda_redox::{BoxError, Mmio, Monotonic, Words};
use scheme_utils::{FpathWriter, HandleMap, ReadinessBased};
use sonus::{Format, Graph, Levels, Link, Mixer, Role, Verb, discover, verb};
use syscall::error::{EACCES, EBADF, EEXIST, EINVAL, ENOENT, ENOTDIR, EWOULDBLOCK, Error, Result};
use syscall::schemev2::NewFdFlags;

const TAG: u8 = 1;
const CORB_WORDS: usize = 256;
const RIRB_WORDS: usize = 512;
const LIST_WORDS: usize = 4 * NUM_SUB_BUFFS;
const WAITING: usize = 2;
const ROOT: u32 = 0;

type Hda = Controller<Mmio, Monotonic, Words, Words>;

enum Handle {
    Root,
    Text(Vec<u8>),
    Audio,
    Volume,
    Mute,
}

struct Sound {
    hda: Hda,
    mixer: Mixer,
    output: usize,
    buffer: Dma<[u8]>,
    _list: Words,
    refill: Refill,
    queue: VecDeque<Vec<u8>>,
    consumer: Option<redox_rings::user::Consumer<AudioChunk>>,
    handles: HandleMap<Handle>,
    catalog: Catalog,
    summary: String,
}

fn say(catalog: &Catalog, message: &Msg) {
    eprintln!("{}", catalog.render(message));
}

fn send(hda: &mut Hda, verbs: Vec<Verb>) {
    for verb in verbs {
        hda.ask(verb);
    }
}

fn role_key(role: Role) -> &'static str {
    match role {
        Role::Speaker => "dev-role-speaker",
        Role::Headphone => "dev-role-headphone",
        Role::LineOut => "dev-role-line-out",
        Role::InternalMic => "dev-role-internal-mic",
        Role::HeadsetMic => "dev-role-headset-mic",
        Role::LineIn => "dev-role-line-in",
    }
}

fn summary(catalog: &Catalog, graph: &Graph) -> String {
    let mut text = String::new();
    for route in graph.outputs().iter().chain(&graph.inputs()) {
        let path = route.path.iter().map(|node| format!("{node:#04x}")).collect::<Vec<_>>().join(" → ");
        let key = if route.role.output() { "dev-route-out" } else { "dev-route-in" };
        text.push_str(&catalog.render(&Msg::new(key).arg("role", catalog.text(role_key(route.role))).arg("path", path)));
        text.push('\n');
    }
    text
}

fn chunk_bytes(chunk: &AudioChunk) -> Vec<u8> {
    chunk.iter().flat_map(|(left, right)| left.to_le_bytes().into_iter().chain(right.to_le_bytes())).collect()
}

impl Sound {
    fn next_chunk(&mut self) -> Vec<u8> {
        if let Some(chunk) = self.queue.pop_front() {
            return chunk;
        }
        if let Some(chunk) = self.consumer.as_mut().and_then(|consumer| consumer.pop().ok()) {
            return chunk_bytes(&chunk);
        }
        vec![0; SUB_BUFF_SIZE]
    }

    fn refill(&mut self) {
        let position = self.hda.position(self.output);
        for index in self.refill.due(position, SUB_BUFF_SIZE as u32) {
            let chunk = self.next_chunk();
            let start = index * SUB_BUFF_SIZE;
            let Some(slot) = self.buffer.get_mut(start..start + SUB_BUFF_SIZE) else { continue };
            let count = chunk.len().min(SUB_BUFF_SIZE);
            slot[..count].copy_from_slice(&chunk[..count]);
            slot[count..].fill(0);
        }
    }

    fn jacks(&mut self) {
        while let Some((codec, response)) = self.hda.unsolicited() {
            let Some(pin) = self.mixer.jack(verb::tag(response)) else { continue };
            let present = self.hda.ask(verb::pin_sense(codec, pin)).is_some_and(verb::present);
            let verbs = self.mixer.sensed(pin, present);
            if !verbs.is_empty() {
                say(&self.catalog, &Msg::new(if present { "redox-plugged" } else { "redox-unplugged" }).arg("pin", format!("{pin:#04x}")));
            }
            send(&mut self.hda, verbs);
        }
    }

    fn interrupt(&mut self) -> bool {
        let interrupt = self.hda.interrupt();
        if interrupt.streams & self.hda.caps.interrupt_bit(self.output) != 0 {
            self.refill();
        }
        if interrupt.responses {
            self.jacks();
        }
        interrupt.streams != 0 || interrupt.responses
    }

    fn level_text(&self) -> Vec<u8> {
        let levels = self.mixer.levels();
        let level = match self.mixer.output().map(|route| route.role) {
            Some(Role::Headphone) => levels.headphone,
            Some(Role::LineOut) => levels.line,
            _ => levels.speaker,
        };
        format!("{level}\n").into_bytes()
    }
}

fn number(buf: &[u8]) -> Result<u8> {
    std::str::from_utf8(buf).ok().and_then(|text| text.trim().parse::<u8>().ok()).ok_or(Error::new(EINVAL))
}

impl SchemeSync for Sound {
    fn scheme_root(&mut self) -> Result<usize> {
        Ok(self.handles.insert(Handle::Root))
    }

    fn openat(&mut self, dirfd: usize, path: &str, _flags: usize, _fcntl_flags: u32, ctx: &CallerCtx) -> Result<OpenResult> {
        if ctx.uid != ROOT {
            return Err(Error::new(EACCES));
        }
        let handle = match (self.handles.get(dirfd)?, path.trim_matches('/')) {
            (Handle::Root, "codec") => Handle::Text(self.summary.clone().into_bytes()),
            (Handle::Root, "volume") => Handle::Volume,
            (Handle::Root, "mute") => Handle::Mute,
            (Handle::Root, _) => Handle::Audio,
            (Handle::Audio, "audio_shm") => {
                if self.consumer.is_some() {
                    return Err(Error::new(EEXIST));
                }
                let shm = libredox::Fd::open(
                    "/scheme/shm/audio_shm",
                    libredox::flag::O_CREAT | libredox::flag::O_RDWR | libredox::flag::O_CLOEXEC | libredox::flag::O_EXCL,
                    0,
                )?;
                let fd = shm.raw();
                self.consumer = Some(redox_rings::user::Consumer::<AudioChunk>::from_fd(shm, true, Some(2))?);
                return Ok(OpenResult::OtherScheme { fd });
            }
            (Handle::Audio, _) => return Err(Error::new(ENOENT)),
            _ => return Err(Error::new(ENOTDIR)),
        };
        Ok(OpenResult::ThisScheme { number: self.handles.insert(handle), flags: NewFdFlags::POSITIONED })
    }

    fn read(&mut self, id: usize, buf: &mut [u8], offset: u64, _flags: u32, _ctx: &CallerCtx) -> Result<usize> {
        let text = match self.handles.get(id)? {
            Handle::Text(text) => text.clone(),
            Handle::Volume => self.level_text(),
            Handle::Mute => format!("{}\n", u8::from(self.mixer.muted())).into_bytes(),
            _ => return Err(Error::new(EBADF)),
        };
        let from = usize::try_from(offset).ok().and_then(|offset| text.get(offset..)).unwrap_or(&[]);
        let count = from.len().min(buf.len());
        buf[..count].copy_from_slice(&from[..count]);
        Ok(count)
    }

    fn write(&mut self, id: usize, buf: &[u8], _offset: u64, _flags: u32, _ctx: &CallerCtx) -> Result<usize> {
        match self.handles.get(id)? {
            Handle::Audio => {
                if self.queue.len() >= WAITING {
                    return Err(Error::new(EWOULDBLOCK));
                }
                let count = buf.len().min(SUB_BUFF_SIZE);
                self.queue.push_back(buf[..count].to_vec());
                Ok(count)
            }
            Handle::Volume => {
                let verbs = self.mixer.set_level(number(buf)?);
                send(&mut self.hda, verbs);
                Ok(buf.len())
            }
            Handle::Mute => {
                let verbs = self.mixer.set_muted(number(buf)? != 0);
                send(&mut self.hda, verbs);
                Ok(buf.len())
            }
            _ => Err(Error::new(EBADF)),
        }
    }

    fn fpath(&mut self, _id: usize, buf: &mut [u8], _ctx: &CallerCtx) -> Result<usize> {
        FpathWriter::with(buf, "audiohw", |_| Ok(()))
    }

    fn on_close(&mut self, id: usize) {
        self.handles.remove(id);
    }
}

fn start(pcid: &mut PciFunctionHandle, catalog: &Catalog) -> std::result::Result<Sound, BoxError> {
    let registers = Mmio::mapped(unsafe { pcid.map_bar(0, MemoryType::Uncacheable) });
    let corb = Words::new(CORB_WORDS)?;
    let rirb = Words::new(RIRB_WORDS)?;
    let (corb_at, rirb_at) = (corb.physical(), rirb.physical());
    let mut hda = Controller::start(registers, Monotonic(Instant::now()), (corb, corb_at), (rirb, rirb_at))
        .map_err(|fault| Fail::new("redox-controller").arg("detail", format!("{fault:?}")))?;
    let addresses: Vec<u8> = hda.codec_addresses().collect();
    let codec = addresses
        .into_iter()
        .find_map(|address| discover(&mut hda, address).filter(|codec| !codec.graph.outputs().is_empty()))
        .ok_or_else(|| Fail::new("redox-no-codec"))?;
    let mut mixer = Mixer::new(codec.address, codec.group, codec.graph.clone(), Levels::default());
    send(&mut hda, mixer.start());
    for pin in mixer.reporting().to_vec() {
        let present = hda.ask(verb::pin_sense(codec.address, pin)).is_some_and(verb::present);
        send(&mut hda, mixer.sensed(pin, present));
    }
    let output = hda.caps.output(0).ok_or_else(|| Fail::new("redox-no-stream"))?;
    let buffer = unsafe { Dma::<[u8]>::zeroed_slice(NUM_SUB_BUFFS * SUB_BUFF_SIZE)?.assume_init() };
    let mut list = Words::new(LIST_WORDS)?;
    describe(&mut list, buffer.physical() as u64, SUB_BUFF_SIZE as u32, NUM_SUB_BUFFS).ok_or_else(|| Fail::new("redox-no-stream"))?;
    let word = Format::CD.word().ok_or_else(|| Fail::new("redox-no-stream"))?;
    hda.program(output, TAG, word, list.physical(), (NUM_SUB_BUFFS * SUB_BUFF_SIZE) as u32, NUM_SUB_BUFFS)
        .map_err(|fault| Fail::new("redox-controller").arg("detail", format!("{fault:?}")))?;
    send(&mut hda, mixer.play(TAG, Format::CD));
    hda.run(output, true);
    say(catalog, &Msg::new("redox-started").arg("codec", format!("{:#010x}", codec.vendor)).arg("address", codec.address));
    Ok(Sound {
        hda,
        mixer,
        output,
        buffer,
        _list: list,
        refill: Refill::new(NUM_SUB_BUFFS),
        queue: VecDeque::new(),
        consumer: None,
        handles: HandleMap::new(),
        summary: summary(catalog, &codec.graph),
        catalog: rtkhda_redox::catalog(),
    })
}

user_data! {
    enum Source {
        Irq,
        Scheme,
    }
}

fn run(daemon: daemon::Daemon, mut pcid: PciFunctionHandle, catalog: &Catalog) -> std::result::Result<Infallible, BoxError> {
    let irq = pci_allocate_interrupt_vector(&mut pcid, "rtkhdad");
    let mut sound = start(&mut pcid, catalog)?;
    let queue = EventQueue::<Source>::new()?;
    let mut ready = ReadinessBased::new(Box::new(Socket::nonblock()?), 16);
    register_sync_scheme(ready.socket(), "audiohw", &mut sound)?;
    daemon.ready();
    queue.subscribe(ready.socket().inner().raw(), Source::Scheme, EventFlags::READ)?;
    queue.subscribe(irq.irq_handle().as_raw_fd() as usize, Source::Irq, EventFlags::READ)?;
    let mut events = [Source::Irq, Source::Scheme].into_iter().map(Ok).chain(queue.map(|event| event.map(|event| event.user_data)));
    loop {
        let Some(event) = events.next() else { return Err(Fail::new("redox-events-ended").into()) };
        match event? {
            Source::Irq => {
                let mut count = [0u8; 8];
                irq.irq_handle().read_exact(&mut count)?;
                if !sound.interrupt() {
                    continue;
                }
                irq.irq_handle().write_all(&count)?;
                ready.poll_all_requests(&mut sound)?;
                ready.write_responses()?;
            }
            Source::Scheme => {
                ready.read_and_process_requests(&mut sound)?;
                ready.write_responses()?;
            }
        }
    }
}

fn daemon(daemon: daemon::Daemon, pcid: PciFunctionHandle) -> ! {
    let catalog = rtkhda_redox::catalog();
    match run(daemon, pcid, &catalog) {
        Ok(never) => match never {},
        Err(e) => {
            say(&catalog, &as_msg(e.as_ref(), "redox-error"));
            process::exit(1);
        }
    }
}

fn main() {
    pcid_interface::pci_daemon(daemon);
}
