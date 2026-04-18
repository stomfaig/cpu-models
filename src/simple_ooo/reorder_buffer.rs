use crate::circular_buffer::CircularBuffer;
use crate::diag::{DiagNode, Diagnosable};
use crate::isa::{InstResult, InstrFormat};
use crate::simple_ooo::{IdRs, ReorderBuffer, RobTag, TaggedResult};


struct SROBEntry {
  _pc:    u32,
  rd:     u8,
  format: InstrFormat,
  result: Option<u32>,
  ready:  bool,
}

pub struct SimpleReorderBuffer {
  buffer: CircularBuffer<SROBEntry>,
}

impl SimpleReorderBuffer {
  pub fn new(capacity: usize) -> Self {
    Self { buffer: CircularBuffer::new(capacity) }
  }
}

impl Diagnosable for SimpleReorderBuffer {
  fn diagnose(&self) -> DiagNode {
    let head = self.buffer.head_tag();
    let mut children = vec![
      DiagNode::leaf("size", format!("{}/{}", self.buffer.len(), self.buffer.capacity())),
      DiagNode::leaf("head", format!("rob[{head}]")),
    ];
    for (tag, e) in self.buffer.iter_tagged() {
      let marker = if tag == head { " ← head" } else { "" };
      let val = if e.ready {
        format!("rd=x{}  result=0x{:08x}  ready{}", e.rd, e.result.unwrap(), marker)
      } else {
        format!("rd=x{}  pending{}", e.rd, marker)
      };
      children.push(DiagNode::leaf(format!("rob[{tag}]"), val));
    }
    DiagNode::inner("reorder_buffer", children)
  }
}

impl ReorderBuffer for SimpleReorderBuffer {
  fn stall(&self) -> bool { self.buffer.is_full() }

  fn enqueue(&mut self, instr: &IdRs) -> usize {
    if self.stall() {
      panic!("ROB full.")
    }

    self.buffer.push(SROBEntry { _pc: instr.pc, rd: instr.instr.rd, format: instr.instr.format, ready: false, result: None })
  }

  fn update(&mut self, rob_tag: RobTag, result: u32) {
    let entry = self.buffer.access_by_tag(rob_tag);
    entry.result = Some(result);
    entry.ready = true;
  }

  fn release(&mut self) -> Option<TaggedResult> {
    if self.buffer.is_empty() {
      return None;
    }
    if !self.buffer.head().ready {
      return None;
    }
    let rob_tag = self.buffer.head_tag();
    let rob_entry = self.buffer.pop();
    let result = rob_entry.result.expect("ROB entry marked ready without a result");
    Some(TaggedResult { rob_tag, result: InstResult { rd: rob_entry.rd, format: rob_entry.format, result } })
  }

  fn is_tag_ready(&self, tag: RobTag) -> Option<u32> {
    self.buffer.read_by_tag(tag).result
  }
}