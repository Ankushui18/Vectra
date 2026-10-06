//! Compiler: AST → flat stack-machine bytecode (Task 2.1 pipeline stage 3).
//!
//! [`compile`] runs [`fold_constants`] (IEEE-exact constant folding — the
//! same operations the VM would execute, just earlier) and then emits a
//! linear [`Instr`] stream. Variables compile to **slot indices**, not names:
//! each evaluation materializes slots once up front, so the hot arithmetic
//! loop never touches a `HashMap`.
//!
//! Example: `$a + 5` → `slots: ["a"], code: [Load(0), Push(5.0), Add]`.

use crate::ast::{BinOp, ExprNode, FunctionName};
use std::collections::{HashMap, HashSet};
use vectra_core::VariableId;

/// Stack-machine instruction. `Copy` and branch-friendly: the evaluator is a
/// single `match` loop over a `&[Instr]` slice with no allocation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Instr {
    Push(f64),
    /// Push variable slot `slots[i]`.
    Load(u32),
    Neg,
    Add,
    Sub,
    Mul,
    Div,
    Sin,
    Cos,
    Abs,
    /// Pop `hi`, `lo`, `x` (pushed in call order `x, lo, hi`); push `clamp`.
    Clamp,
}

impl BinOp {
    fn instr(&self) -> Instr {
        match self {
            Self::Add => Instr::Add,
            Self::Sub => Instr::Sub,
            Self::Mul => Instr::Mul,
            Self::Div => Instr::Div,
        }
    }
}

impl FunctionName {
    fn instr(&self) -> Instr {
        match self {
            Self::Sin => Instr::Sin,
            Self::Cos => Instr::Cos,
            Self::Abs => Instr::Abs,
            Self::Clamp => Instr::Clamp,
        }
    }
}

/// A compiled expression: flat code + ordered variable slots.
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledExpression {
    pub code: Vec<Instr>,
    /// `Load(i)` reads slot `i`. First-occurrence order (deterministic).
    pub slots: Vec<VariableId>,
    /// Original source (inspector display + diagnostics).
    pub source: String,
}

impl CompiledExpression {
    /// All variables this expression reads — the Phase-4 dependency-graph
    /// input: when one of these changes, this expression must re-evaluate.
    /// Includes `"time"` when `$time` is used (the clock is a global dep).
    pub fn dependencies(&self) -> HashSet<VariableId> {
        self.slots.iter().cloned().collect()
    }

    /// True when evaluation needs no context at all (fully folded).
    pub fn is_constant(&self) -> bool {
        self.slots.is_empty()
    }
}

/// Bottom-up constant folding. Only *provably identical* rewrites: the folded
/// operations are bit-for-bit the ones the VM would run (same IEEE ops, same
/// order). No algebraic identities (`x * 1`, `x + 0`) — those are UNSOUND for
/// floats (`inf * 0 == NaN`, `(-0.0) + 0.0 == 0.0`).
pub fn fold_constants(ast: &ExprNode) -> ExprNode {
    match ast {
        ExprNode::Literal(_) | ExprNode::Variable(_) => ast.clone(),
        ExprNode::Neg(inner) => match fold_constants(inner) {
            ExprNode::Literal(n) => ExprNode::Literal(-n),
            folded => ExprNode::Neg(Box::new(folded)),
        },
        ExprNode::Binary { op, left, right } => {
            let (l, r) = (fold_constants(left), fold_constants(right));
            match (&l, &r) {
                (ExprNode::Literal(a), ExprNode::Literal(b)) => ExprNode::Literal(op.apply(*a, *b)),
                _ => ExprNode::Binary {
                    op: *op,
                    left: Box::new(l),
                    right: Box::new(r),
                },
            }
        }
        ExprNode::Call { name, args } => {
            let folded: Vec<ExprNode> = args.iter().map(fold_constants).collect();
            if folded.iter().all(|a| matches!(a, ExprNode::Literal(_))) {
                let values: Vec<f64> = folded
                    .iter()
                    .map(|a| match a {
                        ExprNode::Literal(n) => *n,
                        _ => unreachable!("guarded by all() above"),
                    })
                    .collect();
                ExprNode::Literal(name.apply(&values))
            } else {
                ExprNode::Call {
                    name: *name,
                    args: folded,
                }
            }
        }
    }
}

struct Emitter {
    code: Vec<Instr>,
    slots: Vec<VariableId>,
    index: HashMap<VariableId, u32>,
}

