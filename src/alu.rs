use std::marker::PhantomData;

use crate::utils::diag::{DiagNode, Diagnosable};
use crate::utils::isa::{Opcode, Opcode::*};

/*
  In this abstraction, the ALU is a collection of pipes, each of which
  can perform a given set of operations, each with a given latency.

  Note that kinds like `LoadStore` and `Branch` do not mean that
  the performs a branch or LoadStore operation, but that it can
  performs associated calculations (e.g. address calc, branch
  predicate calc etc.)
*/

#[derive(PartialEq)]
pub enum FuncUnitKind {
    IntAlu,
    IntMul,
    LoadStore,
    Branch,
}

// Predicate matching each functional unit kind
// to operations they can support.
fn kind_supports(kind: &FuncUnitKind, opcode: Opcode) -> bool {
    match kind {
        FuncUnitKind::IntAlu => matches!(
            opcode,
            Add | Sub
                | And
                | Or
                | Xor
                | Sll
                | Srl
                | Sra
                | Slt
                | Sltu
                | Addi
                | Andi
                | Ori
                | Xori
                | Slli
                | Srli
                | Srai
                | Slti
                | Sltiu
                | Lui
                | Auipc
        ),
        FuncUnitKind::IntMul => false, // no M extension yet
        FuncUnitKind::LoadStore => matches!(opcode, Lb | Lh | Lw | Lbu | Lhu | Sb | Sh | Sw),
        FuncUnitKind::Branch => matches!(opcode, Beq | Bne | Blt | Bge | Bltu | Bgeu | Jal | Jalr),
    }
}

pub struct AluPipe {
    supported_kinds: Vec<FuncUnitKind>,
    // also need instruction latencies
}

impl AluPipe {
    pub fn new(supported_kinds: Vec<FuncUnitKind>) -> Self {
        Self { supported_kinds }
    }
}

// Check if a pipe supports a given operation
fn pipe_supports(pipe: &AluPipe, opcode: Opcode) -> bool {
    for kind in &pipe.supported_kinds {
        if kind_supports(kind, opcode) {
            return true;
        }
    }
    false
}

// Traits for communicating with ALU.
// These can be used to pass the ALU metadata along with an instruction,
// for benchmarking, instruction tracking etc., without any ALU noise
pub struct ExecInputs {
    pub pc: u32,
    pub rs1: u32,
    pub rs2: u32,
    pub imm: i32,
}

pub trait OpaqueInstruction {
    fn get_opcode(&self) -> Opcode;
    fn get_inputs(&self) -> ExecInputs;
}

pub trait OpaqueResult<I: OpaqueInstruction> {
    fn from_instr_and_result(instr: I, result: u32) -> Self;
}

pub struct InstructionProgress<I: OpaqueInstruction> {
    instruction: I,         // the instruction being executed
    latency: u8,            // number of latency to complete
    age: u8,                // number of latency spent in ALU
    _resources_used: usize, // index of the resource used.
}

pub struct ALU<I: OpaqueInstruction, O: OpaqueResult<I>> {
    pipes: Vec<AluPipe>,
    in_progress_instructions: Vec<InstructionProgress<I>>,
    resource_usage: Vec<bool>, // `true` if in use
    _phantom: PhantomData<O>,
}

impl<I: OpaqueInstruction, O: OpaqueResult<I>> Diagnosable for ALU<I, O> {
    fn diagnose(&self) -> DiagNode {
        let pipe_usage: String = self
            .resource_usage
            .iter()
            .map(|&u| if u { 'X' } else { '.' })
            .collect();
        let mut children = vec![
            DiagNode::leaf(
                "pipes",
                format!(
                    "{} ({} in-flight)",
                    self.pipes.len(),
                    self.in_progress_instructions.len()
                ),
            ),
            DiagNode::leaf("pipe usage", format!("[{pipe_usage}]")),
        ];
        for (i, p) in self.in_progress_instructions.iter().enumerate() {
            children.push(DiagNode::leaf(
                format!("[{i}]"),
                format!(
                    "{:?}  age={}/{}  pipe={}",
                    p.instruction.get_opcode(),
                    p.age,
                    p.latency,
                    p._resources_used
                ),
            ));
        }
        DiagNode::inner("alu", children)
    }
}

