pub const GCAP: usize = 0x00;
pub const GCTL: usize = 0x08;
pub const STATESTS: usize = 0x0e;
pub const INTCTL: usize = 0x20;
pub const INTSTS: usize = 0x24;
pub const CORBLBASE: usize = 0x40;
pub const CORBUBASE: usize = 0x44;
pub const CORBWP: usize = 0x48;
pub const CORBRP: usize = 0x4a;
pub const CORBCTL: usize = 0x4c;
pub const CORBSIZE: usize = 0x4e;
pub const RIRBLBASE: usize = 0x50;
pub const RIRBUBASE: usize = 0x54;
pub const RIRBWP: usize = 0x58;
pub const RINTCNT: usize = 0x5a;
pub const RIRBCTL: usize = 0x5c;
pub const RIRBSTS: usize = 0x5d;
pub const RIRBSIZE: usize = 0x5e;
pub const STREAMS: usize = 0x80;
pub const STREAM_SIZE: usize = 0x20;

pub const SD_CTL: usize = 0x00;
pub const SD_STS: usize = 0x03;
pub const SD_LPIB: usize = 0x04;
pub const SD_CBL: usize = 0x08;
pub const SD_LVI: usize = 0x0c;
pub const SD_FMT: usize = 0x12;
pub const SD_BDPL: usize = 0x18;
pub const SD_BDPU: usize = 0x1c;

pub const CRST: u32 = 1 << 0;
pub const UNSOL: u32 = 1 << 8;
pub const GIE: u32 = 1 << 31;
pub const CIE: u32 = 1 << 30;
pub const CORBRPRST: u16 = 1 << 15;
pub const RIRBWPRST: u16 = 1 << 15;
pub const CORBRUN: u8 = 1 << 1;
pub const RINTCTL: u8 = 1 << 0;
pub const RIRBDMAEN: u8 = 1 << 1;
pub const RINTFL: u8 = 1 << 0;
pub const RIRBOIS: u8 = 1 << 2;
pub const SRST: u32 = 1 << 0;
pub const RUN: u32 = 1 << 1;
pub const IOCE: u32 = 1 << 2;
pub const STREAM_SHIFT: u32 = 20;
pub const BCIS: u8 = 1 << 2;
pub const FIFOE: u8 = 1 << 3;
pub const DESE: u8 = 1 << 4;
pub const UNSOLICITED: u32 = 1 << 4;

pub trait Registers {
    fn read8(&mut self, at: usize) -> u8;
    fn read16(&mut self, at: usize) -> u16;
    fn read32(&mut self, at: usize) -> u32;
    fn write8(&mut self, at: usize, value: u8);
    fn write16(&mut self, at: usize, value: u16);
    fn write32(&mut self, at: usize, value: u32);
}

pub trait Clock {
    fn micros(&mut self) -> u64;
    fn pause(&mut self, micros: u64);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub outputs: usize,
    pub inputs: usize,
    pub both: usize,
    pub wide: bool,
}

impl Capabilities {
    pub fn decode(gcap: u16) -> Capabilities {
        Capabilities {
            outputs: usize::from((gcap >> 12) & 0xf),
            inputs: usize::from((gcap >> 8) & 0xf),
            both: usize::from((gcap >> 3) & 0x1f),
            wide: gcap & 1 != 0,
        }
    }

    pub fn input(&self, index: usize) -> Option<usize> {
        (index < self.inputs).then_some(STREAMS + index * STREAM_SIZE)
    }

    pub fn output(&self, index: usize) -> Option<usize> {
        (index < self.outputs).then_some(STREAMS + (self.inputs + index) * STREAM_SIZE)
    }

    pub fn interrupt_bit(&self, descriptor: usize) -> u32 {
        1 << ((descriptor - STREAMS) / STREAM_SIZE)
    }
}

pub fn ring_entries(size: u8) -> Option<(u8, usize)> {
    let capable = size >> 4;
    [(0b100, 2, 256), (0b010, 1, 16), (0b001, 0, 2)].into_iter().find(|(bit, _, _)| capable & bit != 0).map(|(_, code, entries)| (code, entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_and_ring_sizes_read_as_the_specification_says() {
        let ich9 = Capabilities::decode(0x4401);
        assert_eq!(ich9, Capabilities { outputs: 4, inputs: 4, both: 0, wide: true });
        assert_eq!((ich9.input(0), ich9.input(3), ich9.input(4)), (Some(0x80), Some(0xe0), None));
        assert_eq!((ich9.output(0), ich9.output(3), ich9.output(4)), (Some(0x100), Some(0x160), None));
        assert_eq!((ich9.interrupt_bit(0x80), ich9.interrupt_bit(0x100)), (1, 1 << 4));
        assert_eq!(ring_entries(0x42), Some((2, 256)));
        assert_eq!(ring_entries(0x20), Some((1, 16)));
        assert_eq!(ring_entries(0x10), Some((0, 2)));
        assert_eq!(ring_entries(0x00), None);
    }
}
