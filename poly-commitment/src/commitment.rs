//! This module implements Dlog-based polynomial commitment schema.
//! The following functionality is implemented
//!
//! 1. Commit to polynomial with its max degree
//! 2. Open polynomial commitment batch at the given evaluation point and
//! scaling factor scalar producing the batched opening proof
//! 3. Verify batch of batched opening proofs

use ark_ec::{
    models::short_weierstrass::Affine as SWJAffine, short_weierstrass::SWCurveConfig, AffineRepr,
    CurveGroup, VariableBaseMSM,
};
use ark_ff::{BigInteger, Field, One, PrimeField, Zero};
use ark_poly::univariate::DensePolynomial;
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use core::ops::{Add, Sub};
use groupmap::{BWParameters, GroupMap};
use mina_poseidon::{sponge::ScalarChallenge, FqSponge};
use o1_utils::ExtendedDensePolynomial as _;
use serde::{de::Visitor, Deserialize, Serialize};
use serde_with::{
    de::DeserializeAsWrap, ser::SerializeAsWrap, serde_as, DeserializeAs, SerializeAs,
};
use std::{iter::Iterator, marker::PhantomData};

/// A polynomial commitment.
#[serde_as]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(bound = "C: CanonicalDeserialize + CanonicalSerialize")]
pub struct PolyComm<C> {
    #[serde_as(as = "Vec<o1_utils::serialization::SerdeAs>")]
    pub elems: Vec<C>,
}

/// A commitment to a polynomial with some blinding factors.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlindedCommitment<G>
where
    G: CommitmentCurve,
{
    pub commitment: PolyComm<G>,
    pub blinders: PolyComm<G::ScalarField>,
}

impl<T> PolyComm<T> {
    pub fn new(elems: Vec<T>) -> Self {
        Self { elems }
    }
}

impl<T, U> SerializeAs<PolyComm<T>> for PolyComm<U>
where
    U: SerializeAs<T>,
{
    fn serialize_as<S>(source: &PolyComm<T>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_seq(source.elems.iter().map(|e| SerializeAsWrap::<T, U>::new(e)))
    }
}

impl<'de, T, U> DeserializeAs<'de, PolyComm<T>> for PolyComm<U>
where
    U: DeserializeAs<'de, T>,
{
    fn deserialize_as<D>(deserializer: D) -> Result<PolyComm<T>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SeqVisitor<T, U> {
            marker: PhantomData<(T, U)>,
        }

        impl<'de, T, U> Visitor<'de> for SeqVisitor<T, U>
        where
            U: DeserializeAs<'de, T>,
        {
            type Value = PolyComm<T>;

            fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                formatter.write_str("a sequence")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                #[allow(clippy::redundant_closure_call)]
                let mut elems = vec![];

                while let Some(value) = seq
                    .next_element()?
                    .map(|v: DeserializeAsWrap<T, U>| v.into_inner())
                {
                    elems.push(value);
                }

                Ok(PolyComm { elems })
            }
        }

        let visitor = SeqVisitor::<T, U> {
            marker: PhantomData,
        };
        deserializer.deserialize_seq(visitor)
    }
}

impl<A: Copy + Clone + CanonicalDeserialize + CanonicalSerialize> PolyComm<A> {
    pub fn map<B, F>(&self, mut f: F) -> PolyComm<B>
    where
        F: FnMut(A) -> B,
        B: CanonicalDeserialize + CanonicalSerialize,
    {
        let elems = self.elems.iter().map(|x| f(*x)).collect();
        PolyComm { elems }
    }

    /// Returns the length of the commitment.
    pub fn len(&self) -> usize {
        self.elems.len()
    }

    /// Returns `true` if the commitment is empty.
    pub fn is_empty(&self) -> bool {
        self.elems.is_empty()
    }

    // TODO: if all callers end up calling unwrap, just call this zip_eq and
    // panic here (and document the panic)
    pub fn zip<B: Copy + CanonicalDeserialize + CanonicalSerialize>(
        &self,
        other: &PolyComm<B>,
    ) -> Option<PolyComm<(A, B)>> {
        if self.elems.len() != other.elems.len() {
            return None;
        }
        let elems = self
            .elems
            .iter()
            .zip(other.elems.iter())
            .map(|(x, y)| (*x, *y))
            .collect();
        Some(PolyComm { elems })
    }
}

