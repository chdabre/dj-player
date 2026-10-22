// SPDX-License-Identifier: AGPL-3.0-or-later
//
// A port of Essentia's KeyExtractor (FrameCutter → Windowing → Spectrum → SpectralPeaks →
// SpectralWhitening → HPCP → Key), Copyright (C) 2006-2021 Music Technology Group, Universitat
// Pompeu Fabra, AGPL-3.0. It follows Essentia's code closely, constants and quirks included,
// so that its keys match Essentia's on the same audio; this file is therefore AGPL-3.0 too.

//! The major and minor key profiles the averaged HPCP is correlated with.

/// Major and minor key profiles, tonic first (Essentia's tables).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Faraldo et al. 2017, automatic profiles from electronic dance music (Essentia's
    /// `KeyExtractor` default in our bridge).
    Edma,
    /// Faraldo's "bgate" profiles for EDM.
    Bgate,
    Krumhansl,
    Temperley,
}

impl Profile {
    pub(super) fn tables(self) -> ([f32; 12], [f32; 12]) {
        match self {
            Profile::Edma => (
                [
                    1.00, 0.29, 0.50, 0.40, 0.60, 0.56, 0.32, 0.80, 0.31, 0.45, 0.42, 0.39,
                ],
                [
                    1.00, 0.31, 0.44, 0.58, 0.33, 0.49, 0.29, 0.78, 0.43, 0.29, 0.53, 0.32,
                ],
            ),
            Profile::Bgate => (
                [
                    1.00, 0.00, 0.42, 0.00, 0.53, 0.37, 0.00, 0.77, 0.00, 0.38, 0.21, 0.30,
                ],
                [
                    1.00, 0.00, 0.36, 0.39, 0.00, 0.38, 0.00, 0.74, 0.27, 0.00, 0.42, 0.23,
                ],
            ),
            Profile::Krumhansl => (
                [
                    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
                ],
                [
                    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
                ],
            ),
            Profile::Temperley => (
                [5.0, 2.0, 3.5, 2.0, 4.5, 4.0, 2.0, 4.5, 2.0, 3.5, 1.5, 4.0],
                [5.0, 2.0, 3.5, 4.5, 2.0, 4.0, 2.0, 4.5, 3.5, 2.0, 1.5, 4.0],
            ),
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Profile::Edma => "edma",
            Profile::Bgate => "bgate",
            Profile::Krumhansl => "krumhansl",
            Profile::Temperley => "temperley",
        }
    }
}
