use crate::isa::Opcode;
use crate::five_stage::stages::{IfId, IdEx};

fn word_rs1(word: u32) -> u8 { ((word >> 15) & 0x1f) as u8 }
fn word_rs2(word: u32) -> u8 { ((word >> 20) & 0x1f) as u8 }

pub trait HazardPolicy {
    /// Return true if the pipeline should stall — i.e. hold fetch/decode and
    /// inject a bubble into execute this cycle.
    fn should_stall(&self, if_id: &Option<IfId>, id_ex: &Option<IdEx>) -> bool;
}

/// No hazard detection — instructions always proceed. Correct only when the
/// caller inserts sufficient NOPs manually, or forwarding covers all cases.
pub struct NoHazardDetection;

impl HazardPolicy for NoHazardDetection {
    fn should_stall(&self, _: &Option<IfId>, _: &Option<IdEx>) -> bool {
        false
    }
}

/// Check if there are any load hazards by checking if there is a load a single cycle ahead of an operation that will use the result of the load. Since we do the check on the IF/ID and ID/EX registers, an additional smaller decoder unit is required to extract the input operands from the operation just fetched.
pub struct StallOnLoad;

impl HazardPolicy for StallOnLoad {
    fn should_stall(&self, if_id: &Option<IfId>, id_ex: &Option<IdEx>) -> bool {
        let (Some(fetch), Some(exec)) = (if_id, id_ex) else { return false; };

        let is_load = matches!(exec.instr.opcode,
            Opcode::Lb | Opcode::Lbu | Opcode::Lh | Opcode::Lhu | Opcode::Lw
        );

        is_load
            && exec.instr.rd != 0
            && (exec.instr.rd == word_rs1(fetch.word) || exec.instr.rd == word_rs2(fetch.word))
    }
}
