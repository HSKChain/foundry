use alloy_primitives::{Address, U256, map::DefaultHashBuilder};
use core::{
    fmt,
    hash::{BuildHasher, Hash, Hasher},
};
use revm::{
    Inspector,
    bytecode::opcode,
    interpreter::{
        Interpreter,
        interpreter_types::{InputsTr, Jumps},
    },
};

// This is the maximum number of edges that can be tracked. There is a tradeoff between performance
// and precision (less collisions).
const MAX_EDGE_COUNT: usize = 65536;

/// An `Inspector` that tracks [edge coverage](https://clang.llvm.org/docs/SanitizerCoverage.html#edge-coverage).
/// Covered edges will not wrap to zero e.g. a loop edge hit more than 255 will still be retained.
// see https://github.com/AFLplusplus/AFLplusplus/blob/5777ceaf23f48ae4ceae60e4f3a79263802633c6/instrumentation/afl-llvm-pass.so.cc#L810-L829
#[derive(Clone)]
pub struct EdgeCovInspector {
    /// Map of hitcounts that can be diffed against to determine if new coverage was reached.
    hitcount: Vec<u8>,
    hash_builder: DefaultHashBuilder,
}

impl fmt::Debug for EdgeCovInspector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EdgeCovInspector").finish_non_exhaustive()
    }
}

impl EdgeCovInspector {
    /// Create a new `EdgeCovInspector` with `MAX_EDGE_COUNT` size.
    pub fn new() -> Self {
        Self { hitcount: vec![0; MAX_EDGE_COUNT], hash_builder: DefaultHashBuilder::default() }
    }

    /// Reset the hitcount to zero.
    pub fn reset(&mut self) {
        self.hitcount.fill(0);
    }

    /// Get an immutable reference to the hitcount.
    pub const fn get_hitcount(&self) -> &[u8] {
        self.hitcount.as_slice()
    }

    /// Consume the inspector and take ownership of the hitcount.
    pub fn into_hitcount(self) -> Vec<u8> {
        self.hitcount
    }

    /// Mark the edge, H(address, pc, jump_dest), as hit.
    fn store_hit(&mut self, address: Address, pc: usize, jump_dest: U256) {
        let mut hasher = self.hash_builder.build_hasher();
        address.hash(&mut hasher);
        pc.hash(&mut hasher);
        jump_dest.hash(&mut hasher);
        // The hash is used to index into the hitcount array,
        // so it must be modulo the maximum edge count.
        let edge_id = (hasher.finish() % MAX_EDGE_COUNT as u64) as usize;
        self.hitcount[edge_id] = self.hitcount[edge_id].checked_add(1).unwrap_or(1);
    }

    #[cold]
    fn do_step(&mut self, interp: &mut Interpreter) {
        let address = interp.input.target_address();
        let current_pc = interp.bytecode.pc();

        match interp.bytecode.opcode() {
            opcode::JUMP => {
                // Unconditional jump.
                if let Ok(jump_dest) = interp.stack.peek(0) {
                    self.store_hit(address, current_pc, jump_dest);
                }
            }
            opcode::JUMPI => {
                if let Ok(stack_value) = interp.stack.peek(1) {
                    let jump_dest = if stack_value.is_zero() {
                        // Fall through.
                        Ok(U256::from(current_pc + 1))
                    } else {
                        // Branch taken.
                        interp.stack.peek(0)
                    };

                    if let Ok(jump_dest) = jump_dest {
                        self.store_hit(address, current_pc, jump_dest);
                    }
                }
            }
            _ => {
                // No-op.
            }
        }
    }
}

impl Default for EdgeCovInspector {
    fn default() -> Self {
        Self::new()
    }
}

impl<CTX> Inspector<CTX> for EdgeCovInspector {
    #[inline]
    fn step(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        if matches!(interp.bytecode.opcode(), opcode::JUMP | opcode::JUMPI) {
            self.do_step(interp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;
    use revm::bytecode::Bytecode;

    fn interpreter_with_opcode(opcode: u8) -> Interpreter {
        Interpreter::default().with_bytecode(Bytecode::new_raw(Bytes::from(vec![opcode])))
    }

    fn nonzero_hitcounts(inspector: &EdgeCovInspector) -> usize {
        inspector.get_hitcount().iter().filter(|&&count| count != 0).count()
    }

    #[test]
    fn jump_records_edge() {
        let mut inspector = EdgeCovInspector::new();
        let mut interpreter = interpreter_with_opcode(opcode::JUMP);
        assert!(interpreter.stack.push(U256::from(7)));

        inspector.do_step(&mut interpreter);

        assert_eq!(nonzero_hitcounts(&inspector), 1);
    }

    #[test]
    fn jumpi_records_taken_and_fallthrough_edges() {
        let mut taken = EdgeCovInspector::new();
        let mut taken_interpreter = interpreter_with_opcode(opcode::JUMPI);
        assert!(taken_interpreter.stack.push(U256::from(1)));
        assert!(taken_interpreter.stack.push(U256::from(7)));

        taken.do_step(&mut taken_interpreter);

        let mut fallthrough = EdgeCovInspector::new();
        let mut fallthrough_interpreter = interpreter_with_opcode(opcode::JUMPI);
        assert!(fallthrough_interpreter.stack.push(U256::ZERO));
        assert!(fallthrough_interpreter.stack.push(U256::from(7)));

        fallthrough.do_step(&mut fallthrough_interpreter);

        assert_eq!(nonzero_hitcounts(&taken), 1);
        assert_eq!(nonzero_hitcounts(&fallthrough), 1);
    }

    #[test]
    fn reset_clears_hitcounts() {
        let mut inspector = EdgeCovInspector::new();
        inspector.store_hit(Address::ZERO, 0, U256::from(1));

        inspector.reset();

        assert_eq!(nonzero_hitcounts(&inspector), 0);
    }

    #[test]
    fn hitcount_never_wraps_to_zero() {
        let mut inspector = EdgeCovInspector::new();

        for _ in 0..=u8::MAX {
            inspector.store_hit(Address::ZERO, 0, U256::from(1));
        }

        assert_eq!(nonzero_hitcounts(&inspector), 1);
        assert_eq!(inspector.get_hitcount().iter().copied().max(), Some(1));
    }
}
