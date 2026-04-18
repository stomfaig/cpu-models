pub struct Latch<T> {
  stage: Option<T>,
  active: Option<T>,
}

impl<T> Latch<T> {

  pub fn new() -> Self {
    Self { stage: None, active: None }
  }

  pub fn stage(&mut self, val: T) {
    self.stage = Some(val);
  }

  pub fn direct_stage(&mut self, maybe_val: Option<T>) {
    self.stage = maybe_val;
  }

  pub fn update(&mut self) {
      self.active = self.stage.take();
  }

  pub fn read(&mut self) -> Option<T> {
    self.active.take()
  }

  // Returns a copy of the active value without consuming it.
  // Used to snapshot start-of-cycle state for hazard/forwarding checks.
  pub fn peek(&self) -> Option<T> where T: Copy {
    self.active
  }
}