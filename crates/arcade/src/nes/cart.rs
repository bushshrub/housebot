//! iNES cartridge loading and the mappers this emulator supports.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mirroring {
    Horizontal,
    Vertical,
    SingleLower,
    SingleUpper,
    FourScreen,
}

#[derive(Debug, Clone, Copy)]
enum Mapper {
    Nrom,
    Mmc1 {
        shift: u8,
        count: u8,
        control: u8,
        chr0: u8,
        chr1: u8,
        prg: u8,
    },
    UxRom {
        bank: u8,
    },
    CnRom {
        bank: u8,
    },
}

pub struct Cartridge {
    prg: Vec<u8>,
    chr: Vec<u8>,
    prg_ram: Vec<u8>,
    chr_is_ram: bool,
    mapper: Mapper,
    mirroring: Mirroring,
}

const PRG_BANK: usize = 0x4000;
const CHR_BANK: usize = 0x2000;

impl Cartridge {
    pub fn from_ines(rom: &[u8]) -> Result<Self, String> {
        if rom.len() < 16 || &rom[0..4] != b"NES\x1A" {
            return Err("not an iNES ROM".into());
        }
        let prg_banks = rom[4] as usize;
        let chr_banks = rom[5] as usize;
        if prg_banks == 0 {
            return Err("ROM declares no PRG banks".into());
        }
        let flags6 = rom[6];
        let flags7 = rom[7];
        let mapper_id = (flags7 & 0xF0) | (flags6 >> 4);

        let mut offset = 16;
        if flags6 & 0x04 != 0 {
            offset += 512;
        }
        let prg_len = prg_banks * PRG_BANK;
        let chr_len = chr_banks * CHR_BANK;
        if rom.len() < offset + prg_len + chr_len {
            return Err("ROM is shorter than its header claims".into());
        }

        let prg = rom[offset..offset + prg_len].to_vec();
        let chr_is_ram = chr_banks == 0;
        let chr = if chr_is_ram {
            vec![0; CHR_BANK]
        } else {
            rom[offset + prg_len..offset + prg_len + chr_len].to_vec()
        };

        let mirroring = if flags6 & 0x08 != 0 {
            Mirroring::FourScreen
        } else if flags6 & 0x01 != 0 {
            Mirroring::Vertical
        } else {
            Mirroring::Horizontal
        };

        let mapper = match mapper_id {
            0 => Mapper::Nrom,
            1 => Mapper::Mmc1 {
                shift: 0,
                count: 0,
                control: 0x0C,
                chr0: 0,
                chr1: 0,
                prg: 0,
            },
            2 => Mapper::UxRom { bank: 0 },
            3 => Mapper::CnRom { bank: 0 },
            other => return Err(format!("mapper {other} is not supported")),
        };

        Ok(Self {
            prg,
            chr,
            prg_ram: vec![0; 0x2000],
            chr_is_ram,
            mapper,
            mirroring,
        })
    }

    pub fn mirroring(&self) -> Mirroring {
        match self.mapper {
            Mapper::Mmc1 { control, .. } => match control & 0x03 {
                0 => Mirroring::SingleLower,
                1 => Mirroring::SingleUpper,
                2 => Mirroring::Vertical,
                _ => Mirroring::Horizontal,
            },
            _ => self.mirroring,
        }
    }

    pub fn read_prg(&self, addr: u16) -> u8 {
        if addr < 0x8000 {
            return self.prg_ram[(addr as usize - 0x6000) & 0x1FFF];
        }
        let banks = self.prg.len() / PRG_BANK;
        let last = banks.saturating_sub(1);
        let slot = (addr as usize - 0x8000) & 0x3FFF;
        let high = addr >= 0xC000;

        let bank = match self.mapper {
            Mapper::Nrom => {
                if high {
                    last
                } else {
                    0
                }
            }
            Mapper::UxRom { bank } => {
                if high {
                    last
                } else {
                    bank as usize % banks.max(1)
                }
            }
            Mapper::CnRom { .. } => {
                if high {
                    last.min(1)
                } else {
                    0
                }
            }
            Mapper::Mmc1 { control, prg, .. } => match (control >> 2) & 0x03 {
                0 | 1 => {
                    let base = (prg as usize & 0x0E) % banks.max(1);
                    base + usize::from(high)
                }
                2 => {
                    if high {
                        prg as usize % banks.max(1)
                    } else {
                        0
                    }
                }
                _ => {
                    if high {
                        last
                    } else {
                        prg as usize % banks.max(1)
                    }
                }
            },
        };
        let index = bank * PRG_BANK + slot;
        self.prg[index % self.prg.len()]
    }

