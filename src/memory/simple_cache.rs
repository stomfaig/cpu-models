use crate::memory::mem_fabric::{
    MemProtocol, Memory, ReadReq, ReadResp, RequestId, SimpleRW, SimpleRWReq, SimpleRWResp,
    WriteReq, WriteResp,
};
use crate::memory::tree_lru::TreeLRUPolicy;
use crate::utils::step::{Port, Step, SteppedProcess};

use std::collections::{HashMap, VecDeque};

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

    fn insert_value(&mut self, addr: u32, bank: u32, val: u32) {
        let index = (self.mask() & addr) as usize;
        let tag = addr >> self.k as u32;
        let bank_size = self.bank_size();
        self.table[index + (bank as usize) * bank_size] = PTE {
            valid: true,
            tag,
            val,
        };
    }
}

// Read port
#[derive(Clone, Copy)]
struct ReadCtx {
    request_id: RequestId,
    addr: u32,
    index: usize,
    tag: u32,
    bank_to_check: usize,
}

struct ReadPort;

struct ReadPortResult {
    request_id: RequestId,
    addr: u32,
    val_and_bank: Option<(u32, usize)>,
}

impl SteppedProcess<ReadReq, CacheData> for ReadPort {
    type Ctx = ReadCtx;
    // (request_id, addr, Some((val, hitting_bank)) on hit | None on miss)
    type Result = ReadPortResult;

    fn create_context(data: &CacheData, task: ReadReq, request_id: u8) -> ReadCtx {
        let addr = task.0;
        let mask = data.mask();
        ReadCtx {
            request_id,
            addr,
            index: (mask & addr) as usize,
            tag: addr >> data.k as u32,
            bank_to_check: 0,
        }
    }

