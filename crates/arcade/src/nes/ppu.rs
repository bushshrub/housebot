//! Picture unit.  Rendering happens once per scanline rather than once per
//! dot: cheap, and accurate enough for the split-screen tricks games play at
//! horizontal blank.  Sprite-zero hit is reported at the dot it would occur on
//! so status polling loops still land in the right place.

use super::cart::{Cartridge, Mirroring};

pub const WIDTH: usize = 256;
pub const HEIGHT: usize = 240;
const DOTS_PER_LINE: u32 = 341;
const LINES_PER_FRAME: i32 = 262;
const PRE_RENDER_LINE: i32 = 261;

const CTRL_NMI: u8 = 0x80;
const CTRL_SPRITE_16: u8 = 0x20;
const CTRL_BG_TABLE: u8 = 0x10;
const CTRL_SPRITE_TABLE: u8 = 0x08;
const CTRL_INCREMENT: u8 = 0x04;

const MASK_GREYSCALE: u8 = 0x01;
const MASK_BG_LEFT: u8 = 0x02;
const MASK_SPRITE_LEFT: u8 = 0x04;
const MASK_BG: u8 = 0x08;
const MASK_SPRITES: u8 = 0x10;

const STATUS_OVERFLOW: u8 = 0x20;
const STATUS_SPRITE0: u8 = 0x40;
const STATUS_VBLANK: u8 = 0x80;

pub struct Ppu {
    ctrl: u8,
    mask: u8,
    status: u8,
    oam_addr: u8,
    v: u16,
    t: u16,
    fine_x: u8,
    write_toggle: bool,
    read_buffer: u8,
    vram: [u8; 0x800],
    palette: [u8; 32],
    oam: [u8; 256],
    scanline: i32,
    dot: u32,
    sprite0_hit_dot: Option<u32>,
    pub frame: Vec<u8>,
    pub frame_ready: bool,
    pub nmi_requested: bool,
}

impl Default for Ppu {
    fn default() -> Self {
        Self::new()
    }
}

impl Ppu {
    pub fn new() -> Self {
        Self {
            ctrl: 0,
            mask: 0,
            status: 0,
            oam_addr: 0,
            v: 0,
            t: 0,
            fine_x: 0,
            write_toggle: false,
            read_buffer: 0,
            vram: [0; 0x800],
            palette: [0; 32],
            oam: [0; 256],
            scanline: 0,
            dot: 0,
            sprite0_hit_dot: None,
            frame: vec![0; WIDTH * HEIGHT],
            frame_ready: false,
            nmi_requested: false,
        }
    }

    pub fn rendering(&self) -> bool {
        self.mask & (MASK_BG | MASK_SPRITES) != 0
    }

    pub fn tick(&mut self, cart: &mut Cartridge) {
        if let Some(hit_dot) = self.sprite0_hit_dot {
            if self.dot >= hit_dot {
                self.status |= STATUS_SPRITE0;
                self.sprite0_hit_dot = None;
            }
        }

        match (self.scanline, self.dot) {
            (0..=239, 1) => self.render_scanline(cart),
            (241, 1) => {
                self.status |= STATUS_VBLANK;
                self.frame_ready = true;
                if self.ctrl & CTRL_NMI != 0 {
                    self.nmi_requested = true;
                }
            }
            (PRE_RENDER_LINE, 1) => {
                self.status &= !(STATUS_VBLANK | STATUS_SPRITE0 | STATUS_OVERFLOW);
                self.sprite0_hit_dot = None;
            }
            _ => {}
        }

        if self.rendering() {
            if self.dot == 256 && (self.scanline < 240 || self.scanline == PRE_RENDER_LINE) {
                self.increment_y();
            }
            if self.dot == 257 && (self.scanline < 240 || self.scanline == PRE_RENDER_LINE) {
                self.v = (self.v & !0x041F) | (self.t & 0x041F);
            }
            if self.scanline == PRE_RENDER_LINE && (280..=304).contains(&self.dot) {
                self.v = (self.v & !0x7BE0) | (self.t & 0x7BE0);
            }
        }

        self.dot += 1;
        if self.dot >= DOTS_PER_LINE {
            self.dot = 0;
            self.scanline += 1;
            if self.scanline >= LINES_PER_FRAME {
                self.scanline = 0;
            }
        }
    }