    pub fn write_prg(&mut self, addr: u16, value: u8) {
        if addr < 0x8000 {
            self.prg_ram[(addr as usize - 0x6000) & 0x1FFF] = value;
            return;
        }
        match &mut self.mapper {
            Mapper::Nrom => {}
            Mapper::UxRom { bank } => *bank = value & 0x0F,
            Mapper::CnRom { bank } => *bank = value & 0x03,
            Mapper::Mmc1 {
                shift,
                count,
                control,
                chr0,
                chr1,
                prg,
            } => {
                // A write with bit 7 set resets the serial port and forces the
                // PRG mode back to "fix last bank", which is how MMC1 games
                // recover a known state on boot.
                if value & 0x80 != 0 {
                    *shift = 0;
                    *count = 0;
                    *control |= 0x0C;
                    return;
                }
                *shift = (*shift >> 1) | ((value & 1) << 4);
                *count += 1;
                if *count < 5 {
                    return;
                }
                let loaded = *shift & 0x1F;
                *shift = 0;
                *count = 0;
                match addr {
                    0x8000..=0x9FFF => *control = loaded,
                    0xA000..=0xBFFF => *chr0 = loaded,
                    0xC000..=0xDFFF => *chr1 = loaded,
                    _ => *prg = loaded,
                }
            }
        }
    }

    pub fn read_chr(&self, addr: u16) -> u8 {
        let index = self.chr_index(addr);
        self.chr[index % self.chr.len()]
    }

    pub fn write_chr(&mut self, addr: u16, value: u8) {
        if !self.chr_is_ram {
            return;
        }
        let index = self.chr_index(addr);
        let len = self.chr.len();
        self.chr[index % len] = value;
    }

    fn chr_index(&self, addr: u16) -> usize {
        let addr = addr as usize & 0x1FFF;
        match self.mapper {
            Mapper::Nrom | Mapper::UxRom { .. } => addr,
            Mapper::CnRom { bank } => bank as usize * CHR_BANK + addr,
            Mapper::Mmc1 {
                control,
                chr0,
                chr1,
                ..
            } => {
                if control & 0x10 == 0 {
                    (chr0 as usize & 0x1E) * 0x1000 + addr
                } else if addr < 0x1000 {
                    chr0 as usize * 0x1000 + addr
                } else {
                    chr1 as usize * 0x1000 + (addr - 0x1000)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rom(mapper: u8, prg_banks: u8, chr_banks: u8) -> Vec<u8> {
        let mut rom = vec![0u8; 16];
        rom[0..4].copy_from_slice(b"NES\x1A");
        rom[4] = prg_banks;
        rom[5] = chr_banks;
        rom[6] = (mapper & 0x0F) << 4;
        rom[7] = mapper & 0xF0;
        rom.resize(
            16 + prg_banks as usize * PRG_BANK + chr_banks as usize * CHR_BANK,
            0,
        );
        rom
    }

    #[test]
    fn rejects_anything_that_is_not_an_ines_image() {
        assert!(Cartridge::from_ines(b"hello").is_err());
        assert!(Cartridge::from_ines(&rom(9, 1, 1)).is_err());
        let mut truncated = rom(0, 2, 1);
        truncated.truncate(200);
        assert!(Cartridge::from_ines(&truncated).is_err());
    }

    #[test]
    fn a_16k_nrom_image_is_mirrored_into_both_slots() {
        let mut image = rom(0, 1, 1);
        image[16] = 0xAB;
        let cart = Cartridge::from_ines(&image).unwrap();
        assert_eq!(cart.read_prg(0x8000), 0xAB);
        assert_eq!(cart.read_prg(0xC000), 0xAB);
    }

    #[test]
    fn uxrom_switches_the_low_bank_and_pins_the_last_one() {
        let mut image = rom(2, 4, 0);
        image[16 + PRG_BANK] = 0x11;
        image[16 + 3 * PRG_BANK] = 0x33;
        let mut cart = Cartridge::from_ines(&image).unwrap();
        cart.write_prg(0x8000, 1);
        assert_eq!(cart.read_prg(0x8000), 0x11);
        assert_eq!(cart.read_prg(0xC000), 0x33);
    }

    #[test]
    fn mmc1_takes_five_writes_to_load_a_register() {
        let mut image = rom(1, 2, 1);
        image[16] = 0x77;
        let mut cart = Cartridge::from_ines(&image).unwrap();
        for bit in [0, 1, 0, 0, 0] {
            cart.write_prg(0x8000, bit);
        }
        assert_eq!(cart.mirroring(), Mirroring::Vertical);

        cart.write_prg(0x8000, 0x80);
        assert_eq!(cart.read_prg(0x8000), 0x77);
    }

    #[test]
    fn chr_ram_is_writable_but_chr_rom_is_not() {
        let mut ram_cart = Cartridge::from_ines(&rom(2, 2, 0)).unwrap();
        ram_cart.write_chr(0x0010, 0x5A);
        assert_eq!(ram_cart.read_chr(0x0010), 0x5A);

        let mut rom_cart = Cartridge::from_ines(&rom(0, 1, 1)).unwrap();
        rom_cart.write_chr(0x0010, 0x5A);
        assert_eq!(rom_cart.read_chr(0x0010), 0x00);
    }
}
