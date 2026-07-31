//! MOS 6502 core (NES variant: no decimal mode).

pub trait Bus {
    fn read(&mut self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, value: u8);
}

pub const CARRY: u8 = 0x01;
pub const ZERO: u8 = 0x02;
pub const INTERRUPT: u8 = 0x04;
pub const DECIMAL: u8 = 0x08;
pub const BREAK: u8 = 0x10;
pub const UNUSED: u8 = 0x20;
pub const OVERFLOW: u8 = 0x40;
pub const NEGATIVE: u8 = 0x80;

const STACK_BASE: u16 = 0x0100;
const NMI_VECTOR: u16 = 0xFFFA;
const RESET_VECTOR: u16 = 0xFFFC;
const IRQ_VECTOR: u16 = 0xFFFE;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Imp,
    Acc,
    Imm,
    Zp,
    ZpX,
    ZpY,
    Abs,
    AbsX,
    AbsY,
    Ind,
    IndX,
    IndY,
    Rel,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    Adc,
    And,
    Asl,
    Bcc,
    Bcs,
    Beq,
    Bit,
    Bmi,
    Bne,
    Bpl,
    Brk,
    Bvc,
    Bvs,
    Clc,
    Cld,
    Cli,
    Clv,
    Cmp,
    Cpx,
    Cpy,
    Dec,
    Dex,
    Dey,
    Eor,
    Inc,
    Inx,
    Iny,
    Jmp,
    Jsr,
    Lda,
    Ldx,
    Ldy,
    Lsr,
    Nop,
    Ora,
    Pha,
    Php,
    Pla,
    Plp,
    Rol,
    Ror,
    Rti,
    Rts,
    Sbc,
    Sec,
    Sed,
    Sei,
    Sta,
    Stx,
    Sty,
    Tax,
    Tay,
    Tsx,
    Txa,
    Txs,
    Tya,
    // Undocumented opcodes that shipped games are known to rely on.
    Lax,
    Sax,
    Dcp,
    Isb,
    Slo,
    Rla,
    Sre,
    Rra,
    Anc,
    Alr,
    Arr,
    Axs,
    // Unstable undocumented opcodes: decoded so timing stays right, then ignored.
    Halt,
}

struct Instruction {
    op: Op,
    mode: Mode,
    cycles: u8,
    page_penalty: bool,
}

const fn ins(op: Op, mode: Mode, cycles: u8, page_penalty: bool) -> Instruction {
    Instruction {
        op,
        mode,
        cycles,
        page_penalty,
    }
}

pub struct Cpu {
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub sp: u8,
    pub pc: u16,
    pub p: u8,
    nmi_pending: bool,
    irq_line: bool,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    pub fn new() -> Self {
        Self {
            a: 0,
            x: 0,
            y: 0,
            sp: 0xFD,
            pc: 0,
            p: INTERRUPT | UNUSED,
            nmi_pending: false,
            irq_line: false,
        }
    }

    pub fn reset(&mut self, bus: &mut impl Bus) {
        self.sp = 0xFD;
        self.p = INTERRUPT | UNUSED;
        self.pc = self.read_word(bus, RESET_VECTOR);
        self.nmi_pending = false;
        self.irq_line = false;
    }

    pub fn trigger_nmi(&mut self) {
        self.nmi_pending = true;
    }

    pub fn set_irq(&mut self, active: bool) {
        self.irq_line = active;
    }

    pub fn step(&mut self, bus: &mut impl Bus) -> u32 {
        if self.nmi_pending {
            self.nmi_pending = false;
            return self.interrupt(bus, NMI_VECTOR);
        }
        if self.irq_line && self.p & INTERRUPT == 0 {
            return self.interrupt(bus, IRQ_VECTOR);
        }

        let opcode = self.fetch(bus);
        let Instruction {
            op,
            mode,
            cycles,
            page_penalty,
        } = decode(opcode);
        let (addr, crossed) = self.operand(bus, mode);
        let mut spent = u32::from(cycles);
        if page_penalty && crossed {
            spent += 1;
        }
        spent += self.execute(bus, op, mode, addr);
        spent
    }

    fn interrupt(&mut self, bus: &mut impl Bus, vector: u16) -> u32 {
        let pc = self.pc;
        self.push(bus, (pc >> 8) as u8);
        self.push(bus, pc as u8);
        self.push(bus, (self.p | UNUSED) & !BREAK);
        self.p |= INTERRUPT;
        self.pc = self.read_word(bus, vector);
        7
    }

    fn fetch(&mut self, bus: &mut impl Bus) -> u8 {
        let value = bus.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        value
    }

    fn fetch_word(&mut self, bus: &mut impl Bus) -> u16 {
        let low = self.fetch(bus);
        let high = self.fetch(bus);
        u16::from_le_bytes([low, high])
    }

