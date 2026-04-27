use crate::five_stage::stages::{ExMem, IdEx, MemWb};

pub trait ForwardingPolicy {
    /// Optionally override rs1/rs2 in the decoded latch using values from
    /// later pipeline stages, bypassing the register file.
    fn forward(&self, id_ex: &mut IdEx, ex_mem: &Option<ExMem>, mem_wb: &Option<MemWb>);
}

/// No forwarding — always use register file values.
pub struct NoForwarding;

impl ForwardingPolicy for NoForwarding {
    fn forward(&self, _: &mut IdEx, _: &Option<ExMem>, _: &Option<MemWb>) {}
}

/// Full forwarding — forward from ex_mem and mem_wb into execute inputs.
pub struct FullForwarding;

impl ForwardingPolicy for FullForwarding {
    fn forward(&self, id_ex: &mut IdEx, ex_mem: &Option<ExMem>, mem_wb: &Option<MemWb>) {
        let rd_ex = ex_mem.map(|l| (l.instr.rd, l.result));
        let rd_mem = mem_wb.map(|l| (l.instr.rd, l.result));

        if let Some((rd, val)) = rd_ex {
            if rd != 0 && rd == id_ex.instr.rs1 {
                id_ex.rs1 = val;
                return;
            }
            if rd != 0 && rd == id_ex.instr.rs2 {
                id_ex.rs2 = val;
                return;
            }
        }
        if let Some((rd, val)) = rd_mem {
            if rd != 0 && rd == id_ex.instr.rs1 {
                id_ex.rs1 = val;
                return;
            }
            if rd != 0 && rd == id_ex.instr.rs2 {
                id_ex.rs2 = val;
            }
        }
    }
}