    fn step(data: &mut CacheData, ctx: &ReadCtx) -> Step<ReadCtx, ReadPortResult> {
        if ctx.bank_to_check >= data.assoc() {
            return Step::Done(ReadPortResult {
                request_id: ctx.request_id,
                addr: ctx.addr,
                val_and_bank: None,
            });
        }
        let pte = &data.table[ctx.bank_to_check * data.bank_size() + ctx.index];
        if pte.valid && pte.tag == ctx.tag {
            return Step::Done(ReadPortResult {
                request_id: ctx.request_id,
                addr: ctx.addr,
                val_and_bank: Some((pte.val, ctx.bank_to_check)),
            });
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

pub trait EvictionPolicy {
    fn new(assoc: u32, lines_per_bank: u32) -> Self
    where
        Self: Sized;

    fn log_hit(&mut self, line: u32, bank: u32);
    fn get_eviction_id(&mut self, line: u32) -> u32;
}

struct PendingRead {
    upstream_request_id: RequestId,
    addr: u32,
    index: u32,
}

pub struct SimpleCache {
    data: CacheData,
    read_port: Port<ReadReq, ReadPort, CacheData>,
    write_port: Port<WriteReq, WritePort, CacheData>,
    id_seq: RequestId,
    outbox: VecDeque<SimpleRWResp>,
    downstream: Box<dyn Memory<SimpleRW>>,
    pending_buffer: HashMap<RequestId, PendingRead>,
    pending_buffer_size: usize,
    eviction_policy: Box<dyn EvictionPolicy>,
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
            pending_buffer: HashMap::new(),
            pending_buffer_size: 16,
            eviction_policy: Box::new(TreeLRUPolicy::new(d as u32, (1 << k) as u32)),
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

        // check if the read port has any results
        if let Some(read_result) = self.read_port.pop() {
            // we recompute the index instead of storing it in the
            //  ReadPortResult, since it is derivable, and would just
            //  bloat otherwise.
            let index = (self.data.mask() & read_result.addr) as u32;
            match read_result.val_and_bank {
                Some((val, bank)) => {
                    self.outbox
                        .push_back(ReadResp(read_result.request_id, val).into());
                    self.eviction_policy.log_hit(index, bank as u32);
                }
                None => {
                    let downstream_request_id =
                        self.downstream.send(ReadReq(read_result.addr).into());
                    self.pending_buffer.insert(
                        downstream_request_id,
                        PendingRead {
                            upstream_request_id: read_result.request_id,
                            addr: read_result.addr,
                            index,
                        },
                    );
                }
            }
        }

        // check if the downstream memory returned any requests
        if let Some(message) = self.downstream.recv() {
            // we ignore downstream write acknowledgements for now
            if let SimpleRWResp::Read(ReadResp(id, val)) = message {
                if let Some(pr) = self.pending_buffer.remove(&id) {
                    let bank = self.eviction_policy.get_eviction_id(pr.index);
                    self.data.insert_value(pr.addr, bank, val);
                    self.eviction_policy.log_hit(pr.index, bank);
                    self.outbox
                        .push_back(ReadResp(pr.upstream_request_id, val).into());
                } else {
                    panic!("response to non-pending read");
                }
            };
        }

        // check if the write module has any results
        if let Some((id, downstream_req)) = self.write_port.pop() {
            // maybe this should be moved to the message sorter
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

    // -----------------------------------------------------------------------
    // Fill-on-read-miss tests
    // Downstream memory responds immediately; fill therefore completes in the
    // same tick the miss is detected.
    // -----------------------------------------------------------------------

    // Backed downstream: responds to reads with pre-loaded data.
    struct BackedMemory {
        data: std::collections::HashMap<u32, u32>,
        pending: VecDeque<SimpleRWResp>,
        read_count: Rc<RefCell<u32>>,
        next_id: RequestId,
    }

    impl BackedMemory {
        fn new(data: std::collections::HashMap<u32, u32>) -> (Self, Rc<RefCell<u32>>) {
            let read_count = Rc::new(RefCell::new(0u32));
            (
                Self {
                    data,
                    pending: VecDeque::new(),
                    read_count: read_count.clone(),
                    next_id: 0,
                },
                read_count,
            )
        }
    }

    impl Memory<SimpleRW> for BackedMemory {
        fn send(&mut self, req: SimpleRWReq) -> RequestId {
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1);
            if let SimpleRWReq::Read(ReadReq(addr)) = req {
                *self.read_count.borrow_mut() += 1;
                let val = self.data.get(&addr).copied().unwrap_or(0);
                self.pending.push_back(ReadResp(id, val).into());
            }
            id
        }
        fn recv(&mut self) -> Option<SimpleRWResp> {
            self.pending.pop_front()
        }
        fn tick(&mut self) {}
    }

    fn make_backed(
        c: usize,
        d: usize,
        data: std::collections::HashMap<u32, u32>,
    ) -> (SimpleCache, Rc<RefCell<u32>>) {
        let (mem, read_count) = BackedMemory::new(data);
        (SimpleCache::new(c, d, Box::new(mem)), read_count)
    }

    fn read_val(cache: &mut SimpleCache) -> u32 {
        match cache.recv() {
            Some(SimpleRWResp::Read(ReadResp(_, v))) => v,
            _ => panic!("expected ReadResp"),
        }
    }

    // A cold read miss fetches from downstream and the value arrives upstream.
    // Direct-mapped (d=0), assoc=1: miss detected on tick 2, fill same tick.
    #[test]
    fn read_miss_fills_cache_and_returns_value() {
        let (mut cache, _) = make_backed(2, 0, [(0x00, 42)].into());
        cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        tick_n(&mut cache, 2);
        assert_eq!(read_val(&mut cache), 42);
    }

    // The upstream request ID is preserved through the fill path.
    #[test]
    fn fill_carries_correct_request_id() {
        let (mut cache, _) = make_backed(2, 0, [(0x00, 1)].into());
        let id = cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        tick_n(&mut cache, 2);
        match cache.recv() {
            Some(SimpleRWResp::Read(ReadResp(resp_id, _))) => assert_eq!(resp_id, id),
            _ => panic!("expected ReadResp"),
        }
    }

    // After a fill the line is resident; a second read hits without going downstream.
    #[test]
    fn filled_line_hits_on_next_read() {
        let (mut cache, read_count) = make_backed(2, 0, [(0x00, 7)].into());
        cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        tick_n(&mut cache, 2);
        cache.recv(); // consume fill response

        cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        cache.tick(); // hit resolves in 1 tick
        assert_eq!(read_val(&mut cache), 7);
        assert_eq!(
            *read_count.borrow(),
            1,
            "second read must not go downstream"
        );
    }

    // Two sequential misses to different sets: both fill correctly and the
    // MSHR entry for the first is removed before the second is processed.
    #[test]
    fn two_sequential_fills_to_different_sets() {
        // k=2 (d=0, c=2): set index = addr & 0x3; addr 0x00 → set 0, addr 0x01 → set 1.
        let (mut cache, _) = make_backed(2, 0, [(0x00, 10), (0x01, 20)].into());

        cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        tick_n(&mut cache, 2);
        assert_eq!(read_val(&mut cache), 10);

        cache.send(SimpleRWReq::Read(ReadReq(0x01)));
        tick_n(&mut cache, 2);
        assert_eq!(read_val(&mut cache), 20);
    }

    // 2-way cache (d=1, c=2 → k=1, 2 sets): two addresses mapping to the same
    // set both miss and fill distinct ways, then both hit on the next read.
    // addr 0x00: set = 0x00 & 1 = 0, tag = 0x00 >> 1 = 0
    // addr 0x02: set = 0x02 & 1 = 0, tag = 0x02 >> 1 = 1  (same set, different tag)
    #[test]
    fn two_way_cache_fills_both_ways_and_both_hit() {
        let (mut cache, read_count) = make_backed(2, 1, [(0x00, 11), (0x02, 22)].into());

        // First miss: cold PLRU → fills way 0.
        cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        tick_n(&mut cache, 3); // 2-way: miss detected on tick 3
        assert_eq!(read_val(&mut cache), 11);

        // Second miss to same set: PLRU updated after first fill → fills way 1.
        cache.send(SimpleRWReq::Read(ReadReq(0x02)));
        tick_n(&mut cache, 3);
        assert_eq!(read_val(&mut cache), 22);

        assert_eq!(*read_count.borrow(), 2, "only two downstream reads");

        // Both lines now resident — subsequent reads hit without going downstream.
        cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        cache.tick();
        assert_eq!(read_val(&mut cache), 11);

        cache.send(SimpleRWReq::Read(ReadReq(0x02)));
        tick_n(&mut cache, 2); // way 1 requires 2 steps
        assert_eq!(read_val(&mut cache), 22);

        assert_eq!(
            *read_count.borrow(),
            2,
            "no new downstream reads after fills"
        );
    }

    // Eviction: once all ways in a set are filled, the next miss to that set
    // evicts the PLRU victim and the new value is accessible.
    // 2-way, same set: fill way 0 (addr 0x00), fill way 1 (addr 0x02),
    // then miss addr 0x04 (set=0, tag=2) → evicts PLRU way → verify new value readable.
    #[test]
    fn eviction_replaces_lru_way_and_new_value_is_readable() {
        let (mut cache, _) = make_backed(2, 1, [(0x00, 1), (0x02, 2), (0x04, 3)].into());

        // Fill way 0, then way 1.
        cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        tick_n(&mut cache, 3);
        cache.recv();

        cache.send(SimpleRWReq::Read(ReadReq(0x02)));
        tick_n(&mut cache, 3);
        cache.recv();

        // Access 0x00 to make 0x02 the LRU.
        cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        cache.tick();
        cache.recv();

        // Miss on 0x04 → should evict 0x02 (LRU) and fill with val=3.
        cache.send(SimpleRWReq::Read(ReadReq(0x04)));
        tick_n(&mut cache, 3);
        assert_eq!(read_val(&mut cache), 3, "evicted way should hold new value");
    }

    // A write that hits the cache must still be forwarded downstream (write-through).
    // write_updates_seeded_entry checks the cache side; this checks the downstream side.
    #[test]
    fn write_hit_also_forwards_to_downstream() {
        let (mut cache, downstream_writes) = make_cache(2, 0);
        cache.seed(0x00, 1);
        cache.send(SimpleRWReq::Write(WriteReq(0x00, 2)));
        tick_n(&mut cache, 2);
        assert!(
            downstream_writes.borrow().contains(&(0x00, 2)),
            "write hit must still be forwarded downstream",
        );
    }

    // fill → write → read: value written after a fill is what a subsequent read sees.
    #[test]
    fn write_to_filled_line_is_readable() {
        let (mut cache, _) = make_backed(2, 0, [(0x00, 42)].into());

        cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        tick_n(&mut cache, 2);
        cache.recv(); // consume fill ReadResp

        cache.send(SimpleRWReq::Write(WriteReq(0x00, 99)));
        tick_n(&mut cache, 2);
        cache.recv(); // consume WriteResp

        cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        cache.tick();
        assert_eq!(read_val(&mut cache), 99);
    }

    // Two reads to the same cold address: the first triggers a fill;
    // the second (still queued in the port inbox) hits the freshly filled line.
    #[test]
    fn second_read_to_same_cold_addr_hits_after_fill() {
        let (mut cache, read_count) = make_backed(2, 0, [(0x00, 77)].into());
        cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        cache.send(SimpleRWReq::Read(ReadReq(0x00)));
        tick_n(&mut cache, 3);
        assert_eq!(read_val(&mut cache), 77); // first: served via fill
        assert_eq!(read_val(&mut cache), 77); // second: cache hit
        assert_eq!(
            *read_count.borrow(),
            1,
            "only one downstream fetch for two reads"
        );
    }
}