impl Emitter {
    fn new() -> Self {
        Self {
            code: Vec::new(),
            slots: Vec::new(),
            index: HashMap::new(),
        }
    }

    fn slot(&mut self, name: &VariableId) -> u32 {
        if let Some(i) = self.index.get(name) {
            return *i;
        }
        let i = self.slots.len() as u32;
        self.slots.push(name.clone());
        self.index.insert(name.clone(), i);
        i
    }

    fn emit(&mut self, node: &ExprNode) {
        match node {
            ExprNode::Literal(n) => self.code.push(Instr::Push(*n)),
            ExprNode::Variable(name) => {
                let i = self.slot(name);
                self.code.push(Instr::Load(i));
            }
            ExprNode::Neg(inner) => {
                self.emit(inner);
                self.code.push(Instr::Neg);
            }
            ExprNode::Binary { op, left, right } => {
                self.emit(left);
                self.emit(right);
                self.code.push(op.instr());
            }
            ExprNode::Call { name, args } => {
                for arg in args {
                    self.emit(arg);
                }
                self.code.push(name.instr());
            }
        }
    }
}

/// Compile an AST: fold constants, then emit code + slots. Infallible — all
/// validation (names, arity, structure) happened during parsing.
pub fn compile(ast: &ExprNode, source: impl Into<String>) -> CompiledExpression {
    let folded = fold_constants(ast);
    let mut emitter = Emitter::new();
    emitter.emit(&folded);
    CompiledExpression {
        code: emitter.code,
        slots: emitter.slots,
        source: source.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codegen_shape_and_slot_interning() {
        // $a + 5 → [Load(0), Push(5.0), Add]; repeated $a reuses slot 0.
        let ast = ExprNode::Binary {
            op: BinOp::Add,
            left: Box::new(ExprNode::Variable("a".to_string())),
            right: Box::new(ExprNode::Literal(5.0)),
        };
        let compiled = compile(&ast, "$a + 5");
        assert_eq!(
            compiled.code,
            vec![Instr::Load(0), Instr::Push(5.0), Instr::Add]
        );
        assert_eq!(compiled.slots, vec!["a".to_string()]);
        assert_eq!(compiled.dependencies(), HashSet::from(["a".to_string()]));
    }

    #[test]
    fn repeated_variables_share_one_slot() {
        let ast = ExprNode::Binary {
            op: BinOp::Mul,
            left: Box::new(ExprNode::Variable("x".to_string())),
            right: Box::new(ExprNode::Variable("x".to_string())),
        };
        let compiled = compile(&ast, "$x * $x");
        assert_eq!(
            compiled.code,
            vec![Instr::Load(0), Instr::Load(0), Instr::Mul]
        );
        assert_eq!(compiled.slots, vec!["x".to_string()]);
    }

    #[test]
    fn constants_fold_to_single_push() {
        let ast = ExprNode::Binary {
            op: BinOp::Add,
            left: Box::new(ExprNode::Literal(2.0)),
            right: Box::new(ExprNode::Binary {
                op: BinOp::Mul,
                left: Box::new(ExprNode::Literal(3.0)),
                right: Box::new(ExprNode::Literal(4.0)),
            }),
        };
        let compiled = compile(&ast, "2 + 3 * 4");
        assert_eq!(compiled.code, vec![Instr::Push(14.0)]);
        assert!(compiled.is_constant());
    }

    #[test]
    fn clamp_arg_order_is_x_lo_hi() {
        let ast = ExprNode::Call {
            name: FunctionName::Clamp,
            args: vec![
                ExprNode::Variable("v".to_string()),
                ExprNode::Literal(0.0),
                ExprNode::Literal(1.0),
            ],
        };
        let compiled = compile(&ast, "clamp($v, 0, 1)");
        assert_eq!(
            compiled.code,
            vec![
                Instr::Load(0),
                Instr::Push(0.0),
                Instr::Push(1.0),
                Instr::Clamp
            ]
        );
    }

    #[test]
    fn no_unsound_algebraic_identities() {
        // $x * 1 must NOT fold (inf * 1 == inf but folding changes nothing
        // here anyway — the point is the variable survives, no rewrite).
        let ast = ExprNode::Binary {
            op: BinOp::Mul,
            left: Box::new(ExprNode::Variable("x".to_string())),
            right: Box::new(ExprNode::Literal(1.0)),
        };
        assert_eq!(fold_constants(&ast), ast);
    }
}