/// Inside the circuit, we have a specialized scalar multiplication which computes
/// either
///
/// ```ignore
/// |g: G, x: G::ScalarField| g.scale(x + 2^n)
/// ```
///
/// if the scalar field of G is greater than the size of the base field
/// and
///
/// ```ignore
/// |g: G, x: G::ScalarField| g.scale(2*x + 2^n)
/// ```
///
/// otherwise. So, if we want to actually scale by `s`, we need to apply the
/// inverse function of `|x| x + 2^n` (or of `|x| 2*x + 2^n` in the other case),
/// before supplying the scalar to our in-circuit scalar-multiplication
/// function. This computes that inverse function.
/// Namely,
///
/// ```ignore
/// |x: G::ScalarField| x - 2^n
/// ```
///
/// when the scalar field is larger than the base field and
///
/// ```ignore
/// |x: G::ScalarField| (x - 2^n) / 2
/// ```
///
/// in the other case.
pub fn shift_scalar<G: AffineRepr>(x: G::ScalarField) -> G::ScalarField
where
    G::BaseField: PrimeField,
{
    let n1 = <G::ScalarField as PrimeField>::MODULUS;
    let n2 = <G::ScalarField as PrimeField>::BigInt::from_bits_le(
        &<G::BaseField as PrimeField>::MODULUS.to_bits_le()[..],
    );
    let two: G::ScalarField = (2u64).into();
    let two_pow = two.pow([<G::ScalarField as PrimeField>::MODULUS_BIT_SIZE as u64]);
    if n1 < n2 {
        (x - (two_pow + G::ScalarField::one())) / two
    } else {
        x - two_pow
    }
}

impl<'a, 'b, C: AffineRepr> Add<&'a PolyComm<C>> for &'b PolyComm<C> {
    type Output = PolyComm<C>;

    fn add(self, other: &'a PolyComm<C>) -> PolyComm<C> {
        let mut elems = vec![];
        let n1 = self.elems.len();
        let n2 = other.elems.len();
        for i in 0..std::cmp::max(n1, n2) {
            let pt = if i < n1 && i < n2 {
                (self.elems[i] + other.elems[i]).into_affine()
            } else if i < n1 {
                self.elems[i]
            } else {
                other.elems[i]
            };
            elems.push(pt);
        }
        PolyComm { elems }
    }
}

impl<'a, 'b, C: AffineRepr + Sub<Output = C::Group>> Sub<&'a PolyComm<C>> for &'b PolyComm<C> {
    type Output = PolyComm<C>;

    fn sub(self, other: &'a PolyComm<C>) -> PolyComm<C> {
        let mut elems = vec![];
        let n1 = self.elems.len();
        let n2 = other.elems.len();
        for i in 0..std::cmp::max(n1, n2) {
            let pt = if i < n1 && i < n2 {
                (self.elems[i] - other.elems[i]).into_affine()
            } else if i < n1 {
                self.elems[i]
            } else {
                other.elems[i]
            };
            elems.push(pt);
        }
        PolyComm { elems }
    }
}

impl<C: AffineRepr> PolyComm<C> {
    pub fn scale(&self, c: C::ScalarField) -> PolyComm<C> {
        PolyComm {
            elems: self.elems.iter().map(|g| g.mul(c).into_affine()).collect(),
        }
    }

    /// Performs a multi-scalar multiplication between scalars `elm` and commitments `com`.
    /// If both are empty, returns a commitment of length 1 containing the point at infinity.
    ///
    /// ## Panics
    ///
    /// Panics if `com` and `elm` are not of the same size.
    pub fn multi_scalar_mul(com: &[&PolyComm<C>], elm: &[C::ScalarField]) -> Self {
        assert_eq!(com.len(), elm.len());

        if com.is_empty() || elm.is_empty() {
            return Self::new(vec![C::zero()]);
        }

        let all_scalars: Vec<_> = elm.iter().map(|s| s.into_bigint()).collect();

        let elems_size = Iterator::max(com.iter().map(|c| c.elems.len())).unwrap();
        let mut elems = Vec::with_capacity(elems_size);

        for chunk in 0..elems_size {
            let (points, scalars): (Vec<_>, Vec<_>) = com
                .iter()
                .zip(&all_scalars)
                // get rid of scalars that don't have an associated chunk
                .filter_map(|(com, scalar)| com.elems.get(chunk).map(|c| (c, scalar)))
                .unzip();

            let chunk_msm = C::Group::msm_bigint(&points, &scalars);
            elems.push(chunk_msm.into_affine());
        }
        Self::new(elems)
    }
}

