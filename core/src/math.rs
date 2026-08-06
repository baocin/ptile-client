//! Float functions that libcore does not provide.
//!
//! `sin`, `cos`, `sqrt`, `atan2` and `round` are inherent `f64` methods only
//! when `std` is linked; under `no_std` they need a software implementation.
//! Mirrors h3o's own strategy (`h3o::math::functions-libm.rs`): native methods
//! under `std`, `libm` otherwise.
//!
//! This lives in one module rather than one per consumer. `route_graph.rs`
//! reached for the inherent methods directly and so never compiled under
//! `no_std` -- the crate advertised `no_std` support that had never been built.

#[cfg(feature = "std")]
mod imp {
    #[inline]
    pub fn sin(x: f64) -> f64 {
        x.sin()
    }
    #[inline]
    pub fn cos(x: f64) -> f64 {
        x.cos()
    }
    #[inline]
    pub fn sqrt(x: f64) -> f64 {
        x.sqrt()
    }
    #[inline]
    pub fn atan2(y: f64, x: f64) -> f64 {
        y.atan2(x)
    }
    #[inline]
    pub fn round(x: f64) -> f64 {
        x.round()
    }
    #[inline]
    pub fn ceil(x: f64) -> f64 {
        x.ceil()
    }
}

#[cfg(not(feature = "std"))]
mod imp {
    #[inline]
    pub fn sin(x: f64) -> f64 {
        libm::sin(x)
    }
    #[inline]
    pub fn cos(x: f64) -> f64 {
        libm::cos(x)
    }
    #[inline]
    pub fn sqrt(x: f64) -> f64 {
        libm::sqrt(x)
    }
    #[inline]
    pub fn atan2(y: f64, x: f64) -> f64 {
        libm::atan2(y, x)
    }
    #[inline]
    pub fn round(x: f64) -> f64 {
        libm::round(x)
    }
    #[inline]
    pub fn ceil(x: f64) -> f64 {
        libm::ceil(x)
    }
}

pub(crate) use imp::{atan2, ceil, cos, round, sin, sqrt};

#[cfg(test)]
mod tests {
    use super::*;

    /// Both arms must agree, or a `no_std` build silently computes different
    /// distances from a `std` one.
    #[test]
    fn matches_the_inherent_methods() {
        for &x in &[0.0f64, 0.5, 1.0, -1.0, 2.5, -3.75, 1e6] {
            assert!((sin(x) - x.sin()).abs() < 1e-12, "sin({x})");
            assert!((cos(x) - x.cos()).abs() < 1e-12, "cos({x})");
            assert_eq!(round(x), x.round(), "round({x})");
            assert_eq!(ceil(x), x.ceil(), "ceil({x})");
            if x >= 0.0 {
                assert!((sqrt(x) - x.sqrt()).abs() < 1e-12, "sqrt({x})");
            }
        }
        assert!((atan2(1.0, 2.0) - 1.0f64.atan2(2.0)).abs() < 1e-12);
    }

    /// `round` is half-away-from-zero in both std and libm; the routing weight
    /// quantisation depends on that agreeing.
    #[test]
    fn round_is_half_away_from_zero() {
        assert_eq!(round(0.5), 1.0);
        assert_eq!(round(-0.5), -1.0);
        assert_eq!(round(1.5), 2.0);
        assert_eq!(round(2.5), 3.0);
    }
}