    pub fn read_register(&mut self, addr: u16, cart: &mut Cartridge) -> u8 {
        match addr & 7 {
            2 => {
                let value = self.status;
                self.status &= !STATUS_VBLANK;
                self.write_toggle = false;
                value
            }
            4 => self.oam[self.oam_addr as usize],
            7 => {
                let addr = self.v & 0x3FFF;
                let value = if addr >= 0x3F00 {
                    self.read_buffer = self.read_vram(addr - 0x1000, cart);
                    self.palette_read(addr)
                } else {
                    let buffered = self.read_buffer;
                    self.read_buffer = self.read_vram(addr, cart);
                    buffered
                };
                self.increment_v();
                value
            }
            _ => 0,
        }
    }

    pub fn write_register(&mut self, addr: u16, value: u8, cart: &mut Cartridge) {
        match addr & 7 {
            0 => {
                let had_nmi = self.ctrl & CTRL_NMI != 0;
                self.ctrl = value;
                self.t = (self.t & !0x0C00) | (u16::from(value & 0x03) << 10);
                if !had_nmi && value & CTRL_NMI != 0 && self.status & STATUS_VBLANK != 0 {
                    self.nmi_requested = true;
                }
            }
            1 => self.mask = value,
            3 => self.oam_addr = value,
            4 => {
                self.oam[self.oam_addr as usize] = value;
                self.oam_addr = self.oam_addr.wrapping_add(1);
            }
            5 => {
                if self.write_toggle {
                    self.t = (self.t & !0x73E0)
                        | (u16::from(value & 0xF8) << 2)
                        | (u16::from(value & 0x07) << 12);
                } else {
                    self.t = (self.t & !0x001F) | u16::from(value >> 3);
                    self.fine_x = value & 0x07;
                }
                self.write_toggle = !self.write_toggle;
            }
            6 => {
                if self.write_toggle {
                    self.t = (self.t & 0xFF00) | u16::from(value);
                    self.v = self.t;
                } else {
                    self.t = (self.t & 0x00FF) | (u16::from(value & 0x3F) << 8);
                }
                self.write_toggle = !self.write_toggle;
            }
            7 => {
                let addr = self.v & 0x3FFF;
                self.write_vram(addr, value, cart);
                self.increment_v();
            }
            _ => {}
        }
    }

    pub fn write_oam(&mut self, offset: u8, value: u8) {
        self.oam[self.oam_addr.wrapping_add(offset) as usize] = value;
    }

    fn increment_v(&mut self) {
        let step = if self.ctrl & CTRL_INCREMENT != 0 {
            32
        } else {
            1
        };
        self.v = self.v.wrapping_add(step) & 0x7FFF;
    }

    fn increment_y(&mut self) {
        if self.v & 0x7000 != 0x7000 {
            self.v += 0x1000;
            return;
        }
        self.v &= !0x7000;
        let mut coarse_y = (self.v & 0x03E0) >> 5;
        if coarse_y == 29 {
            coarse_y = 0;
            self.v ^= 0x0800;
        } else if coarse_y == 31 {
            coarse_y = 0;
        } else {
            coarse_y += 1;
        }
        self.v = (self.v & !0x03E0) | (coarse_y << 5);
    }

    fn mirror(&self, addr: u16, cart: &Cartridge) -> usize {
        let index = (addr as usize - 0x2000) & 0x0FFF;
        let table = index / 0x400;
        let offset = index % 0x400;
        let bank = match cart.mirroring() {
            Mirroring::Horizontal => table / 2,
            Mirroring::Vertical => table % 2,
            Mirroring::SingleLower => 0,
            Mirroring::SingleUpper => 1,
            Mirroring::FourScreen => table % 2,
        };
        bank * 0x400 + offset
    }

    fn read_vram(&self, addr: u16, cart: &Cartridge) -> u8 {
        match addr {
            0x0000..=0x1FFF => cart.read_chr(addr),
            0x2000..=0x3EFF => self.vram[self.mirror(addr & 0x2FFF, cart)],
            _ => self.palette_read(addr),
        }
    }

    fn write_vram(&mut self, addr: u16, value: u8, cart: &mut Cartridge) {
        match addr {
            0x0000..=0x1FFF => cart.write_chr(addr, value),
            0x2000..=0x3EFF => {
                let index = self.mirror(addr & 0x2FFF, cart);
                self.vram[index] = value;
            }
            _ => {
                let index = palette_index(addr);
                self.palette[index] = value & 0x3F;
            }
        }
    }

    fn palette_read(&self, addr: u16) -> u8 {
        self.palette[palette_index(addr)]
    }

