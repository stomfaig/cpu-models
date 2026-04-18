pub struct DiagNode {
  pub label: String,
  pub value: Option<String>,
  pub children: Vec<DiagNode>,
}

impl DiagNode {
  pub fn leaf(label: impl Into<String>, value: impl Into<String>) -> Self {
    Self { label: label.into(), value: Some(value.into()), children: vec![] }
  }

  pub fn inner(label: impl Into<String>, children: Vec<DiagNode>) -> Self {
    Self { label: label.into(), value: None, children }
  }

  // Entry point: print the root label then recurse into children.
  pub fn print(&self) {
    println!("{}", self.label);
    let n = self.children.len();
    for (i, child) in self.children.iter().enumerate() {
      child.render("", i == n - 1);
    }
  }

  fn render(&self, prefix: &str, is_last: bool) {
    let connector = if is_last { "└── " } else { "├── " };
    match &self.value {
      Some(v) => println!("{}{}{}: {}", prefix, connector, self.label, v),
      None    => println!("{}{}{}", prefix, connector, self.label),
    }
    let child_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });
    let n = self.children.len();
    for (i, child) in self.children.iter().enumerate() {
      child.render(&child_prefix, i == n - 1);
    }
  }
}

pub trait Diagnosable {
  fn diagnose(&self) -> DiagNode;
}
