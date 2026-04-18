use std::collections::HashMap;

#[derive(Debug)]
pub enum AsmError {
    UnknownInstruction(String),
    UnknownRegister(String),
    BadOperands(String),
    UndefinedLabel(String),
    ImmediateOutOfRange(String),
}

impl std::fmt::Display for AsmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AsmError::UnknownInstruction(s) => write!(f, "unknown instruction: {s}"),
            AsmError::UnknownRegister(s)    => write!(f, "unknown register: {s}"),
            AsmError::BadOperands(s)        => write!(f, "bad operands: {s}"),
            AsmError::UndefinedLabel(s)     => write!(f, "undefined label: {s}"),
            AsmError::ImmediateOutOfRange(s) => write!(f, "immediate out of range: {s}"),
        }
    }
}

pub fn assemble(source: &str) -> Result<Vec<u32>, AsmError> {
    // Two passes: first collect labels, then emit.
    let lines = clean(source);
    let labels = collect_labels(&lines)?;
    emit(&lines, &labels)
}

// ── Pass 1: strip comments, blank lines, record label positions ───────────────

fn clean(source: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut addr = 0usize;
    for raw in source.lines() {
        let line = raw.split('#').next().unwrap().trim().to_string();
        if line.is_empty() { continue; }
        if line.ends_with(':') { out.push((addr, line)); continue; }
        out.push((addr, line));
        addr += 4;
    }
    out
}

fn collect_labels(lines: &[(usize, String)]) -> Result<HashMap<String, u32>, AsmError> {
    let mut labels = HashMap::new();
    for (addr, line) in lines {
        if let Some(label) = line.strip_suffix(':') {
            labels.insert(label.trim().to_string(), *addr as u32);
        }
    }
    Ok(labels)
}

// ── Pass 2: emit instruction words ───────────────────────────────────────────

fn emit(lines: &[(usize, String)], labels: &HashMap<String, u32>) -> Result<Vec<u32>, AsmError> {
    let mut out = Vec::new();
    for (addr, line) in lines {
        if line.ends_with(':') { continue; }
        let word = assemble_line(line, *addr as u32, labels)?;
        out.push(word);
    }
    Ok(out)
}