    fn render_scanline(&mut self, cart: &mut Cartridge) {
        let line = self.scanline as usize;
        let backdrop = self.palette[0];
        let mut background = [0u8; WIDTH];
        let mut colors = [backdrop; WIDTH];

        if self.mask & MASK_BG != 0 {
            let mut tiles = [0u8; WIDTH + 8];
            let mut palettes = [0u8; WIDTH + 8];
            let mut v = self.v;
            let fine_y = (v >> 12) & 7;
            let table = if self.ctrl & CTRL_BG_TABLE != 0 {
                0x1000
            } else {
                0
            };

            for tile in 0..33 {
                let tile_id = self.read_vram(0x2000 | (v & 0x0FFF), cart);
                let attribute = self.read_vram(
                    0x23C0 | (v & 0x0C00) | ((v >> 4) & 0x38) | ((v >> 2) & 0x07),
                    cart,
                );
                let quadrant = (((v >> 4) & 4) | (v & 2)) as u8;
                let palette_high = (attribute >> quadrant) & 0x03;

                let pattern = table + u16::from(tile_id) * 16 + fine_y;
                let low = self.read_vram(pattern, cart);
                let high = self.read_vram(pattern + 8, cart);

                for bit in 0..8usize {
                    let index = tile * 8 + bit;
                    if index >= tiles.len() {
                        break;
                    }
                    let shift = 7 - bit;
                    let value = ((low >> shift) & 1) | (((high >> shift) & 1) << 1);
                    tiles[index] = value;
                    palettes[index] = palette_high;
                }

                if v & 0x001F == 31 {
                    v &= !0x001F;
                    v ^= 0x0400;
                } else {
                    v += 1;
                }
            }

            for x in 0..WIDTH {
                let source = x + self.fine_x as usize;
                let value = tiles[source];
                background[x] = value;
                if value != 0 && (x >= 8 || self.mask & MASK_BG_LEFT != 0) {
                    colors[x] = self.palette[palette_index(
                        0x3F00 + u16::from(palettes[source]) * 4 + u16::from(value),
                    )];
                } else if value != 0 {
                    background[x] = 0;
                }
            }
        }

        if self.mask & MASK_SPRITES != 0 {
            self.render_sprites(cart, line, &background, &mut colors);
        }

        let greyscale = self.mask & MASK_GREYSCALE != 0;
        let row = &mut self.frame[line * WIDTH..(line + 1) * WIDTH];
        for (pixel, color) in row.iter_mut().zip(colors) {
            *pixel = if greyscale { color & 0x30 } else { color };
        }
    }

    fn render_sprites(
        &mut self,
        cart: &mut Cartridge,
        line: usize,
        background: &[u8; WIDTH],
        colors: &mut [u8; WIDTH],
    ) {
        let height: i32 = if self.ctrl & CTRL_SPRITE_16 != 0 {
            16
        } else {
            8
        };
        let mut drawn = 0;
        let mut covered = [false; WIDTH];

        for index in 0..64 {
            let entry = index * 4;
            let sprite_y = i32::from(self.oam[entry]);
            let row = line as i32 - sprite_y - 1;
            if row < 0 || row >= height {
                continue;
            }
            drawn += 1;
            if drawn > 8 {
                self.status |= STATUS_OVERFLOW;
                break;
            }

            let tile = self.oam[entry + 1];
            let attributes = self.oam[entry + 2];
            let sprite_x = usize::from(self.oam[entry + 3]);
            let flip_x = attributes & 0x40 != 0;
            let flip_y = attributes & 0x80 != 0;
            let behind = attributes & 0x20 != 0;
            let palette_high = attributes & 0x03;

            let row = if flip_y { height - 1 - row } else { row } as u16;
            let pattern = if height == 16 {
                let table = u16::from(tile & 1) * 0x1000;
                let tile = u16::from(tile & 0xFE) + u16::from(row >= 8);
                table + tile * 16 + (row & 7)
            } else {
                let table = if self.ctrl & CTRL_SPRITE_TABLE != 0 {
                    0x1000
                } else {
                    0
                };
                table + u16::from(tile) * 16 + row
            };
            let low = self.read_vram(pattern, cart);
            let high = self.read_vram(pattern + 8, cart);

            for bit in 0..8usize {
                let x = sprite_x + bit;
                if x >= WIDTH || covered[x] {
                    continue;
                }
                if x < 8 && self.mask & MASK_SPRITE_LEFT == 0 {
                    continue;
                }
                let shift = if flip_x { bit } else { 7 - bit };
                let value = ((low >> shift) & 1) | (((high >> shift) & 1) << 1);
                if value == 0 {
                    continue;
                }
                covered[x] = true;

                if index == 0
                    && background[x] != 0
                    && x != 255
                    && self.mask & MASK_BG != 0
                    && self.sprite0_hit_dot.is_none()
                    && self.status & STATUS_SPRITE0 == 0
                {
                    self.sprite0_hit_dot = Some(x as u32 + 2);
                }

                if !behind || background[x] == 0 {
                    colors[x] = self.palette
                        [palette_index(0x3F10 + u16::from(palette_high) * 4 + u16::from(value))];
                }
            }
        }
    }
}

