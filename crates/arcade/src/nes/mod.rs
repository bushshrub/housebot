//! A NES emulator core: 6502, PPU, controllers, and mappers 0/1/2/3.
//!
//! No ROMs ship with this crate.  Point `HOUSEBOT_ARCADE_ROMS` at a directory
//! of cartridge images you are entitled to run — homebrew, public domain, or
//! your own dumps.
//!
//! Not emulated: the APU (the arcade is silent), and mappers beyond the four
//! above.  Timing is scanline-accurate rather than dot-accurate, which is
//! enough for the common status-bar split but not for raster effects that
//! change state mid-scanline.

pub mod bus;
pub mod cart;
pub mod cpu;
pub mod demo;
pub mod palette;
pub mod ppu;

use bus::Bus;
use cart::Cartridge;
use cpu::Cpu;

/// A frame is 29781 CPU cycles; the ceiling stops a wedged ROM from spinning
/// forever inside a request handler.
const MAX_CYCLES_PER_FRAME: u32 = 50_000;

pub const BUTTON_A: u8 = 0x01;
pub const BUTTON_B: u8 = 0x02;
pub const BUTTON_SELECT: u8 = 0x04;
pub const BUTTON_START: u8 = 0x08;
pub const BUTTON_UP: u8 = 0x10;
pub const BUTTON_DOWN: u8 = 0x20;
pub const BUTTON_LEFT: u8 = 0x40;
pub const BUTTON_RIGHT: u8 = 0x80;

pub struct Nes {
    cpu: Cpu,
    bus: Bus,
}

impl Nes {
    pub fn load(rom: &[u8]) -> Result<Self, String> {
        let mut bus = Bus::new(Cartridge::from_ines(rom)?);
        let mut cpu = Cpu::new();
        cpu.reset(&mut bus);
        Ok(Self { cpu, bus })
    }

    pub fn reset(&mut self) {
        self.cpu.reset(&mut self.bus);
    }

    pub fn set_buttons(&mut self, player: usize, state: u8) {
        self.bus.set_buttons(player, state);
    }

    pub fn frame(&self) -> &[u8] {
        &self.bus.ppu.frame
    }

    pub fn run_frame(&mut self) {
        let mut spent = 0;
        while spent < MAX_CYCLES_PER_FRAME {
            let cycles = self.cpu.step(&mut self.bus) + self.bus.take_dma_cycles();
            spent += cycles;
            for _ in 0..cycles * 3 {
                self.bus.tick_ppu();
            }
            if std::mem::take(&mut self.bus.ppu.nmi_requested) {
                self.cpu.trigger_nmi();
            }
            if std::mem::take(&mut self.bus.ppu.frame_ready) {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ppu::{HEIGHT, WIDTH};

    /// A cartridge whose reset vector runs `program`, with the PPU told to
    /// render so frames actually advance.
    fn cartridge(program: &[u8]) -> Vec<u8> {
        let mut rom = vec![0u8; 16];
        rom[0..4].copy_from_slice(b"NES\x1A");
        rom[4] = 1;
        rom[5] = 1;
        rom.resize(16 + 0x4000 + 0x2000, 0);
        rom[16..16 + program.len()].copy_from_slice(program);
        // Reset vector -> $8000, which is where PRG is mapped.
        rom[16 + 0x3FFC] = 0x00;
        rom[16 + 0x3FFD] = 0x80;
        rom
    }

    #[test]
    fn refuses_a_rom_it_cannot_run() {
        assert!(Nes::load(b"junk").is_err());
    }

    #[test]
    fn runs_a_frame_and_produces_a_full_framebuffer() {
        // LDA #$80 / STA $2000 (enable NMI), then spin.
        let mut nes = Nes::load(&cartridge(&[
            0xA9, 0x80, 0x8D, 0x00, 0x20, 0x4C, 0x05, 0x80,
        ]))
        .expect("cartridge loads");
        nes.run_frame();
        assert_eq!(nes.frame().len(), WIDTH * HEIGHT);
    }

    #[test]
    fn a_wedged_rom_still_returns_from_run_frame() {
        // JAM: the CPU never advances, so only the cycle ceiling ends the frame.
        let mut nes = Nes::load(&cartridge(&[0x02])).expect("cartridge loads");
        nes.run_frame();
        assert_eq!(nes.frame().len(), WIDTH * HEIGHT);
    }

    #[test]
    fn button_state_reaches_the_controller_port() {
        let program = [
            0xA9, 0x01, 0x8D, 0x16, 0x40, // strobe on
            0xA9, 0x00, 0x8D, 0x16, 0x40, // strobe off
            0xAD, 0x16, 0x40, // read A
            0x8D, 0x00, 0x03, // store to $0300
            0x4C, 0x10, 0x80,
        ];
        let mut nes = Nes::load(&cartridge(&program)).expect("cartridge loads");
        nes.set_buttons(0, BUTTON_A);
        for _ in 0..8 {
            nes.cpu.step(&mut nes.bus);
        }
        assert_eq!(nes.cpu.a & 1, 1);
    }
}