    fn read_word(&mut self, bus: &mut impl Bus, addr: u16) -> u16 {
        let low = bus.read(addr);
        let high = bus.read(addr.wrapping_add(1));
        u16::from_le_bytes([low, high])
    }

    /// Reads a word without letting the high byte leave the page — the 6502's
    /// indirect-jump bug, which some games use deliberately.
    fn read_word_wrapped(&mut self, bus: &mut impl Bus, addr: u16) -> u16 {
        let low = bus.read(addr);
        let high = bus.read((addr & 0xFF00) | u16::from((addr as u8).wrapping_add(1)));
        u16::from_le_bytes([low, high])
    }

    fn operand(&mut self, bus: &mut impl Bus, mode: Mode) -> (u16, bool) {
        match mode {
            Mode::Imp | Mode::Acc => (0, false),
            Mode::Imm => {
                let addr = self.pc;
                self.pc = self.pc.wrapping_add(1);
                (addr, false)
            }
            Mode::Zp => (u16::from(self.fetch(bus)), false),
            Mode::ZpX => (u16::from(self.fetch(bus).wrapping_add(self.x)), false),
            Mode::ZpY => (u16::from(self.fetch(bus).wrapping_add(self.y)), false),
            Mode::Abs => (self.fetch_word(bus), false),
            Mode::AbsX => {
                let base = self.fetch_word(bus);
                let addr = base.wrapping_add(u16::from(self.x));
                (addr, page_crossed(base, addr))
            }
            Mode::AbsY => {
                let base = self.fetch_word(bus);
                let addr = base.wrapping_add(u16::from(self.y));
                (addr, page_crossed(base, addr))
            }
            Mode::Ind => {
                let pointer = self.fetch_word(bus);
                (self.read_word_wrapped(bus, pointer), false)
            }
            Mode::IndX => {
                let pointer = self.fetch(bus).wrapping_add(self.x);
                (self.read_word_wrapped(bus, u16::from(pointer)), false)
            }
            Mode::IndY => {
                let pointer = self.fetch(bus);
                let base = self.read_word_wrapped(bus, u16::from(pointer));
                let addr = base.wrapping_add(u16::from(self.y));
                (addr, page_crossed(base, addr))
            }
            Mode::Rel => {
                let offset = self.fetch(bus) as i8;
                (self.pc.wrapping_add(offset as u16), false)
            }
        }
    }

    fn load(&mut self, bus: &mut impl Bus, mode: Mode, addr: u16) -> u8 {
        match mode {
            Mode::Acc => self.a,
            _ => bus.read(addr),
        }
    }

    fn store(&mut self, bus: &mut impl Bus, mode: Mode, addr: u16, value: u8) {
        match mode {
            Mode::Acc => self.a = value,
            _ => bus.write(addr, value),
        }
    }

    fn push(&mut self, bus: &mut impl Bus, value: u8) {
        bus.write(STACK_BASE | u16::from(self.sp), value);
        self.sp = self.sp.wrapping_sub(1);
    }

