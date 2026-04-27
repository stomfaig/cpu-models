use crate::memory::mem_fabric::{
    MemProtocol, Memory, ReadReq, ReadResp, RequestId, SimpleRW, SimpleRWReq, SimpleRWResp,
    WriteReq, WriteResp,
};
use crate::utils::step::{Port, Step, SteppedProcess};

use std::collections::VecDeque;

/*
  Simple Cache

  The cache has a read port, and a write port, which are not exposed through
  the interface, but the events are distributed according to their type.

  ** Read port **
  The read port is essentially a stepper, that, given a read checks if
  any of the banks of the cache already contains the value.
  If not, it
    (1) issues a read command to the next level of memory,
    (2) when the result of the read comes back, forwards it to the previous level of memory
    (3) issues a write to the local cache table

  ** Write port **
  (1) Start by finding the cache entry in the cache associated with a write,
  (2) If found, rewrite. If not, _we do not insert the value into cache_ (TODO: do actually bring the write miss elements into cache too.)
  (3) Once cache table access is done, dispatch a call to lower memory levels with the write as well.
*/

struct PTE {
    valid: bool,
    tag: u32,
    val: u32,
}

pub struct CacheData {
    k: usize, // log2 number of sets
    d: usize, // log2 associativity
    table: Vec<PTE>,
}

impl CacheData {
    fn mask(&self) -> u32 {
        (1u32 << self.k as u32).wrapping_sub(1)
    }
    fn assoc(&self) -> usize {
        1 << self.d
    }
    fn bank_size(&self) -> usize {
        1 << self.k
    }
}

// Read port
#[derive(Clone, Copy)]
struct ReadCtx {
    request_id: RequestId,
    index: usize,
    tag: u32,
    bank_to_check: usize,
}

struct ReadPort;

impl SteppedProcess<ReadReq, CacheData> for ReadPort {
    type Ctx = ReadCtx;
    type Result = (RequestId, Option<u32>);

    fn create_context(data: &CacheData, task: ReadReq, request_id: u8) -> ReadCtx {
        let addr = task.0;
        let mask = data.mask();
        ReadCtx {
            request_id,
            index: (mask & addr) as usize,
            tag: addr >> data.k as u32,
            bank_to_check: 0,
        }
    }

    fn step(data: &mut CacheData, ctx: &ReadCtx) -> Step<ReadCtx, (RequestId, Option<u32>)> {
        if ctx.bank_to_check >= data.assoc() {
            return Step::Done((ctx.request_id, None));
        }
        let pte = &data.table[ctx.bank_to_check * data.bank_size() + ctx.index];
        if pte.valid && pte.tag == ctx.tag {
            return Step::Done((ctx.request_id, Some(pte.val)));
        }
        Step::Continue(ReadCtx {
            bank_to_check: ctx.bank_to_check + 1,
            ..*ctx
        })
    }
}

// Write port
#[derive(Clone, Copy)]
struct WriteCtx {
    request_id: RequestId,
    addr: u32,
    val: u32,
    index: usize,
    tag: u32,
    bank_to_check: usize,
}

struct WritePort;

// Result carries the downstream write — always Some for write-through.
// SimpleCache::tick() is responsible for dispatching it to downstream.
impl SteppedProcess<WriteReq, CacheData> for WritePort {
    type Ctx = WriteCtx;
    type Result = (RequestId, WriteReq);

    fn create_context(data: &CacheData, task: WriteReq, request_id: u8) -> WriteCtx {
        let mask = data.mask();
        WriteCtx {
            request_id,
            addr: task.0,
            val: task.1,
            index: (mask & task.0) as usize,
            tag: task.0 >> data.k as u32,
            bank_to_check: 0,
        }
    }