fn assemble_line(line: &str, pc: u32, labels: &HashMap<String, u32>) -> Result<u32, AsmError> {
    let (mnemonic, rest) = line.split_once(char::is_whitespace)
        .map(|(m, r)| (m, r.trim()))
        .unwrap_or((line, ""));

    // Expand pseudo-instructions first
    match mnemonic {
        "nop"  => return Ok(0x00000013),                                // addi x0, x0, 0
        "mv"   => { let (rd, rs) = reg2(rest)?; return Ok(i_type(rd, rs, 0, 0x0, 0b0010011)); }
        "li"   => { let (rd, imm) = reg_imm(rest, pc, labels)?; return Ok(i_type(rd, 0, imm, 0x0, 0b0010011)); }
        "j"    => { let off = resolve(rest, pc, labels)?; return Ok(j_type(0, off)); }
        "jr"   => { let rs = reg(rest)?; return Ok(i_type(0, rs, 0, 0x0, 0b1100111)); }
        "ret"  => return Ok(i_type(0, 1, 0, 0x0, 0b1100111)),           // jalr x0, x1, 0
        "not"  => { let (rd, rs) = reg2(rest)?; return Ok(i_type(rd, rs, -1, 0x4, 0b0010011)); } // xori rd, rs, -1
        "neg"  => { let (rd, rs) = reg2(rest)?; return Ok(r_type(rd, 0, rs, 0x0, 0x20, 0b0110011)); } // sub rd, x0, rs
        _ => {}
    }

    match mnemonic {
        // R-type
        "add"  => { let (rd, rs1, rs2) = reg3(rest)?; Ok(r_type(rd, rs1, rs2, 0x0, 0x00, 0b0110011)) }
        "sub"  => { let (rd, rs1, rs2) = reg3(rest)?; Ok(r_type(rd, rs1, rs2, 0x0, 0x20, 0b0110011)) }
        "and"  => { let (rd, rs1, rs2) = reg3(rest)?; Ok(r_type(rd, rs1, rs2, 0x7, 0x00, 0b0110011)) }
        "or"   => { let (rd, rs1, rs2) = reg3(rest)?; Ok(r_type(rd, rs1, rs2, 0x6, 0x00, 0b0110011)) }
        "xor"  => { let (rd, rs1, rs2) = reg3(rest)?; Ok(r_type(rd, rs1, rs2, 0x4, 0x00, 0b0110011)) }
        "sll"  => { let (rd, rs1, rs2) = reg3(rest)?; Ok(r_type(rd, rs1, rs2, 0x1, 0x00, 0b0110011)) }
        "srl"  => { let (rd, rs1, rs2) = reg3(rest)?; Ok(r_type(rd, rs1, rs2, 0x5, 0x00, 0b0110011)) }
        "sra"  => { let (rd, rs1, rs2) = reg3(rest)?; Ok(r_type(rd, rs1, rs2, 0x5, 0x20, 0b0110011)) }
        "slt"  => { let (rd, rs1, rs2) = reg3(rest)?; Ok(r_type(rd, rs1, rs2, 0x2, 0x00, 0b0110011)) }
        "sltu" => { let (rd, rs1, rs2) = reg3(rest)?; Ok(r_type(rd, rs1, rs2, 0x3, 0x00, 0b0110011)) }

        // I-type ALU
        "addi"  => { let (rd, rs1, imm) = reg_reg_imm(rest, pc, labels)?; Ok(i_type(rd, rs1, imm, 0x0, 0b0010011)) }
        "andi"  => { let (rd, rs1, imm) = reg_reg_imm(rest, pc, labels)?; Ok(i_type(rd, rs1, imm, 0x7, 0b0010011)) }
        "ori"   => { let (rd, rs1, imm) = reg_reg_imm(rest, pc, labels)?; Ok(i_type(rd, rs1, imm, 0x6, 0b0010011)) }
        "xori"  => { let (rd, rs1, imm) = reg_reg_imm(rest, pc, labels)?; Ok(i_type(rd, rs1, imm, 0x4, 0b0010011)) }
        "slli"  => { let (rd, rs1, imm) = reg_reg_imm(rest, pc, labels)?; Ok(i_type(rd, rs1, imm, 0x1, 0b0010011)) }
        "srli"  => { let (rd, rs1, imm) = reg_reg_imm(rest, pc, labels)?; Ok(i_type(rd, rs1, imm, 0x5, 0b0010011)) }
        "srai"  => { let (rd, rs1, imm) = reg_reg_imm(rest, pc, labels)?; Ok(i_type(rd, rs1, imm | (0x20 << 5), 0x5, 0b0010011)) }
        "slti"  => { let (rd, rs1, imm) = reg_reg_imm(rest, pc, labels)?; Ok(i_type(rd, rs1, imm, 0x2, 0b0010011)) }
        "sltiu" => { let (rd, rs1, imm) = reg_reg_imm(rest, pc, labels)?; Ok(i_type(rd, rs1, imm, 0x3, 0b0010011)) }

        // Loads
        "lb"  => { let (rd, rs1, imm) = reg_mem(rest)?; Ok(i_type(rd, rs1, imm, 0x0, 0b0000011)) }
        "lh"  => { let (rd, rs1, imm) = reg_mem(rest)?; Ok(i_type(rd, rs1, imm, 0x1, 0b0000011)) }
        "lw"  => { let (rd, rs1, imm) = reg_mem(rest)?; Ok(i_type(rd, rs1, imm, 0x2, 0b0000011)) }
        "lbu" => { let (rd, rs1, imm) = reg_mem(rest)?; Ok(i_type(rd, rs1, imm, 0x4, 0b0000011)) }
        "lhu" => { let (rd, rs1, imm) = reg_mem(rest)?; Ok(i_type(rd, rs1, imm, 0x5, 0b0000011)) }

        // Stores
        "sb" => { let (rs1, rs2, imm) = reg_mem_store(rest)?; Ok(s_type(rs1, rs2, imm, 0x0)) }
        "sh" => { let (rs1, rs2, imm) = reg_mem_store(rest)?; Ok(s_type(rs1, rs2, imm, 0x1)) }
        "sw" => { let (rs1, rs2, imm) = reg_mem_store(rest)?; Ok(s_type(rs1, rs2, imm, 0x2)) }

        // Branches
        "beq"  => { let (rs1, rs2, off) = reg_reg_label(rest, pc, labels)?; Ok(b_type(rs1, rs2, off, 0x0)) }
        "bne"  => { let (rs1, rs2, off) = reg_reg_label(rest, pc, labels)?; Ok(b_type(rs1, rs2, off, 0x1)) }
        "blt"  => { let (rs1, rs2, off) = reg_reg_label(rest, pc, labels)?; Ok(b_type(rs1, rs2, off, 0x4)) }
        "bge"  => { let (rs1, rs2, off) = reg_reg_label(rest, pc, labels)?; Ok(b_type(rs1, rs2, off, 0x5)) }
        "bltu" => { let (rs1, rs2, off) = reg_reg_label(rest, pc, labels)?; Ok(b_type(rs1, rs2, off, 0x6)) }
        "bgeu" => { let (rs1, rs2, off) = reg_reg_label(rest, pc, labels)?; Ok(b_type(rs1, rs2, off, 0x7)) }

        // Jumps
        "jal"  => { let (rd, off) = reg_label(rest, pc, labels)?; Ok(j_type(rd, off)) }
        "jalr" => { let (rd, rs1, imm) = reg_reg_imm(rest, pc, labels)?; Ok(i_type(rd, rs1, imm, 0x0, 0b1100111)) }

        // Upper immediate
        "lui"   => { let (rd, imm) = reg_imm(rest, pc, labels)?; Ok(u_type(rd, imm, 0b0110111)) }
        "auipc" => { let (rd, imm) = reg_imm(rest, pc, labels)?; Ok(u_type(rd, imm, 0b0010111)) }

        other => Err(AsmError::UnknownInstruction(other.to_string())),
    }
}