fn compute(opcode: Opcode, i: &ExecInputs) -> u32 {
    let (a, b, imm) = (i.rs1, i.rs2, i.imm as u32);
    match opcode {
        Add => a.wrapping_add(b),
        Sub => a.wrapping_sub(b),
        And => a & b,
        Or => a | b,
        Xor => a ^ b,
        Sll => a << (b & 0x1f),
        Srl => a >> (b & 0x1f),
        Sra => ((a as i32) >> (b & 0x1f)) as u32,
        Slt => ((a as i32) < (b as i32)) as u32,
        Sltu => (a < b) as u32,

        Addi => a.wrapping_add(imm),
        Andi => a & imm,
        Ori => a | imm,
        Xori => a ^ imm,
        Slli => a << (imm & 0x1f),
        Srli => a >> (imm & 0x1f),
        Srai => ((a as i32) >> (imm & 0x1f)) as u32,
        Slti => ((a as i32) < i.imm) as u32,
        Sltiu => (a < imm) as u32,

        Lb | Lbu | Lh | Lhu | Lw => a.wrapping_add(imm), // address calc
        Sb | Sh | Sw => a.wrapping_add(imm),             // address calc

        Lui => imm,
        Auipc => i.pc.wrapping_add(imm),

        Jal | Jalr => i.pc.wrapping_add(4), // return address

        Beq => {
            if i.rs1 == i.rs2 {
                i.pc.wrapping_add(imm)
            } else {
                i.pc + 4
            }
        }
        Bne => {
            if i.rs1 != i.rs2 {
                i.pc.wrapping_add(imm)
            } else {
                i.pc + 4
            }
        }
        Blt => {
            if (i.rs1 as i32) < (i.rs2 as i32) {
                i.pc.wrapping_add(imm)
            } else {
                i.pc + 4
            }
        }
        Bge => {
            if (i.rs1 as i32) >= (i.rs2 as i32) {
                i.pc.wrapping_add(imm)
            } else {
                i.pc + 4
            }
        }
        Bltu => {
            if i.rs1 < i.rs2 {
                i.pc.wrapping_add(imm)
            } else {
                i.pc + 4
            }
        }
        Bgeu => {
            if i.rs1 >= i.rs2 {
                i.pc.wrapping_add(imm)
            } else {
                i.pc + 4
            }
        }
    }
}

impl<I: OpaqueInstruction, O: OpaqueResult<I>> ALU<I, O> {
    pub fn new(pipes: Vec<AluPipe>) -> Self {
        let n = pipes.len();
        Self {
            pipes,
            in_progress_instructions: vec![],
            resource_usage: vec![false; n],
            _phantom: PhantomData,
        }
    }

    // TODO: implement proper resolution from latency table or similar
    fn get_instr_execution_latency(&self, _opcode: Opcode) -> u8 {
        2
    }

    pub fn try_enqueue(&mut self, instr: I) -> bool {
        for i in 0..self.pipes.len() {
            if (!self.resource_usage[i]) && (pipe_supports(&self.pipes[i], instr.get_opcode())) {
                self.resource_usage[i] = true;
                self.in_progress_instructions.push(InstructionProgress {
                    latency: self.get_instr_execution_latency(instr.get_opcode()),
                    instruction: instr,
                    age: 0,
                    _resources_used: i,
                });
                return true;
            }
        }
        false
    }

    // Advance the state of the operations in the ALU
    // If there is a ready operation, return it. In the
    // case of multiple ready instructions, return the oldest.
    pub fn tick(&mut self) -> Option<O> {
        let mut max_age: u8 = 0;
        let mut index: Option<usize> = None;
        for i in 0..self.in_progress_instructions.len() {
            let mut record = true;
            let instr = &mut self.in_progress_instructions[i];
            if instr.latency < instr.age {
                record = false;
            } // Not ready yet
            if instr.age <= max_age {
                record = false;
            } // Not the oldest

            instr.age += 1;

            if !record {
                continue;
            }

            max_age = instr.age;
            index = Some(i);
        }

        if let Some(index_to_remove) = index {
            let in_progress_instruction = self.in_progress_instructions.remove(index_to_remove);
            let instruction = in_progress_instruction.instruction;
            self.resource_usage[in_progress_instruction._resources_used] = false;
            let result = compute(instruction.get_opcode(), &instruction.get_inputs());
            return Some(O::from_instr_and_result(instruction, result));
        }

        None
    }
}