    fn step(data: &mut CacheData, ctx: &WriteCtx) -> Step<WriteCtx, (RequestId, WriteReq)> {
        let downstream = (ctx.request_id, WriteReq(ctx.addr, ctx.val));

        if ctx.bank_to_check >= data.assoc() {
            return Step::Done(downstream);
        }

        let idx = ctx.bank_to_check * data.bank_size() + ctx.index;
        let pte = &mut data.table[idx];
        if pte.valid && pte.tag == ctx.tag {
            pte.val = ctx.val;
            return Step::Done(downstream);
        }

        Step::Continue(WriteCtx {
            bank_to_check: ctx.bank_to_check + 1,
            ..*ctx
        })
    }
}

pub struct SimpleCache {
    data: CacheData,
    read_port: Port<ReadReq, ReadPort, CacheData>,
    write_port: Port<WriteReq, WritePort, CacheData>,
    id_seq: RequestId,
    outbox: VecDeque<SimpleRWResp>,
    downstream: Box<dyn Memory<SimpleRW>>,
}

impl SimpleCache {
    pub fn new(c: usize, d: usize, downstream: Box<dyn Memory<SimpleRW>>) -> Self {
        assert!(d <= c, "associativity cannot exceed total capacity");
        let k = c - d;
        Self {
            data: CacheData {
                k,
                d,
                table: (0..1 << c)
                    .map(|_| PTE {
                        valid: false,
                        tag: 0,
                        val: 0,
                    })
                    .collect(),
            },
            read_port: Port::new(),
            write_port: Port::new(),
            id_seq: 0,
            outbox: VecDeque::new(),
            downstream,
        }
    }

    #[cfg(test)]
    fn seed(&mut self, addr: u32, val: u32) {
        let mask = self.data.mask();
        let index = (mask & addr) as usize;
        let tag = addr >> self.data.k as u32;
        let bank_size = self.data.bank_size();
        for i in 0..self.data.assoc() {
            let pte = &mut self.data.table[i * bank_size + index];
            if !pte.valid {
                *pte = PTE {
                    valid: true,
                    tag,
                    val,
                };
                return;
            }
        }
        panic!("no free way to seed addr 0x{addr:08x}");
    }
}

impl Memory<SimpleRW> for SimpleCache {
    fn send(&mut self, req: <SimpleRW as MemProtocol>::Req) -> RequestId {
        let id = self.id_seq;
        self.id_seq = self.id_seq.wrapping_add(1);
        match req {
            SimpleRWReq::Read(req) => self.read_port.send(req, id),
            SimpleRWReq::Write(req) => self.write_port.send(req, id),
        }
        id
    }

    fn recv(&mut self) -> Option<SimpleRWResp> {
        self.outbox.pop_front()
    }

    fn tick(&mut self) {
        self.read_port.tick(&mut self.data);
        self.write_port.tick(&mut self.data);

        if let Some((id, result)) = self.read_port.pop() {
            match result {
                Some(val) => self.outbox.push_back(ReadResp(id, val).into()),
                None => { /* cache miss — downstream fetch not yet modelled */ }
            }
        }

        if let Some((id, downstream_req)) = self.write_port.pop() {
            self.downstream.send(downstream_req.into());
            self.outbox.push_back(WriteResp(id).into());
        }
    }
}