// ── Encoding helpers ──────────────────────────────────────────────────────────

fn r_type(rd: u8, rs1: u8, rs2: u8, funct3: u32, funct7: u32, opcode: u32) -> u32 {
    opcode | ((rd as u32) << 7) | (funct3 << 12) | ((rs1 as u32) << 15) | ((rs2 as u32) << 20) | (funct7 << 25)
}

fn i_type(rd: u8, rs1: u8, imm: i32, funct3: u32, opcode: u32) -> u32 {
    opcode | ((rd as u32) << 7) | (funct3 << 12) | ((rs1 as u32) << 15) | (((imm as u32) & 0xfff) << 20)
}

fn s_type(rs1: u8, rs2: u8, imm: i32, funct3: u32) -> u32 {
    let imm = imm as u32;
    0b0100011 | (funct3 << 12) | ((rs1 as u32) << 15) | ((rs2 as u32) << 20)
        | ((imm & 0x1f) << 7) | ((imm >> 5) << 25)
}

fn b_type(rs1: u8, rs2: u8, imm: i32, funct3: u32) -> u32 {
    let imm = imm as u32;
    0b1100011 | (funct3 << 12) | ((rs1 as u32) << 15) | ((rs2 as u32) << 20)
        | (((imm >> 11) & 1) << 7)
        | (((imm >> 1) & 0xf) << 8)
        | (((imm >> 5) & 0x3f) << 25)
        | (((imm >> 12) & 1) << 31)
}

fn u_type(rd: u8, imm: i32, opcode: u32) -> u32 {
    opcode | ((rd as u32) << 7) | ((imm as u32) & 0xfffff000)
}

fn j_type(rd: u8, imm: i32) -> u32 {
    let imm = imm as u32;
    0b1101111 | ((rd as u32) << 7)
        | ((imm & 0xff000))
        | (((imm >> 11) & 1) << 20)
        | (((imm >> 1) & 0x3ff) << 21)
        | (((imm >> 20) & 1) << 31)
}

// ── Operand parsers ───────────────────────────────────────────────────────────

fn reg(s: &str) -> Result<u8, AsmError> {
    let s = s.trim();
    // ABI names
    let abi = match s {
        "zero" => Some(0),  "ra" => Some(1),  "sp" => Some(2),  "gp" => Some(3),
        "tp"   => Some(4),  "t0" => Some(5),  "t1" => Some(6),  "t2" => Some(7),
        "s0" | "fp" => Some(8), "s1" => Some(9),
        "a0" => Some(10), "a1" => Some(11), "a2" => Some(12), "a3" => Some(13),
        "a4" => Some(14), "a5" => Some(15), "a6" => Some(16), "a7" => Some(17),
        "s2" => Some(18), "s3" => Some(19), "s4" => Some(20), "s5" => Some(21),
        "s6" => Some(22), "s7" => Some(23), "s8" => Some(24), "s9" => Some(25),
        "s10" => Some(26), "s11" => Some(27),
        "t3" => Some(28), "t4" => Some(29), "t5" => Some(30), "t6" => Some(31),
        _ => None,
    };
    if let Some(n) = abi { return Ok(n); }

    if let Some(n) = s.strip_prefix('x') {
        n.parse::<u8>().map_err(|_| AsmError::UnknownRegister(s.to_string()))
    } else {
        Err(AsmError::UnknownRegister(s.to_string()))
    }
}

