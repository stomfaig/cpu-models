/// RV32I base integer instruction set

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Opcode {
    // R-type
    Add, Sub, And, Or, Xor, Sll, Srl, Sra, Slt, Sltu,
    // I-type ALU
    Addi, Andi, Ori, Xori, Slli, Srli, Srai, Slti, Sltiu,
    // Loads
    Lb, Lh, Lw, Lbu, Lhu,
    // Stores
    Sb, Sh, Sw,
    // Branches
    Beq, Bne, Blt, Bge, Bltu, Bgeu,
    // Jumps
    Jal, Jalr,
    // Upper immediate
    Lui, Auipc,
}

#[derive(Clone, Copy, Debug)]
pub enum InstrFormat { R, I, S, B, U, J }

#[derive(Clone, Copy, Debug)]
pub struct DecodedInstr {
    pub opcode: Opcode,
    pub format: InstrFormat,
    pub rd:     u8,
    pub rs1:    u8,
    pub rs2:    u8,
    pub imm:    i32,
}

pub struct InstResult {
    pub rd:     u8,
    pub result: u32,
    pub format: InstrFormat,
}

/// Decode a raw 32-bit RV32I instruction word.
/// Returns None if the encoding is unrecognized.
pub fn decode(word: u32) -> Option<DecodedInstr> {
    let opcode_bits = word & 0x7f;
    let rd          = ((word >> 7)  & 0x1f) as u8;
    let funct3      = ((word >> 12) & 0x07) as u8;
    let rs1         = ((word >> 15) & 0x1f) as u8;
    let rs2         = ((word >> 20) & 0x1f) as u8;
    let funct7      = ((word >> 25) & 0x7f) as u8;

    let imm_i = (word as i32) >> 20;
    let imm_s = (((word >> 25) as i32) << 5) | ((word >> 7) & 0x1f) as i32;
    let imm_b = (((word as i32) >> 31) << 12)
        | (((word >> 7) & 1) as i32) << 11
        | (((word >> 25) & 0x3f) as i32) << 5
        | (((word >> 8) & 0xf) as i32) << 1;
    let imm_u = (word & 0xfffff000) as i32;
    let imm_j = (((word as i32) >> 31) << 20)
        | (((word >> 12) & 0xff) as i32) << 12
        | (((word >> 20) & 1) as i32) << 11
        | (((word >> 21) & 0x3ff) as i32) << 1;

    let (opcode, format, imm) = match opcode_bits {
        0b0110011 => {
            let op = match (funct3, funct7) {
                (0x0, 0x00) => Opcode::Add,
                (0x0, 0x20) => Opcode::Sub,
                (0x7, 0x00) => Opcode::And,
                (0x6, 0x00) => Opcode::Or,
                (0x4, 0x00) => Opcode::Xor,
                (0x1, 0x00) => Opcode::Sll,
                (0x5, 0x00) => Opcode::Srl,
                (0x5, 0x20) => Opcode::Sra,
                (0x2, 0x00) => Opcode::Slt,
                (0x3, 0x00) => Opcode::Sltu,
                _ => return None,
            };
            (op, InstrFormat::R, 0)
        }
        0b0010011 => {
            let op = match funct3 {
                0x0 => Opcode::Addi,
                0x7 => Opcode::Andi,
                0x6 => Opcode::Ori,
                0x4 => Opcode::Xori,
                0x1 => Opcode::Slli,
                0x5 => if funct7 == 0x20 { Opcode::Srai } else { Opcode::Srli },
                0x2 => Opcode::Slti,
                0x3 => Opcode::Sltiu,
                _ => return None,
            };
            (op, InstrFormat::I, imm_i)
        }
        0b0000011 => {
            let op = match funct3 {
                0x0 => Opcode::Lb,
                0x1 => Opcode::Lh,
                0x2 => Opcode::Lw,
                0x4 => Opcode::Lbu,
                0x5 => Opcode::Lhu,
                _ => return None,
            };
            (op, InstrFormat::I, imm_i)
        }
        0b0100011 => {
            let op = match funct3 {
                0x0 => Opcode::Sb,
                0x1 => Opcode::Sh,
                0x2 => Opcode::Sw,
                _ => return None,
            };
            (op, InstrFormat::S, imm_s)
        }
        0b1100011 => {
            let op = match funct3 {
                0x0 => Opcode::Beq,
                0x1 => Opcode::Bne,
                0x4 => Opcode::Blt,
                0x5 => Opcode::Bge,
                0x6 => Opcode::Bltu,
                0x7 => Opcode::Bgeu,
                _ => return None,
            };
            (op, InstrFormat::B, imm_b)
        }
        0b1101111 => (Opcode::Jal,  InstrFormat::J, imm_j),
        0b1100111 => (Opcode::Jalr, InstrFormat::I, imm_i),
        0b0110111 => (Opcode::Lui,  InstrFormat::U, imm_u),
        0b0010111 => (Opcode::Auipc, InstrFormat::U, imm_u),
        _ => return None,
    };

    Some(DecodedInstr { opcode, format, rd, rs1, rs2, imm })
}
