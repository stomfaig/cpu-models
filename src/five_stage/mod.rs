pub mod stages;
pub mod hazard;
pub mod forwarding;

use crate::isa::{self, Opcode, InstrFormat};
use crate::alu::{ALU, AluPipe, ExecInputs, FuncUnitKind, OpaqueInstruction, OpaqueResult};
use crate::diag::{DiagNode, Diagnosable};
use crate::latch::Latch;
use stages::{IfId, IdEx, ExMem, MemWb};
use hazard::HazardPolicy;
use forwarding::ForwardingPolicy;

// The five-stage pipeline wraps each inter-stage latch in the Latch type,
// which enforces the two-phase (stage then update) discipline that models
// real pipeline registers: a stage's output is only visible to the next
// stage on the following cycle.

// Wrapper types so the generic ALU can carry the pipeline's latch data.
struct FiveStageInstr(IdEx);
struct FiveStageResult { ex_mem: ExMem }

impl OpaqueInstruction for FiveStageInstr {
  fn get_opcode(&self) -> Opcode { self.0.instr.opcode }
  fn get_inputs(&self) -> ExecInputs {
    ExecInputs { pc: self.0.pc, rs1: self.0.rs1, rs2: self.0.rs2, imm: self.0.instr.imm }
  }
}

impl OpaqueResult<FiveStageInstr> for FiveStageResult {
  fn from_instr_and_result(instr: FiveStageInstr, result: u32) -> Self {
    FiveStageResult { ex_mem: ExMem { instr: instr.0.instr, result, rs2: instr.0.rs2 } }
  }
}

type FiveALU = ALU<FiveStageInstr, FiveStageResult>;

fn default_alu() -> FiveALU {
  FiveALU::new(vec![
    AluPipe::new(vec![FuncUnitKind::IntAlu, FuncUnitKind::LoadStore, FuncUnitKind::Branch]),
  ])
}

pub struct FiveStageCpu {
  pub regs:  [u32; 32],
  pub mem:   Vec<u8>,
  pub pc:    u32,
  pub cycle: u64,

  if_id:  Latch<IfId>,
  id_ex:  Latch<IdEx>,
  ex_mem: Latch<ExMem>,
  mem_wb: Latch<MemWb>,

  alu:        FiveALU,
  hazard:     Box<dyn HazardPolicy>,
  forwarding: Box<dyn ForwardingPolicy>,
}

impl FiveStageCpu {
  pub fn new(
    mem_size:   usize,
    hazard:     Box<dyn HazardPolicy>,
    forwarding: Box<dyn ForwardingPolicy>,
  ) -> Self {
    Self {
      regs: [0u32; 32], mem: vec![0u8; mem_size], pc: 0, cycle: 0,
      if_id: Latch::new(), id_ex: Latch::new(), ex_mem: Latch::new(), mem_wb: Latch::new(),
      alu: default_alu(), hazard, forwarding,
    }
  }

  pub fn load(&mut self, addr: usize, bytes: &[u8]) {
    self.mem[addr..addr + bytes.len()].copy_from_slice(bytes);
  }

  pub fn tick(&mut self) {
    self.cycle += 1;

    // Snapshot latches for hazard/forwarding checks — these must reflect
    // start-of-cycle state so stages don't observe each other's outputs.
    let if_id_snap  = self.if_id.peek();
    let id_ex_snap  = self.id_ex.peek();
    let ex_mem_snap = self.ex_mem.peek();
    let mem_wb_snap = self.mem_wb.peek();

    let stall = self.hazard.should_stall(&if_id_snap, &id_ex_snap);

    // IF: fetch instruction word from memory.
    if !stall {
      self.if_id.stage(IfId { pc: self.pc, word: self.mem_read_u32(self.pc) });
      self.pc += 4;
    }

    // ID: decode and read register file.
    if !stall {
      if let Some(latch) = if_id_snap {
        if let Some(instr) = isa::decode(latch.word) {
          self.id_ex.stage(IdEx {
            pc:  latch.pc,
            rs1: self.regs[instr.rs1 as usize],
            rs2: self.regs[instr.rs2 as usize],
            instr,
          });
        }
      }
    }

    // EX: apply forwarding then dispatch to the ALU.
    // The ALU is pipelined — tick() returns a result when one is ready.
    if let Some(mut latch) = id_ex_snap {
      self.forwarding.forward(&mut latch, &ex_mem_snap, &mem_wb_snap);
      self.alu.try_enqueue(FiveStageInstr(latch));
    }
    if let Some(res) = self.alu.tick() {
      self.ex_mem.stage(res.ex_mem);
    }

    // MEM: perform load/store against memory.
    if let Some(latch) = ex_mem_snap {
      let (result, write) = self.compute_memory(&latch);
      if let Some(w) = write { self.apply_mem_write(w); }
      self.mem_wb.stage(MemWb { instr: latch.instr, result });
    }

    // WB: write result to register file.
    if let Some(latch) = mem_wb_snap {
      let rd = latch.instr.rd;
      let writes = matches!(latch.instr.format, InstrFormat::R | InstrFormat::I | InstrFormat::U | InstrFormat::J);
      if writes && rd != 0 { self.regs[rd as usize] = latch.result; }
    }

    // Advance all latches: staged values become active for the next cycle.
    self.if_id.update();
    self.id_ex.update();
    self.ex_mem.update();
    self.mem_wb.update();

    self.regs[0] = 0;
  }

