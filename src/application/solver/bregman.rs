//! Solver utilities for LMSR Bregman divergence.
//!
//! For the Logarithmic Market Scoring Rule (LMSR):
//!
//! - Cost function: `C(q) = b * log(sum(exp(q_i/b)))`
//! - Conjugate: `R(mu) = sum(mu_i * ln(mu_i))` (negative entropy)
//! - Bregman divergence: `D(mu||theta) = KL divergence`
//!
//! The divergence `D(mu*||theta)` equals the maximum arbitrage profit,
//! making these calculations central to combinatorial arbitrage detection.

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

/// Compute Bregman divergence `D(mu||theta)` for LMSR.
///
/// This is the Kullback-Leibler divergence when R is negative entropy:
/// `D(mu||theta) = sum(mu_i * ln(mu_i/theta_i))`
///
/// The result measures how far the market prices `theta` are from the
/// target distribution `mu`, with larger values indicating more arbitrage potential.
///
/// # Arguments
///
/// * `mu` - Target probability vector (must be a valid distribution summing to 1).
/// * `theta` - Current market prices (may not sum to 1 if mispriced).
///
/// # Returns
///
/// The divergence value, or zero if inputs are empty or mismatched.
#[must_use]
pub fn bregman_divergence(mu: &[Decimal], theta: &[Decimal]) -> Decimal {
    if mu.len() != theta.len() || mu.is_empty() {
        return Decimal::ZERO;
    }

    let mut divergence = Decimal::ZERO;
    let epsilon = Decimal::new(1, 10); // 1e-10 for numerical stability

    for (m, t) in mu.iter().zip(theta.iter()) {
        divergence += *t - *m;
        if *m > epsilon && *t > epsilon {
            // μ * ln(μ/θ) = μ * (ln(μ) - ln(θ))
            let m_f64 = m.to_f64().unwrap_or(0.0);
            let t_f64 = t.to_f64().unwrap_or(1.0);

            if m_f64 > 0.0 && t_f64 > 0.0 {
                let term = m_f64 * (m_f64.ln() - t_f64.ln());
                divergence += Decimal::try_from(term).unwrap_or(Decimal::ZERO);
            }
        }
    }

    divergence
}

/// Compute the gradient of Bregman divergence with respect to mu.
///
/// For KL divergence: `dD/dmu_i = ln(mu_i/theta_i) + 1`
///
/// Used by the Frank-Wolfe algorithm to determine the descent direction.
///
/// # Arguments
///
/// * `mu` - Current iterate (probability vector being optimized).
/// * `theta` - Target prices (fixed during optimization).
///
/// # Returns
///
/// Gradient vector with one element per outcome.
#[must_use]
pub fn bregman_gradient(mu: &[Decimal], theta: &[Decimal]) -> Vec<Decimal> {
    let epsilon = Decimal::new(1, 10);

    mu.iter()
        .zip(theta.iter())
        .map(|(m, t)| {
            let m_safe = (*m).max(epsilon);
            let t_safe = (*t).max(epsilon);

            let m_f64 = m_safe.to_f64().unwrap_or(1.0);
            let t_f64 = t_safe.to_f64().unwrap_or(1.0);

            // ln(μ/θ) + 1 = ln(μ) - ln(θ) + 1
            let grad = m_f64.ln() - t_f64.ln();
            Decimal::try_from(grad).unwrap_or(Decimal::ZERO)
        })
        .collect()
}

/// Compute the LMSR cost function C(q).
///
/// `C(q) = b * ln(sum(exp(q_i/b)))`
///
/// This is the standard cost function for Logarithmic Market Scoring Rule
/// automated market makers.
///
/// # Arguments
///
/// * `q` - Quantity vector (shares outstanding for each outcome).
/// * `b` - Liquidity parameter (controls price sensitivity).
///
/// # Returns
///
/// The market cost, or zero if inputs are empty or b is zero.
#[must_use]
pub fn lmsr_cost(q: &[Decimal], b: Decimal) -> Decimal {
    if q.is_empty() || b == Decimal::ZERO {
        return Decimal::ZERO;
    }

    let b_f64 = b.to_f64().unwrap_or(1.0);

    let sum_exp: f64 = q
        .iter()
        .map(|qi| {
            let qi_f64 = qi.to_f64().unwrap_or(0.0);
            (qi_f64 / b_f64).exp()
        })
        .sum();

    let cost = b_f64 * sum_exp.ln();
    Decimal::try_from(cost).unwrap_or(Decimal::ZERO)
}

