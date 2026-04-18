use crate::isa::DecodedInstr;

/// Output of the Fetch stage
#[derive(Clone, Copy, Debug)]
pub struct IfId {
    pub pc:   u32,
    pub word: u32,
}

/// Output of the Decode stage
#[derive(Clone, Copy, Debug)]
pub struct IdEx {
    pub pc:    u32,
    pub instr: DecodedInstr,
    pub rs1:   u32,
    pub rs2:   u32,
}

/// Output of the Execute stage
#[derive(Clone, Copy, Debug)]
pub struct ExMem {
    pub instr:  DecodedInstr,
    pub result: u32,
    pub rs2:    u32,  // store value
}

/// Output of the Memory stage
#[derive(Clone, Copy, Debug)]
pub struct MemWb {
    pub instr:  DecodedInstr,
    pub result: u32,
}
