use kimchi::circuits::expr::{CacheId, ConstantExpr, Expr, FormattedOutput};
use std::collections::HashMap;
use strum_macros::{EnumCount as EnumCountMacro, EnumIter};

/// This enum represents the different gadgets that can be used in the circuit.
/// The selectors are defined at setup time, can take only the values `0` or
/// `1` and are public.
// IMPROVEME: should we merge it with the Instruction enum?
// It might not be that obvious to do so, as the Instruction enum could be
// defining operations that are not "fixed" in the circuit, but rather
// depend on runtime values (e.g. in a zero-knowledge virtual machine).
#[derive(Debug, Clone, Copy, PartialEq, EnumCountMacro, EnumIter)]
pub enum Gadget {
    App,
    // Permutation argument
    PermutationArgument,
    // Two old gadgets
    /// Decompose a 255 bits scalar into 16 chunks of 16 bits.
    SixteenBitsDecomposition,
    /// Decompose a 16bits chunk into individual bits.
    BitDecompositionFrom16Bits,
    /// This gadget decomposes a 255 bits value into bits using 17 lines and 17
    /// columns.
    BitDecomposition,
    // Elliptic curve related gadgets
    EllipticCurveAddition,
    EllipticCurveScaling,
    /// This gadget implement the Poseidon hash instance described in the
    /// top-level documentation. The implementation does not use the "next row"
    /// and is unsafe at the moment as no permutation argument is implemented.
    Poseidon,
    /// This gadget implement the Poseidon hash instance described in the
    /// top-level documentation. Compared to the previous one (that might be
    /// deprecated in the future), this implementation does use the "next row"
    /// to allow the computation of one additional round per row. In the current
    /// setup, with [crate::NUMBER_OF_COLUMNS] columns, we can compute 5 full
    /// rounds per row.
    PoseidonNextRow,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Column {
    Selector(Gadget),
    PublicInput(usize),
    X(usize),
}

pub type E<Fp> = Expr<ConstantExpr<Fp>, Column>;

// Code to allow for pretty printing of the expressions
impl FormattedOutput for Column {
    fn latex(&self, _cache: &mut HashMap<CacheId, Self>) -> String {
        match self {
            Column::Selector(sel) => match sel {
                Gadget::App => "q_app".to_string(),
                Gadget::PermutationArgument => "q_perm".to_string(),
                Gadget::SixteenBitsDecomposition => "q_16bits".to_string(),
                Gadget::BitDecompositionFrom16Bits => "q_bit_from_16bits".to_string(),
                Gadget::BitDecomposition => "q_bits".to_string(),
                Gadget::EllipticCurveAddition => "q_ec_add".to_string(),
                Gadget::EllipticCurveScaling => "q_ec_mul".to_string(),
                Gadget::Poseidon => "q_pos".to_string(),
                Gadget::PoseidonNextRow => "q_pos_next_row".to_string(),
            },
            Column::PublicInput(i) => format!("pi_{{{i}}}").to_string(),
            Column::X(i) => format!("x_{{{i}}}").to_string(),
        }
    }

    fn text(&self, _cache: &mut HashMap<CacheId, Self>) -> String {
        match self {
            Column::Selector(sel) => match sel {
                Gadget::App => "q_app".to_string(),
                Gadget::PermutationArgument => "q_perm".to_string(),
                Gadget::SixteenBitsDecomposition => "q_16bits".to_string(),
                Gadget::BitDecompositionFrom16Bits => "q_bit_from_16bits".to_string(),
                Gadget::BitDecomposition => "q_bits".to_string(),
                Gadget::EllipticCurveAddition => "q_ec_add".to_string(),
                Gadget::EllipticCurveScaling => "q_ec_mul".to_string(),
                Gadget::Poseidon => "q_pos".to_string(),
                Gadget::PoseidonNextRow => "q_pos_next_row".to_string(),
            },
            Column::PublicInput(i) => format!("pi[{i}]"),
            Column::X(i) => format!("x[{i}]"),
        }
    }

    fn ocaml(&self, _cache: &mut HashMap<CacheId, Self>) -> String {
        // FIXME
        unimplemented!("Not used at the moment")
    }

    fn is_alpha(&self) -> bool {
        // FIXME
        unimplemented!("Not used at the moment")
    }
}
