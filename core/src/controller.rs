use std::collections::VecDeque;

use sonus::{Link, Verb};

use crate::regs::*;

const RESET_PATIENCE: u64 = 100_000;
const CODEC_WAKE: u64 = 1_000;
const COMMAND_PATIENCE: u64 = 50_000;
const POLL: u64 = 10;
const CODEC_MASK: u16 = 0x7fff;
const BDL_WORDS: usize = 4;

pub trait Ring {
    fn words(&self) -> usize;
    fn get(&self, index: usize) -> u32;
    fn set(&mut self, index: usize, value: u32);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    Reset,
    NoCodec,
    RingSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Interrupt {
    pub streams: u32,
    pub responses: bool,
}

pub struct Controller<G, K, C, R> {
    regs: G,
    clock: K,
    corb: C,
    rirb: R,
    corb_entries: usize,
    rirb_entries: usize,
    read: usize,
    unsolicited: VecDeque<(u8, u32)>,
    pub caps: Capabilities,
    pub codecs: u16,
}

fn wait<G: Registers, K: Clock>(regs: &mut G, clock: &mut K, mut done: impl FnMut(&mut G) -> bool) -> bool {
    let until = clock.micros().saturating_add(RESET_PATIENCE);
    loop {
        if done(regs) {
            return true;
        }
        if clock.micros() >= until {
            return false;
        }
        clock.pause(POLL);
    }
}

pub fn describe(list: &mut impl Ring, buffer: u64, chunk: u32, count: usize) -> Option<()> {
    if count < 2 || list.words() < count * BDL_WORDS {
        return None;
    }
    for index in 0..count {
        let address = buffer.checked_add(u64::from(chunk) * index as u64)?;
        list.set(index * BDL_WORDS, address as u32);
        list.set(index * BDL_WORDS + 1, (address >> 32) as u32);
        list.set(index * BDL_WORDS + 2, chunk);
        list.set(index * BDL_WORDS + 3, 1);
    }
    Some(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refill {
    count: usize,
    next: usize,
}

impl Refill {
    pub fn new(count: usize) -> Refill {
        Refill { count: count.max(1), next: 0 }
    }

    pub fn due(&mut self, position: u32, chunk: u32) -> Vec<usize> {
        let current = (position / chunk.max(1)) as usize % self.count;
        let mut due = Vec::new();
        while self.next != current {
            due.push(self.next);
            self.next = (self.next + 1) % self.count;
        }
        due
    }
}

impl<G: Registers, K: Clock, C: Ring, R: Ring> Controller<G, K, C, R> {
    pub fn start(mut regs: G, mut clock: K, corb: (C, u64), rirb: (R, u64)) -> Result<Self, Fault> {
        let control = regs.read32(GCTL);
        regs.write32(GCTL, control & !CRST);
        if !wait(&mut regs, &mut clock, |regs| regs.read32(GCTL) & CRST == 0) {
            return Err(Fault::Reset);
        }
        regs.write32(GCTL, control | CRST);
        if !wait(&mut regs, &mut clock, |regs| regs.read32(GCTL) & CRST != 0) {
            return Err(Fault::Reset);
        }
        clock.pause(CODEC_WAKE);
        let codecs = regs.read16(STATESTS) & CODEC_MASK;
        regs.write16(STATESTS, codecs);
        if codecs == 0 {
            return Err(Fault::NoCodec);
        }
        let caps = Capabilities::decode(regs.read16(GCAP));
        regs.write8(CORBCTL, 0);
        regs.write8(RIRBCTL, 0);
        let (corb_code, corb_entries) = ring_entries(regs.read8(CORBSIZE)).ok_or(Fault::RingSize)?;
        let (rirb_code, rirb_entries) = ring_entries(regs.read8(RIRBSIZE)).ok_or(Fault::RingSize)?;
        if corb.0.words() < corb_entries || rirb.0.words() < rirb_entries * 2 {
            return Err(Fault::RingSize);
        }
        regs.write8(CORBSIZE, corb_code);
        regs.write32(CORBLBASE, corb.1 as u32);
        regs.write32(CORBUBASE, (corb.1 >> 32) as u32);
        regs.write16(CORBRP, CORBRPRST);
        wait(&mut regs, &mut clock, |regs| regs.read16(CORBRP) & CORBRPRST != 0);
        regs.write16(CORBRP, 0);
        if !wait(&mut regs, &mut clock, |regs| regs.read16(CORBRP) & CORBRPRST == 0) {
            return Err(Fault::Reset);
        }
        regs.write16(CORBWP, 0);
        regs.write8(CORBCTL, CORBRUN);
        regs.write8(RIRBSIZE, rirb_code);
        regs.write32(RIRBLBASE, rirb.1 as u32);
        regs.write32(RIRBUBASE, (rirb.1 >> 32) as u32);
        regs.write16(RIRBWP, RIRBWPRST);
        regs.write16(RINTCNT, 1);
        regs.write8(RIRBCTL, RINTCTL | RIRBDMAEN);
        let control = regs.read32(GCTL);
        regs.write32(GCTL, control | UNSOL);
        regs.write32(INTCTL, GIE | CIE);
        Ok(Controller { regs, clock, corb: corb.0, rirb: rirb.0, corb_entries, rirb_entries, read: 0, unsolicited: VecDeque::new(), caps, codecs })
    }

    pub fn codec_addresses(&self) -> impl Iterator<Item = u8> + '_ {
        (0..15u8).filter(|address| self.codecs & (1 << address) != 0)
    }

    pub fn send(&mut self, verb: Verb) {
        let written = usize::from(self.regs.read16(CORBWP) & 0xff);
        let next = (written + 1) % self.corb_entries;
        self.corb.set(next, verb.0);
        self.regs.write16(CORBWP, next as u16);
    }

    fn collect(&mut self) -> Option<u32> {
        let written = usize::from(self.regs.read16(RIRBWP) & 0xff) % self.rirb_entries;
        let mut answer = None;
        while self.read != written {
            self.read = (self.read + 1) % self.rirb_entries;
            let (response, extended) = (self.rirb.get(self.read * 2), self.rirb.get(self.read * 2 + 1));
            if extended & UNSOLICITED != 0 {
                self.unsolicited.push_back(((extended & 0xf) as u8, response));
            } else if answer.is_none() {
                answer = Some(response);
            }
        }
        self.regs.write8(RIRBSTS, RINTFL | RIRBOIS);
        answer
    }

    pub fn unsolicited(&mut self) -> Option<(u8, u32)> {
        if self.unsolicited.is_empty() {
            self.collect();
        }
        self.unsolicited.pop_front()
    }

    pub fn interrupt(&mut self) -> Interrupt {
        let status = self.regs.read32(INTSTS);
        let responses = status & CIE != 0;
        if responses {
            self.collect();
        }
        let streams = status & 0x3fff_ffff;
        for index in 0..30 {
            if streams & (1 << index) != 0 {
                let descriptor = STREAMS + index * STREAM_SIZE;
                let flags = self.regs.read8(descriptor + SD_STS);
                self.regs.write8(descriptor + SD_STS, flags & (BCIS | FIFOE | DESE));
            }
        }
        Interrupt { streams, responses }
    }

    pub fn program(&mut self, descriptor: usize, tag: u8, format: u16, list: u64, total: u32, entries: usize) -> Result<(), Fault> {
        let control = self.regs.read32(descriptor + SD_CTL) & 0x00ff_ffff;
        self.regs.write32(descriptor + SD_CTL, control & !RUN | SRST);
        if !wait(&mut self.regs, &mut self.clock, |regs| regs.read32(descriptor + SD_CTL) & SRST != 0) {
            return Err(Fault::Reset);
        }
        self.regs.write32(descriptor + SD_CTL, 0);
        if !wait(&mut self.regs, &mut self.clock, |regs| regs.read32(descriptor + SD_CTL) & SRST == 0) {
            return Err(Fault::Reset);
        }
        self.regs.write32(descriptor + SD_BDPL, list as u32);
        self.regs.write32(descriptor + SD_BDPU, (list >> 32) as u32);
        self.regs.write32(descriptor + SD_CBL, total);
        self.regs.write16(descriptor + SD_LVI, u16::try_from(entries.saturating_sub(1)).unwrap_or(0xff) & 0xff);
        self.regs.write16(descriptor + SD_FMT, format);
        self.regs.write32(descriptor + SD_CTL, u32::from(tag & 0xf) << STREAM_SHIFT | IOCE);
        self.regs.write8(descriptor + SD_STS, BCIS | FIFOE | DESE);
        let enabled = self.regs.read32(INTCTL);
        self.regs.write32(INTCTL, enabled | GIE | self.caps.interrupt_bit(descriptor));
        Ok(())
    }

    pub fn run(&mut self, descriptor: usize, on: bool) {
        let control = self.regs.read32(descriptor + SD_CTL) & 0x00ff_ffff;
        self.regs.write32(descriptor + SD_CTL, if on { control | RUN } else { control & !RUN });
    }

    pub fn position(&mut self, descriptor: usize) -> u32 {
        self.regs.read32(descriptor + SD_LPIB)
    }

    pub fn registers(&mut self) -> &mut G {
        &mut self.regs
    }
}

impl<G: Registers, K: Clock, C: Ring, R: Ring> Link for Controller<G, K, C, R> {
    fn ask(&mut self, verb: Verb) -> Option<u32> {
        self.send(verb);
        let until = self.clock.micros().saturating_add(COMMAND_PATIENCE);
        loop {
            if let Some(answer) = self.collect() {
                return Some(answer);
            }
            if self.clock.micros() >= until {
                return None;
            }
            self.clock.pause(POLL);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::rc::Rc;

    #[derive(Clone)]
    pub struct Memory(pub Rc<RefCell<Vec<u32>>>);

    impl Memory {
        pub fn new(words: usize) -> Memory {
            Memory(Rc::new(RefCell::new(vec![0; words])))
        }
    }

    impl Ring for Memory {
        fn words(&self) -> usize {
            self.0.borrow().len()
        }

        fn get(&self, index: usize) -> u32 {
            self.0.borrow()[index]
        }

        fn set(&mut self, index: usize, value: u32) {
            self.0.borrow_mut()[index] = value;
        }
    }

    pub struct Hda {
        pub bytes: BTreeMap<usize, u8>,
        pub corb: Memory,
        pub rirb: Memory,
        pub answers: BTreeMap<u32, u32>,
        pub heard: Vec<Verb>,
        pub done: usize,
        pub silent: bool,
    }

    impl Hda {
        pub fn new(corb: Memory, rirb: Memory) -> Hda {
            let mut hda = Hda { bytes: BTreeMap::new(), corb, rirb, answers: BTreeMap::new(), heard: Vec::new(), done: 0, silent: false };
            hda.put(GCAP, 2, 0x4401);
            hda.put(CORBSIZE, 1, 0x42);
            hda.put(RIRBSIZE, 1, 0x42);
            hda
        }

        fn get(&self, at: usize, width: usize) -> u32 {
            (0..width).fold(0, |value, byte| value | u32::from(*self.bytes.get(&(at + byte)).unwrap_or(&0)) << (8 * byte))
        }

        fn put(&mut self, at: usize, width: usize, value: u32) {
            for byte in 0..width {
                self.bytes.insert(at + byte, (value >> (8 * byte)) as u8);
            }
        }

        pub fn respond(&mut self, response: u32, extended: u32) {
            let next = (self.get(RIRBWP, 1) as usize + 1) % 256;
            self.rirb.set(next * 2, response);
            self.rirb.set(next * 2 + 1, extended);
            self.put(RIRBWP, 1, next as u32);
            let status = self.get(INTSTS, 4);
            self.put(INTSTS, 4, status | CIE | GIE);
        }

        fn write(&mut self, at: usize, width: usize, value: u32) {
            match at {
                GCTL => {
                    self.put(GCTL, 4, value);
                    if value & CRST != 0 {
                        self.put(STATESTS, 2, 1);
                    }
                }
                STATESTS | INTSTS => {
                    let now = self.get(at, width);
                    self.put(at, width, now & !value);
                }
                CORBRP => self.put(CORBRP, 2, value & u32::from(CORBRPRST)),
                CORBSIZE | RIRBSIZE => {
                    let caps = self.get(at, 1) & 0xf0;
                    self.put(at, 1, caps | (value & 0x3));
                }
                RIRBWP => {
                    if value & u32::from(RIRBWPRST) != 0 {
                        self.put(RIRBWP, 2, 0);
                    }
                }
                CORBWP => {
                    self.put(CORBWP, 2, value & 0xff);
                    let target = (value & 0xff) as usize;
                    while self.done != target {
                        self.done = (self.done + 1) % 256;
                        let verb = Verb(self.corb.get(self.done));
                        self.heard.push(verb);
                        if !self.silent {
                            let answer = *self.answers.get(&verb.0).unwrap_or(&0);
                            self.respond(answer, u32::from(verb.codec()));
                        }
                    }
                }
                at if at >= STREAMS && (at - STREAMS) % STREAM_SIZE == SD_STS => {
                    let now = self.get(at, 1);
                    self.put(at, 1, now & !value);
                }
                _ => self.put(at, width, value),
            }
        }
    }

    impl Registers for Hda {
        fn read8(&mut self, at: usize) -> u8 {
            self.get(at, 1) as u8
        }

        fn read16(&mut self, at: usize) -> u16 {
            self.get(at, 2) as u16
        }

        fn read32(&mut self, at: usize) -> u32 {
            self.get(at, 4)
        }

        fn write8(&mut self, at: usize, value: u8) {
            self.write(at, 1, u32::from(value));
        }

        fn write16(&mut self, at: usize, value: u16) {
            self.write(at, 2, u32::from(value));
        }

        fn write32(&mut self, at: usize, value: u32) {
            self.write(at, 4, value);
        }
    }

    pub struct Ticks(pub u64);

    impl Clock for Ticks {
        fn micros(&mut self) -> u64 {
            self.0
        }

        fn pause(&mut self, micros: u64) {
            self.0 += micros;
        }
    }

    pub fn started() -> (Controller<Hda, Ticks, Memory, Memory>, Memory, Memory) {
        let (corb, rirb) = (Memory::new(256), Memory::new(512));
        let hda = Hda::new(corb.clone(), rirb.clone());
        let controller = Controller::start(hda, Ticks(0), (corb.clone(), 0x1_0000_0000), (rirb.clone(), 0x2000)).unwrap();
        (controller, corb, rirb)
    }

    #[test]
    fn the_controller_resets_finds_the_codec_and_sets_up_both_rings() {
        let (mut controller, _, _) = started();
        assert_eq!(controller.codec_addresses().collect::<Vec<_>>(), [0]);
        assert_eq!(controller.caps.outputs, 4);
        let regs = controller.registers();
        assert_eq!((regs.read32(CORBLBASE), regs.read32(CORBUBASE), regs.read32(RIRBLBASE)), (0, 1, 0x2000));
        assert_eq!((regs.read8(CORBSIZE) & 3, regs.read8(RIRBSIZE) & 3, regs.read8(CORBCTL), regs.read8(RIRBCTL)), (2, 2, CORBRUN, RINTCTL | RIRBDMAEN));
        assert_eq!(regs.read32(GCTL) & (CRST | UNSOL), CRST | UNSOL);
        assert_eq!(regs.read32(INTCTL), GIE | CIE);
    }

    #[test]
    fn commands_get_their_own_answers_and_jacks_are_kept_aside_across_wraps() {
        let (mut controller, _, _) = started();
        let vendor = sonus::verb::parameter(0, 0, sonus::verb::parameter::VENDOR);
        controller.registers().answers.insert(vendor.0, 0x10ec_0257);
        assert_eq!(controller.ask(vendor), Some(0x10ec_0257));
        controller.registers().respond(0x0400_0000, UNSOLICITED);
        assert_eq!(controller.ask(vendor), Some(0x10ec_0257), "a jack report before the answer does not take its place");
        assert_eq!(controller.unsolicited(), Some((0, 0x0400_0000)));
        assert_eq!(controller.unsolicited(), None);
        for _ in 0..600 {
            assert_eq!(controller.ask(vendor), Some(0x10ec_0257));
        }
        assert_eq!(controller.registers().heard.len(), 602);
        controller.registers().silent = true;
        assert_eq!(controller.ask(vendor), None, "no answer within its patience");
        controller.registers().respond(0x0800_0000, UNSOLICITED);
        let interrupt = controller.interrupt();
        assert!(interrupt.responses);
        assert_eq!(controller.unsolicited(), Some((0, 0x0800_0000)));
    }

    #[test]
    fn a_stream_is_described_programmed_and_run() {
        let (mut controller, _, _) = started();
        let mut list = Memory::new(16);
        assert_eq!(describe(&mut list, 0x10_0000, 2048, 4), Some(()));
        assert_eq!(list.0.borrow()[..8], [0x10_0000, 0, 2048, 1, 0x10_0800, 0, 2048, 1]);
        assert_eq!(describe(&mut list, 0, 2048, 1), None);
        assert_eq!(describe(&mut list, 0, 2048, 5), None);
        let output = controller.caps.output(0).unwrap();
        controller.program(output, 1, 0x4011, 0x30_0000, 8192, 4).unwrap();
        let regs = controller.registers();
        assert_eq!(
            (regs.read32(output + SD_BDPL), regs.read32(output + SD_CBL), regs.read16(output + SD_LVI), regs.read16(output + SD_FMT)),
            (0x30_0000, 8192, 3, 0x4011)
        );
        assert_eq!(regs.read32(output + SD_CTL) & 0x00ff_ffff, 1 << STREAM_SHIFT | IOCE);
        assert_eq!(regs.read32(INTCTL) & (1 << 4), 1 << 4);
        controller.run(output, true);
        assert_eq!(controller.registers().read32(output + SD_CTL) & RUN, RUN);
        controller.registers().write8(output + SD_STS, 0);
        controller.registers().bytes.insert(output + SD_STS, BCIS);
        let status = controller.registers().read32(INTSTS);
        controller.registers().write32(INTSTS, status);
        controller.registers().bytes.insert(INTSTS, 1 << 4);
        assert_eq!(controller.interrupt().streams, 1 << 4);
        assert_eq!(controller.registers().read8(output + SD_STS) & BCIS, 0, "the completed buffer is acknowledged");
        controller.run(output, false);
        assert_eq!(controller.registers().read32(output + SD_CTL) & RUN, 0);
    }

    #[test]
    fn played_buffers_are_handed_back_in_order_however_many_went_by() {
        let mut refill = Refill::new(4);
        assert!(refill.due(0, 2048).is_empty());
        assert_eq!(refill.due(2048, 2048), [0]);
        assert_eq!(refill.due(3 * 2048 + 10, 2048), [1, 2]);
        assert_eq!(refill.due(100, 2048), [3]);
        assert_eq!(refill.due(2048, 2048), [0]);
        assert!(Refill::new(0).due(0, 0).is_empty());
    }

    #[test]
    fn a_controller_without_a_codec_or_with_rings_too_small_is_refused() {
        let (corb, rirb) = (Memory::new(256), Memory::new(512));
        let mut hda = Hda::new(corb.clone(), rirb.clone());
        hda.put(RIRBSIZE, 1, 0x02);
        assert_eq!(Controller::start(hda, Ticks(0), (corb.clone(), 0), (rirb.clone(), 0)).err(), Some(Fault::RingSize));
        let small = Memory::new(8);
        let hda = Hda::new(small.clone(), rirb.clone());
        assert_eq!(Controller::start(hda, Ticks(0), (small, 0), (rirb, 0)).err(), Some(Fault::RingSize));
    }
}