// Tests
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    // Records all writes forwarded downstream; reads return nothing.
    struct MockMemory {
        writes: Rc<RefCell<Vec<(u32, u32)>>>,
    }

    impl MockMemory {
        fn new() -> (Self, Rc<RefCell<Vec<(u32, u32)>>>) {
            let writes = Rc::new(RefCell::new(vec![]));
            (
                Self {
                    writes: writes.clone(),
                },
                writes,
            )
        }
    }

    impl Memory<SimpleRW> for MockMemory {
        fn send(&mut self, req: SimpleRWReq) -> RequestId {
            if let SimpleRWReq::Write(WriteReq(addr, val)) = req {
                self.writes.borrow_mut().push((addr, val));
            }
            0
        }
        fn recv(&mut self) -> Option<SimpleRWResp> {
            None
        }
        fn tick(&mut self) {}
    }

    fn make_cache(c: usize, d: usize) -> (SimpleCache, Rc<RefCell<Vec<(u32, u32)>>>) {
        let (mock, writes) = MockMemory::new();
        (SimpleCache::new(c, d, Box::new(mock)), writes)
    }

    fn tick_n(cache: &mut SimpleCache, n: usize) {
        for _ in 0..n {
            cache.tick();
        }
    }

    // Direct-mapped (d=0), 4 sets (c=2). Cold read produces no response.
    #[test]
    fn cold_read_is_a_miss() {
        let (mut cache, _) = make_cache(2, 0);
        cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        tick_n(&mut cache, 4);
        assert!(cache.recv().is_none());
    }

    // Seeded read returns the value in one tick (way 0 hit on first step).
    #[test]
    fn seeded_read_hits() {
        let (mut cache, _) = make_cache(2, 0);
        cache.seed(0x10, 42);
        cache.send(SimpleRWReq::Read(ReadReq(0x10)));
        cache.tick();
        match cache.recv() {
            Some(SimpleRWResp::Read(ReadResp(_, val))) => assert_eq!(val, 42),
            other => panic!("expected ReadResp, got {:?}", other.is_some()),
        }
    }

    // Cold write is always forwarded to downstream.
    #[test]
    fn cold_write_forwards_downstream() {
        let (mut cache, downstream_writes) = make_cache(2, 0);
        cache.send(SimpleRWReq::Write(WriteReq(0x04, 99)));
        tick_n(&mut cache, 4);
        assert!(downstream_writes.borrow().contains(&(0x04, 99)));
    }

    // Write to a seeded address updates the cached value.
    #[test]
    fn write_updates_seeded_entry() {
        let (mut cache, _) = make_cache(2, 0);
        cache.seed(0x08, 1);
        cache.send(SimpleRWReq::Write(WriteReq(0x08, 2)));
        tick_n(&mut cache, 2);
        cache.recv(); // consume WriteResp

        cache.send(SimpleRWReq::Read(ReadReq(0x08)));
        tick_n(&mut cache, 2);
        match cache.recv() {
            Some(SimpleRWResp::Read(ReadResp(_, val))) => assert_eq!(val, 2),
            other => panic!("expected updated ReadResp, got {:?}", other.is_some()),
        }
    }

    // Writes to different sets don't interfere.
    // k=2 → index = addr & 0b11, so set 0 = addr 0, set 1 = addr 1.
    #[test]
    fn different_sets_are_independent() {
        let (mut cache, _) = make_cache(2, 0); // 4 sets, 1 way
        cache.seed(0x00, 10); // set 0
        cache.seed(0x01, 20); // set 1

        cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        cache.send(SimpleRWReq::Read(ReadReq(0x01)));
        tick_n(&mut cache, 4);

        let resp1 = cache.recv();
        let resp2 = cache.recv();
        let val = |r: Option<SimpleRWResp>| match r {
            Some(SimpleRWResp::Read(ReadResp(_, v))) => v,
            _ => panic!("expected ReadResp"),
        };
        assert_eq!(val(resp1), 10);
        assert_eq!(val(resp2), 20);
    }

    // 2-way cache: value seeded in way 1 is found after way 0 misses.
    #[test]
    fn two_way_hit_in_second_way() {
        // c=2 (4 entries total), d=1 (2-way). k=1 → 2 sets.
        let (mut cache, _) = make_cache(2, 1);
        // Seed way 0, set 0 with a different tag so way 0 misses.
        cache.seed(0x00, 111); // addr 0x00 → set 0, tag 0
        cache.seed(0x04, 222); // addr 0x04 → set 0, tag 1 (k=1, so tag = addr >> 1)

        // Read 0x04: step 1 checks way 0 (tag 0 ≠ tag 1 → miss),
        //            step 2 checks way 1 (tag 1 = tag 1 → hit).
        cache.send(SimpleRWReq::Read(ReadReq(0x04)));
        tick_n(&mut cache, 4);
        match cache.recv() {
            Some(SimpleRWResp::Read(ReadResp(_, val))) => assert_eq!(val, 222),
            other => panic!("expected ReadResp, got {:?}", other.is_some()),
        }
    }

    // Write produces a WriteResp in the outbox with the correct request ID.
    #[test]
    fn write_response_carries_correct_id() {
        let (mut cache, _) = make_cache(2, 0);
        let id = cache.send(SimpleRWReq::Write(WriteReq(0x00, 7)));
        tick_n(&mut cache, 4);
        match cache.recv() {
            Some(SimpleRWResp::Write(WriteResp(resp_id))) => assert_eq!(resp_id, id),
            _ => panic!("expected WriteResp"),
        }
    }

    // Read response carries the ID that was returned by send().
    #[test]
    fn read_response_carries_correct_id() {
        let (mut cache, _) = make_cache(2, 0);
        cache.seed(0x00, 5);
        let id = cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        tick_n(&mut cache, 2);
        match cache.recv() {
            Some(SimpleRWResp::Read(ReadResp(resp_id, _))) => assert_eq!(resp_id, id),
            _ => panic!("expected ReadResp"),
        }
    }

    // Cold write does NOT allocate: a subsequent read to the same address still misses.
    #[test]
    fn cold_write_does_not_allocate() {
        let (mut cache, _) = make_cache(2, 0);
        cache.send(SimpleRWReq::Write(WriteReq(0x00, 42)));
        tick_n(&mut cache, 4);
        cache.recv(); // consume WriteResp

        cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        tick_n(&mut cache, 4);
        assert!(
            cache.recv().is_none(),
            "cold write must not allocate a cache line"
        );
    }

    // Two reads queued back-to-back are processed serially in FIFO order.
    #[test]
    fn queued_reads_processed_in_order() {
        let (mut cache, _) = make_cache(2, 0);
        cache.seed(0x00, 10);
        cache.seed(0x01, 20);

        // Both enqueued before any ticks — port processes one at a time.
        cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        cache.send(SimpleRWReq::Read(ReadReq(0x01)));
        tick_n(&mut cache, 8);

        let r1 = cache.recv();
        let r2 = cache.recv();
        let val = |r: Option<SimpleRWResp>| match r {
            Some(SimpleRWResp::Read(ReadResp(_, v))) => v,
            _ => panic!("expected ReadResp"),
        };
        assert_eq!(val(r1), 10, "first response should be for first request");
        assert_eq!(val(r2), 20, "second response should be for second request");
    }

    // A seeded read completes in exactly 1 tick (hit on way 0, first step).
    #[test]
    fn seeded_read_latency_is_one_tick() {
        let (mut cache, _) = make_cache(2, 0);
        cache.seed(0x00, 1);
        cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        cache.tick();
        assert!(cache.recv().is_some(), "hit should resolve in 1 tick");
    }

    // A cold read in a 1-way cache takes exactly 2 ticks (check way 0 → miss, then done).
    #[test]
    fn cold_read_latency_is_assoc_plus_one_ticks() {
        let (mut cache, _) = make_cache(2, 0); // assoc = 1
        cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        cache.tick();
        assert!(cache.recv().is_none(), "should not resolve after 1 tick");
        cache.tick();
        // Miss produces no ReadResp — outbox stays empty — but port is now idle.
        assert!(cache.recv().is_none());
    }

    // Aliases: two addresses mapping to the same set/tag collide correctly.
    // With k=2 and the same low 2 bits, a write to one alias updates the cached entry.
    #[test]
    fn same_index_and_tag_aliases() {
        let (mut cache, _) = make_cache(2, 0); // k=2, mask=0b11
        // addr 0x00 and addr 0x10 share index 0 and tag 0 (0x00>>2 = 0, 0x10>>2 = 4 ≠ 0)
        // Use addr 0x00 (tag=0) and addr 0x04 (tag=1, index=0) — same set, different tag.
        cache.seed(0x00, 55);
        cache.send(SimpleRWReq::Write(WriteReq(0x00, 77)));
        tick_n(&mut cache, 2);
        cache.recv(); // consume WriteResp

        cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        cache.tick();
        match cache.recv() {
            Some(SimpleRWResp::Read(ReadResp(_, val))) => assert_eq!(val, 77),
            _ => panic!("expected updated value after write to aliased address"),
        }
    }
}
