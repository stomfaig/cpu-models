use crate::memory::simple_cache::EvictionPolicy;

/* Tree-based approximation for the
    LRU (Least recently used) eviction policy

*/
pub struct TreeLRUPolicy {
    assoc: u32,
    lines_per_bank: u32,
    pred_table: Vec<Vec<bool>>,
}

fn extract_binary(bit_width: u32, val: u32) -> Vec<bool> {
    if val.leading_zeros() < 32 - bit_width {
        panic!("");
    }

    let mut le_bits = vec![];
    for i in 0..bit_width {
        le_bits.push(val & (1 << i) != 0);
    }
    le_bits
}

impl EvictionPolicy for TreeLRUPolicy {
    fn new(assoc: u32, lines_per_bank: u32) -> Self {
        let pred_table = (0..lines_per_bank)
            .map(|_i| vec![false; (1 << assoc) - 1 as usize])
            .collect();

        Self {
            assoc,
            lines_per_bank,
            pred_table,
        }
    }

    fn log_hit(&mut self, line: u32, bank: u32) {
        let mut idx = 0;
        for (_l, bit) in extract_binary(self.assoc, bank).iter().enumerate() {
            self.pred_table[line as usize][idx] = !*bit;
            let offset = if *bit { 1 } else { 0 };
            idx = 2 * idx + 1 + offset;
        }
    }

    fn get_eviction_id(&mut self, line: u32) -> u32 {
        let mut idx = 0;
        let mut way = 0u32;
        for l in 0..self.assoc {
            let bit = self.pred_table[line as usize][idx] as u32;
            way |= bit << l;
            idx = 2 * idx + 1 + bit as usize;
        }
        way
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(assoc: u32, sets: u32) -> TreeLRUPolicy {
        TreeLRUPolicy::new(assoc, sets)
    }

    // -----------------------------------------------------------------------
    // extract_binary
    // -----------------------------------------------------------------------

    #[test]
    fn extract_binary_zero() {
        assert_eq!(extract_binary(2, 0), vec![false, false]);
    }

    #[test]
    fn extract_binary_one() {
        assert_eq!(extract_binary(2, 1), vec![true, false]);
    }

    #[test]
    fn extract_binary_all_set() {
        assert_eq!(extract_binary(3, 7), vec![true, true, true]);
    }

    #[test]
    #[should_panic]
    fn extract_binary_overflow_panics() {
        extract_binary(2, 4); // 4 doesn't fit in 2 bits
    }

    // -----------------------------------------------------------------------
    // 2-way (assoc=1): one node per set, two ways
    // -----------------------------------------------------------------------

    #[test]
    fn two_way_cold_evicts_way_0() {
        let mut p = make(1, 1);
        assert_eq!(p.get_eviction_id(0), 0);
    }

    #[test]
    fn two_way_after_hit_on_0_evicts_1() {
        let mut p = make(1, 1);
        p.log_hit(0, 0);
        assert_eq!(p.get_eviction_id(0), 1);
    }

    #[test]
    fn two_way_after_hit_on_1_evicts_0() {
        let mut p = make(1, 1);
        p.log_hit(0, 1);
        assert_eq!(p.get_eviction_id(0), 0);
    }

    #[test]
    fn two_way_mru_never_evicted() {
        let mut p = make(1, 1);
        p.log_hit(0, 0);
        p.log_hit(0, 1);
        assert_ne!(p.get_eviction_id(0), 1, "MRU way 1 should not be evicted");

        let mut p = make(1, 1);
        p.log_hit(0, 1);
        p.log_hit(0, 0);
        assert_ne!(p.get_eviction_id(0), 0, "MRU way 0 should not be evicted");
    }

    #[test]
    fn two_way_toggle_alternates() {
        let mut p = make(1, 1);
        p.log_hit(0, 0);
        assert_eq!(p.get_eviction_id(0), 1);
        p.log_hit(0, 1);
        assert_eq!(p.get_eviction_id(0), 0);
        p.log_hit(0, 0);
        assert_eq!(p.get_eviction_id(0), 1);
    }

    // -----------------------------------------------------------------------
    // 4-way (assoc=2): three nodes per set, four ways
    // -----------------------------------------------------------------------

    #[test]
    fn four_way_cold_evicts_way_0() {
        let mut p = make(2, 1);
        assert_eq!(p.get_eviction_id(0), 0);
    }

    #[test]
    fn four_way_just_accessed_not_evicted() {
        for way in 0..4 {
            let mut p = make(2, 1);
            p.log_hit(0, way);
            assert_ne!(
                p.get_eviction_id(0),
                way,
                "evicted the way we just accessed (way {way})"
            );
        }
    }

    // Accessing ways 0-3 in order: each step the candidate advances to the
    // next un-accessed way, completing a full cycle.
    #[test]
    fn four_way_sequential_access_evicts_lru() {
        let mut p = make(2, 1);
        p.log_hit(0, 0);
        assert_eq!(p.get_eviction_id(0), 1);
        p.log_hit(0, 1);
        assert_eq!(p.get_eviction_id(0), 2);
        p.log_hit(0, 2);
        assert_eq!(p.get_eviction_id(0), 3);
        p.log_hit(0, 3);
        assert_eq!(p.get_eviction_id(0), 0);
    }

    // -----------------------------------------------------------------------
    // Multiple sets are independent
    // -----------------------------------------------------------------------

    #[test]
    fn sets_are_independent() {
        let mut p = make(1, 4);
        p.log_hit(0, 0); // set 0: evict 1
        p.log_hit(1, 1); // set 1: evict 0
        p.log_hit(2, 0); // set 2: evict 1
        // set 3 untouched: cold → evict 0

        assert_eq!(p.get_eviction_id(0), 1);
        assert_eq!(p.get_eviction_id(1), 0);
        assert_eq!(p.get_eviction_id(2), 1);
        assert_eq!(p.get_eviction_id(3), 0);
    }
}