    fn pull(&mut self, bus: &mut impl Bus) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        bus.read(STACK_BASE | u16::from(self.sp))
    }

    fn set_flag(&mut self, flag: u8, on: bool) {
        if on {
            self.p |= flag;
        } else {
            self.p &= !flag;
        }
    }

    fn set_zn(&mut self, value: u8) {
        self.set_flag(ZERO, value == 0);
        self.set_flag(NEGATIVE, value & 0x80 != 0);
    }

    fn branch(&mut self, target: u16, take: bool) -> u32 {
        if !take {
            return 0;
        }
        let extra = if page_crossed(self.pc, target) { 2 } else { 1 };
        self.pc = target;
        extra
    }

    fn compare(&mut self, register: u8, value: u8) {
        let result = register.wrapping_sub(value);
        self.set_flag(CARRY, register >= value);
        self.set_zn(result);
    }

    fn adc(&mut self, value: u8) {
        let carry = u16::from(self.p & CARRY);
        let sum = u16::from(self.a) + u16::from(value) + carry;
        let result = sum as u8;
        self.set_flag(CARRY, sum > 0xFF);
        self.set_flag(OVERFLOW, (self.a ^ result) & (value ^ result) & 0x80 != 0);
        self.a = result;
        self.set_zn(result);
    }

    fn sbc(&mut self, value: u8) {
        self.adc(!value);
    }

    fn asl(&mut self, value: u8) -> u8 {
        self.set_flag(CARRY, value & 0x80 != 0);
        let result = value << 1;
        self.set_zn(result);
        result
    }

    fn lsr(&mut self, value: u8) -> u8 {
        self.set_flag(CARRY, value & 0x01 != 0);
        let result = value >> 1;
        self.set_zn(result);
        result
    }

    fn rol(&mut self, value: u8) -> u8 {
        let carry_in = self.p & CARRY;
        self.set_flag(CARRY, value & 0x80 != 0);
        let result = (value << 1) | carry_in;
        self.set_zn(result);
        result
    }

    fn ror(&mut self, value: u8) -> u8 {
        let carry_in = (self.p & CARRY) << 7;
        self.set_flag(CARRY, value & 0x01 != 0);
        let result = (value >> 1) | carry_in;
        self.set_zn(result);
        result
    }

    fn execute(&mut self, bus: &mut impl Bus, op: Op, mode: Mode, addr: u16) -> u32 {
        match op {
            Op::Adc => {
                let value = self.load(bus, mode, addr);
                self.adc(value);
            }
            Op::And => {
                self.a &= self.load(bus, mode, addr);
                let a = self.a;
                self.set_zn(a);
            }
            Op::Asl => {
                let value = self.load(bus, mode, addr);
                let result = self.asl(value);
                self.store(bus, mode, addr, result);
            }
            Op::Bcc => return self.branch(addr, self.p & CARRY == 0),
            Op::Bcs => return self.branch(addr, self.p & CARRY != 0),
            Op::Beq => return self.branch(addr, self.p & ZERO != 0),
            Op::Bmi => return self.branch(addr, self.p & NEGATIVE != 0),
            Op::Bne => return self.branch(addr, self.p & ZERO == 0),
            Op::Bpl => return self.branch(addr, self.p & NEGATIVE == 0),
            Op::Bvc => return self.branch(addr, self.p & OVERFLOW == 0),
            Op::Bvs => return self.branch(addr, self.p & OVERFLOW != 0),
            Op::Bit => {
                let value = self.load(bus, mode, addr);
                self.set_flag(ZERO, self.a & value == 0);
                self.set_flag(OVERFLOW, value & OVERFLOW != 0);
                self.set_flag(NEGATIVE, value & NEGATIVE != 0);
            }
            Op::Brk => {
                self.pc = self.pc.wrapping_add(1);
                let pc = self.pc;
                self.push(bus, (pc >> 8) as u8);
                self.push(bus, pc as u8);
                self.push(bus, self.p | BREAK | UNUSED);
                self.p |= INTERRUPT;
                self.pc = self.read_word(bus, IRQ_VECTOR);
            }
            Op::Clc => self.set_flag(CARRY, false),
            Op::Cld => self.set_flag(DECIMAL, false),
            Op::Cli => self.set_flag(INTERRUPT, false),
            Op::Clv => self.set_flag(OVERFLOW, false),
            Op::Cmp => {
                let value = self.load(bus, mode, addr);
                self.compare(self.a, value);
            }
            Op::Cpx => {
                let value = self.load(bus, mode, addr);
                self.compare(self.x, value);
            }
            Op::Cpy => {
                let value = self.load(bus, mode, addr);
                self.compare(self.y, value);
            }
            Op::Dec => {
                let value = self.load(bus, mode, addr).wrapping_sub(1);
                self.store(bus, mode, addr, value);
                self.set_zn(value);
            }
            Op::Dex => {
                self.x = self.x.wrapping_sub(1);
                let x = self.x;
                self.set_zn(x);
            }
            Op::Dey => {
                self.y = self.y.wrapping_sub(1);
                let y = self.y;
                self.set_zn(y);
            }
            Op::Eor => {
                self.a ^= self.load(bus, mode, addr);
                let a = self.a;
                self.set_zn(a);
            }
            Op::Inc => {
                let value = self.load(bus, mode, addr).wrapping_add(1);
                self.store(bus, mode, addr, value);
                self.set_zn(value);
            }
            Op::Inx => {
                self.x = self.x.wrapping_add(1);
                let x = self.x;
                self.set_zn(x);
            }
            Op::Iny => {
                self.y = self.y.wrapping_add(1);
                let y = self.y;
                self.set_zn(y);
            }
            Op::Jmp => self.pc = addr,
            Op::Jsr => {
                let return_to = self.pc.wrapping_sub(1);
                self.push(bus, (return_to >> 8) as u8);
                self.push(bus, return_to as u8);
                self.pc = addr;
            }
            Op::Lda => {
                self.a = self.load(bus, mode, addr);
                let a = self.a;
                self.set_zn(a);
            }
            Op::Ldx => {
                self.x = self.load(bus, mode, addr);
                let x = self.x;
                self.set_zn(x);
            }
            Op::Ldy => {
                self.y = self.load(bus, mode, addr);
                let y = self.y;
                self.set_zn(y);
            }
            Op::Lsr => {
                let value = self.load(bus, mode, addr);
                let result = self.lsr(value);
                self.store(bus, mode, addr, result);
            }
            Op::Nop => {
                if mode != Mode::Imp && mode != Mode::Acc {
                    self.load(bus, mode, addr);
                }
            }
            Op::Ora => {
                self.a |= self.load(bus, mode, addr);
                let a = self.a;
                self.set_zn(a);
            }
            Op::Pha => self.push(bus, self.a),
            Op::Php => self.push(bus, self.p | BREAK | UNUSED),
            Op::Pla => {
                self.a = self.pull(bus);
                let a = self.a;
                self.set_zn(a);
            }
            Op::Plp => {
                self.p = (self.pull(bus) & !BREAK) | UNUSED;
            }
            Op::Rol => {
                let value = self.load(bus, mode, addr);
                let result = self.rol(value);
                self.store(bus, mode, addr, result);
            }
            Op::Ror => {
                let value = self.load(bus, mode, addr);
                let result = self.ror(value);
                self.store(bus, mode, addr, result);
            }
            Op::Rti => {
                self.p = (self.pull(bus) & !BREAK) | UNUSED;
                let low = self.pull(bus);
                let high = self.pull(bus);
                self.pc = u16::from_le_bytes([low, high]);
            }
            Op::Rts => {
                let low = self.pull(bus);
                let high = self.pull(bus);
                self.pc = u16::from_le_bytes([low, high]).wrapping_add(1);
            }
            Op::Sbc => {
                let value = self.load(bus, mode, addr);
                self.sbc(value);
            }
            Op::Sec => self.set_flag(CARRY, true),
            Op::Sed => self.set_flag(DECIMAL, true),
            Op::Sei => self.set_flag(INTERRUPT, true),
            Op::Sta => bus.write(addr, self.a),
            Op::Stx => bus.write(addr, self.x),
            Op::Sty => bus.write(addr, self.y),
            Op::Tax => {
                self.x = self.a;
                let x = self.x;
                self.set_zn(x);
            }
            Op::Tay => {
                self.y = self.a;
                let y = self.y;
                self.set_zn(y);
            }
            Op::Tsx => {
                self.x = self.sp;
                let x = self.x;
                self.set_zn(x);
            }
            Op::Txa => {
                self.a = self.x;
                let a = self.a;
                self.set_zn(a);
            }
            Op::Txs => self.sp = self.x,
            Op::Tya => {
                self.a = self.y;
                let a = self.a;
                self.set_zn(a);
            }
            Op::Lax => {
                let value = self.load(bus, mode, addr);
                self.a = value;
                self.x = value;
                self.set_zn(value);
            }
            Op::Sax => bus.write(addr, self.a & self.x),
            Op::Dcp => {
                let value = self.load(bus, mode, addr).wrapping_sub(1);
                bus.write(addr, value);
                self.compare(self.a, value);
            }
            Op::Isb => {
                let value = self.load(bus, mode, addr).wrapping_add(1);
                bus.write(addr, value);
                self.sbc(value);
            }
            Op::Slo => {
                let value = self.load(bus, mode, addr);
                let result = self.asl(value);
                bus.write(addr, result);
                self.a |= result;
                let a = self.a;
                self.set_zn(a);
            }
            Op::Rla => {
                let value = self.load(bus, mode, addr);
                let result = self.rol(value);
                bus.write(addr, result);
                self.a &= result;
                let a = self.a;
                self.set_zn(a);
            }
            Op::Sre => {
                let value = self.load(bus, mode, addr);
                let result = self.lsr(value);
                bus.write(addr, result);
                self.a ^= result;
                let a = self.a;
                self.set_zn(a);
            }
            Op::Rra => {
                let value = self.load(bus, mode, addr);
                let result = self.ror(value);
                bus.write(addr, result);
                self.adc(result);
            }
            Op::Anc => {
                self.a &= self.load(bus, mode, addr);
                let a = self.a;
                self.set_zn(a);
                self.set_flag(CARRY, a & 0x80 != 0);
            }
            Op::Alr => {
                self.a &= self.load(bus, mode, addr);
                let a = self.a;
                self.a = self.lsr(a);
            }
            Op::Arr => {
                self.a &= self.load(bus, mode, addr);
                let a = self.a;
                self.a = self.ror(a);
                let result = self.a;
                self.set_flag(CARRY, result & 0x40 != 0);
                self.set_flag(OVERFLOW, (result & 0x40) ^ ((result & 0x20) << 1) != 0);
            }
            Op::Axs => {
                let value = self.load(bus, mode, addr);
                let base = self.a & self.x;
                self.x = base.wrapping_sub(value);
                self.set_flag(CARRY, base >= value);
                let x = self.x;
                self.set_zn(x);
            }
            Op::Halt => self.pc = self.pc.wrapping_sub(1),
        }
        0
    }
}

