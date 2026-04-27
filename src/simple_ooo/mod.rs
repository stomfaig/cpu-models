pub mod reorder_buffer;
pub mod reservation_stations;

use core::panic;

use crate::alu::{ALU, AluPipe, ExecInputs, FuncUnitKind, OpaqueInstruction, OpaqueResult};
use crate::simple_ooo::reorder_buffer::SimpleReorderBuffer;
use crate::simple_ooo::reservation_stations::{Operand, SREntry, SimpleReservationStation};
use crate::utils::diag::{DiagNode, Diagnosable};
use crate::utils::isa::{DecodedInstr, InstResult, Opcode, decode};
use crate::utils::latch::Latch;

/*
  Simple out-of-order CPU with the following pipeline:
    IF → ID → RS → EX (ALU) → MEM → ROB → Commit

  Out-of-order execution allows the CPU to execute instructions in an order
  different from program order, exploiting instruction-level parallelism while
  preserving the illusion of sequential execution for the programmer.

  The key structures are:
  - Reservation Stations (RS): hold instructions waiting for their operands.
    An instruction is dispatched to the execution unit only when all its
    operands are available.
  - Reorder Buffer (ROB): tracks all in-flight instructions in program order.
    Results are written to the register file only when the instruction at the
    head of the ROB completes — this is the "commit" step that restores
    sequential semantics.
  - Register Alias Table (RAT): maps architectural registers to the ROB entry
    that will produce their next value. This allows operand forwarding from
    in-flight results rather than waiting for commit.

  The frontend is 1-wide. The execution backend is parameterized:
  the reservation station and reorder buffer implementations are
  injected via trait objects, and the ALU is composed of typed functional units.
*/

type RobTag = usize;

// InstrFormat and ResultFormat are the types that flow through the ALU.
// They carry the ROB tag alongside the instruction so the result can be
// matched back to the correct ROB entry on completion.
// The OpaqueInstruction/OpaqueResult traits keep the ALU generic and
// independent of pipeline-specific metadata.
pub struct InstrFormat {
    rob_tag: RobTag,
    instr: SREntry,
}

impl OpaqueInstruction for InstrFormat {
    fn get_opcode(&self) -> Opcode {
        self.instr.opcode
    }