/// Compute LMSR prices from quantities.
///
/// `P_i = exp(q_i/b) / sum_k(exp(q_k/b))`
///
/// Derives the implied probability (price) for each outcome from the
/// current quantity state. Prices always sum to 1.
///
/// # Arguments
///
/// * `q` - Quantity vector (shares outstanding for each outcome).
/// * `b` - Liquidity parameter.
///
/// # Returns
///
/// Price vector summing to 1, or empty if inputs are invalid.
#[must_use]
pub fn lmsr_prices(q: &[Decimal], b: Decimal) -> Vec<Decimal> {
    if q.is_empty() || b == Decimal::ZERO {
        return vec![];
    }

    let b_f64 = b.to_f64().unwrap_or(1.0);

    let exps: Vec<f64> = q
        .iter()
        .map(|qi| {
            let qi_f64 = qi.to_f64().unwrap_or(0.0);
            (qi_f64 / b_f64).exp()
        })
        .collect();

    let sum_exp: f64 = exps.iter().sum();

    exps.iter()
        .map(|e| Decimal::try_from(e / sum_exp).unwrap_or(Decimal::ZERO))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_bregman_divergence_same_distribution() {
        let mu = vec![dec!(0.5), dec!(0.5)];
        let theta = vec![dec!(0.5), dec!(0.5)];

        let d = bregman_divergence(&mu, &theta);
        assert!(
            d.abs() < dec!(0.001),
            "Same distribution should have ~0 divergence"
        );
    }

    #[test]
    fn test_bregman_divergence_different() {
        let mu = vec![dec!(0.7), dec!(0.3)];
        let theta = vec![dec!(0.5), dec!(0.5)];

        let d = bregman_divergence(&mu, &theta);
        assert!(
            d > Decimal::ZERO,
            "Different distributions should have positive divergence"
        );
    }

    #[test]
    fn test_bregman_gradient_at_same_point() {
        let mu = vec![dec!(0.5), dec!(0.5)];
        let theta = vec![dec!(0.5), dec!(0.5)];

        let grad = bregman_gradient(&mu, &theta);

        // At same point, gradient should be [1, 1] (ln(1) + 1 = 1)
        for g in &grad {
            assert!(g.abs() < dec!(0.01), "Gradient at same point should be ~0");
        }
    }

    #[test]
    fn test_lmsr_prices_sum_to_one() {
        let q = vec![dec!(1), dec!(2), dec!(3)];
        let b = dec!(1);

        let prices = lmsr_prices(&q, b);
        let sum: Decimal = prices.iter().sum();

        assert!(
            (sum - Decimal::ONE).abs() < dec!(0.001),
            "LMSR prices should sum to 1, got {}",
            sum
        );
    }

    #[test]
    fn test_lmsr_prices_equal_quantities() {
        let q = vec![dec!(1), dec!(1), dec!(1)];
        let b = dec!(1);

        let prices = lmsr_prices(&q, b);

        // Equal quantities should give equal prices
        let expected = Decimal::ONE / Decimal::from(3);
        for p in &prices {
            assert!(
                (*p - expected).abs() < dec!(0.01),
                "Equal quantities should give ~1/3 prices"
            );
        }
    }

    #[test]
    fn test_lmsr_cost_basic() {
        let q = vec![dec!(0), dec!(0)];
        let b = dec!(1);

        let cost = lmsr_cost(&q, b);

        // C([0,0]) = b * ln(2) ≈ 0.693
        assert!(
            cost > dec!(0.6) && cost < dec!(0.8),
            "LMSR cost for [0,0] should be ~ln(2)"
        );
    }

    #[test]
    fn test_lmsr_empty_inputs() {
        assert_eq!(lmsr_cost(&[], dec!(1)), Decimal::ZERO);
        assert_eq!(lmsr_prices(&[], dec!(1)), Vec::<Decimal>::new());
        assert_eq!(bregman_divergence(&[], &[]), Decimal::ZERO);
    }
}
