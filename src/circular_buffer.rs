pub struct CircularBuffer<T> {
  head: usize,
  tail: usize,
  size: usize,
  data: Vec<Option<T>>,
}

impl<T> CircularBuffer<T> {

  pub fn new(capacity: usize) -> Self {
    Self { head: 0, tail: 0, size: 0, data: (0..capacity).map(|_| None).collect() }
  }

  pub fn capacity(&self) -> usize { self.data.len() }

  pub fn is_full(&self) -> bool { self.size == self.data.len() }

  pub fn is_empty(&self) -> bool { self.size == 0 }

  pub fn push(&mut self, item: T) -> usize {
    if self.is_full() { panic!("Trying to push to a full circular buffer."); }
    let idx = self.tail;
    self.data[idx] = Some(item);
    self.tail = (self.tail + 1) % self.data.len();
    self.size += 1;
    idx
  }

  pub fn pop(&mut self) -> T {
    if self.is_empty() { panic!("Trying to pop from an empty circular buffer"); }
    let elem = self.data[self.head].take().unwrap();
    self.head = (self.head + 1) % self.data.len();
    self.size -= 1;
    elem
  }

  pub fn head_tag(&self) -> usize {
    return self.head
  }

  pub fn head(&self) -> &T {
    self.data[self.head].as_ref().unwrap()
  }

  pub fn read_by_tag(&self, tag: usize) -> &T {
    self.data[tag].as_ref().expect("Invalid access in circular buffer")
  }

  pub fn access_by_tag(&mut self, tag: usize) -> &mut T {
    self.data[tag].as_mut().expect("Invalid access in circular buffer")
  }

  pub fn len(&self) -> usize { self.size }

  pub fn iter(&self) -> impl Iterator<Item = &T> {
    let n = self.data.len();
    let head = self.head;
    (0..self.size).map(move |i| self.data[(head + i) % n].as_ref().unwrap())
  }

  // Yields (tag, entry) pairs where tag is the physical slot index used as rob tag.
  pub fn iter_tagged(&self) -> impl Iterator<Item = (usize, &T)> {
    let n = self.data.len();
    let head = self.head;
    (0..self.size).map(move |i| {
      let tag = (head + i) % n;
      (tag, self.data[tag].as_ref().unwrap())
    })
  }
}