fn palette_index(addr: u16) -> usize {
    let index = (addr as usize) & 0x1F;
    // Sprite palette entry 0 of each set is an alias of the backdrop colour.
    match index {
        0x10 | 0x14 | 0x18 | 0x1C => index - 0x10,
        _ => index,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cart() -> Cartridge {
        let mut rom = vec![0u8; 16];
        rom[0..4].copy_from_slice(b"NES\x1A");
        rom[4] = 1;
        rom[5] = 1;
        rom.resize(16 + 0x4000 + 0x2000, 0);
        Cartridge::from_ines(&rom).unwrap()
    }

    #[test]
    fn vblank_is_raised_and_cleared_once_per_frame() {
        let mut ppu = Ppu::new();
        let mut cart = cart();
        ppu.write_register(0x2000, CTRL_NMI, &mut cart);

        let mut nmis = 0;
        for _ in 0..DOTS_PER_LINE * LINES_PER_FRAME as u32 {
            ppu.tick(&mut cart);
            if std::mem::take(&mut ppu.nmi_requested) {
                nmis += 1;
            }
        }
        assert_eq!(nmis, 1);
        assert!(ppu.frame_ready);
    }

    #[test]
    fn reading_status_clears_vblank_and_the_address_latch() {
        let mut ppu = Ppu::new();
        let mut cart = cart();
        ppu.status |= STATUS_VBLANK;
        ppu.write_toggle = true;
        let value = ppu.read_register(0x2002, &mut cart);
        assert_ne!(value & STATUS_VBLANK, 0);
        assert_eq!(ppu.status & STATUS_VBLANK, 0);
        assert!(!ppu.write_toggle);
    }

    #[test]
    fn vram_reads_are_delayed_by_one_fetch_but_palettes_are_not() {
        let mut ppu = Ppu::new();
        let mut cart = cart();
        ppu.write_register(0x2006, 0x20, &mut cart);
        ppu.write_register(0x2006, 0x00, &mut cart);
        ppu.write_register(0x2007, 0x42, &mut cart);

        ppu.write_register(0x2006, 0x20, &mut cart);
        ppu.write_register(0x2006, 0x00, &mut cart);
        assert_eq!(ppu.read_register(0x2007, &mut cart), 0x00);
        assert_eq!(ppu.read_register(0x2007, &mut cart), 0x42);

        ppu.write_register(0x2006, 0x3F, &mut cart);
        ppu.write_register(0x2006, 0x01, &mut cart);
        ppu.write_register(0x2007, 0x15, &mut cart);
        ppu.write_register(0x2006, 0x3F, &mut cart);
        ppu.write_register(0x2006, 0x01, &mut cart);
        assert_eq!(ppu.read_register(0x2007, &mut cart), 0x15);
    }

    #[test]
    fn writes_land_in_the_mirrored_nametable() {
        let mut ppu = Ppu::new();
        let mut cart = cart();
        ppu.write_register(0x2006, 0x24, &mut cart);
        ppu.write_register(0x2006, 0x05, &mut cart);
        ppu.write_register(0x2007, 0x99, &mut cart);
        assert_eq!(ppu.vram[0x0005], 0x99);
    }

    #[test]
    fn a_background_tile_reaches_the_framebuffer() {
        let mut ppu = Ppu::new();
        let mut rom = vec![0u8; 16];
        rom[0..4].copy_from_slice(b"NES\x1A");
        rom[4] = 1;
        rom[5] = 1;
        rom.resize(16 + 0x4000 + 0x2000, 0);
        // Solid tile 1: every pixel uses colour 1 of the palette.
        for row in 0..8 {
            rom[16 + 0x4000 + 16 + row] = 0xFF;
        }
        let mut cart = Cartridge::from_ines(&rom).unwrap();

        ppu.write_register(0x2006, 0x3F, &mut cart);
        ppu.write_register(0x2006, 0x01, &mut cart);
        ppu.write_register(0x2007, 0x21, &mut cart);
        ppu.write_register(0x2006, 0x20, &mut cart);
        ppu.write_register(0x2006, 0x00, &mut cart);
        ppu.write_register(0x2007, 0x01, &mut cart);
        ppu.write_register(0x2001, MASK_BG | MASK_BG_LEFT, &mut cart);
        ppu.v = 0;

        ppu.render_scanline(&mut cart);
        assert_eq!(ppu.frame[0], 0x21);
        assert_eq!(ppu.frame[8], 0x00);
    }
}