  fn compute_memory(&self, latch: &ExMem) -> (u32, Option<MemWrite>) {
    match latch.instr.opcode {
      Opcode::Lb  => (self.mem_read_u8(latch.result)  as i8  as i32 as u32, None),
      Opcode::Lbu => (self.mem_read_u8(latch.result)  as u32,               None),
      Opcode::Lh  => (self.mem_read_u16(latch.result) as i16 as i32 as u32, None),
      Opcode::Lhu => (self.mem_read_u16(latch.result) as u32,               None),
      Opcode::Lw  => (self.mem_read_u32(latch.result),                       None),
      Opcode::Sb  => (0, Some(MemWrite::Byte(latch.result, latch.rs2 as u8))),
      Opcode::Sh  => (0, Some(MemWrite::Half(latch.result, latch.rs2 as u16))),
      Opcode::Sw  => (0, Some(MemWrite::Word(latch.result, latch.rs2))),
      _           => (latch.result, None),
    }
  }

  fn apply_mem_write(&mut self, w: MemWrite) {
    match w {
      MemWrite::Byte(addr, val) => self.mem[addr as usize] = val,
      MemWrite::Half(addr, val) => self.mem[addr as usize..addr as usize+2].copy_from_slice(&val.to_le_bytes()),
      MemWrite::Word(addr, val) => self.mem[addr as usize..addr as usize+4].copy_from_slice(&val.to_le_bytes()),
    }
  }

  fn mem_read_u8(&self,  addr: u32) -> u8  { self.mem[addr as usize] }
  fn mem_read_u16(&self, addr: u32) -> u16 { u16::from_le_bytes(self.mem[addr as usize..addr as usize+2].try_into().unwrap()) }
  fn mem_read_u32(&self, addr: u32) -> u32 { u32::from_le_bytes(self.mem[addr as usize..addr as usize+4].try_into().unwrap()) }
}

impl Diagnosable for FiveStageCpu {
  fn diagnose(&self) -> DiagNode {
    // Non-zero registers only
    /* let reg_entries: Vec<DiagNode> = self.regs.iter().enumerate().skip(1)
      .filter(|(_, v)| v != 0)
      .map(|(i, &v)| DiagNode::leaf(format!("x{i}"), format!("0x{v:08x} ({v})")))
      .collect(); */
   /*  let regs_node = if reg_entries.is_empty() {
      DiagNode::leaf("regs", "all zero")
    } else {
      DiagNode::inner("regs", reg_entries)
    }; */

    DiagNode::inner("five_stage_cpu", vec![
      DiagNode::leaf("pc",     format!("0x{:08x}", self.pc)),
      DiagNode::leaf("cycle",  self.cycle.to_string()),
      DiagNode::leaf("if_id",  format!("{:?}", self.if_id.peek())),
      DiagNode::leaf("id_ex",  format!("{:?}", self.id_ex.peek())),
      DiagNode::leaf("ex_mem", format!("{:?}", self.ex_mem.peek())),
      DiagNode::leaf("mem_wb", format!("{:?}", self.mem_wb.peek())),
      self.alu.diagnose(),
      //regs_node,
    ])
  }
}

enum MemWrite {
  Byte(u32, u8),
  Half(u32, u16),
  Word(u32, u32),
}