fn split_ops(s: &str) -> Vec<&str> {
    s.splitn(4, ',').map(|p| p.trim()).collect()
}

fn reg2(s: &str) -> Result<(u8, u8), AsmError> {
    let ops = split_ops(s);
    if ops.len() < 2 { return Err(AsmError::BadOperands(s.to_string())); }
    Ok((reg(ops[0])?, reg(ops[1])?))
}

fn reg3(s: &str) -> Result<(u8, u8, u8), AsmError> {
    let ops = split_ops(s);
    if ops.len() < 3 { return Err(AsmError::BadOperands(s.to_string())); }
    Ok((reg(ops[0])?, reg(ops[1])?, reg(ops[2])?))
}

fn reg_imm(s: &str, pc: u32, labels: &HashMap<String, u32>) -> Result<(u8, i32), AsmError> {
    let ops = split_ops(s);
    if ops.len() < 2 { return Err(AsmError::BadOperands(s.to_string())); }
    Ok((reg(ops[0])?, resolve(ops[1], pc, labels)?))
}

fn reg_reg_imm(s: &str, pc: u32, labels: &HashMap<String, u32>) -> Result<(u8, u8, i32), AsmError> {
    let ops = split_ops(s);
    if ops.len() < 3 { return Err(AsmError::BadOperands(s.to_string())); }
    Ok((reg(ops[0])?, reg(ops[1])?, resolve(ops[2], pc, labels)?))
}

fn reg_reg_label(s: &str, pc: u32, labels: &HashMap<String, u32>) -> Result<(u8, u8, i32), AsmError> {
    let ops = split_ops(s);
    if ops.len() < 3 { return Err(AsmError::BadOperands(s.to_string())); }
    let target = resolve(ops[2], pc, labels)?;
    Ok((reg(ops[0])?, reg(ops[1])?, target))
}

fn reg_label(s: &str, pc: u32, labels: &HashMap<String, u32>) -> Result<(u8, i32), AsmError> {
    let ops = split_ops(s);
    if ops.len() < 2 { return Err(AsmError::BadOperands(s.to_string())); }
    Ok((reg(ops[0])?, resolve(ops[1], pc, labels)?))
}

/// Parse `offset(base)` into (rd, base_reg, offset)
fn reg_mem(s: &str) -> Result<(u8, u8, i32), AsmError> {
    let comma = s.find(',').ok_or_else(|| AsmError::BadOperands(s.to_string()))?;
    let rd = reg(s[..comma].trim())?;
    let rest = s[comma+1..].trim();
    let (off_str, base_str) = parse_mem_operand(rest)?;
    Ok((rd, reg(base_str)?, off_str))
}

/// Parse `rs2, offset(base)` for stores — returns (base, rs2, offset)
fn reg_mem_store(s: &str) -> Result<(u8, u8, i32), AsmError> {
    let comma = s.find(',').ok_or_else(|| AsmError::BadOperands(s.to_string()))?;
    let rs2 = reg(s[..comma].trim())?;
    let rest = s[comma+1..].trim();
    let (off_str, base_str) = parse_mem_operand(rest)?;
    Ok((reg(base_str)?, rs2, off_str))
}

fn parse_mem_operand(s: &str) -> Result<(i32, &str), AsmError> {
    let lparen = s.find('(').ok_or_else(|| AsmError::BadOperands(s.to_string()))?;
    let rparen = s.find(')').ok_or_else(|| AsmError::BadOperands(s.to_string()))?;
    let offset = s[..lparen].trim().parse::<i32>().unwrap_or(0);
    let base = &s[lparen+1..rparen];
    Ok((offset, base))
}

fn resolve(s: &str, pc: u32, labels: &HashMap<String, u32>) -> Result<i32, AsmError> {
    let s = s.trim();
    if let Ok(n) = s.parse::<i32>() {
        return Ok(n);
    }
    if let Some(n) = parse_hex(s) {
        return Ok(n);
    }
    // Label — return PC-relative offset
    labels.get(s)
        .map(|&addr| (addr as i32) - (pc as i32))
        .ok_or_else(|| AsmError::UndefinedLabel(s.to_string()))
}

fn parse_hex(s: &str) -> Option<i32> {
    s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"))
        .and_then(|h| i32::from_str_radix(h, 16).ok())
}