/// Returns the product of all the field elements belonging to an iterator.
pub fn product<F: Field>(xs: impl Iterator<Item = F>) -> F {
    let mut res = F::one();
    for x in xs {
        res *= &x;
    }
    res
}

/// Returns (1 + chal[-1] x)(1 + chal[-2] x^2)(1 + chal[-3] x^4) ...
/// It's "step 8: Define the univariate polynomial" of
/// appendix A.2 of <https://eprint.iacr.org/2020/499>
pub fn b_poly<F: Field>(chals: &[F], x: F) -> F {
    let k = chals.len();

    let mut pow_twos = vec![x];

    for i in 1..k {
        pow_twos.push(pow_twos[i - 1].square());
    }

    product((0..k).map(|i| (F::one() + (chals[i] * pow_twos[k - 1 - i]))))
}

pub fn b_poly_coefficients<F: Field>(chals: &[F]) -> Vec<F> {
    let rounds = chals.len();
    let s_length = 1 << rounds;
    let mut s = vec![F::one(); s_length];
    let mut k: usize = 0;
    let mut pow: usize = 1;
    for i in 1..s_length {
        k += if i == pow { 1 } else { 0 };
        pow <<= if i == pow { 1 } else { 0 };
        s[i] = s[i - (pow >> 1)] * chals[rounds - 1 - (k - 1)];
    }
    s
}

pub fn squeeze_prechallenge<Fq: Field, G, Fr: Field, EFqSponge: FqSponge<Fq, G, Fr>>(
    sponge: &mut EFqSponge,
) -> ScalarChallenge<Fr> {
    ScalarChallenge(sponge.challenge())
}

pub fn squeeze_challenge<Fq: Field, G, Fr: PrimeField, EFqSponge: FqSponge<Fq, G, Fr>>(
    endo_r: &Fr,
    sponge: &mut EFqSponge,
) -> Fr {
    squeeze_prechallenge(sponge).to_field(endo_r)
}

pub fn absorb_commitment<Fq: Field, G: Clone, Fr: PrimeField, EFqSponge: FqSponge<Fq, G, Fr>>(
    sponge: &mut EFqSponge,
    commitment: &PolyComm<G>,
) {
    sponge.absorb_g(&commitment.elems);
}

/// A useful trait extending AffineRepr for commitments.
/// Unfortunately, we can't specify that `AffineRepr<BaseField : PrimeField>`,
/// so usage of this traits must manually bind `G::BaseField: PrimeField`.
pub trait CommitmentCurve: AffineRepr + Sub<Output = Self::Group> {
    type Params: SWCurveConfig;
    type Map: GroupMap<Self::BaseField>;

    fn to_coordinates(&self) -> Option<(Self::BaseField, Self::BaseField)>;
    fn of_coordinates(x: Self::BaseField, y: Self::BaseField) -> Self;
}

/// A trait extending CommitmentCurve for endomorphisms.
/// Unfortunately, we can't specify that `AffineRepr<BaseField : PrimeField>`,
/// so usage of this traits must manually bind `G::BaseField: PrimeField`.
pub trait EndoCurve: CommitmentCurve {
    /// Combine where x1 = one
    fn combine_one(g1: &[Self], g2: &[Self], x2: Self::ScalarField) -> Vec<Self> {
        crate::combine::window_combine(g1, g2, Self::ScalarField::one(), x2)
    }

    /// Combine where x1 = one
    fn combine_one_endo(
        endo_r: Self::ScalarField,
        _endo_q: Self::BaseField,
        g1: &[Self],
        g2: &[Self],
        x2: ScalarChallenge<Self::ScalarField>,
    ) -> Vec<Self> {
        crate::combine::window_combine(g1, g2, Self::ScalarField::one(), x2.to_field(&endo_r))
    }

