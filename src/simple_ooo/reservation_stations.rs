use crate::diag::{DiagNode, Diagnosable};
use crate::isa::Opcode;
use crate::simple_ooo::{IdRs, InstrFormat, OOOALU, ReservationStation};

#[derive(Debug, Clone, Copy)]
pub enum Operand {
  Ready(u32),
  Waiting(usize),
}

#[derive(Debug, Clone, Copy)]
pub struct SREntry {
  rob_tag: usize,
  pub opcode: Opcode,
  pub pc: u32,
  pub imm: i32,
  pub rs1: Operand,
  pub rs2: Operand,
  ready: bool,
}

// Dispatches at most one ready instruction per cycle.
pub struct SimpleReservationStation {
  num_stations: usize,
  station_entries: Vec<SREntry>,
}

impl SimpleReservationStation {
  pub fn new(num_stations: usize) -> Self {
    Self { num_stations, station_entries: vec![] }
  }
}

impl Diagnosable for SimpleReservationStation {
  fn diagnose(&self) -> DiagNode {
    let mut children = vec![
      DiagNode::leaf("capacity", format!("{}/{}", self.station_entries.len(), self.num_stations)),
    ];
    for e in self.station_entries.iter() {
      let rs1 = match e.rs1 {
        Operand::Ready(v)   => format!("0x{v:08x}"),
        Operand::Waiting(t) => format!("waiting(rob[{t}])"),
      };
      let rs2 = match e.rs2 {
        Operand::Ready(v)   => format!("0x{v:08x}"),
        Operand::Waiting(t) => format!("waiting(rob[{t}])"),
      };
      let status = if e.ready { "ready" } else { "blocked" };
      children.push(DiagNode::leaf(
        format!("rob[{}]", e.rob_tag),
        format!("{:?}  rs1={}  rs2={}  {}", e.opcode, rs1, rs2, status),
      ));
    }
    DiagNode::inner("reservation_station", children)
  }
}

impl ReservationStation for SimpleReservationStation {
  fn stall(&self) -> bool { 
    self.station_entries.len() >= self.num_stations
  }
  
  fn enqueue(&mut self, rob_tag: usize, rs1: Operand, rs2: Operand, instr: &IdRs) {
    if self.stall() {
      panic!("Reservation station is full.");
    }

    let ready = matches!(rs1, Operand::Ready(_)) && matches!(rs2, Operand::Ready(_));
    self.station_entries.push(SREntry {
      rob_tag,
      opcode: instr.instr.opcode,
      pc: instr.pc as u32,
      imm: instr.instr.imm,
      rs1,
      rs2,
      ready,
    });
  }

  fn update(&mut self, rob_tag: usize, val: u32) {
    for entry in &mut self.station_entries {
      if let Operand::Waiting(t) = entry.rs1 { if t == rob_tag { entry.rs1 = Operand::Ready(val); } }
      if let Operand::Waiting(t) = entry.rs2 { if t == rob_tag { entry.rs2 = Operand::Ready(val); } }
      if matches!(entry.rs1, Operand::Ready(_)) && matches!(entry.rs2, Operand::Ready(_)) {
        entry.ready = true;
      }
    }
  }

  fn dispatch(&mut self, alu: &mut OOOALU) {
    for i in 0..self.station_entries.len() {
      if self.station_entries[i].ready {
        let entry = self.station_entries.remove(i);
        let instr = InstrFormat { rob_tag: entry.rob_tag, instr: entry };
        if alu.try_enqueue(instr) {
          return;
        } else {
          self.station_entries.insert(0, entry);
        }
      }
    }
  }
}