    fn get_inputs(&self) -> ExecInputs {
        let rs1 = match self.instr.rs1 {
            Operand::Ready(v) => v,
            _ => panic!("rs1 not ready at dispatch"),
        };
        let rs2 = match self.instr.rs2 {
            Operand::Ready(v) => v,
            _ => panic!("rs2 not ready at dispatch"),
        };
        ExecInputs {
            pc: self.instr.pc,
            rs1,
            rs2,
            imm: self.instr.imm,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ResultFormat {
    rob_tag: RobTag,
    instr: SREntry,
    result: u32,
}

impl OpaqueResult<InstrFormat> for ResultFormat {
    fn from_instr_and_result(instr: InstrFormat, result: u32) -> Self {
        ResultFormat {
            rob_tag: instr.rob_tag,
            instr: instr.instr,
            result,
        }
    }
}

type OOOALU = ALU<InstrFormat, ResultFormat>;

// Latch types carrying state between pipeline stages.
// Each latch is written this cycle and read next cycle, modelling
// the register between stages in a real pipeline.
#[derive(Debug, Clone, Copy)]
struct IfId {
    pc: u32,
    word: u32,
}

#[derive(Debug, Clone, Copy)]
struct IdRs {
    pc: u32,
    instr: DecodedInstr,
}

// The ReservationStation trait abstracts over different RS implementations
// (centralised, distributed per functional unit, etc.).
//
// - stall:    true if the RS is full and the frontend must stop fetching.
// - enqueue:  add a newly decoded instruction. Operands may already be ready
//             (from the register file) or waiting (for a ROB result).
// - update:   called on writeback to broadcast a completed result to all
//             waiting entries. Any entry whose operand matches the tag
//             is updated to Ready, potentially making it eligible to dispatch.
// - dispatch: inspect all ready entries and send one to the ALU.
trait ReservationStation: Diagnosable {
    fn stall(&self) -> bool;
    fn enqueue(&mut self, rob_tag: usize, rs1: Operand, rs2: Operand, instr: &IdRs);
    fn update(&mut self, rob_tag: usize, val: u32);
    fn dispatch(&mut self, alu: &mut OOOALU);
}

pub struct TaggedResult {
    rob_tag: RobTag,
    result: InstResult,
}

// The ReorderBuffer trait abstracts over ROB implementations.
//
// - stall:   true if the ROB is full and the frontend must stop fetching.
// - enqueue: reserve a slot for a newly decoded instruction. Returns the
//            ROB tag, which is used to track this instruction through the
//            pipeline and in the RAT.
// - update:  mark a ROB entry as complete with its result, ready to commit.
// - release: if the head of the ROB is complete, retire it in program order
//            and return the result to be written to the register file.
trait ReorderBuffer: Diagnosable {
    fn stall(&self) -> bool;
    fn enqueue(&mut self, instr: &IdRs) -> usize;
    fn update(&mut self, rob_tag: RobTag, result: u32);
    fn release(&mut self) -> Option<TaggedResult>;
    fn is_tag_ready(&self, tag: RobTag) -> Option<u32>;
}

pub struct SimpleOOO {
    mem: Vec<u8>,
    pub regs: [u32; 32],

    // Register Alias Table: rat[r] = Some(tag) means register r's current value
    // is being produced by the instruction with that ROB tag, and has not yet
    // committed. None means the register file holds the current value.
    rat: [Option<usize>; 32],

    pc: u32,
    // When a branch or jump commits we know the true next-PC. Until then the
    // frontend is stalled so no speculative instructions enter the pipeline.
    branch_stall: bool,

    if_id: Latch<IfId>,
    id_rs: Latch<IdRs>,
    reservation_station: Box<dyn ReservationStation>,
    reorder_buffer: Box<dyn ReorderBuffer>,

    alu: OOOALU,
    alu_mem: Latch<ResultFormat>,
    mem_rob_release: Latch<ResultFormat>,
}

impl SimpleOOO {
    pub fn load(&mut self, addr: usize, bytes: &[u8]) {
        self.mem[addr..addr + bytes.len()].copy_from_slice(bytes);
    }

    pub fn tick(&mut self) {
        // Stall the frontend if the RS or ROB are full. This models backpressure
        // from the backend: we can't issue new instructions if there is nowhere
        // to put them.
        let stall_frontend =
            self.reservation_station.stall() || self.reorder_buffer.stall() || self.branch_stall;

        // IF: fetch the next instruction word from memory.
        // If stalled, the latch produces no output next cycle.
        if !stall_frontend {
            let fetched = self.fetch();
            self.if_id.stage(fetched);
        }

        // ID: decode the fetched instruction word into its fields.
        if let Some(buffer_state) = self.if_id.read() {
            let decoded = decode(buffer_state.word);
            if let Some(instr) = decoded {
                self.id_rs.stage(IdRs {
                    pc: buffer_state.pc,
                    instr,
                });
            } else {
                if buffer_state.word == 0 {
                    return;
                }
                panic!("Unrecognized instruction!");
            }
        }

        // IS (Issue): allocate a ROB entry, resolve operands via the RAT,
        // and enqueue the instruction into the reservation station.
        // Operands that are already available come from the register file;
        // operands produced by in-flight instructions are tagged with the
        // ROB entry that will produce them.
        if let Some(instr) = self.id_rs.read() {
            if Self::is_branch(instr.instr.opcode) {
                self.branch_stall = true;
            }
            let rob_tag = self.reorder_buffer.enqueue(&instr);
            let rs1 = self.resolve_operands(instr.instr.rs1);
            let rs2 = self.resolve_operands(instr.instr.rs2);
            // Only update the RAT for instructions that actually write a result to rd.
            // B-type and S-type encode immediates in the rd field — treating those
            // bits as a destination register would corrupt the RAT.
            if Self::writes_rd(instr.instr.format) && instr.instr.rd != 0 {
                self.rat[instr.instr.rd as usize] = Some(rob_tag);
            }
            self.reservation_station.enqueue(rob_tag, rs1, rs2, &instr);
        }

        // EX: dispatch a ready instruction to the ALU, then tick the ALU.
        // On completion, broadcast the result on the CDB, waking any RS entries
        // that were waiting for this tag. For non-load instructions the ALU result
        // is final, so we also mark the ROB entry ready here. This means
        // resolve_operands can observe the value in the very next cycle via
        // is_tag_ready, rather than waiting two more stages.
        // Loads are the exception: the ALU only computed the address; the actual
        // loaded value comes from MEM and will update the ROB there instead.
        self.reservation_station.dispatch(&mut self.alu);
        if let Some(res) = self.alu.tick() {
            if Self::is_branch(res.instr.opcode) {
                self.pc = res.result;
                self.branch_stall = false;
            }
            // Non-loads: the ALU result is the final value. Broadcast on the CDB
            // and mark the ROB entry ready immediately. Loads must wait until MEM
            // has the actual data — broadcasting the address here would give waiting
            // instructions the wrong value.
            if !Self::is_load(res.instr.opcode) {
                self.reservation_station.update(res.rob_tag, res.result);
                self.reorder_buffer.update(res.rob_tag, res.result);
            }
            self.alu_mem.stage(res);
        }

        // MEM: for loads and stores the ALU computed the effective address.
        // Here we perform the actual memory access. Non-memory instructions
        // pass through unchanged (result forwarded as-is).
        if let Some(instr) = self.alu_mem.read() {
            let result = self.mem_stage(instr);
            self.mem_rob_release.stage(result);
        }

        // WB: for loads, the loaded data is now the final result. Broadcast on
        // the CDB (waking any RS entries that were waiting on this load's rd)
        // and mark the ROB entry ready. The ROB update and RS broadcast are
        // always paired so no waiting entry can be left behind.
        if let Some(instr) = self.mem_rob_release.read() {
            if Self::is_load(instr.instr.opcode) {
                self.reservation_station.update(instr.rob_tag, instr.result);
                self.reorder_buffer.update(instr.rob_tag, instr.result);
            }
        }

        // Commit: retire the head of the ROB if it is complete. Writing to the
        // register file only happens here, ensuring results become visible in
        // program order. The RAT entry is cleared only if it still points to
        // this instruction (a later write to the same register may have already
        // replaced it).
        if let Some(instr) = self.reorder_buffer.release() {
            let rd = instr.result.rd as usize;
            if Self::writes_rd(instr.result.format) && rd != 0 {
                if let Some(rat_tag) = self.rat[rd] {
                    if rat_tag == instr.rob_tag {
                        self.rat[rd] = None;
                    }
                }
                self.regs[rd] = instr.result.result;
            }
        }

        // Advance all latches: the staged value becomes the readable value
        // for the next cycle.
        self.if_id.update();
        self.id_rs.update();
        self.alu_mem.update();
        self.mem_rob_release.update();
    }

    fn fetch(&mut self) -> IfId {
        let pc = self.pc as usize;
        let result = IfId {
            pc: self.pc,
            word: u32::from_le_bytes(self.mem[pc..pc + 4].try_into().unwrap()),
        };
        self.pc += 4;
        result
    }

    fn is_branch(opcode: Opcode) -> bool {
        matches!(
            opcode,
            Opcode::Beq
                | Opcode::Bne
                | Opcode::Blt
                | Opcode::Bge
                | Opcode::Bltu
                | Opcode::Bgeu
                | Opcode::Jal
                | Opcode::Jalr
        )
    }

    fn is_load(opcode: Opcode) -> bool {
        matches!(
            opcode,
            Opcode::Lb | Opcode::Lh | Opcode::Lw | Opcode::Lbu | Opcode::Lhu
        )
    }

    // Only R, I, U, J instructions write a meaningful value to rd.
    // B-type and S-type encode immediates in bits[11:7], so that field must
    // not be treated as a destination register.
    fn writes_rd(format: crate::utils::isa::InstrFormat) -> bool {
        use crate::utils::isa::InstrFormat::*;
        matches!(format, R | I | U | J)
    }

    // Check the RAT to determine how to read a source register.
    // If the RAT maps this register to a ROB tag, the value is still
    // in-flight; the instruction must wait for that tag to be broadcast.
    // Otherwise the register file holds the current committed value.
    fn resolve_operands(&self, rs: u8) -> Operand {
        match self.rat[rs as usize] {
            Some(rob_tag) => match self.reorder_buffer.is_tag_ready(rob_tag) {
                Some(val) => Operand::Ready(val),
                None => Operand::Waiting(rob_tag),
            },
            None => Operand::Ready(self.regs[rs as usize]),
        }
    }

    // Perform the memory access for load/store instructions.
    // The ALU has already computed the effective address and stored it in
    // `result`. For stores, rs2 must be Ready by the time we reach this stage.
    // Non-memory instructions pass their ALU result through unchanged.
    fn mem_stage(&mut self, instr: ResultFormat) -> ResultFormat {
        use crate::simple_ooo::reservation_stations::Operand::Ready;
        use crate::utils::isa::Opcode::*;

        let addr = instr.result as usize;

        let result = match instr.instr.opcode {
            Lb => (self.mem[addr] as i8) as u32,
            Lbu => self.mem[addr] as u32,
            Lh => (u16::from_le_bytes(self.mem[addr..addr + 2].try_into().unwrap()) as i16) as u32,
            Lhu => u16::from_le_bytes(self.mem[addr..addr + 2].try_into().unwrap()) as u32,
            Lw => u32::from_le_bytes(self.mem[addr..addr + 4].try_into().unwrap()),

            Sb | Sh | Sw => {
                let Ready(data) = instr.instr.rs2 else {
                    panic!("store data not ready at mem stage")
                };
                match instr.instr.opcode {
                    Sb => self.mem[addr] = (data & 0xff) as u8,
                    Sh => self.mem[addr..addr + 2].copy_from_slice(&(data as u16).to_le_bytes()),
                    Sw => self.mem[addr..addr + 4].copy_from_slice(&data.to_le_bytes()),
                    _ => unreachable!(),
                }
                0
            }

            _ => instr.result,
        };

        ResultFormat {
            rob_tag: instr.rob_tag,
            instr: instr.instr,
            result,
        }
    }
}

impl Diagnosable for SimpleOOO {
    fn diagnose(&self) -> DiagNode {
        let stall =
            self.reservation_station.stall() || self.reorder_buffer.stall() || self.branch_stall;

        // RAT: only show registers that have a pending mapping
        let rat_entries: Vec<DiagNode> = self
            .rat
            .iter()
            .enumerate()
            .filter_map(|(r, slot)| {
                slot.map(|tag| DiagNode::leaf(format!("x{r}"), format!("rob[{tag}]")))
            })
            .collect();
        let rat_node = if rat_entries.is_empty() {
            DiagNode::leaf("rat", "empty")
        } else {
            DiagNode::inner("rat", rat_entries)
        };

        DiagNode::inner(
            "simple_ooo",
            vec![
                DiagNode::leaf("pc", format!("0x{:08x}", self.pc)),
                DiagNode::leaf("stall", stall.to_string()),
                DiagNode::leaf("branch_stall", self.branch_stall.to_string()),
                DiagNode::leaf("if_id", format!("{:?}", self.if_id.peek())),
                DiagNode::leaf("id_rs", format!("{:?}", self.id_rs.peek())),
                DiagNode::leaf("alu_mem", format!("{:?}", self.alu_mem.peek())),
                DiagNode::leaf(
                    "mem_rob_release",
                    format!("{:?}", self.mem_rob_release.peek()),
                ),
                rat_node,
                self.alu.diagnose(),
                self.reservation_station.diagnose(),
                self.reorder_buffer.diagnose(),
            ],
        )
    }
}

pub fn build_default() -> SimpleOOO {
    SimpleOOO {
        mem: vec![0u8; 256],
        regs: [0u32; 32],
        rat: [None; 32],
        pc: 0,
        branch_stall: false,

        if_id: Latch::new(),
        id_rs: Latch::new(),

        reservation_station: Box::new(SimpleReservationStation::new(8)),
        reorder_buffer: Box::new(SimpleReorderBuffer::new(16)),

        alu: OOOALU::new(vec![
            AluPipe::new(vec![FuncUnitKind::IntAlu]),
            AluPipe::new(vec![FuncUnitKind::LoadStore]),
            AluPipe::new(vec![FuncUnitKind::Branch]),
        ]),

        alu_mem: Latch::new(),
        mem_rob_release: Latch::new(),
    }
}