    fn combine(
        g1: &[Self],
        g2: &[Self],
        x1: Self::ScalarField,
        x2: Self::ScalarField,
    ) -> Vec<Self> {
        crate::combine::window_combine(g1, g2, x1, x2)
    }
}

impl<P: SWCurveConfig + Clone> CommitmentCurve for SWJAffine<P> {
    type Params = P;
    type Map = BWParameters<P>;

    fn to_coordinates(&self) -> Option<(Self::BaseField, Self::BaseField)> {
        if self.infinity {
            None
        } else {
            Some((self.x, self.y))
        }
    }

    fn of_coordinates(x: P::BaseField, y: P::BaseField) -> SWJAffine<P> {
        SWJAffine::<P>::new_unchecked(x, y)
    }
}

impl<P: SWCurveConfig + Clone> EndoCurve for SWJAffine<P> {
    fn combine_one(g1: &[Self], g2: &[Self], x2: Self::ScalarField) -> Vec<Self> {
        crate::combine::affine_window_combine_one(g1, g2, x2)
    }

    fn combine_one_endo(
        _endo_r: Self::ScalarField,
        endo_q: Self::BaseField,
        g1: &[Self],
        g2: &[Self],
        x2: ScalarChallenge<Self::ScalarField>,
    ) -> Vec<Self> {
        crate::combine::affine_window_combine_one_endo(endo_q, g1, g2, x2)
    }

    fn combine(
        g1: &[Self],
        g2: &[Self],
        x1: Self::ScalarField,
        x2: Self::ScalarField,
    ) -> Vec<Self> {
        crate::combine::affine_window_combine(g1, g2, x1, x2)
    }
}

/// Computes the linearization of the evaluations of a (potentially
/// split) polynomial.
///
/// Each polynomial in `polys` is represented by a matrix where the
/// rows correspond to evaluated points, and the columns represent
/// potential segments (if a polynomial was split in several parts).
///
/// Elements in `evaluation_points` are several discrete points on which
/// we evaluate polynomials, e.g. `[zeta,zeta*w]`. See `PointEvaluations`.
///
/// Note that if one of the polynomial comes specified with a degree
/// bound, the evaluation for the last segment is potentially shifted
/// to meet the proof.
///
/// Returns
/// ```text
/// |polys| |segments[k]|
///    Σ         Σ         polyscale^{k*n+i} (Σ polys[k][j][i] * evalscale^j)
///  k = 1     i = 1                          j
/// ```
#[allow(clippy::type_complexity)]
pub fn combined_inner_product<F: PrimeField>(
    polyscale: &F,
    evalscale: &F,
    // TODO(mimoo): needs a type that can get you evaluations or segments
    polys: &[Vec<Vec<F>>],
) -> F {
    // final combined evaluation result
    let mut res = F::zero();
    // polyscale^i
    let mut xi_i = F::one();

    for evals_tr in polys.iter().filter(|evals_tr| !evals_tr[0].is_empty()) {
        // Transpose the evaluations.
        // evals[i] = {evals_tr[j][i]}_j now corresponds to a column in evals_tr,
        // representing a segment.
        let evals: Vec<_> = (0..evals_tr[0].len())
            .map(|i| evals_tr.iter().map(|v| v[i]).collect::<Vec<_>>())
            .collect();

        // Iterating over the polynomial segments.
        // Each segment gets its own polyscale^i, each segment element j is multiplied by evalscale^j.
        // Given that xi_i = polyscale^i0 at this point, after this loop we have:
        //
        //    res += Σ polyscale^{i0+i} ( Σ evals_tr[j][i] * evalscale^j )
        //           i                    j
        //
        for eval in &evals {
            // p_i(evalscale)
            let term = DensePolynomial::<F>::eval_polynomial(eval, *evalscale);
            res += &(xi_i * term);
            xi_i *= polyscale;
        }
    }
    res
}

