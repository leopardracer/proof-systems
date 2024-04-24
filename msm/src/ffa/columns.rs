use crate::columns::{Column, ColumnIndexer};

use crate::N_LIMBS;

/// Number of columns in the FFA circuits.
pub const FFA_N_COLUMNS: usize = 5 * N_LIMBS;
pub const FFA_NPUB_COLUMNS: usize = N_LIMBS;

#[derive(Clone, Copy, Debug, PartialEq)]
/// Column indexer for MSM columns.
///
/// They represent the equation
///   `InputA(i) + InputB(i) = ModulusF(i) * Quotient + Carry(i) * 2^LIMB_SIZE - Carry(i-1)`
pub enum FFAColumn {
    InputA(usize),
    InputB(usize),
    ModulusF(usize),
    Remainder(usize),
    Carry(usize),
    Quotient,
}

impl ColumnIndexer for FFAColumn {
    const COL_N: usize = FFA_N_COLUMNS;
    fn to_column(self) -> Column {
        let to_column_inner = |offset, i| {
            assert!(i < N_LIMBS);
            Column::X(N_LIMBS * offset + i)
        };
        match self {
            FFAColumn::InputA(i) => to_column_inner(0, i),
            FFAColumn::InputB(i) => to_column_inner(1, i),
            FFAColumn::ModulusF(i) => to_column_inner(2, i),
            FFAColumn::Remainder(i) => to_column_inner(3, i),
            FFAColumn::Carry(i) => {
                assert!(i < N_LIMBS - 1);
                to_column_inner(4, i)
            }
            FFAColumn::Quotient => to_column_inner(4, N_LIMBS - 1),
        }
    }
}
