//! An original homebrew cartridge, assembled at runtime.
//!
//! It exists so the NES cabinet boots something legal out of the box, and so
//! the emulator is exercised end to end — palettes, nametable writes, OAM DMA,
//! NMI, controller reads and scrolling — without shipping anyone's ROM.

const PRG_BASE: u16 = 0x8000;
const BUTTONS: u8 = 0x10;
const SCROLL: u8 = 0x11;

struct Asm {
    code: Vec<u8>,
}

impl Asm {
    fn new() -> Self {
        Self { code: Vec::new() }
    }

    fn here(&self) -> u16 {
        PRG_BASE + self.code.len() as u16
    }

    fn emit(&mut self, bytes: &[u8]) {
        self.code.extend_from_slice(bytes);
    }

    /// Emits a branch to a label that is not written yet, returning the index
    /// of the displacement byte for [`Asm::land`] to fill in.
    fn branch_forward(&mut self, opcode: u8) -> usize {
        self.emit(&[opcode, 0]);
        self.code.len() - 1
    }

    fn land(&mut self, slot: usize) {
        let from = slot as i32 + 1;
        let to = self.code.len() as i32;
        self.code[slot] = (to - from) as u8;
    }

    fn branch_back(&mut self, opcode: u8, target: u16) {
        let from = self.here() as i32 + 2;
        self.emit(&[opcode, (target as i32 - from) as u8]);
    }
}

fn program() -> Vec<u8> {
    let mut asm = Asm::new();

    asm.emit(&[0x78, 0xD8]); // SEI, CLD
    asm.emit(&[0xA2, 0xFF, 0x9A]); // LDX #$FF, TXS
    asm.emit(&[0xA9, 0x00, 0x8D, 0x00, 0x20, 0x8D, 0x01, 0x20]); // silence PPU

    let wait_one = asm.here();
    asm.emit(&[0x2C, 0x02, 0x20]); // BIT $2002
    asm.branch_back(0x10, wait_one); // BPL
    let wait_two = asm.here();
    asm.emit(&[0x2C, 0x02, 0x20]);
    asm.branch_back(0x10, wait_two);

    // Palette: $3F00..$3F1F from the table at the end of the ROM.
    asm.emit(&[0xA9, 0x3F, 0x8D, 0x06, 0x20]);
    asm.emit(&[0xA9, 0x00, 0x8D, 0x06, 0x20]);
    asm.emit(&[0xA2, 0x00]); // LDX #0
    let palette_loop = asm.here();
    let palette_operand = asm.code.len() + 1;
    asm.emit(&[0xBD, 0x00, 0x00]); // LDA palette,X  (patched)
    asm.emit(&[0x8D, 0x07, 0x20, 0xE8, 0xE0, 0x20]); // STA $2007, INX, CPX #$20
    asm.branch_back(0xD0, palette_loop); // BNE

    // Nametable: 1024 bytes of a repeating tile pattern.
    asm.emit(&[0xA9, 0x20, 0x8D, 0x06, 0x20]);
    asm.emit(&[0xA9, 0x00, 0x8D, 0x06, 0x20]);
    asm.emit(&[0xA2, 0x04, 0xA0, 0x00]); // LDX #4 pages, LDY #0
    let fill_loop = asm.here();
    asm.emit(&[0x98, 0x4A, 0x4A, 0x29, 0x03]); // TYA, LSR, LSR, AND #3
    asm.emit(&[0x8D, 0x07, 0x20]); // STA $2007
    asm.emit(&[0xC8]); // INY
    asm.branch_back(0xD0, fill_loop); // BNE fill_loop
    asm.emit(&[0xCA]); // DEX
    asm.branch_back(0xD0, fill_loop); // BNE fill_loop

    // The player sprite lives in the DMA page at $0200.
    asm.emit(&[0xA9, 0x78, 0x8D, 0x00, 0x02]); // y
    asm.emit(&[0xA9, 0x04, 0x8D, 0x01, 0x02]); // tile
    asm.emit(&[0xA9, 0x00, 0x8D, 0x02, 0x02]); // attributes
    asm.emit(&[0xA9, 0x78, 0x8D, 0x03, 0x02]); // x
    asm.emit(&[0xA9, 0x00, 0x85, SCROLL, 0x85, BUTTONS]);

    asm.emit(&[0xA9, 0x80, 0x8D, 0x00, 0x20]); // NMI on
    asm.emit(&[0xA9, 0x1E, 0x8D, 0x01, 0x20]); // background + sprites
    let forever = asm.here();
    asm.emit(&[0x4C, forever as u8, (forever >> 8) as u8]);

    let nmi = asm.here();
    asm.emit(&[0xA9, 0x00, 0x8D, 0x03, 0x20]); // OAMADDR = 0
    asm.emit(&[0xA9, 0x02, 0x8D, 0x14, 0x40]); // OAM DMA from $0200

    asm.emit(&[0xA9, 0x01, 0x8D, 0x16, 0x40]); // strobe on
    asm.emit(&[0xA9, 0x00, 0x8D, 0x16, 0x40]); // strobe off
    asm.emit(&[0xA2, 0x08]); // LDX #8
    let read_loop = asm.here();
    asm.emit(&[0xAD, 0x16, 0x40, 0x4A, 0x26, BUTTONS, 0xCA]); // LDA, LSR, ROL zp, DEX
    asm.branch_back(0xD0, read_loop);

    // After eight rotations the first button read (A) sits in bit 7 and the
    // last (Right) in bit 0.
    for (mask, opcode, target) in [
        (0x08u8, 0xCEu8, 0x0200u16), // up    -> DEC sprite Y
        (0x04, 0xEE, 0x0200),        // down  -> INC sprite Y
        (0x02, 0xCE, 0x0203),        // left  -> DEC sprite X
        (0x01, 0xEE, 0x0203),        // right -> INC sprite X
    ] {
        asm.emit(&[0xA5, BUTTONS, 0x29, mask]);
        let skip = asm.branch_forward(0xF0); // BEQ
        asm.emit(&[opcode, target as u8, (target >> 8) as u8]);
        asm.land(skip);
    }

    asm.emit(&[0xE6, SCROLL]); // INC scroll
    asm.emit(&[0xA5, SCROLL, 0x4A, 0x8D, 0x05, 0x20]); // LDA scroll, LSR, STA $2005
    asm.emit(&[0xA9, 0x00, 0x8D, 0x05, 0x20]); // STA $2005 (Y = 0)
    asm.emit(&[0x40]); // RTI

    let palette = asm.here();
    asm.emit(&[
        0x0F, 0x21, 0x2A, 0x30, 0x0F, 0x21, 0x2A, 0x30, 0x0F, 0x21, 0x2A, 0x30, 0x0F, 0x21, 0x2A,
        0x30, 0x0F, 0x16, 0x27, 0x30, 0x0F, 0x16, 0x27, 0x30, 0x0F, 0x16, 0x27, 0x30, 0x0F, 0x16,
        0x27, 0x30,
    ]);

    asm.code[palette_operand] = palette as u8;
    asm.code[palette_operand + 1] = (palette >> 8) as u8;

    let mut prg = asm.code;
    prg.resize(0x4000, 0xEA);
    prg[0x3FFA] = nmi as u8;
    prg[0x3FFB] = (nmi >> 8) as u8;
    prg[0x3FFC] = PRG_BASE as u8;
    prg[0x3FFD] = (PRG_BASE >> 8) as u8;
    prg[0x3FFE] = nmi as u8;
    prg[0x3FFF] = (nmi >> 8) as u8;
    prg
}

