//! CPU address space: RAM, PPU registers, controllers and the cartridge.

use super::cart::Cartridge;
use super::cpu;
use super::ppu::Ppu;

pub struct Bus {
    ram: [u8; 0x800],
    pub ppu: Ppu,
    pub cart: Cartridge,
    buttons: [u8; 2],
    shift: [u8; 2],
    strobe: bool,
    dma_cycles: u32,
}

impl Bus {
    pub fn new(cart: Cartridge) -> Self {
        Self {
            ram: [0; 0x800],
            ppu: Ppu::new(),
            cart,
            buttons: [0; 2],
            shift: [0; 2],
            strobe: false,
            dma_cycles: 0,
        }
    }

    pub fn set_buttons(&mut self, player: usize, state: u8) {
        if let Some(slot) = self.buttons.get_mut(player) {
            *slot = state;
        }
    }

    pub fn take_dma_cycles(&mut self) -> u32 {
        std::mem::take(&mut self.dma_cycles)
    }

    pub fn tick_ppu(&mut self) {
        self.ppu.tick(&mut self.cart);
    }
}

impl cpu::Bus for Bus {
    fn read(&mut self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x1FFF => self.ram[addr as usize & 0x7FF],
            0x2000..=0x3FFF => self.ppu.read_register(addr, &mut self.cart),
            0x4016 | 0x4017 => {
                let port = usize::from(addr == 0x4017);
                let value = self.shift[port] & 1;
                self.shift[port] = (self.shift[port] >> 1) | 0x80;
                value
            }
            0x4000..=0x401F => 0,
            _ => self.cart.read_prg(addr),
        }
    }

    fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x1FFF => self.ram[addr as usize & 0x7FF] = value,
            0x2000..=0x3FFF => self.ppu.write_register(addr, value, &mut self.cart),
            0x4014 => {
                let page = u16::from(value) << 8;
                for offset in 0..=255u8 {
                    let byte = self.read(page | u16::from(offset));
                    self.ppu.write_oam(offset, byte);
                }
                self.dma_cycles += 513;
            }
            0x4016 => {
                self.strobe = value & 1 != 0;
                if self.strobe {
                    self.shift = self.buttons;
                }
            }
            0x4000..=0x401F => {}
            _ => self.cart.write_prg(addr, value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nes::cpu::Bus as _;

    fn bus() -> Bus {
        let mut rom = vec![0u8; 16];
        rom[0..4].copy_from_slice(b"NES\x1A");
        rom[4] = 1;
        rom[5] = 1;
        rom.resize(16 + 0x4000 + 0x2000, 0);
        Bus::new(Cartridge::from_ines(&rom).unwrap())
    }

    #[test]
    fn ram_is_mirrored_every_two_kilobytes() {
        let mut bus = bus();
        bus.write(0x0001, 0x5A);
        assert_eq!(bus.read(0x0801), 0x5A);
        assert_eq!(bus.read(0x1801), 0x5A);
    }

    #[test]
    fn a_latched_controller_shifts_out_one_button_per_read() {
        let mut bus = bus();
        bus.set_buttons(0, 0b0000_1001);
        bus.write(0x4016, 1);
        bus.write(0x4016, 0);
        let reads: Vec<u8> = (0..8).map(|_| bus.read(0x4016)).collect();
        assert_eq!(reads, vec![1, 0, 0, 1, 0, 0, 0, 0]);
        assert_eq!(bus.read(0x4016), 1);
    }

    #[test]
    fn oam_dma_copies_a_page_and_charges_the_cpu() {
        let mut bus = bus();
        for offset in 0..256usize {
            bus.write(0x0300 + offset as u16, offset as u8);
        }
        bus.write(0x4014, 0x03);
        assert_eq!(bus.take_dma_cycles(), 513);
        assert_eq!(bus.ppu.read_register(0x2004, &mut bus.cart), 0x00);
    }
}