fn page_crossed(from: u16, to: u16) -> bool {
    from & 0xFF00 != to & 0xFF00
}

fn decode(opcode: u8) -> Instruction {
    use Mode::*;
    use Op::*;
    match opcode {
        0x00 => ins(Brk, Imp, 7, false),
        0x01 => ins(Ora, IndX, 6, false),
        0x03 => ins(Slo, IndX, 8, false),
        0x04 => ins(Nop, Zp, 3, false),
        0x05 => ins(Ora, Zp, 3, false),
        0x06 => ins(Asl, Zp, 5, false),
        0x07 => ins(Slo, Zp, 5, false),
        0x08 => ins(Php, Imp, 3, false),
        0x09 => ins(Ora, Imm, 2, false),
        0x0A => ins(Asl, Acc, 2, false),
        0x0B => ins(Anc, Imm, 2, false),
        0x0C => ins(Nop, Abs, 4, false),
        0x0D => ins(Ora, Abs, 4, false),
        0x0E => ins(Asl, Abs, 6, false),
        0x0F => ins(Slo, Abs, 6, false),
        0x10 => ins(Bpl, Rel, 2, false),
        0x11 => ins(Ora, IndY, 5, true),
        0x13 => ins(Slo, IndY, 8, false),
        0x14 => ins(Nop, ZpX, 4, false),
        0x15 => ins(Ora, ZpX, 4, false),
        0x16 => ins(Asl, ZpX, 6, false),
        0x17 => ins(Slo, ZpX, 6, false),
        0x18 => ins(Clc, Imp, 2, false),
        0x19 => ins(Ora, AbsY, 4, true),
        0x1A => ins(Nop, Imp, 2, false),
        0x1B => ins(Slo, AbsY, 7, false),
        0x1C => ins(Nop, AbsX, 4, true),
        0x1D => ins(Ora, AbsX, 4, true),
        0x1E => ins(Asl, AbsX, 7, false),
        0x1F => ins(Slo, AbsX, 7, false),
        0x20 => ins(Jsr, Abs, 6, false),
        0x21 => ins(And, IndX, 6, false),
        0x23 => ins(Rla, IndX, 8, false),
        0x24 => ins(Bit, Zp, 3, false),
        0x25 => ins(And, Zp, 3, false),
        0x26 => ins(Rol, Zp, 5, false),
        0x27 => ins(Rla, Zp, 5, false),
        0x28 => ins(Plp, Imp, 4, false),
        0x29 => ins(And, Imm, 2, false),
        0x2A => ins(Rol, Acc, 2, false),
        0x2B => ins(Anc, Imm, 2, false),
        0x2C => ins(Bit, Abs, 4, false),
        0x2D => ins(And, Abs, 4, false),
        0x2E => ins(Rol, Abs, 6, false),
        0x2F => ins(Rla, Abs, 6, false),
        0x30 => ins(Bmi, Rel, 2, false),
        0x31 => ins(And, IndY, 5, true),
        0x33 => ins(Rla, IndY, 8, false),
        0x34 => ins(Nop, ZpX, 4, false),
        0x35 => ins(And, ZpX, 4, false),
        0x36 => ins(Rol, ZpX, 6, false),
        0x37 => ins(Rla, ZpX, 6, false),
        0x38 => ins(Sec, Imp, 2, false),
        0x39 => ins(And, AbsY, 4, true),
        0x3A => ins(Nop, Imp, 2, false),
        0x3B => ins(Rla, AbsY, 7, false),
        0x3C => ins(Nop, AbsX, 4, true),
        0x3D => ins(And, AbsX, 4, true),
        0x3E => ins(Rol, AbsX, 7, false),
        0x3F => ins(Rla, AbsX, 7, false),
        0x40 => ins(Rti, Imp, 6, false),
        0x41 => ins(Eor, IndX, 6, false),
        0x43 => ins(Sre, IndX, 8, false),
        0x44 => ins(Nop, Zp, 3, false),
        0x45 => ins(Eor, Zp, 3, false),
        0x46 => ins(Lsr, Zp, 5, false),
        0x47 => ins(Sre, Zp, 5, false),
        0x48 => ins(Pha, Imp, 3, false),
        0x49 => ins(Eor, Imm, 2, false),
        0x4A => ins(Lsr, Acc, 2, false),
        0x4B => ins(Alr, Imm, 2, false),
        0x4C => ins(Jmp, Abs, 3, false),
        0x4D => ins(Eor, Abs, 4, false),
        0x4E => ins(Lsr, Abs, 6, false),
        0x4F => ins(Sre, Abs, 6, false),
        0x50 => ins(Bvc, Rel, 2, false),
        0x51 => ins(Eor, IndY, 5, true),
        0x53 => ins(Sre, IndY, 8, false),
        0x54 => ins(Nop, ZpX, 4, false),
        0x55 => ins(Eor, ZpX, 4, false),
        0x56 => ins(Lsr, ZpX, 6, false),
        0x57 => ins(Sre, ZpX, 6, false),
        0x58 => ins(Cli, Imp, 2, false),
        0x59 => ins(Eor, AbsY, 4, true),
        0x5A => ins(Nop, Imp, 2, false),
        0x5B => ins(Sre, AbsY, 7, false),
        0x5C => ins(Nop, AbsX, 4, true),
        0x5D => ins(Eor, AbsX, 4, true),
        0x5E => ins(Lsr, AbsX, 7, false),
        0x5F => ins(Sre, AbsX, 7, false),
        0x60 => ins(Rts, Imp, 6, false),
        0x61 => ins(Adc, IndX, 6, false),
        0x63 => ins(Rra, IndX, 8, false),
        0x64 => ins(Nop, Zp, 3, false),
        0x65 => ins(Adc, Zp, 3, false),
        0x66 => ins(Ror, Zp, 5, false),
        0x67 => ins(Rra, Zp, 5, false),
        0x68 => ins(Pla, Imp, 4, false),
        0x69 => ins(Adc, Imm, 2, false),
        0x6A => ins(Ror, Acc, 2, false),
        0x6B => ins(Arr, Imm, 2, false),
        0x6C => ins(Jmp, Ind, 5, false),
        0x6D => ins(Adc, Abs, 4, false),
        0x6E => ins(Ror, Abs, 6, false),
        0x6F => ins(Rra, Abs, 6, false),
        0x70 => ins(Bvs, Rel, 2, false),
        0x71 => ins(Adc, IndY, 5, true),
        0x73 => ins(Rra, IndY, 8, false),
        0x74 => ins(Nop, ZpX, 4, false),
        0x75 => ins(Adc, ZpX, 4, false),
        0x76 => ins(Ror, ZpX, 6, false),
        0x77 => ins(Rra, ZpX, 6, false),
        0x78 => ins(Sei, Imp, 2, false),
        0x79 => ins(Adc, AbsY, 4, true),
        0x7A => ins(Nop, Imp, 2, false),
        0x7B => ins(Rra, AbsY, 7, false),
        0x7C => ins(Nop, AbsX, 4, true),
        0x7D => ins(Adc, AbsX, 4, true),
        0x7E => ins(Ror, AbsX, 7, false),
        0x7F => ins(Rra, AbsX, 7, false),
        0x80 => ins(Nop, Imm, 2, false),
        0x81 => ins(Sta, IndX, 6, false),
        0x82 => ins(Nop, Imm, 2, false),
        0x83 => ins(Sax, IndX, 6, false),
        0x84 => ins(Sty, Zp, 3, false),
        0x85 => ins(Sta, Zp, 3, false),
        0x86 => ins(Stx, Zp, 3, false),
        0x87 => ins(Sax, Zp, 3, false),
        0x88 => ins(Dey, Imp, 2, false),
        0x89 => ins(Nop, Imm, 2, false),
        0x8A => ins(Txa, Imp, 2, false),
        0x8C => ins(Sty, Abs, 4, false),
        0x8D => ins(Sta, Abs, 4, false),
        0x8E => ins(Stx, Abs, 4, false),
        0x8F => ins(Sax, Abs, 4, false),
        0x90 => ins(Bcc, Rel, 2, false),
        0x91 => ins(Sta, IndY, 6, false),
        0x94 => ins(Sty, ZpX, 4, false),
        0x95 => ins(Sta, ZpX, 4, false),
        0x96 => ins(Stx, ZpY, 4, false),
        0x97 => ins(Sax, ZpY, 4, false),
        0x98 => ins(Tya, Imp, 2, false),
        0x99 => ins(Sta, AbsY, 5, false),
        0x9A => ins(Txs, Imp, 2, false),
        0x9D => ins(Sta, AbsX, 5, false),
        0xA0 => ins(Ldy, Imm, 2, false),
        0xA1 => ins(Lda, IndX, 6, false),
        0xA2 => ins(Ldx, Imm, 2, false),
        0xA3 => ins(Lax, IndX, 6, false),
        0xA4 => ins(Ldy, Zp, 3, false),
        0xA5 => ins(Lda, Zp, 3, false),
        0xA6 => ins(Ldx, Zp, 3, false),
        0xA7 => ins(Lax, Zp, 3, false),
        0xA8 => ins(Tay, Imp, 2, false),
        0xA9 => ins(Lda, Imm, 2, false),
        0xAA => ins(Tax, Imp, 2, false),
        0xAC => ins(Ldy, Abs, 4, false),
        0xAD => ins(Lda, Abs, 4, false),
        0xAE => ins(Ldx, Abs, 4, false),
        0xAF => ins(Lax, Abs, 4, false),
        0xB0 => ins(Bcs, Rel, 2, false),
        0xB1 => ins(Lda, IndY, 5, true),
        0xB3 => ins(Lax, IndY, 5, true),
        0xB4 => ins(Ldy, ZpX, 4, false),
        0xB5 => ins(Lda, ZpX, 4, false),
        0xB6 => ins(Ldx, ZpY, 4, false),
        0xB7 => ins(Lax, ZpY, 4, false),
        0xB8 => ins(Clv, Imp, 2, false),
        0xB9 => ins(Lda, AbsY, 4, true),
        0xBA => ins(Tsx, Imp, 2, false),
        0xBC => ins(Ldy, AbsX, 4, true),
        0xBD => ins(Lda, AbsX, 4, true),
        0xBE => ins(Ldx, AbsY, 4, true),
        0xBF => ins(Lax, AbsY, 4, true),
        0xC0 => ins(Cpy, Imm, 2, false),
        0xC1 => ins(Cmp, IndX, 6, false),
        0xC2 => ins(Nop, Imm, 2, false),
        0xC3 => ins(Dcp, IndX, 8, false),
        0xC4 => ins(Cpy, Zp, 3, false),
        0xC5 => ins(Cmp, Zp, 3, false),
        0xC6 => ins(Dec, Zp, 5, false),
        0xC7 => ins(Dcp, Zp, 5, false),
        0xC8 => ins(Iny, Imp, 2, false),
        0xC9 => ins(Cmp, Imm, 2, false),
        0xCA => ins(Dex, Imp, 2, false),
        0xCB => ins(Axs, Imm, 2, false),
        0xCC => ins(Cpy, Abs, 4, false),
        0xCD => ins(Cmp, Abs, 4, false),
        0xCE => ins(Dec, Abs, 6, false),
        0xCF => ins(Dcp, Abs, 6, false),
        0xD0 => ins(Bne, Rel, 2, false),
        0xD1 => ins(Cmp, IndY, 5, true),
        0xD3 => ins(Dcp, IndY, 8, false),
        0xD4 => ins(Nop, ZpX, 4, false),
        0xD5 => ins(Cmp, ZpX, 4, false),
        0xD6 => ins(Dec, ZpX, 6, false),
        0xD7 => ins(Dcp, ZpX, 6, false),
        0xD8 => ins(Cld, Imp, 2, false),
        0xD9 => ins(Cmp, AbsY, 4, true),
        0xDA => ins(Nop, Imp, 2, false),
        0xDB => ins(Dcp, AbsY, 7, false),
        0xDC => ins(Nop, AbsX, 4, true),
        0xDD => ins(Cmp, AbsX, 4, true),
        0xDE => ins(Dec, AbsX, 7, false),
        0xDF => ins(Dcp, AbsX, 7, false),
        0xE0 => ins(Cpx, Imm, 2, false),
        0xE1 => ins(Sbc, IndX, 6, false),
        0xE2 => ins(Nop, Imm, 2, false),
        0xE3 => ins(Isb, IndX, 8, false),
        0xE4 => ins(Cpx, Zp, 3, false),
        0xE5 => ins(Sbc, Zp, 3, false),
        0xE6 => ins(Inc, Zp, 5, false),
        0xE7 => ins(Isb, Zp, 5, false),
        0xE8 => ins(Inx, Imp, 2, false),
        0xE9 => ins(Sbc, Imm, 2, false),
        0xEA => ins(Nop, Imp, 2, false),
        0xEB => ins(Sbc, Imm, 2, false),
        0xEC => ins(Cpx, Abs, 4, false),
        0xED => ins(Sbc, Abs, 4, false),
        0xEE => ins(Inc, Abs, 6, false),
        0xEF => ins(Isb, Abs, 6, false),
        0xF0 => ins(Beq, Rel, 2, false),
        0xF1 => ins(Sbc, IndY, 5, true),
        0xF3 => ins(Isb, IndY, 8, false),
        0xF4 => ins(Nop, ZpX, 4, false),
        0xF5 => ins(Sbc, ZpX, 4, false),
        0xF6 => ins(Inc, ZpX, 6, false),
        0xF7 => ins(Isb, ZpX, 6, false),
        0xF8 => ins(Sed, Imp, 2, false),
        0xF9 => ins(Sbc, AbsY, 4, true),
        0xFA => ins(Nop, Imp, 2, false),
        0xFB => ins(Isb, AbsY, 7, false),
        0xFC => ins(Nop, AbsX, 4, true),
        0xFD => ins(Sbc, AbsX, 4, true),
        0xFE => ins(Inc, AbsX, 7, false),
        0xFF => ins(Isb, AbsX, 7, false),
        _ => ins(Halt, Imp, 2, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ram(Vec<u8>);

    impl Bus for Ram {
        fn read(&mut self, addr: u16) -> u8 {
            self.0[addr as usize]
        }
        fn write(&mut self, addr: u16, value: u8) {
            self.0[addr as usize] = value;
        }
    }

    fn run(program: &[u8], steps: usize) -> (Cpu, Ram) {
        let mut ram = Ram(vec![0; 0x10000]);
        ram.0[0x8000..0x8000 + program.len()].copy_from_slice(program);
        ram.0[RESET_VECTOR as usize] = 0x00;
        ram.0[RESET_VECTOR as usize + 1] = 0x80;
        let mut cpu = Cpu::new();
        cpu.reset(&mut ram);
        for _ in 0..steps {
            cpu.step(&mut ram);
        }
        (cpu, ram)
    }

    #[test]
    fn loads_adds_and_stores() {
        let (cpu, ram) = run(&[0xA9, 0x2A, 0x69, 0x08, 0x8D, 0x00, 0x02], 3);
        assert_eq!(cpu.a, 0x32);
        assert_eq!(ram.0[0x0200], 0x32);
        assert_eq!(cpu.p & ZERO, 0);
    }

    #[test]
    fn adc_sets_carry_and_overflow_like_the_hardware() {
        let (cpu, _) = run(&[0xA9, 0x50, 0x69, 0x50], 2);
        assert_eq!(cpu.a, 0xA0);
        assert_ne!(cpu.p & OVERFLOW, 0);
        assert_eq!(cpu.p & CARRY, 0);

        let (cpu, _) = run(&[0xA9, 0xFF, 0x69, 0x02], 2);
        assert_eq!(cpu.a, 0x01);
        assert_ne!(cpu.p & CARRY, 0);
        assert_eq!(cpu.p & OVERFLOW, 0);
    }

    #[test]
    fn sbc_borrows_when_carry_is_clear() {
        let (cpu, _) = run(&[0x38, 0xA9, 0x10, 0xE9, 0x01], 3);
        assert_eq!(cpu.a, 0x0F);
        assert_ne!(cpu.p & CARRY, 0);
    }

    #[test]
    fn branches_are_taken_and_cost_an_extra_cycle() {
        let mut ram = Ram(vec![0; 0x10000]);
        ram.0[0x8000..0x8003].copy_from_slice(&[0xA9, 0x00, 0xF0]);
        ram.0[0x8003] = 0x02;
        ram.0[RESET_VECTOR as usize] = 0x00;
        ram.0[RESET_VECTOR as usize + 1] = 0x80;
        let mut cpu = Cpu::new();
        cpu.reset(&mut ram);
        cpu.step(&mut ram);
        let cycles = cpu.step(&mut ram);
        assert_eq!(cpu.pc, 0x8006);
        assert_eq!(cycles, 3);
    }

    #[test]
    fn jsr_and_rts_round_trip_through_the_stack() {
        let mut ram = Ram(vec![0; 0x10000]);
        ram.0[0x8000..0x8003].copy_from_slice(&[0x20, 0x10, 0x80]);
        ram.0[0x8010] = 0x60;
        ram.0[RESET_VECTOR as usize] = 0x00;
        ram.0[RESET_VECTOR as usize + 1] = 0x80;
        let mut cpu = Cpu::new();
        cpu.reset(&mut ram);
        cpu.step(&mut ram);
        assert_eq!(cpu.pc, 0x8010);
        cpu.step(&mut ram);
        assert_eq!(cpu.pc, 0x8003);
    }

    #[test]
    fn indirect_jump_wraps_inside_its_page() {
        let mut ram = Ram(vec![0; 0x10000]);
        ram.0[0x8000..0x8003].copy_from_slice(&[0x6C, 0xFF, 0x30]);
        ram.0[0x30FF] = 0x34;
        ram.0[0x3000] = 0x12;
        ram.0[RESET_VECTOR as usize] = 0x00;
        ram.0[RESET_VECTOR as usize + 1] = 0x80;
        let mut cpu = Cpu::new();
        cpu.reset(&mut ram);
        cpu.step(&mut ram);
        assert_eq!(cpu.pc, 0x1234);
    }

    #[test]
    fn nmi_pushes_state_and_jumps_through_the_vector() {
        let mut ram = Ram(vec![0; 0x10000]);
        ram.0[RESET_VECTOR as usize] = 0x00;
        ram.0[RESET_VECTOR as usize + 1] = 0x80;
        ram.0[NMI_VECTOR as usize] = 0x00;
        ram.0[NMI_VECTOR as usize + 1] = 0x90;
        let mut cpu = Cpu::new();
        cpu.reset(&mut ram);
        cpu.trigger_nmi();
        let cycles = cpu.step(&mut ram);
        assert_eq!(cpu.pc, 0x9000);
        assert_eq!(cycles, 7);
        assert_eq!(cpu.sp, 0xFA);
    }

    #[test]
    fn every_opcode_decodes_without_panicking() {
        for opcode in 0..=u8::MAX {
            let instruction = decode(opcode);
            assert!(instruction.cycles >= 2);
        }
    }
}