fn tiles() -> Vec<u8> {
    let mut chr = vec![0u8; 0x2000];
    let mut tile = |index: usize, low: [u8; 8], high: [u8; 8]| {
        chr[index * 16..index * 16 + 8].copy_from_slice(&low);
        chr[index * 16 + 8..index * 16 + 16].copy_from_slice(&high);
    };

    tile(1, [0xFF; 8], [0x00; 8]);
    tile(
        2,
        [0x00; 8],
        [0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55],
    );
    tile(
        3,
        [0x3C, 0x7E, 0xFF, 0xFF, 0xFF, 0xFF, 0x7E, 0x3C],
        [0xFF; 8],
    );
    tile(
        4,
        [0x3C, 0x7E, 0xDB, 0xFF, 0xFF, 0xDB, 0x66, 0x3C],
        [0x3C, 0x42, 0x81, 0x81, 0x81, 0x81, 0x42, 0x3C],
    );
    chr
}

/// A complete iNES image for the built-in demo.
pub fn rom() -> Vec<u8> {
    let mut rom = vec![0u8; 16];
    rom[0..4].copy_from_slice(b"NES\x1A");
    rom[4] = 1;
    rom[5] = 1;
    rom[6] = 0x01; // vertical mirroring, mapper 0
    rom.extend_from_slice(&program());
    rom.extend_from_slice(&tiles());
    rom
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nes::ppu::{HEIGHT, WIDTH};
    use crate::nes::{Nes, BUTTON_RIGHT};

    #[test]
    fn the_demo_draws_something_and_answers_the_pad() {
        let mut nes = Nes::load(&rom()).expect("demo rom loads");
        for _ in 0..12 {
            nes.run_frame();
        }

        let frame = nes.frame();
        assert_eq!(frame.len(), WIDTH * HEIGHT);
        let distinct: std::collections::BTreeSet<u8> = frame.iter().copied().collect();
        assert!(
            distinct.len() >= 3,
            "expected a drawn background, got {distinct:?}"
        );

        // The sprite's top row is its OAM Y plus one, so scan the band around it.
        let sprite_column = |nes: &Nes| {
            (HEIGHT / 2..HEIGHT / 2 + 10).find_map(|row| {
                nes.frame()[row * WIDTH..(row + 1) * WIDTH]
                    .iter()
                    .position(|&pixel| pixel == 0x16)
            })
        };
        let before = sprite_column(&nes);
        nes.set_buttons(0, BUTTON_RIGHT);
        for _ in 0..30 {
            nes.run_frame();
        }
        let after = sprite_column(&nes);
        assert!(
            before.is_some() && after > before,
            "sprite should have moved right: {before:?} -> {after:?}"
        );
    }
}