/// Contains the evaluation of a polynomial commitment at a set of points.
pub struct Evaluation<G>
where
    G: AffineRepr,
{
    /// The commitment of the polynomial being evaluated.
    /// Note that PolyComm contains a vector of commitments, which handles the
    /// case when chunking is used, i.e. when the polynomial degree is higher
    /// than the SRS size.
    pub commitment: PolyComm<G>,

    /// Contains an evaluation table. For instance, for vanilla PlonK, it
    /// would be a vector of (chunked) evaluations at ζ and ζω.
    /// The outer vector would be the evaluations at the different points (e.g.
    /// ζ and ζω for vanilla PlonK) and the inner vector would be the chunks of
    /// the polynomial.
    pub evaluations: Vec<Vec<G::ScalarField>>,
}

/// Contains the batch evaluation
// TODO: I think we should really change this name to something more correct
pub struct BatchEvaluationProof<'a, G, EFqSponge, OpeningProof>
where
    G: AffineRepr,
    EFqSponge: FqSponge<G::BaseField, G, G::ScalarField>,
{
    /// The sponge used to generate/absorb the challenges.
    pub sponge: EFqSponge,
    /// A list of evaluations, each supposed to correspond to a different
    /// polynomial.
    pub evaluations: Vec<Evaluation<G>>,
    /// The actual evaluation points. Each field `evaluations` of each structure
    /// of `Evaluation` should have the same (outer) length.
    pub evaluation_points: Vec<G::ScalarField>,
    /// scaling factor for evaluation point powers
    pub polyscale: G::ScalarField,
    /// scaling factor for polynomials
    pub evalscale: G::ScalarField,
    /// batched opening proof
    pub opening: &'a OpeningProof,
    pub combined_inner_product: G::ScalarField,
}

/// This function populates the parameters `scalars` and `points`.
/// It iterates over the evaluations and adds each commitment to the
/// vector `points`.
/// The parameter `scalars` is populated with the values:
/// `rand_base * polyscale^i` for each commitment.
/// For instance, if we have 3 commitments, the `scalars` vector will
/// contain the values
/// ```text
/// [rand_base, rand_base * polyscale, rand_base * polyscale^2]`
/// ```
/// and the vector `points` will contain the commitments.
///
/// Note that the function skips the commitments that are empty.
///
/// If more than one commitment is present in a single evaluation (i.e. if
/// `elems` is larger than one), it means that probably chunking was used (i.e.
/// it is a commitment to a polynomial larger than the SRS).
pub fn combine_commitments<G: CommitmentCurve>(
    evaluations: &[Evaluation<G>],
    scalars: &mut Vec<G::ScalarField>,
    points: &mut Vec<G>,
    polyscale: G::ScalarField,
    rand_base: G::ScalarField,
) {
    // will contain the power of polyscale
    let mut xi_i = G::ScalarField::one();

    for Evaluation { commitment, .. } in evaluations
        .iter()
        .filter(|x| !x.commitment.elems.is_empty())
    {
        // iterating over the polynomial segments
        for comm_ch in &commitment.elems {
            scalars.push(rand_base * xi_i);
            points.push(*comm_ch);

            // compute next power of polyscale
            xi_i *= polyscale;
        }
    }
}

