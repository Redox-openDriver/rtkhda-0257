use std::time::{Duration, Instant};

use common::dma::Dma;
use rtkhda_core::{Clock, Registers, Ring};

pub const LOCALES: [(&str, &str); 2] = [("ru", include_str!("../../locales/ru.ftl")), ("en", include_str!("../../locales/en.ftl"))];
const SPIN_BELOW: u64 = 200;

pub type BoxError = Box<dyn std::error::Error>;

pub fn catalog() -> drv_i18n::Catalog {
    drv_i18n::Catalog::new(&LOCALES)
}

pub struct Mmio {
    base: *mut u8,
    size: usize,
}

impl Mmio {
    pub fn mapped(bar: &pcid_interface::MappedBar) -> Mmio {
        Mmio { base: bar.ptr.as_ptr(), size: bar.bar_size }
    }

    fn fits(&self, at: usize, width: usize) -> bool {
        at.is_multiple_of(width) && at.checked_add(width).is_some_and(|end| end <= self.size)
    }
}

impl Registers for Mmio {
    fn read8(&mut self, at: usize) -> u8 {
        if !self.fits(at, 1) {
            return 0;
        }
        unsafe { std::ptr::read_volatile(self.base.add(at)) }
    }

    fn read16(&mut self, at: usize) -> u16 {
        if !self.fits(at, 2) {
            return 0;
        }
        unsafe { std::ptr::read_volatile(self.base.add(at).cast::<u16>()) }
    }

    fn read32(&mut self, at: usize) -> u32 {
        if !self.fits(at, 4) {
            return 0;
        }
        unsafe { std::ptr::read_volatile(self.base.add(at).cast::<u32>()) }
    }

    fn write8(&mut self, at: usize, value: u8) {
        if self.fits(at, 1) {
            unsafe { std::ptr::write_volatile(self.base.add(at), value) }
        }
    }

    fn write16(&mut self, at: usize, value: u16) {
        if self.fits(at, 2) {
            unsafe { std::ptr::write_volatile(self.base.add(at).cast::<u16>(), value) }
        }
    }

    fn write32(&mut self, at: usize, value: u32) {
        if self.fits(at, 4) {
            unsafe { std::ptr::write_volatile(self.base.add(at).cast::<u32>(), value) }
        }
    }
}

pub struct Words(pub Dma<[u32]>);

impl Words {
    pub fn new(count: usize) -> Result<Words, BoxError> {
        Ok(Words(unsafe { Dma::<[u32]>::zeroed_slice(count)?.assume_init() }))
    }

    pub fn physical(&self) -> u64 {
        self.0.physical() as u64
    }
}

impl Ring for Words {
    fn words(&self) -> usize {
        self.0.len()
    }

    fn get(&self, index: usize) -> u32 {
        if index >= self.0.len() {
            return 0;
        }
        unsafe { std::ptr::read_volatile(self.0.as_ptr().add(index)) }
    }

    fn set(&mut self, index: usize, value: u32) {
        if index < self.0.len() {
            unsafe { std::ptr::write_volatile(self.0.as_mut_ptr().add(index), value) }
        }
    }
}

pub struct Monotonic(pub Instant);

impl Clock for Monotonic {
    fn micros(&mut self) -> u64 {
        u64::try_from(self.0.elapsed().as_micros()).unwrap_or(u64::MAX)
    }

    fn pause(&mut self, micros: u64) {
        if micros >= SPIN_BELOW {
            std::thread::sleep(Duration::from_micros(micros));
            return;
        }
        let until = Instant::now() + Duration::from_micros(micros);
        while Instant::now() < until {
            std::hint::spin_loop();
        }
    }
}