/// Combine the (chunked) evaluations of multiple polynomials.
/// This function returns the accumulation of the evaluations, scaled by
/// `polyscale`.
/// If no evaluation is given, the function returns an empty vector.
/// It does also suppose that for each evaluation, the number of evaluations is
/// the same. It is not constrained yet in the interface, but it should be. If
/// one list has not the same size, it will be shrunk to the size of the first
/// element of the list.
/// For instance, if we have 3 polynomials P1, P2, P3 evaluated at the points
/// ζ and ζω (like in vanilla PlonK), and for each polynomial, we have two
/// chunks, i.e. we have
/// ```text
///         2 chunks of P1
///        /---------------\
/// E1 = [(P1_1(ζ), P1_2(ζ)), (P1_1(ζω), P1_2(ζω))]
/// E2 = [(P2_1(ζ), P2_2(ζ)), (P2_1(ζω), P2_2(ζω))]
/// E3 = [(P3_1(ζ), P3_2(ζ)), (P3_1(ζω), P3_2(ζω))]
/// ```
/// The output will be a list of 3 elements, equal to:
/// ```text
/// P1_1(ζ) + P1_2(ζ) * polyscale + P1_1(ζω) polyscale^2 + P1_2(ζω) * polyscale^3
/// P2_1(ζ) + P2_2(ζ) * polyscale + P2_1(ζω) polyscale^2 + P2_2(ζω) * polyscale^3
/// ```
pub fn combine_evaluations<G: CommitmentCurve>(
    evaluations: &Vec<Evaluation<G>>,
    polyscale: G::ScalarField,
) -> Vec<G::ScalarField> {
    let mut xi_i = G::ScalarField::one();
    let mut acc = {
        let num_evals = if !evaluations.is_empty() {
            evaluations[0].evaluations.len()
        } else {
            0
        };
        vec![G::ScalarField::zero(); num_evals]
    };

    for Evaluation { evaluations, .. } in evaluations
        .iter()
        .filter(|x| !x.commitment.elems.is_empty())
    {
        // IMPROVEME: we could have a flat array that would contain all the
        // evaluations and all the chunks. It would avoid fetching the memory
        // and avoid indirection into RAM.
        // We could have a single flat array.
        // iterating over the polynomial segments
        for chunk_idx in 0..evaluations[0].len() {
            // supposes that all evaluations are of the same size
            for eval_pt_idx in 0..evaluations.len() {
                acc[eval_pt_idx] += evaluations[eval_pt_idx][chunk_idx] * xi_i;
            }
            xi_i *= polyscale;
        }
    }

    acc
}

pub fn inner_prod<F: Field>(xs: &[F], ys: &[F]) -> F {
    let mut res = F::zero();
    for (&x, y) in xs.iter().zip(ys) {
        res += &(x * y);
    }
    res
}

#[cfg(feature = "ocaml_types")]
pub mod caml {
    // polynomial commitment
    use super::PolyComm;
    use ark_ec::AffineRepr;

    #[derive(Clone, Debug, ocaml::IntoValue, ocaml::FromValue, ocaml_gen::Struct)]
    pub struct CamlPolyComm<CamlG> {
        pub unshifted: Vec<CamlG>,
        pub shifted: Option<CamlG>,
    }

    // handy conversions

    impl<G, CamlG> From<PolyComm<G>> for CamlPolyComm<CamlG>
    where
        G: AffineRepr,
        CamlG: From<G>,
    {
        fn from(polycomm: PolyComm<G>) -> Self {
            Self {
                unshifted: polycomm.elems.into_iter().map(CamlG::from).collect(),
                shifted: None,
            }
        }
    }

    impl<'a, G, CamlG> From<&'a PolyComm<G>> for CamlPolyComm<CamlG>
    where
        G: AffineRepr,
        CamlG: From<G> + From<&'a G>,
    {
        fn from(polycomm: &'a PolyComm<G>) -> Self {
            Self {
                unshifted: polycomm.elems.iter().map(Into::<CamlG>::into).collect(),
                shifted: None,
            }
        }
    }

    impl<G, CamlG> From<CamlPolyComm<CamlG>> for PolyComm<G>
    where
        G: AffineRepr + From<CamlG>,
    {
        fn from(camlpolycomm: CamlPolyComm<CamlG>) -> PolyComm<G> {
            assert!(
                camlpolycomm.shifted.is_none(),
                "mina#14628: Shifted commitments are deprecated and must not be used"
            );
            PolyComm {
                elems: camlpolycomm
                    .unshifted
                    .into_iter()
                    .map(Into::<G>::into)
                    .collect(),
            }
        }
    }

    impl<'a, G, CamlG> From<&'a CamlPolyComm<CamlG>> for PolyComm<G>
    where
        G: AffineRepr + From<&'a CamlG> + From<CamlG>,
    {
        fn from(camlpolycomm: &'a CamlPolyComm<CamlG>) -> PolyComm<G> {
            assert!(
                camlpolycomm.shifted.is_none(),
                "mina#14628: Shifted commitments are deprecated and must not be used"
            );
            PolyComm {
                //FIXME something with as_ref()
                elems: camlpolycomm.unshifted.iter().map(Into::into).collect(),
            }
        }
    }
}
